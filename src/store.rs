//! The progress file: one small JSON document, written atomically, that holds
//! mastery per concept plus a rolling log of the last few hundred verdicts.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::snippet::Asks;
use crate::srs::{ConceptState, Verdict};

const MAX_EVENTS: usize = 500;
const FILE_NAME: &str = "progress.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub at: u64,
    pub snippet: String,
    /// 0-based index into the sitting's prompts. Older progress files call it
    /// `block`; both are read.
    #[serde(default, alias = "block")]
    pub prompt: usize,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    #[serde(default = "current_version")]
    pub version: u32,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
    #[serde(default)]
    pub sessions: u32,
    /// The learner's declared level, 1..=5. Unset until they say, which keeps
    /// the picker unfiltered for anyone who never answers the question.
    #[serde(default)]
    pub level: Option<u8>,
    /// The snippet shown last, so the next pick can be a different one.
    #[serde(default)]
    pub last_snippet: Option<String>,
    #[serde(default)]
    pub seen_snippets: Vec<String>,
    /// The granularity each snippet was last asked at, so the next visit can
    /// alternate between blocks and lines.
    #[serde(default)]
    pub last_modes: BTreeMap<String, Asks>,
    #[serde(default)]
    pub concepts: BTreeMap<String, ConceptState>,
    #[serde(default)]
    pub events: Vec<Event>,
}

fn current_version() -> u32 {
    1
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            version: current_version(),
            created_at: 0,
            updated_at: 0,
            sessions: 0,
            level: None,
            last_snippet: None,
            seen_snippets: Vec::new(),
            last_modes: BTreeMap::new(),
            concepts: BTreeMap::new(),
            events: Vec::new(),
        }
    }
}

impl Progress {
    /// Reads progress, or starts fresh when the file does not exist yet.
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("{} is not valid readcode progress", path.display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Progress::default()),
            Err(err) => Err(err).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    /// Write to a sibling temp file, then rename, so a killed process cannot
    /// leave half a progress file behind.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self)?;
        fs::write(&tmp, text).with_context(|| format!("cannot write {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| format!("cannot replace {}", path.display()))?;
        Ok(())
    }

    pub fn concept_mut(&mut self, id: &str) -> &mut ConceptState {
        self.concepts
            .entry(id.to_string())
            .or_insert_with(|| ConceptState::new(id))
    }

    pub fn record(&mut self, event: Event) {
        self.events.push(event);
        if self.events.len() > MAX_EVENTS {
            let drop = self.events.len() - MAX_EVENTS;
            self.events.drain(0..drop);
        }
    }

    pub fn set_last_mode(&mut self, snippet_id: &str, mode: Asks) {
        self.last_modes.insert(snippet_id.to_string(), mode);
    }

    pub fn last_mode(&self, snippet_id: &str) -> Option<Asks> {
        self.last_modes.get(snippet_id).copied()
    }

    pub fn seen(&self, snippet_id: &str) -> bool {
        self.seen_snippets.iter().any(|s| s == snippet_id)
    }

    /// Concept ids this profile has met at least once.
    pub fn known_concepts(&self) -> usize {
        self.concepts.values().filter(|c| c.reps > 0).count()
    }
}

/// Where progress lives: `$READCODE_DATA_DIR`, else `$XDG_DATA_HOME/readcode`,
/// else `~/.local/share/readcode`.
pub fn default_data_dir() -> Result<PathBuf> {
    if let Some(dir) = env_dir("READCODE_DATA_DIR") {
        return Ok(dir);
    }
    if let Some(dir) = env_dir("XDG_DATA_HOME") {
        return Ok(dir.join("readcode"));
    }
    if let Some(home) = env_dir("HOME") {
        return Ok(home.join(".local/share/readcode"));
    }
    anyhow::bail!("cannot locate a data directory: set HOME or READCODE_DATA_DIR")
}

pub fn progress_path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

fn env_dir(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::srs::Verdict;

    #[test]
    fn missing_file_is_a_fresh_profile() {
        let dir = tempfile::tempdir().unwrap();
        let progress = Progress::load(&progress_path(dir.path())).unwrap();
        assert_eq!(progress.sessions, 0);
        assert!(progress.concepts.is_empty());
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = progress_path(dir.path());
        let mut progress = Progress::default();
        progress
            .concept_mut("rust.ownership.move")
            .apply(Verdict::Correct, 100);
        progress.sessions = 1;
        progress.seen_snippets.push("rust-001".into());
        progress.record(Event {
            at: 100,
            snippet: "rust-001".into(),
            prompt: 1,
            verdict: Verdict::Correct,
        });
        progress.save(&path).unwrap();

        let loaded = Progress::load(&path).unwrap();
        assert_eq!(loaded.sessions, 1);
        assert_eq!(loaded.concepts.len(), 1);
        assert_eq!(loaded.events.len(), 1);
        assert!(loaded.seen("rust-001"));
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn event_log_is_capped() {
        let mut progress = Progress::default();
        for i in 0..MAX_EVENTS + 10 {
            progress.record(Event {
                at: i as u64,
                snippet: "s".into(),
                prompt: 1,
                verdict: Verdict::Correct,
            });
        }
        assert_eq!(progress.events.len(), MAX_EVENTS);
        assert_eq!(progress.events[0].at, 10);
    }

    #[test]
    fn corrupt_file_is_an_error_not_a_silent_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = progress_path(dir.path());
        fs::write(&path, "{not json").unwrap();
        assert!(Progress::load(&path).is_err());
    }
}
