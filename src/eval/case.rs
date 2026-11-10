//! Eval cases: one TOML file per behaviour we require of a reviewer.
//!
//! A case carries a snippet id, the learner annotations to feed the reviewer,
//! and what the reviewer must say about them. The point is not volume: each case
//! is a bug that would otherwise ship — a misconception credited, a paraphrase
//! punished, "I don't know" treated as wrong.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::snippet::Asks;
use crate::srs::Verdict;

#[derive(Debug, Clone, Deserialize)]
pub struct Case {
    pub id: String,
    #[serde(default)]
    pub description: String,
    /// Snippet id in the corpus, exact or an unambiguous suffix.
    pub snippet: String,
    /// Granularity to ask at; defaults to what the corpus prefers.
    #[serde(default)]
    pub mode: Option<Asks>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// A failure here is a broken promise, not a noisy model.
    #[serde(default)]
    pub must_pass: bool,
    #[serde(default)]
    pub annotations: Vec<CaseAnnotation>,
    #[serde(default)]
    pub expect: Vec<Expectation>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CaseAnnotation {
    /// 1-based prompt number.
    pub prompt: usize,
    pub text: String,
}

/// One assertion about one prompt, or about one concept inside it.
#[derive(Debug, Clone, Deserialize)]
pub struct Expectation {
    /// 1-based prompt number.
    pub prompt: usize,
    /// Absent means "the prompt's overall verdict".
    #[serde(default)]
    pub concept: Option<String>,
    /// The exact verdict required.
    #[serde(default)]
    pub verdict: Option<String>,
    /// Verdicts that must not appear (e.g. a misconception must not be credited).
    #[serde(default, alias = "not_verdict")]
    pub not: Vec<String>,
    /// Require the reviewer to explain the code, not just mark it. An answer of
    /// "I don't know" is exactly when the lesson matters most.
    #[serde(default)]
    pub teaches: bool,
}

impl Expectation {
    pub fn required(&self) -> Result<Verdict> {
        let label = self
            .verdict
            .as_deref()
            .context("expectation has neither `verdict` nor `not`")?;
        parse_verdict(label)
    }

    pub fn forbidden(&self) -> Result<Vec<Verdict>> {
        self.not.iter().map(|label| parse_verdict(label)).collect()
    }

    pub fn describe(&self) -> String {
        let target = match &self.concept {
            Some(concept) => format!("prompt {} `{concept}`", self.prompt),
            None => format!("prompt {}", self.prompt),
        };
        if let Some(verdict) = &self.verdict {
            format!("{target} must be `{verdict}`")
        } else {
            format!("{target} must not be `{}`", self.not.join("` or `"))
        }
    }
}

fn parse_verdict(label: &str) -> Result<Verdict> {
    let trimmed = label.trim().to_ascii_lowercase();
    if !matches!(
        trimmed.as_str(),
        "correct" | "partial" | "wrong" | "unknown"
    ) {
        bail!("`{label}` is not one of correct, partial, wrong, unknown");
    }
    Ok(Verdict::from_label(&trimmed))
}

impl Case {
    pub fn parse(text: &str) -> Result<Self> {
        let case: Case = toml::from_str(text)?;
        Ok(case)
    }

    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            bail!("case has an empty id");
        }
        if self.expect.is_empty() {
            bail!("case `{}` asserts nothing", self.id);
        }
        if self.annotations.is_empty() {
            bail!("case `{}` has no annotations to grade", self.id);
        }
        for expectation in &self.expect {
            if expectation.prompt == 0 {
                bail!("case `{}`: prompts are 1-based", self.id);
            }
            // `not` empty means "not set", so the pair reads as (verdict, not-set).
            match (&expectation.verdict, expectation.not.is_empty()) {
                (Some(_), false) => bail!(
                    "case `{}`: expectation for prompt {} has both `verdict` and `not`",
                    self.id,
                    expectation.prompt
                ),
                // A teaching-only expectation is legitimate: it asserts that
                // the reviewer explained the code, whatever the verdict was.
                (None, true) if !expectation.teaches => bail!(
                    "case `{}`: expectation for prompt {} needs `verdict`, `not` or `teaches`",
                    self.id,
                    expectation.prompt
                ),
                _ => {}
            }
            if expectation.verdict.is_some() {
                expectation.required()?;
            }
            expectation.forbidden()?;
        }
        let mut seen = BTreeSet::new();
        for annotation in &self.annotations {
            if annotation.prompt == 0 {
                bail!("case `{}`: prompts are 1-based", self.id);
            }
            if !seen.insert(annotation.prompt) {
                bail!(
                    "case `{}`: two annotations for prompt {}",
                    self.id,
                    annotation.prompt
                );
            }
        }
        Ok(())
    }
}

/// Loads every `*.toml` under `dir`, recursively, ordered by id.
pub fn load_dir(dir: &Path) -> Result<Vec<Case>> {
    if !dir.is_dir() {
        bail!("{} is not a case directory", dir.display());
    }
    let mut files = Vec::new();
    collect(dir, &mut files)?;
    files.sort();
    if files.is_empty() {
        bail!("no cases found under {}", dir.display());
    }

    let mut sources = Vec::new();
    for path in files {
        let text =
            fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
        sources.push((path.display().to_string(), text));
    }
    load_sources(
        sources
            .iter()
            .map(|(name, text)| (name.as_str(), text.as_str())),
    )
}

/// Parses a set of named case documents and checks ids are unique.
fn load_sources<'a>(sources: impl Iterator<Item = (&'a str, &'a str)>) -> Result<Vec<Case>> {
    let mut cases = Vec::new();
    for (name, text) in sources {
        let case = Case::parse(text).with_context(|| format!("in {name}"))?;
        case.validate()
            .with_context(|| format!("invalid case in {name}"))?;
        cases.push(case);
    }
    cases.sort_by(|a, b| a.id.cmp(&b.id));
    for pair in cases.windows(2) {
        if pair[0].id == pair[1].id {
            bail!("two cases share the id `{}`", pair[0].id);
        }
    }
    Ok(cases)
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        fs::read_dir(dir).with_context(|| format!("cannot read directory {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out)?;
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("toml") {
            out.push(path);
        }
    }
    Ok(())
}

/// The eval cases ship in the binary too, so `readcode eval` works from a
/// Homebrew install where `evals/cases/` does not exist.
const EMBEDDED: &[(&str, &str)] = &[
    (
        "blank-is-not-wrong.toml",
        include_str!("../../evals/cases/blank-is-not-wrong.toml"),
    ),
    (
        "credits-a-paraphrase.toml",
        include_str!("../../evals/cases/credits-a-paraphrase.toml"),
    ),
    (
        "line-level-entry-api.toml",
        include_str!("../../evals/cases/line-level-entry-api.toml"),
    ),
    (
        "never-credits-a-misconception.toml",
        include_str!("../../evals/cases/never-credits-a-misconception.toml"),
    ),
    (
        "off-topic-is-wrong.toml",
        include_str!("../../evals/cases/off-topic-is-wrong.toml"),
    ),
    (
        "says-i-dont-know.toml",
        include_str!("../../evals/cases/says-i-dont-know.toml"),
    ),
    (
        "teaches-when-the-answer-is-unknown.toml",
        include_str!("../../evals/cases/teaches-when-the-answer-is-unknown.toml"),
    ),
];

#[cfg(test)]
mod embedded_tests {
    use super::*;

    /// Every case file on disk has to be in the binary too: an installed
    /// `readcode eval` runs from a machine with no checkout, and shipping a
    /// suite that silently lost a case is worse than shipping none.
    #[test]
    fn every_case_on_disk_is_embedded() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("evals/cases");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let name = entry.file_name().to_string_lossy().to_string();
                name.ends_with(".toml").then_some(name)
            })
            .collect();
        on_disk.sort();

        let mut embedded: Vec<String> = EMBEDDED.iter().map(|(name, _)| name.to_string()).collect();
        embedded.sort();

        assert_eq!(on_disk, embedded);
    }
}

/// Loads cases from a directory when one is given (or when `./evals/cases`
/// exists), otherwise from the embedded copy.
pub fn load(dir: Option<&std::path::Path>) -> Result<Vec<Case>> {
    match dir {
        Some(dir) => load_dir(dir),
        None => {
            let default = std::path::Path::new("evals/cases");
            if default.is_dir() {
                load_dir(default)
            } else {
                load_embedded()
            }
        }
    }
}

pub fn load_embedded() -> Result<Vec<Case>> {
    load_sources(EMBEDDED.iter().map(|(name, text)| (*name, *text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CASE: &str = r#"
id = "says-i-dont-know"
description = "Not knowing is not the same as being wrong."
snippet = "rust-005"
mode = "lines"
must_pass = true
tags = ["pedagogy"]

[[annotations]]
prompt = 1
text = "I don't know."

[[expect]]
prompt = 1
concept = "rust.use.import"
verdict = "unknown"
"#;

    #[test]
    fn parses_and_validates() {
        let case = Case::parse(CASE).unwrap();
        case.validate().unwrap();
        assert!(case.must_pass);
        assert_eq!(case.mode, Some(Asks::Lines));
        assert_eq!(case.expect[0].required().unwrap(), Verdict::Unknown);
    }

    #[test]
    fn rejects_an_expectation_that_asserts_nothing() {
        let text = CASE.replace("verdict = \"unknown\"", "");
        let case = Case::parse(&text).unwrap();
        let err = case.validate().unwrap_err().to_string();
        assert!(err.contains("needs `verdict`, `not` or `teaches`"), "{err}");
    }

    #[test]
    fn rejects_a_typo_in_a_verdict() {
        let text = CASE.replace("verdict = \"unknown\"", "verdict = \"unknwon\"");
        let case = Case::parse(&text).unwrap();
        let err = case.validate().unwrap_err().to_string();
        assert!(err.contains("not one of"), "{err}");
    }

    #[test]
    fn rejects_both_verdict_and_not() {
        let text = CASE.replace(
            "verdict = \"unknown\"",
            "verdict = \"unknown\"\nnot = [\"wrong\"]",
        );
        let case = Case::parse(&text).unwrap();
        let err = case.validate().unwrap_err().to_string();
        assert!(err.contains("both"), "{err}");
    }

    #[test]
    fn the_embedded_cases_load_and_validate() {
        let cases = super::load_embedded().unwrap();
        assert_eq!(
            cases.len(),
            7,
            "{:?}",
            cases.iter().map(|c| &c.id).collect::<Vec<_>>()
        );
        assert!(cases.iter().any(|case| case.must_pass));
    }

    #[test]
    fn rejects_a_case_with_no_annotations() {
        let text = CASE.replace(
            "[[annotations]]\nprompt = 1\ntext = \"I don't know.\"\n",
            "",
        );
        let case = Case::parse(&text).unwrap();
        assert!(case.validate().is_err());
    }
}
