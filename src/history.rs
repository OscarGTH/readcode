//! Past sittings, so the review is not a thing you only see once.
//!
//! A grade used to exist for exactly as long as the sitting did: the verdicts
//! folded into mastery and the page you read them on was gone. This keeps the
//! page — the code, what you answered, what was said about it, and when — so it
//! can be read again a week later, which is when it is actually useful.
//!
//! The snippet is stored with the sitting rather than referenced: a corpus can be
//! edited or replaced (`--corpus`, an upgrade) and a history that renders
//! "snippet not found" is not a history.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::review::Grade;
use crate::snippet::{Asks, Snippet};

/// How many sittings to keep. Enough to look back weeks; small enough that the
/// file stays a file.
const KEEP: usize = 200;

/// One prompt as it was asked: the span it covered, the question, the concepts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredPrompt {
    pub start: usize,
    pub end: usize,
    pub ask: String,
    pub concepts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sitting {
    /// Unix seconds, when the answers were graded.
    pub at: u64,
    pub snippet: String,
    pub title: String,
    pub language: String,
    pub code: String,
    /// `lines` or `blocks`, as the prompts were actually asked.
    pub mode: String,
    /// The questions as asked, in order: the grade does not carry them, and
    /// without them the page cannot say what an answer was an answer to.
    pub prompts: Vec<StoredPrompt>,
    pub answers: Vec<String>,
    pub grade: Grade,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct History {
    #[serde(default)]
    pub sittings: Vec<Sitting>,
}

impl Sitting {
    pub fn new(snippet: &Snippet, mode: Asks, grade: Grade, answers: Vec<String>, at: u64) -> Self {
        let prompts = snippet
            .prompts(mode)
            .into_iter()
            .map(|prompt| StoredPrompt {
                start: prompt.start,
                end: prompt.end,
                ask: prompt.ask,
                concepts: prompt.concepts,
            })
            .collect();
        Self {
            at,
            snippet: snippet.id.clone(),
            title: snippet.title.clone(),
            language: snippet.language.clone(),
            code: snippet.code.clone(),
            mode: match mode {
                Asks::Lines => "lines".to_string(),
                _ => "blocks".to_string(),
            },
            prompts,
            answers,
            grade,
        }
    }

    pub fn asks(&self) -> Asks {
        match self.mode.as_str() {
            "lines" => Asks::Lines,
            _ => Asks::Blocks,
        }
    }

    /// The snippet as it was, rebuilt from the stored code and prompts.
    ///
    /// Rebuilt as TOML rather than assembled by hand so it goes through the same
    /// parser and validation every other snippet does — one code path, no struct
    /// reconstruction to keep in step with the real one.
    pub fn snippet(&self) -> Snippet {
        let mut blocks = String::new();
        for prompt in &self.prompts {
            let ask = format!(
                "ask = \"{}\"\n",
                prompt.ask.replace('\\', "\\\\").replace('"', "\\\"")
            );
            blocks.push_str(&format!(
                "\n[[blocks]]\nlines = [{}, {}]\n{ask}concepts = [\"showing\"]\n",
                prompt.start, prompt.end
            ));
        }
        let toml = format!(
            "id = \"{}\"\nlanguage = \"{}\"\ntitle = \"{}\"\ncode = '''\n{}\n'''\n{}",
            self.snippet,
            self.language,
            self.title.replace('"', "\\\""),
            self.code.trim_end_matches('\n'),
            blocks
        );
        Snippet::parse(&toml).unwrap_or_else(|_| {
            // Unparseable history is still history: fall back to a snippet made
            // of the code alone, so the page shows something rather than an error.
            Snippet::parse(&format!(
                "id = \"{}\"\nlanguage = \"{}\"\ntitle = \"{}\"\ncode = '''\n{}\n'''\n\n[[blocks]]\nlines = [1, {}]\nconcepts = [\"showing\"]\n",
                self.snippet,
                self.language,
                self.title.replace('"', "\\\""),
                self.code.trim_end_matches('\n'),
                self.code.lines().count().max(1)
            ))
            .expect("a snippet of one block always parses")
        })
    }

    /// The prompts as they were asked, for rendering the page again.
    pub fn prompts(&self) -> Vec<crate::snippet::Prompt> {
        self.snippet().prompts(self.asks())
    }
}

impl History {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("history.json")
    }

    pub fn load(data_dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(data_dir))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(data_dir)
            .with_context(|| format!("cannot create {}", data_dir.display()))?;
        let path = Self::path(data_dir);
        let text = serde_json::to_string_pretty(self).context("cannot write the history")?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, text)
            .with_context(|| format!("cannot write {}", temporary.display()))?;
        std::fs::rename(&temporary, &path)
            .with_context(|| format!("cannot replace {}", path.display()))?;
        Ok(())
    }

    /// Newest first, capped: the file is a record, not an archive.
    pub fn record(&mut self, sitting: Sitting) {
        self.sittings.insert(0, sitting);
        self.sittings.truncate(KEEP);
    }
}

/// `2 correct · 1 partial`, for a list where the marks matter more than the text.
pub fn summary(grade: &Grade) -> String {
    let mut correct = 0;
    let mut partial = 0;
    let mut wrong = 0;
    let mut unknown = 0;
    for graded in &grade.prompts {
        match graded.verdict {
            crate::srs::Verdict::Correct => correct += 1,
            crate::srs::Verdict::Partial => partial += 1,
            crate::srs::Verdict::Wrong => wrong += 1,
            crate::srs::Verdict::Unknown => unknown += 1,
        }
    }
    let mut parts = Vec::new();
    for (count, name) in [
        (correct, "correct"),
        (partial, "partial"),
        (wrong, "wrong"),
        (unknown, "unknown"),
    ] {
        if count > 0 {
            parts.push(format!("{count} {name}"));
        }
    }
    if parts.is_empty() {
        "no verdicts".to_string()
    } else {
        parts.join(" · ")
    }
}

/// `2 hours ago · 2026-11-11 08:14`, so a page can be dated at a glance and
/// placed exactly when it matters.
pub fn when(at: u64, now: u64) -> String {
    let ago = now.saturating_sub(at);
    let relative = match ago {
        0..=45 => "just now".to_string(),
        46..=5399 => format!("{} minutes ago", (ago + 30) / 60),
        5400..=86_399 => format!("{} hours ago", (ago + 1800) / 3600),
        86_400..=172_799 => "yesterday".to_string(),
        _ => format!("{} days ago", ago / 86_400),
    };
    format!("{relative} · {}", stamp(at))
}

/// UTC `YYYY-MM-DD HH:MM`, computed rather than pulled in: one date format does
/// not justify a dependency on a small machine.
pub fn stamp(at: u64) -> String {
    let (days, seconds) = ((at / 86_400) as i64, at % 86_400);
    let (hour, minute) = (seconds / 3600, (seconds % 3600) / 60);
    // Howard Hinnant's days-from-civil, inverted.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timestamp_reads_as_a_date() {
        // 2026-11-11 08:14 UTC
        assert_eq!(stamp(1_794_384_840), "2026-11-11 08:14");
    }

    #[test]
    fn recent_sittings_say_how_long_ago() {
        let now = 1_794_384_840;
        assert_eq!(when(now, now), "just now · 2026-11-11 08:14");
        assert!(when(now - 7_200, now).starts_with("2 hours ago"));
        assert!(when(now - 86_400, now).starts_with("yesterday"));
        assert!(when(now - 3 * 86_400, now).starts_with("3 days ago"));
    }

    fn a_grade() -> Grade {
        use crate::srs::Verdict;
        Grade {
            reviewer: "acp:opencode (test)".into(),
            summary: "fine".into(),
            prompts: vec![
                crate::review::PromptGrade {
                    prompt: 0,
                    verdict: Verdict::Correct,
                    feedback: "yes".into(),
                    missing: vec![],
                    teach: Some("a lesson".into()),
                    concepts: vec![("rust.binding.immutable".into(), Verdict::Correct)],
                },
                crate::review::PromptGrade {
                    prompt: 1,
                    verdict: Verdict::Partial,
                    feedback: "partly".into(),
                    missing: vec!["the type".into()],
                    teach: None,
                    concepts: vec![("rust.type.inference".into(), Verdict::Partial)],
                },
            ],
        }
    }

    fn a_sitting() -> Sitting {
        let corpus = crate::corpus::Corpus::builtin().unwrap();
        let snippet = corpus.find("rust-006").unwrap();
        Sitting::new(
            snippet,
            Asks::Blocks,
            a_grade(),
            vec!["first answer".into(), "second answer".into()],
            1_794_384_840,
        )
    }

    #[test]
    fn a_sitting_round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = History::default();
        history.record(a_sitting());
        history.save(dir.path()).unwrap();

        let loaded = History::load(dir.path());
        assert_eq!(loaded.sittings.len(), 1);
        let sitting = &loaded.sittings[0];
        assert_eq!(sitting.at, 1_794_384_840);
        assert_eq!(sitting.snippet, "rust-006");
        assert_eq!(sitting.answers, vec!["first answer", "second answer"]);
        assert_eq!(sitting.grade.prompts.len(), 2);
        assert_eq!(
            sitting.grade.prompts[1].verdict,
            crate::srs::Verdict::Partial
        );
    }

    #[test]
    fn the_page_can_be_rebuilt_from_what_was_stored() {
        // Rebuilding means the questions and spans come back, not just the code:
        // a review without the question beside the answer is not the page.
        let sitting = a_sitting();
        let snippet = sitting.snippet();
        assert_eq!(snippet.id, "rust-006");
        assert!(snippet.code.contains("let apples = 3;"));

        let prompts = sitting.prompts();
        let original = crate::corpus::Corpus::builtin()
            .unwrap()
            .find("rust-006")
            .unwrap()
            .prompts(Asks::Blocks);
        assert_eq!(prompts.len(), original.len());
        for (rebuilt, original) in prompts.iter().zip(&original) {
            assert_eq!(rebuilt.ask, original.ask);
            assert_eq!(rebuilt.start, original.start);
            assert_eq!(rebuilt.end, original.end);
        }
    }

    #[test]
    fn the_newest_sitting_is_first_and_the_file_is_capped() {
        let mut history = History::default();
        for at in 1..=(KEEP as u64 + 5) {
            let mut sitting = a_sitting();
            sitting.at = at;
            history.record(sitting);
        }
        assert_eq!(history.sittings.len(), KEEP);
        assert_eq!(history.sittings[0].at, KEEP as u64 + 5);
    }

    #[test]
    fn a_summary_counts_only_what_happened() {
        assert_eq!(summary(&a_grade()), "1 correct · 1 partial");
    }
}
