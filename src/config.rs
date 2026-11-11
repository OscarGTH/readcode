//! Settings that outlive a single run.
//!
//! The reviewer, its agent command and the model used to come only from the
//! environment, which meant every new terminal had to re-export three variables
//! or nothing graded. They now have a home next to `progress.json`, written by
//! `readcode setup` (which the first run opens by itself).
//!
//! API keys are deliberately *not* here. A file that holds a secret gets copied
//! into dotfile repos and pasted into bug reports; the key stays in the
//! environment, where the user put it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    /// `acp` (your own agent) or `api` (a key in the environment).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<String>,
    /// The command that hosts an agent, e.g. `opencode acp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The model to grade with. Never left to the agent's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Base URL for the `api` reviewer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_base: Option<String>,
    /// `openai` or `anthropic`, for the `api` reviewer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_style: Option<String>,
}

impl Config {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("config.toml")
    }

    /// Missing file, unreadable file, or a file from an older shape: all mean
    /// "nothing configured yet", which sends the caller to the wizard rather
    /// than refusing to run.
    pub fn load(data_dir: &Path) -> Self {
        let path = Self::path(data_dir);
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(data_dir)
            .with_context(|| format!("cannot create {}", data_dir.display()))?;
        let path = Self::path(data_dir);
        let text = toml::to_string_pretty(self).context("cannot write the settings")?;
        let header = "\
# readcode settings — written by `readcode setup`.
# The API key is not here on purpose: it stays in READCODE_API_KEY.
";
        let temporary = path.with_extension("toml.tmp");
        std::fs::write(&temporary, header.to_string() + &text)
            .with_context(|| format!("cannot write {}", temporary.display()))?;
        std::fs::rename(&temporary, &path)
            .with_context(|| format!("cannot replace {}", path.display()))?;
        Ok(())
    }

    /// True when there is enough here to grade without asking again.
    pub fn grades(&self) -> bool {
        match self.reviewer.as_deref() {
            Some("acp") => self.agent.is_some() && self.model.is_some(),
            Some("api") => self.model.is_some(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_say_nothing_about_keys() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            reviewer: Some("acp".into()),
            agent: Some("opencode acp".into()),
            model: Some("deepseek/deepseek-v4-pro".into()),
            ..Config::default()
        };
        config.save(dir.path()).unwrap();

        let text = std::fs::read_to_string(Config::path(dir.path())).unwrap();
        assert!(text.contains("model = "), "{text}");
        assert!(
            !text.contains("sk-") && !text.contains("api_key ="),
            "the settings file must never hold a key: {text}"
        );
        assert_eq!(Config::load(dir.path()), config);
        assert!(Config::load(dir.path()).grades());
    }

    #[test]
    fn no_file_means_nothing_configured() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load(dir.path());
        assert_eq!(config, Config::default());
        assert!(!config.grades());
    }

    #[test]
    fn a_half_filled_file_asks_again_rather_than_grading_with_a_default() {
        // An agent without a model would fall back to whatever the agent prefers,
        // which is the behaviour 0.10.1 removed.
        let config = Config {
            reviewer: Some("acp".into()),
            agent: Some("opencode acp".into()),
            model: None,
            ..Config::default()
        };
        assert!(!config.grades());
    }
}
