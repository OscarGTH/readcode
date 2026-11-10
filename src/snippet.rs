//! A snippet is a short piece of real code plus the ground truth we grade
//! against.
//!
//! Authoring is deliberately thin: a snippet is code plus blocks, each with the
//! concepts a learner must name. `notes` and `blocks.line` are optional and only
//! there to make grading sharper.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// How a snippet is asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Asks {
    /// Whatever the corpus is best authored for; a repeat visit alternates.
    #[default]
    Auto,
    /// One prompt per semantic block.
    Blocks,
    /// One prompt per significant line.
    Lines,
}

impl Asks {
    pub fn label(self) -> &'static str {
        match self {
            Asks::Auto => "auto",
            Asks::Blocks => "blocks",
            Asks::Lines => "lines",
        }
    }

    pub fn flipped(self) -> Asks {
        match self {
            Asks::Lines => Asks::Blocks,
            _ => Asks::Lines,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Granularity for this snippet. `auto` lets the session decide.
    #[serde(default)]
    pub asks: Asks,
    /// The level this snippet starts at, as sugar for `levels = [n, n]`.
    #[serde(default)]
    pub level: Option<u8>,
    /// The band of learner levels this snippet suits.
    #[serde(default)]
    pub levels: Option<[u8; 2]>,
    /// Reading for this snippet specifically, shown in the review before the
    /// per-concept links.
    #[serde(default)]
    pub docs: Vec<SnippetDoc>,
    pub code: String,
    pub blocks: Vec<Block>,
}

/// The five rungs a corpus is sorted into, easiest first.
pub const MAX_LEVEL: u8 = 5;

/// What each rung means, in the learner's terms rather than the language's.
pub const LEVELS: [&str; MAX_LEVEL as usize] = [
    "just started — values, arithmetic, printing",
    "small functions — branches, loops, one borrow at a time",
    "whole modules — structs, methods, iterators, error handling",
    "unfamiliar crates — traits, generics, lifetimes",
    "reads Rust for a living",
];

/// The band of levels a snippet is written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Levels {
    pub from: u8,
    pub to: u8,
}

impl Default for Levels {
    /// A snippet that says nothing about its level is offered to everyone, so
    /// hand-written corpora keep working without the field.
    fn default() -> Self {
        Self {
            from: 1,
            to: MAX_LEVEL,
        }
    }
}

impl Levels {
    pub fn label(&self) -> String {
        if self.from == self.to {
            format!("level {}", self.from)
        } else {
            format!("levels {}-{}", self.from, self.to)
        }
    }

    /// Whether a learner at `level` should be offered this at all. One rung
    /// above is allowed — that is the stretch that teaches — but no further.
    pub fn reachable(&self, level: u8) -> bool {
        self.from <= level.saturating_add(1)
    }

    /// How far this sits from the learner, for ranking. Being thrown in above
    /// your level costs more than revising below it.
    pub fn distance(&self, level: u8) -> f64 {
        if self.from > level {
            f64::from(self.from - level) * 0.6
        } else if self.to < level {
            f64::from(level - self.to) * 0.25
        } else {
            0.0
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Block {
    /// `[start, end]`, 1-based and inclusive.
    pub lines: Vec<usize>,
    /// Defaults to a generic question; a specific one grades better.
    #[serde(default = "default_ask")]
    pub ask: String,
    pub concepts: Vec<String>,
    /// Optional ground truth per concept: what a correct answer has to say.
    /// The reviewer is shown these, so they are the difference between "grade
    /// this from the code" and "grade it against a rubric".
    #[serde(default)]
    pub notes: BTreeMap<String, String>,
    /// Lines inside this block that are asked on their own in line mode.
    #[serde(default)]
    pub line: Vec<LinePrompt>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinePrompt {
    /// The line this prompt is about.
    pub at: usize,
    /// Defaults to a generic "what does this line do" question.
    #[serde(default)]
    pub ask: Option<String>,
    /// Defaults to the owning block's concepts.
    #[serde(default)]
    pub concepts: Vec<String>,
    /// Defaults to the owning block's notes, for these concepts.
    #[serde(default)]
    pub notes: BTreeMap<String, String>,
}

/// A link an author wants shown with their snippet.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnippetDoc {
    pub title: String,
    pub url: String,
}

fn default_ask() -> String {
    "Explain what this block does, in your own words.".to_string()
}

/// One unit of asking and grading: a block, or a single line.
#[derive(Debug, Clone)]
pub struct Prompt {
    /// Index of the owning block.
    pub block: usize,
    pub start: usize,
    pub end: usize,
    pub ask: String,
    pub concepts: Vec<String>,
    /// Ground truth per concept, when the corpus says something about it.
    pub notes: BTreeMap<String, String>,
    /// The source lines this prompt covers, for display.
    pub lines: Vec<String>,
}

impl Prompt {
    pub fn is_line(&self) -> bool {
        self.start == self.end
    }

    /// "line 8" or "lines 1-5", for headers and the grading request.
    pub fn span_label(&self) -> String {
        if self.is_line() {
            format!("line {}", self.start)
        } else {
            format!("lines {}-{}", self.start, self.end)
        }
    }
}

impl Snippet {
    pub fn parse(text: &str) -> Result<Self> {
        let snippet: Snippet = toml::from_str(text)?;
        Ok(snippet)
    }

    /// Which rungs this snippet is written for.
    pub fn levels(&self) -> Levels {
        match (self.level, self.levels) {
            (_, Some([from, to])) => Levels { from, to },
            (Some(level), None) => Levels {
                from: level,
                to: level,
            },
            (None, None) => Levels::default(),
        }
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

    /// Does this snippet have authored line-level questions? If so, `auto`
    /// prefers line mode.
    pub fn has_line_prompts(&self) -> bool {
        self.blocks.iter().any(|block| !block.line.is_empty())
    }

    /// Resolves `Auto` against how the corpus is authored.
    pub fn resolve_mode(&self, asks: Asks) -> Asks {
        match asks {
            Asks::Auto => {
                if self.has_line_prompts() {
                    Asks::Lines
                } else {
                    Asks::Blocks
                }
            }
            explicit => explicit,
        }
    }

    /// The prompts for one sitting, in code order.
    pub fn prompts(&self, asks: Asks) -> Vec<Prompt> {
        match self.resolve_mode(asks) {
            Asks::Lines => self.line_prompts(),
            _ => self.block_prompts(),
        }
    }

    fn block_prompts(&self) -> Vec<Prompt> {
        self.blocks
            .iter()
            .enumerate()
            .filter_map(|(index, block)| {
                let (start, end) = (*block.lines.first()?, *block.lines.get(1)?);
                Some(Prompt {
                    block: index,
                    start,
                    end,
                    ask: block.ask.clone(),
                    concepts: block.concepts.clone(),
                    notes: block.notes.clone(),
                    lines: self.slice(start, end),
                })
            })
            .collect()
    }

    fn line_prompts(&self) -> Vec<Prompt> {
        let mut prompts = Vec::new();
        for (index, block) in self.blocks.iter().enumerate() {
            let (start, end) = match (block.lines.first(), block.lines.get(1)) {
                (Some(start), Some(end)) => (*start, *end),
                _ => continue,
            };
            for line in start..=end {
                let authored = block.line.iter().find(|prompt| prompt.at == line);
                // A closing brace is not a question. Authored prompts are asked
                // even where the heuristic would skip the line.
                if authored.is_none() && !self.is_annotatable(line) {
                    continue;
                }
                let concepts = authored
                    .map(|prompt| prompt.concepts.clone())
                    .filter(|concepts| !concepts.is_empty())
                    .unwrap_or_else(|| block.concepts.clone());
                let notes = match authored {
                    Some(prompt) if !prompt.notes.is_empty() => prompt.notes.clone(),
                    _ => block
                        .notes
                        .iter()
                        .filter(|(concept, _)| concepts.contains(concept))
                        .map(|(concept, note)| (concept.clone(), note.clone()))
                        .collect(),
                };
                let ask = authored
                    .and_then(|prompt| prompt.ask.clone())
                    .unwrap_or_else(|| format!("What does line {line} do?"));
                prompts.push(Prompt {
                    block: index,
                    start: line,
                    end: line,
                    ask,
                    concepts,
                    notes,
                    lines: self.slice(line, line),
                });
            }
        }
        prompts
    }

    fn slice(&self, start: usize, end: usize) -> Vec<String> {
        self.code_lines()
            .iter()
            .skip(start.saturating_sub(1))
            .take(end + 1 - start)
            .map(|line| (*line).to_string())
            .collect()
    }

    /// A line worth its own question: it has to contain something nameable.
    /// Blank lines and lone delimiters do not.
    fn is_annotatable(&self, number: usize) -> bool {
        if number == 0 {
            return false;
        }
        match self.code.lines().nth(number - 1) {
            Some(line) => line.chars().any(|c| c.is_alphanumeric() || c == '_'),
            None => false,
        }
    }

    /// Rejects a snippet we cannot grade honestly: uncovered lines, overlapping
    /// blocks, a concept with no id, a note about a concept the block does not
    /// declare, a line prompt outside its block.
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            bail!("snippet has an empty id");
        }
        if self.level.is_some() && self.levels.is_some() {
            bail!(
                "snippet `{}`: set either `level` (one rung) or `levels` (a range), not both",
                self.id
            );
        }
        for doc in &self.docs {
            if !doc.url.starts_with("https://") && !doc.url.starts_with("http://") {
                bail!(
                    "snippet `{}`: `docs` entry `{}` needs a full http(s) URL",
                    self.id,
                    doc.title
                );
            }
        }
        let levels = self.levels();
        if levels.from == 0 || levels.to > MAX_LEVEL || levels.from > levels.to {
            bail!(
                "snippet `{}`: `{}` is out of range — levels run 1 to {MAX_LEVEL}, lowest first",
                self.id,
                levels.label()
            );
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
                bail!("snippet `{}` block {n}: has an empty `ask`", self.id);
            }
            if block.concepts.is_empty() {
                bail!(
                    "snippet `{}` block {n}: needs at least one concept",
                    self.id
                );
            }
            check_concepts_and_notes(&self.id, n, &block.concepts, &block.notes, "the block")?;

            for prompt in &block.line {
                if prompt.at < start || prompt.at > end {
                    bail!(
                        "snippet `{}` block {n}: line prompt at {} is outside the block's range {start}-{end}",
                        self.id,
                        prompt.at
                    );
                }
                if block
                    .line
                    .iter()
                    .filter(|other| other.at == prompt.at)
                    .count()
                    > 1
                {
                    bail!(
                        "snippet `{}` block {n}: two line prompts for line {}",
                        self.id,
                        prompt.at
                    );
                }
                if prompt.ask.as_ref().is_some_and(|ask| ask.trim().is_empty()) {
                    bail!(
                        "snippet `{}` block {n}: line {} has an empty ask",
                        self.id,
                        prompt.at
                    );
                }
                for concept in &prompt.concepts {
                    if !block.concepts.contains(concept) {
                        bail!(
                            "snippet `{}` block {n}: line {} uses concept `{concept}` which the block does not declare",
                            self.id,
                            prompt.at
                        );
                    }
                }
                let concepts = if prompt.concepts.is_empty() {
                    &block.concepts
                } else {
                    &prompt.concepts
                };
                check_concepts_and_notes(
                    &self.id,
                    n,
                    concepts,
                    &prompt.notes,
                    &format!("line {}", prompt.at),
                )?;
            }
            last_end = end;
        }

        if last_end != total {
            bail!(
                "snippet `{}`: blocks cover lines 1-{last_end} but the snippet has {total} lines",
                self.id
            );
        }

        if self.resolve_mode(self.asks) == Asks::Lines {
            for (index, block) in self.blocks.iter().enumerate() {
                let (start, end) = (block.lines[0], block.lines[1]);
                let asked = (start..=end).any(|line| {
                    block.line.iter().any(|prompt| prompt.at == line) || self.is_annotatable(line)
                });
                if !asked {
                    bail!(
                        "snippet `{}` block {}: asked line by line, but every line in {start}-{end} is structural",
                        self.id,
                        index + 1
                    );
                }
            }
        }

        Ok(())
    }

    /// The code with line numbers, so a bad range is easy to fix.
    pub fn numbered_code(&self) -> String {
        let width = self.line_count().to_string().len().max(2);
        self.code_lines()
            .iter()
            .enumerate()
            .map(|(index, line)| format!("{:>width$} | {line}", index + 1))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn check_concepts_and_notes(
    snippet: &str,
    block: usize,
    concepts: &[String],
    notes: &BTreeMap<String, String>,
    where_: &str,
) -> Result<()> {
    for concept in concepts {
        if concept.trim().is_empty() {
            bail!("snippet `{snippet}` block {block}: has an empty concept id");
        }
    }
    for (concept, note) in notes {
        if !concepts.contains(concept) {
            bail!(
                "snippet `{snippet}` block {block}: a note for `{concept}` on {where_}, but the concepts there are {concepts:?}"
            );
        }
        if note.trim().is_empty() {
            bail!(
                "snippet `{snippet}` block {block}: the note for `{concept}` on {where_} is empty; drop it instead"
            );
        }
    }
    Ok(())
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
concepts = ["rust.binding"]

[[blocks]]
lines = [2, 2]
ask = "what is b?"
concepts = ["rust.binding"]
"#;

    /// The same snippet with a level clause injected after the title.
    fn with_level(clause: &str) -> Snippet {
        let text = OK.replace("title = \"test\"", &format!("title = \"test\"\n{clause}"));
        Snippet::parse(&text).expect("parses")
    }

    #[test]
    fn a_snippet_can_declare_one_level_or_a_band() {
        let one = with_level("level = 1");
        assert_eq!(one.levels(), Levels { from: 1, to: 1 });
        assert_eq!(one.levels().label(), "level 1");

        let band = with_level("levels = [2, 4]");
        assert_eq!(band.levels(), Levels { from: 2, to: 4 });
        assert_eq!(band.levels().label(), "levels 2-4");
        band.validate().unwrap();
    }

    #[test]
    fn a_snippet_without_a_level_is_for_everyone() {
        let plain = with_level("");
        assert_eq!(
            plain.levels(),
            Levels {
                from: 1,
                to: MAX_LEVEL
            }
        );
        plain.validate().unwrap();
    }

    #[test]
    fn impossible_levels_are_rejected() {
        for clause in [
            "level = 0",
            "level = 6",
            "levels = [3, 2]",
            "levels = [0, 2]",
        ] {
            let snippet = with_level(clause);
            let error = snippet
                .validate()
                .expect_err(&format!("`{clause}` should be rejected"));
            assert!(
                format!("{error}").contains("out of range"),
                "{clause}: {error}"
            );
        }
    }

    #[test]
    fn level_and_levels_together_are_ambiguous() {
        let snippet = with_level("level = 1\nlevels = [1, 2]");
        let error = snippet.validate().expect_err("ambiguous");
        assert!(format!("{error}").contains("not both"), "{error}");
    }

    #[test]
    fn an_author_can_point_at_their_own_reading() {
        let snippet = with_level(
            "docs = [{ title = \"The Book\", url = \"https://doc.rust-lang.org/book/\" }]",
        );
        snippet.validate().unwrap();
        assert_eq!(snippet.docs.len(), 1);
        assert_eq!(snippet.docs[0].title, "The Book");
    }

    #[test]
    fn a_link_that_is_not_a_url_is_rejected() {
        let snippet = with_level("docs = [{ title = \"Somewhere\", url = \"doc.rust-lang.org\" }]");
        let error = snippet.validate().expect_err("not a URL");
        assert!(format!("{error}").contains("http(s) URL"), "{error}");
    }

    #[test]
    fn one_rung_of_stretch_is_reachable_and_no_more() {
        let one = Levels { from: 1, to: 1 };
        assert!(one.reachable(1));
        assert!(Levels { from: 2, to: 2 }.reachable(1));
        assert!(!Levels { from: 3, to: 4 }.reachable(1));
        assert!(Levels { from: 3, to: 4 }.reachable(2));
        // Distance: above your level costs more than revising below it.
        assert_eq!(one.distance(1), 0.0);
        assert!(Levels { from: 3, to: 3 }.distance(1) > one.distance(3));
    }

    /// Two blocks; the second carries a closing brace and an authored line prompt.
    const WITH_LINES: &str = r#"
id = "t-2"
language = "rust"
title = "line prompts"
asks = "lines"
code = '''
let mut count = 0;
count += 1;
}
'''

[[blocks]]
lines = [1, 1]
ask = "what is count?"
concepts = ["rust.binding.mut"]

[[blocks]]
lines = [2, 3]
ask = "what happens here?"
concepts = ["rust.operator.compound"]
notes = { "rust.operator.compound" = "`+=` reads, adds and writes back." }
  [[blocks.line]]
  at = 2
  ask = "Why is there an equals sign in `+=`?"
  concepts = ["rust.operator.compound"]
  notes = { "rust.operator.compound" = "`+=` assigns the sum back to the same binding." }
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
    fn ask_is_optional_and_notes_are_optional() {
        let snippet = Snippet::parse(OK).unwrap();
        assert_eq!(
            snippet.blocks[0].ask,
            "Explain what this block does, in your own words."
        );
        assert_eq!(snippet.blocks[1].ask, "what is b?");
        assert!(snippet.blocks[0].notes.is_empty());
    }

    #[test]
    fn block_mode_asks_one_prompt_per_block() {
        let snippet = Snippet::parse(OK).unwrap();
        let prompts = snippet.prompts(Asks::Blocks);
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[0].span_label(), "line 1");
        assert_eq!(prompts[1].lines, vec!["let b = 2;".to_string()]);
    }

    #[test]
    fn line_mode_asks_significant_lines_and_skips_delimiters() {
        let snippet = Snippet::parse(WITH_LINES).unwrap();
        snippet.validate().unwrap();
        let prompts = snippet.prompts(Asks::Lines);
        // Line 3 is a lone `}`: never its own question.
        assert_eq!(
            prompts.iter().map(|p| p.start).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(prompts.iter().all(Prompt::is_line));
    }

    #[test]
    fn authored_line_prompts_win_over_the_generic_question() {
        let snippet = Snippet::parse(WITH_LINES).unwrap();
        let prompts = snippet.prompts(Asks::Lines);
        assert_eq!(prompts[1].ask, "Why is there an equals sign in `+=`?");
        assert_eq!(
            prompts[1]
                .notes
                .get("rust.operator.compound")
                .map(String::as_str),
            Some("`+=` assigns the sum back to the same binding.")
        );
        // The unauthored line keeps a generic question and its block's notes.
        assert_eq!(prompts[0].ask, "What does line 1 do?");
        assert!(prompts[0].notes.is_empty());
    }

    #[test]
    fn auto_prefers_lines_only_when_the_corpus_authored_them() {
        assert_eq!(
            Snippet::parse(OK).unwrap().resolve_mode(Asks::Auto),
            Asks::Blocks
        );
        assert_eq!(
            Snippet::parse(WITH_LINES).unwrap().resolve_mode(Asks::Auto),
            Asks::Lines
        );
        assert_eq!(
            Snippet::parse(OK).unwrap().resolve_mode(Asks::Lines),
            Asks::Lines
        );
    }

    #[test]
    fn rejects_a_note_about_a_concept_the_block_does_not_declare() {
        let text = WITH_LINES.replace(
            r#"notes = { "rust.operator.compound" = "`+=` reads, adds and writes back." }"#,
            r#"notes = { "rust.other" = "nope" }"#,
        );
        let snippet = Snippet::parse(&text).unwrap();
        let err = snippet.validate().unwrap_err().to_string();
        assert!(err.contains("but the concepts there are"), "{err}");
    }

    #[test]
    fn rejects_a_line_prompt_outside_its_block() {
        let text = WITH_LINES.replace("at = 2", "at = 7");
        let snippet = Snippet::parse(&text).unwrap();
        let err = snippet.validate().unwrap_err().to_string();
        assert!(err.contains("outside the block's range"), "{err}");
    }

    #[test]
    fn rejects_a_line_prompt_for_a_concept_the_block_does_not_declare() {
        let text = WITH_LINES.replace(
            "  concepts = [\"rust.operator.compound\"]\n  notes",
            "  concepts = [\"rust.other\"]\n  notes",
        );
        let snippet = Snippet::parse(&text).unwrap();
        let err = snippet.validate().unwrap_err().to_string();
        assert!(err.contains("which the block does not declare"), "{err}");
    }

    #[test]
    fn rejects_an_empty_note() {
        let text = WITH_LINES.replace(
            r#"notes = { "rust.operator.compound" = "`+=` reads, adds and writes back." }"#,
            r#"notes = { "rust.operator.compound" = "  " }"#,
        );
        let snippet = Snippet::parse(&text).unwrap();
        let err = snippet.validate().unwrap_err().to_string();
        assert!(err.contains("is empty; drop it instead"), "{err}");
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
    fn rejects_a_block_past_the_code() {
        let text = OK.replace("lines = [2, 2]", "lines = [2, 9]");
        let snippet = Snippet::parse(&text).unwrap();
        let err = snippet.validate().unwrap_err().to_string();
        assert!(err.contains("exceeds"), "{err}");
    }

    #[test]
    fn unknown_fields_are_an_error_not_a_silent_ignore() {
        // A corpus written for 0.2 still carries `expects`; it must not load
        // quietly with its rubric silently dropped.
        let text = OK.replace(
            "concepts = [\"rust.binding\"]\n\n[[blocks]]",
            "concepts = [\"rust.binding\"]\nexpects = []\n\n[[blocks]]",
        );
        let err = Snippet::parse(&text).unwrap_err().to_string();
        assert!(err.contains("unknown field"), "{err}");
    }

    #[test]
    fn numbered_code_lines_up_with_the_ranges() {
        let snippet = Snippet::parse(OK).unwrap();
        assert_eq!(snippet.numbered_code(), " 1 | let a = 1;\n 2 | let b = 2;");
    }
}
