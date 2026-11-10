//! The snippet corpus. Snippets are compiled into the binary so `readcode`
//! works from any directory with no data files to install; `--snippets <dir>`
//! loads a directory of TOML files instead, which is how you write your own.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::snippet::Snippet;

const EMBEDDED: &[(&str, &str)] = &[
    (
        "rust/001-ownership-move.toml",
        include_str!("../snippets/rust/001-ownership-move.toml"),
    ),
    (
        "rust/002-question-mark.toml",
        include_str!("../snippets/rust/002-question-mark.toml"),
    ),
    (
        "rust/003-iterator-chain.toml",
        include_str!("../snippets/rust/003-iterator-chain.toml"),
    ),
    (
        "rust/004-struct-impl.toml",
        include_str!("../snippets/rust/004-struct-impl.toml"),
    ),
];

#[derive(Debug, Default)]
pub struct Corpus {
    snippets: Vec<Snippet>,
}

impl Corpus {
    pub fn builtin() -> Result<Self> {
        let pairs: Vec<(String, String)> = EMBEDDED
            .iter()
            .map(|(name, text)| ((*name).to_string(), (*text).to_string()))
            .collect();
        Self::from_sources(pairs.iter().map(|(n, t)| (n.as_str(), t.as_str())))
    }

    pub fn from_dir(dir: &Path) -> Result<Self> {
        if !dir.is_dir() {
            bail!("{} is not a directory", dir.display());
        }
        let mut files = Vec::new();
        collect_toml(dir, &mut files)?;
        files.sort();
        if files.is_empty() {
            bail!("no .toml snippets found under {}", dir.display());
        }
        let mut owned = Vec::new();
        for path in &files {
            let text = fs::read_to_string(path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            owned.push((path.display().to_string(), text));
        }
        Self::from_sources(owned.iter().map(|(n, t)| (n.as_str(), t.as_str())))
    }

    /// `--snippets <dir>` when given, otherwise the embedded corpus.
    pub fn load(override_dir: Option<&Path>) -> Result<Self> {
        match override_dir {
            Some(dir) => Self::from_dir(dir),
            None => Self::builtin(),
        }
    }

    fn from_sources<'a>(sources: impl Iterator<Item = (&'a str, &'a str)>) -> Result<Self> {
        let mut snippets = Vec::new();
        for (name, text) in sources {
            let snippet = Snippet::parse(text).with_context(|| format!("in {name}"))?;
            snippet
                .validate()
                .with_context(|| format!("invalid snippet in {name}"))?;
            snippets.push(snippet);
        }
        snippets.sort_by(|a, b| a.id.cmp(&b.id));
        for pair in snippets.windows(2) {
            if pair[0].id == pair[1].id {
                bail!("two snippets share the id `{}`", pair[0].id);
            }
        }
        Ok(Self { snippets })
    }

    pub fn snippets(&self) -> &[Snippet] {
        &self.snippets
    }

    pub fn is_empty(&self) -> bool {
        self.snippets.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&Snippet> {
        self.snippets.iter().find(|s| s.id == id)
    }

    /// Accepts a full id, or any unambiguous suffix of one (`002`, `...question-mark`).
    pub fn find(&self, needle: &str) -> Result<&Snippet> {
        let needle = needle.trim();
        if let Some(exact) = self.get(needle) {
            return Ok(exact);
        }
        let matches: Vec<&Snippet> = self
            .snippets
            .iter()
            .filter(|s| {
                s.id.ends_with(needle) || s.title.to_lowercase().contains(&needle.to_lowercase())
            })
            .collect();
        match matches.len() {
            0 => bail!("no snippet matches `{needle}`"),
            1 => Ok(matches[0]),
            _ => bail!(
                "`{needle}` matches several snippets: {}",
                matches
                    .iter()
                    .map(|s| s.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// Everything wrong with the corpus, for `readcode check`. Empty is good.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.snippets.is_empty() {
            problems.push("the corpus is empty".to_string());
        }
        for snippet in &self.snippets {
            if let Err(err) = snippet.validate() {
                problems.push(format!("{}: {err}", snippet.id));
            }
            for block in &snippet.blocks {
                for concept in &block.concepts {
                    if !block.expects.iter().any(|e| &e.concept == concept) {
                        problems.push(format!(
                            "{}: concept `{concept}` has no keywords, only an LLM can grade it",
                            snippet.id
                        ));
                    }
                }
            }
        }
        problems
    }
}

fn collect_toml(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        fs::read_dir(dir).with_context(|| format!("cannot read directory {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_toml(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_corpus_is_valid() {
        let corpus = Corpus::builtin().unwrap();
        assert!(corpus.snippets().len() >= 4);
        assert_eq!(corpus.problems(), Vec::<String>::new());
        for snippet in corpus.snippets() {
            assert_eq!(snippet.language, "rust");
            assert!(!snippet.title.is_empty());
            // Every snippet must actually teach something.
            assert!(snippet.concepts().len() >= 2);
        }
    }

    #[test]
    fn find_accepts_a_suffix() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("002").unwrap();
        assert!(snippet.id.starts_with("rust-002"), "{}", snippet.id);
        assert!(corpus.find("ownership").is_ok());
        assert!(corpus.find("nope-nope").is_err());
    }

    #[test]
    fn loads_a_directory_and_orders_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let a = fixture("rust-900", "one");
        let b = fixture("rust-100", "two");
        fs::write(dir.path().join("b.toml"), b).unwrap();
        fs::write(dir.path().join("a.toml"), a).unwrap();
        let corpus = Corpus::from_dir(dir.path()).unwrap();
        assert_eq!(corpus.snippets()[0].id, "rust-100");
        assert_eq!(corpus.snippets()[1].id, "rust-900");
    }

    pub(super) fn fixture(id: &str, title: &str) -> String {
        format!(
            r#"
id = "{id}"
language = "rust"
title = "{title}"
code = '''
let x = 1;
'''

[[blocks]]
lines = [1, 1]
ask = "what is x?"
concepts = ["rust.binding"]
  [[blocks.expects]]
  concept = "rust.binding"
  keywords = ["binding"]
"#
        )
    }
}
