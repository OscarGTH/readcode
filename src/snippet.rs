//! A snippet is a short piece of real code plus the ground truth we grade
//! against: which lines form a block, what that block does, and which concepts
//! the learner has to name to have understood it.

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Snippet {
    pub id: String,
    pub language: String,
    pub title: String,
    /// Where the code came from, so the learner can read the original.
    #[serde(default)]
    pub source: Option<String>,
    /// One line of framing, printed above the code.
    #[serde(default)]
    pub intro: Option<String>,
    pub code: String,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Block {
    /// `[start, end]`, 1-based and inclusive.
    pub lines: Vec<usize>,
    /// The question shown to the learner.
    pub ask: String,
    pub concepts: Vec<String>,
    /// Keyword rubric used by the offline reviewer, and the ground truth the
    /// LLM reviewers are given so they grade against something specific.
    #[serde(default)]
    pub expects: Vec<Expect>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Expect {
    pub concept: String,
    pub keywords: Vec<String>,
    #[serde(default)]
    pub note: String,
}

impl Snippet {
    pub fn parse(text: &str) -> Result<Self> {
        let snippet: Snippet = toml::from_str(text)?;
        Ok(snippet)
    }

    pub fn code_lines(&self) -> Vec<&str> {
        self.code.lines().collect()
    }

    pub fn line_count(&self) -> usize {
        self.code.lines().count()
    }

    /// The `(start, end)` of a block, or an error if the block is malformed.
    pub fn span(&self, index: usize) -> Result<(usize, usize)> {
        let block = self
            .blocks
            .get(index)
            .with_context(|| format!("snippet `{}` has no block {index}", self.id))?;
        if block.lines.len() != 2 {
            bail!(
                "snippet `{}` block {}: `lines` must be [start, end]",
                self.id,
                index + 1
            );
        }
        Ok((block.lines[0], block.lines[1]))
    }

    /// Every concept the snippet can teach.
    pub fn concepts(&self) -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        for block in &self.blocks {
            for concept in &block.concepts {
                set.insert(concept.clone());
            }
        }
        set
    }

    /// Rejects a snippet we cannot grade honestly: uncovered lines, overlapping
    /// blocks, a concept with no expectation attached.
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            bail!("snippet has an empty id");
        }
        if self.code.trim().is_empty() {
            bail!("snippet `{}` has no code", self.id);
        }
        if self.blocks.is_empty() {
            bail!("snippet `{}` has no blocks", self.id);
        }

        let total = self.line_count();
        let mut last_end = 0usize;

        for (index, block) in self.blocks.iter().enumerate() {
            let n = index + 1;
            if block.lines.len() != 2 {
                bail!(
                    "snippet `{}` block {n}: `lines` must be [start, end]",
                    self.id
                );
            }
            let (start, end) = (block.lines[0], block.lines[1]);
            if start == 0 || start > end {
                bail!(
                    "snippet `{}` block {n}: invalid range {start}-{end}",
                    self.id
                );
            }
            if end > total {
                bail!(
                    "snippet `{}` block {n}: range {start}-{end} exceeds the {total} code lines",
                    self.id
                );
            }
            if start != last_end + 1 {
                bail!(
                    "snippet `{}` block {n}: starts at line {start} but the previous block ended at {last_end}; every line must belong to a block",
                    self.id
                );
            }
            if block.ask.trim().is_empty() {
                bail!("snippet `{}` block {n}: has no `ask`", self.id);
            }
            if block.concepts.is_empty() {
                bail!(
                    "snippet `{}` block {n}: needs at least one concept",
                    self.id
                );
            }
            for expect in &block.expects {
                if !block.concepts.contains(&expect.concept) {
                    bail!(
                        "snippet `{}` block {n}: expectation for `{}` which is not one of the block's concepts",
                        self.id,
                        expect.concept
                    );
                }
                if expect.keywords.is_empty() {
                    bail!(
                        "snippet `{}` block {n}: expectation for `{}` has no keywords",
                        self.id,
                        expect.concept
                    );
                }
            }
            last_end = end;
        }

        if last_end != total {
            bail!(
                "snippet `{}`: blocks cover lines 1-{last_end} but the snippet has {total} lines",
                self.id
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"
id = "t-1"
language = "rust"
title = "test"
code = '''
let a = 1;
let b = 2;
'''

[[blocks]]
lines = [1, 1]
ask = "what is a?"
concepts = ["rust.binding"]
  [[blocks.expects]]
  concept = "rust.binding"
  keywords = ["binding", "bound"]

[[blocks]]
lines = [2, 2]
ask = "what is b?"
concepts = ["rust.binding"]
"#;

    #[test]
    fn parses_and_validates() {
        let snippet = Snippet::parse(OK).unwrap();
        assert_eq!(snippet.line_count(), 2);
        assert_eq!(snippet.span(1).unwrap(), (2, 2));
        assert_eq!(snippet.concepts().len(), 1);
        snippet.validate().unwrap();
    }

    #[test]
    fn rejects_a_gap() {
        let text = OK
            .replace("let b = 2;\n", "let b = 2;\nlet c = 3;\n")
            .replace("lines = [2, 2]", "lines = [3, 3]");
        let snippet = Snippet::parse(&text).unwrap();
        let err = snippet.validate().unwrap_err().to_string();
        assert!(err.contains("every line must belong"), "{err}");
    }

    #[test]
    fn rejects_out_of_order_blocks() {
        let text = OK.replace("lines = [1, 1]", "lines = [2, 2]").replace(
            "lines = [2, 2]\nask = \"what is b?\"",
            "lines = [1, 1]\nask = \"what is b?\"",
        );
        let snippet = Snippet::parse(&text).unwrap();
        assert!(snippet.validate().is_err());
    }

    #[test]
    fn rejects_expectation_for_unknown_concept() {
        let text = OK.replace(
            "concept = \"rust.binding\"\n  keywords",
            "concept = \"rust.other\"\n  keywords",
        );
        let snippet = Snippet::parse(&text).unwrap();
        let err = snippet.validate().unwrap_err().to_string();
        assert!(err.contains("not one of the block's concepts"), "{err}");
    }

    #[test]
    fn rejects_a_block_past_the_code() {
        let text = OK.replace("lines = [2, 2]", "lines = [2, 9]");
        let snippet = Snippet::parse(&text).unwrap();
        let err = snippet.validate().unwrap_err().to_string();
        assert!(err.contains("exceeds"), "{err}");
    }
}
