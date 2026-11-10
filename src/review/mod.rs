//! Grading: the contract every reviewer follows, and the three implementations.
//!
//! * [`local`] — deterministic keyword rubric, always available, no model.
//! * [`api`] — BYOK HTTP client for OpenAI- or Anthropic-compatible endpoints.
//! * [`acp`] — hosts the user's own coding agent over the Agent Client Protocol,
//!   so grading runs on the agent/subscription they already pay for.

pub mod acp;
pub mod api;
pub mod contract;
pub mod local;

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::snippet::Snippet;
use crate::srs::{Verdict, combine};

/// What the learner wrote for one block.
#[derive(Debug, Clone)]
pub struct Annotation {
    /// 0-based index into `Snippet::blocks`.
    pub block: usize,
    pub text: String,
}

impl Annotation {
    pub fn new(block: usize, text: impl Into<String>) -> Self {
        Self {
            block,
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BlockGrade {
    pub block: usize,
    pub verdict: Verdict,
    pub feedback: String,
    pub missing: Vec<String>,
    /// Per-concept verdicts, in the block's concept order.
    pub concepts: Vec<(String, Verdict)>,
}

#[derive(Debug, Clone)]
pub struct Grade {
    pub reviewer: String,
    pub blocks: Vec<BlockGrade>,
    pub summary: String,
}

impl Grade {
    /// One verdict per concept, folded across every block that carries it.
    pub fn concept_verdicts(&self) -> Vec<(String, Verdict)> {
        let mut grouped: BTreeMap<&str, Vec<Verdict>> = BTreeMap::new();
        for block in &self.blocks {
            for (concept, verdict) in &block.concepts {
                grouped.entry(concept.as_str()).or_default().push(*verdict);
            }
        }
        grouped
            .into_iter()
            .map(|(concept, verdicts)| (concept.to_string(), combine(&verdicts)))
            .collect()
    }

    pub fn correct_blocks(&self) -> usize {
        self.blocks
            .iter()
            .filter(|b| b.verdict == Verdict::Correct)
            .count()
    }
}

pub trait Reviewer {
    fn name(&self) -> String;

    fn grade(&self, snippet: &Snippet, annotations: &[Annotation]) -> Result<Grade>;
}

#[derive(Debug, Deserialize)]
pub struct RawGrade {
    #[serde(default)]
    pub blocks: Vec<RawBlock>,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Deserialize)]
pub struct RawBlock {
    #[serde(default, alias = "index", alias = "blockIndex", alias = "block_number")]
    pub block: Option<usize>,
    #[serde(default)]
    pub verdict: String,
    #[serde(default)]
    pub concepts: BTreeMap<String, String>,
    #[serde(default)]
    pub missing: Vec<String>,
    #[serde(default)]
    pub feedback: String,
}

impl Grade {
    /// Turns whatever the model meant into one grade per snippet block. Missing
    /// blocks become `unknown` rather than a punishment — a reviewer that forgot
    /// a block is not evidence the learner got it wrong.
    pub fn from_raw(snippet: &Snippet, raw: RawGrade, reviewer: &str) -> Result<Grade> {
        if raw.blocks.is_empty() {
            bail!("the reviewer returned no block grades");
        }
        let positional = raw.blocks.len() == snippet.blocks.len();

        let mut blocks = Vec::with_capacity(snippet.blocks.len());
        for (index, block) in snippet.blocks.iter().enumerate() {
            let number = index + 1;
            let raw_block = raw
                .blocks
                .iter()
                .find(|b| b.block == Some(number))
                .or_else(|| {
                    if positional {
                        raw.blocks.get(index)
                    } else {
                        None
                    }
                });

            let Some(raw_block) = raw_block else {
                blocks.push(BlockGrade {
                    block: index,
                    verdict: Verdict::Unknown,
                    feedback: "the reviewer did not grade this block".to_string(),
                    missing: Vec::new(),
                    concepts: block
                        .concepts
                        .iter()
                        .map(|c| (c.clone(), Verdict::Unknown))
                        .collect(),
                });
                continue;
            };

            let concepts: Vec<(String, Verdict)> = block
                .concepts
                .iter()
                .map(|concept| {
                    let verdict = raw_block
                        .concepts
                        .get(concept)
                        .map(|label| Verdict::from_label(label))
                        .unwrap_or_else(|| Verdict::from_label(&raw_block.verdict));
                    (concept.clone(), verdict)
                })
                .collect();

            let mut verdict = Verdict::from_label(&raw_block.verdict);
            if raw_block.verdict.trim().is_empty() {
                let only: Vec<Verdict> = concepts.iter().map(|(_, v)| *v).collect();
                verdict = combine(&only);
            }

            blocks.push(BlockGrade {
                block: index,
                verdict,
                feedback: raw_block.feedback.trim().to_string(),
                missing: raw_block
                    .missing
                    .iter()
                    .map(|m| m.trim().to_string())
                    .filter(|m| !m.is_empty())
                    .collect(),
                concepts,
            });
        }

        Ok(Grade {
            reviewer: reviewer.to_string(),
            blocks,
            summary: raw.summary.trim().to_string(),
        })
    }
}

/// Models wrap JSON in prose or fences even when told not to. Take the outermost
/// object and ignore the rest.
pub fn parse_json_object(text: &str) -> Result<RawGrade> {
    let body = extract_json(text)?;
    serde_json::from_str::<RawGrade>(&body).with_context(|| {
        format!(
            "the reviewer's JSON did not match the expected shape: {}",
            truncate(&body, 300)
        )
    })
}

pub fn extract_json(text: &str) -> Result<String> {
    let trimmed = text.trim();
    let start = trimmed
        .find('{')
        .context("the reviewer did not return a JSON object")?;
    let end = trimmed
        .rfind('}')
        .context("the reviewer returned an unterminated JSON object")?;
    if end <= start {
        bail!("the reviewer returned a malformed JSON object");
    }
    Ok(trimmed[start..=end].to_string())
}

pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snippet::Snippet;

    const SNIPPET: &str = r#"
id = "t-1"
language = "rust"
title = "test"
code = '''
let a = 1;
let b = 2;
'''

[[blocks]]
lines = [1, 1]
ask = "a?"
concepts = ["rust.binding"]

[[blocks]]
lines = [2, 2]
ask = "b?"
concepts = ["rust.binding"]
"#;

    fn snippet() -> Snippet {
        Snippet::parse(SNIPPET).unwrap()
    }

    #[test]
    fn extracts_json_from_fences_and_prose() {
        let text =
            "Sure! Here you go:\n```json\n{\"blocks\":[],\"summary\":\"ok\"}\n```\nHope it helps.";
        let raw = parse_json_object(text).unwrap();
        assert_eq!(raw.summary, "ok");
    }

    #[test]
    fn a_missing_object_is_an_error() {
        assert!(parse_json_object("I cannot grade this.").is_err());
    }

    #[test]
    fn maps_blocks_by_number() {
        let raw = parse_json_object(
            r#"{"blocks":[
                {"block":2,"verdict":"wrong","concepts":{"rust.binding":"wrong"},"feedback":"no","missing":["x"]},
                {"block":1,"verdict":"correct","concepts":{"rust.binding":"correct"},"feedback":"yes"}
            ],"summary":"mixed"}"#,
        )
        .unwrap();
        let grade = Grade::from_raw(&snippet(), raw, "test").unwrap();
        assert_eq!(grade.blocks[0].verdict, Verdict::Correct);
        assert_eq!(grade.blocks[1].verdict, Verdict::Wrong);
        assert_eq!(grade.blocks[1].missing, vec!["x".to_string()]);
        assert_eq!(grade.correct_blocks(), 1);
        // Same concept graded correct in one block and wrong in another.
        assert_eq!(
            grade.concept_verdicts(),
            vec![("rust.binding".to_string(), Verdict::Partial)]
        );
    }

    #[test]
    fn ungraded_blocks_are_unknown_not_wrong() {
        let raw = parse_json_object(r#"{"blocks":[{"block":1,"verdict":"correct"}],"summary":""}"#)
            .unwrap();
        let grade = Grade::from_raw(&snippet(), raw, "test").unwrap();
        assert_eq!(grade.blocks.len(), 2);
        assert_eq!(grade.blocks[1].verdict, Verdict::Unknown);
    }

    #[test]
    fn a_missing_verdict_falls_back_to_the_concepts() {
        let raw =
            parse_json_object(r#"{"blocks":[{"block":1,"concepts":{"rust.binding":"correct"}}]}"#)
                .unwrap();
        let grade = Grade::from_raw(&snippet(), raw, "test").unwrap();
        assert_eq!(grade.blocks[0].verdict, Verdict::Correct);
    }

    #[test]
    fn unknown_concept_labels_do_not_become_correct() {
        let raw = parse_json_object(r#"{"blocks":[{"block":1,"verdict":"meh"}]}"#).unwrap();
        let grade = Grade::from_raw(&snippet(), raw, "test").unwrap();
        assert_eq!(grade.blocks[0].verdict, Verdict::Unknown);
    }
}
