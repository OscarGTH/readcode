//! Grading: the contract every reviewer follows, and the three implementations.
//!
//! A sitting is a list of [`Prompt`]s — blocks or single lines. Reviewers only
//! ever see prompts, so line mode and block mode use the same grading path.
//!
//! * [`api`] — BYOK HTTP client for OpenAI- or Anthropic-compatible endpoints.
//! * [`acp`] — hosts the user's own coding agent over the Agent Client Protocol,
//!   so grading runs on the agent/subscription they already pay for.

pub mod acp;
pub mod api;
pub mod contract;

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::snippet::{Prompt, Snippet};
use crate::srs::{Verdict, combine};

/// What the learner wrote for one prompt.
#[derive(Debug, Clone)]
pub struct Annotation {
    /// 0-based index into the sitting's prompts.
    pub prompt: usize,
    pub text: String,
}

impl Annotation {
    pub fn new(prompt: usize, text: impl Into<String>) -> Self {
        Self {
            prompt,
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PromptGrade {
    pub prompt: usize,
    pub verdict: Verdict,
    pub feedback: String,
    pub missing: Vec<String>,
    /// The lesson: what the code does, when the answer was not `correct`.
    pub teach: Option<String>,
    /// Per-concept verdicts, in the prompt's concept order.
    pub concepts: Vec<(String, Verdict)>,
}

#[derive(Debug, Clone)]
pub struct Grade {
    pub reviewer: String,
    pub prompts: Vec<PromptGrade>,
    pub summary: String,
}

impl Grade {
    /// One verdict per concept, folded across every prompt that carries it.
    /// Line mode grades the same concept on several lines; the worst reading
    /// wins, which is what makes a half-understood line show up as `partial`.
    pub fn concept_verdicts(&self) -> Vec<(String, Verdict)> {
        let mut grouped: BTreeMap<&str, Vec<Verdict>> = BTreeMap::new();
        for prompt in &self.prompts {
            for (concept, verdict) in &prompt.concepts {
                grouped.entry(concept.as_str()).or_default().push(*verdict);
            }
        }
        grouped
            .into_iter()
            .map(|(concept, verdicts)| (concept.to_string(), combine(&verdicts)))
            .collect()
    }

    pub fn correct_prompts(&self) -> usize {
        self.prompts
            .iter()
            .filter(|prompt| prompt.verdict == Verdict::Correct)
            .count()
    }
}

pub trait Reviewer {
    fn name(&self) -> String;

    fn grade(
        &self,
        snippet: &Snippet,
        prompts: &[Prompt],
        annotations: &[Annotation],
    ) -> Result<Grade>;
}

#[derive(Debug, Deserialize)]
pub struct RawGrade {
    /// `prompts` is the current key; `blocks` is accepted because models that
    /// saw the older contract still answer with it.
    #[serde(default, alias = "blocks")]
    pub prompts: Vec<RawPrompt>,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Deserialize)]
pub struct RawPrompt {
    #[serde(
        default,
        alias = "index",
        alias = "block",
        alias = "blockIndex",
        alias = "block_number",
        alias = "number"
    )]
    pub prompt: Option<usize>,
    #[serde(default)]
    pub verdict: String,
    #[serde(default)]
    pub concepts: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "one_or_many")]
    pub missing: Vec<String>,
    #[serde(default)]
    pub feedback: String,
    /// Some models call the lesson "explanation" or "lesson"; accept the useful
    /// spellings rather than dropping the best part of the answer.
    #[serde(
        default,
        alias = "explanation",
        alias = "lesson",
        alias = "teaching",
        alias = "what_it_does"
    )]
    pub teach: String,
}

/// Models sometimes answer `"missing": "one thing"` instead of a list.
fn one_or_many<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match Option::<OneOrMany>::deserialize(deserializer)? {
        Some(OneOrMany::One(item)) => vec![item],
        Some(OneOrMany::Many(items)) => items,
        None => Vec::new(),
    })
}

impl Grade {
    /// Turns whatever the model meant into one grade per prompt. Missing
    /// prompts become `unknown` rather than a punishment — a reviewer that
    /// forgot a prompt is not evidence the learner got it wrong.
    pub fn from_raw(prompts: &[Prompt], raw: RawGrade, reviewer: &str) -> Result<Grade> {
        if raw.prompts.is_empty() {
            bail!("the reviewer returned no prompt grades");
        }
        let positional = raw.prompts.len() == prompts.len();

        let mut graded = Vec::with_capacity(prompts.len());
        for (index, prompt) in prompts.iter().enumerate() {
            let number = index + 1;
            let raw_prompt = raw
                .prompts
                .iter()
                .find(|candidate| candidate.prompt == Some(number))
                .or_else(|| {
                    if positional {
                        raw.prompts.get(index)
                    } else {
                        None
                    }
                });

            let Some(raw_prompt) = raw_prompt else {
                graded.push(PromptGrade {
                    prompt: index,
                    verdict: Verdict::Unknown,
                    feedback: "the reviewer did not grade this prompt".to_string(),
                    missing: Vec::new(),
                    teach: None,
                    concepts: prompt
                        .concepts
                        .iter()
                        .map(|concept| (concept.clone(), Verdict::Unknown))
                        .collect(),
                });
                continue;
            };

            // A prompt may list more concepts than the annotation can address.
            // Omitted concepts were not asked of this prompt, so they are not
            // graded here — only an explicit verdict (or a prompt-level verdict
            // with no per-concept detail at all) counts.
            let concepts: Vec<(String, Verdict)> = if raw_prompt.concepts.is_empty() {
                prompt
                    .concepts
                    .iter()
                    .map(|concept| (concept.clone(), Verdict::from_label(&raw_prompt.verdict)))
                    .collect()
            } else {
                raw_prompt
                    .concepts
                    .iter()
                    .filter(|(concept, _)| prompt.concepts.contains(concept))
                    .map(|(concept, label)| (concept.clone(), Verdict::from_label(label)))
                    .collect()
            };

            let mut verdict = Verdict::from_label(&raw_prompt.verdict);
            if raw_prompt.verdict.trim().is_empty() {
                let only: Vec<Verdict> = concepts.iter().map(|(_, verdict)| *verdict).collect();
                // Nothing graded and no prompt verdict: nothing to say about it.
                verdict = if only.is_empty() {
                    Verdict::Unknown
                } else {
                    combine(&only)
                };
            }

            graded.push(PromptGrade {
                prompt: index,
                verdict,
                feedback: raw_prompt.feedback.trim().to_string(),
                missing: raw_prompt
                    .missing
                    .iter()
                    .map(|item| item.trim().to_string())
                    .filter(|item| !item.is_empty())
                    .collect(),
                concepts,
                teach: {
                    let lesson = raw_prompt.teach.trim().to_string();
                    (!lesson.is_empty()).then_some(lesson)
                },
            });
        }

        Ok(Grade {
            reviewer: reviewer.to_string(),
            prompts: graded,
            summary: raw.summary.trim().to_string(),
        })
    }
}

/// Models wrap JSON in prose or fences even when told not to. Take the outermost
/// object and ignore the rest.
pub fn parse_json_object(text: &str) -> Result<RawGrade> {
    let body = extract_json(text)?;
    // A model that emits a key twice (a repeated `feedback`, say) makes serde's
    // derive reject the whole object, and one bad key would throw away a
    // finished sitting. Round-tripping through `Value` collapses duplicates
    // (last wins) before the strict parse.
    let value: serde_json::Value = serde_json::from_str(&body)
        .with_context(|| format!("the reviewer's JSON is not valid: {}", truncate(&body, 300)))?;
    let collapsed = serde_json::to_string(&value)?;
    serde_json::from_str::<RawGrade>(&collapsed).with_context(|| {
        format!(
            "the reviewer's JSON did not match the expected shape: {}",
            truncate(&collapsed, 300)
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
    use crate::snippet::{Asks, Snippet};

    const SNIPPET: &str = r#"
id = "t-1"
language = "rust"
title = "test"
code = '''
let a = 1;
}
'''

[[blocks]]
lines = [1, 2]
ask = "a?"
concepts = ["rust.binding"]
"#;

    fn snippet() -> Snippet {
        Snippet::parse(SNIPPET).unwrap()
    }

    fn prompts(mode: Asks) -> Vec<Prompt> {
        snippet().prompts(mode)
    }

    fn grade(prompts: &[Prompt], json: &str) -> Grade {
        Grade::from_raw(prompts, parse_json_object(json).unwrap(), "test").unwrap()
    }

    #[test]
    fn extracts_json_from_fences_and_prose() {
        let text =
            "Sure! Here you go:\n```json\n{\"prompts\":[],\"summary\":\"ok\"}\n```\nHope it helps.";
        let raw = parse_json_object(text).unwrap();
        assert_eq!(raw.summary, "ok");
    }

    #[test]
    fn a_duplicate_key_keeps_the_last_value_instead_of_failing() {
        // Observed from a real agent: it repeated `feedback` in one prompt and
        // the strict parse threw the whole grade away.
        let text = r#"{"prompts":[{"prompt":1,"verdict":"partial","feedback":"first","feedback":"last"}],"summary":"ok"}"#;
        let raw = parse_json_object(text).unwrap();
        assert_eq!(raw.prompts[0].feedback, "last");
    }

    #[test]
    fn missing_accepts_a_single_string() {
        let raw = parse_json_object(r#"{"prompts":[{"prompt":1,"missing":"one thing"}]}"#).unwrap();
        assert_eq!(raw.prompts[0].missing, vec!["one thing".to_string()]);
    }

    #[test]
    fn a_missing_object_is_an_error() {
        assert!(parse_json_object("I cannot grade this.").is_err());
    }

    #[test]
    fn maps_prompts_by_number() {
        let prompts = prompts(Asks::Blocks);
        let grade = grade(
            &prompts,
            r#"{"prompts":[
                {"prompt":1,"verdict":"correct","concepts":{"rust.binding":"correct"},"feedback":"yes"}
            ],"summary":"ok"}"#,
        );
        assert_eq!(grade.prompts[0].verdict, Verdict::Correct);
        assert_eq!(grade.correct_prompts(), 1);
    }

    #[test]
    fn accepts_the_older_blocks_key() {
        let prompts = prompts(Asks::Blocks);
        let grade = grade(
            &prompts,
            r#"{"blocks":[{"block":1,"verdict":"wrong","missing":["x"],"feedback":"no"}]}"#,
        );
        assert_eq!(grade.prompts[0].verdict, Verdict::Wrong);
        assert_eq!(grade.prompts[0].missing, vec!["x".to_string()]);
    }

    #[test]
    fn an_empty_review_is_an_error_not_a_silent_pass() {
        let prompts = prompts(Asks::Lines);
        assert_eq!(prompts.len(), 1, "the lone brace is not asked");
        let raw = parse_json_object(r#"{"prompts":[]}"#).unwrap();
        assert!(Grade::from_raw(&prompts, raw, "test").is_err());
    }

    #[test]
    fn a_concept_the_reviewer_omitted_is_not_graded() {
        // Two concepts, one addressed by the answer: the other must not be
        // invented as `unknown`, or the scheduler punishes a line for a concept
        // that belongs to a sibling line.
        let snippet = Snippet::parse(
            r#"id = "t-4"
language = "rust"
title = "two concepts"
code = '''
let a = 1;
'''
[[blocks]]
lines = [1, 1]
ask = "what happens?"
concepts = ["rust.a", "rust.b"]
"#,
        )
        .unwrap();
        let prompts = snippet.prompts(Asks::Blocks);
        let grade = grade(
            &prompts,
            r#"{"prompts":[{"prompt":1,"verdict":"correct","concepts":{"rust.a":"correct"}}]}"#,
        );
        assert_eq!(
            grade.prompts[0].concepts,
            vec![("rust.a".to_string(), Verdict::Correct)]
        );
        assert_eq!(
            grade.concept_verdicts(),
            vec![("rust.a".to_string(), Verdict::Correct)]
        );
    }

    #[test]
    fn an_invented_concept_id_is_ignored() {
        let prompts = prompts(Asks::Blocks);
        let grade = grade(
            &prompts,
            r#"{"prompts":[{"prompt":1,"verdict":"correct","concepts":{"rust.invented":"correct","rust.binding":"correct"}}]}"#,
        );
        assert_eq!(
            grade.prompts[0].concepts,
            vec![("rust.binding".to_string(), Verdict::Correct)]
        );
    }

    #[test]
    fn a_missing_verdict_falls_back_to_the_concepts() {
        let prompts = prompts(Asks::Blocks);
        let grade = grade(
            &prompts,
            r#"{"prompts":[{"prompt":1,"concepts":{"rust.binding":"correct"}}]}"#,
        );
        assert_eq!(grade.prompts[0].verdict, Verdict::Correct);
    }

    #[test]
    fn unknown_concept_labels_do_not_become_correct() {
        let prompts = prompts(Asks::Blocks);
        let grade = grade(&prompts, r#"{"prompts":[{"prompt":1,"verdict":"meh"}]}"#);
        assert_eq!(grade.prompts[0].verdict, Verdict::Unknown);
    }

    #[test]
    fn a_prompt_the_reviewer_skipped_is_unknown_with_its_concepts() {
        let snippet = Snippet::parse(
            r#"
id = "t-2"
language = "rust"
title = "two"
code = '''
let a = 1;
let b = 2;
'''

[[blocks]]
lines = [1, 1]
ask = "a?"
concepts = ["rust.a"]

[[blocks]]
lines = [2, 2]
ask = "b?"
concepts = ["rust.b"]
"#,
        )
        .unwrap();
        let prompts = snippet.prompts(Asks::Blocks);
        let grade = grade(
            &prompts,
            r#"{"prompts":[{"prompt":1,"verdict":"correct"}]}"#,
        );
        assert_eq!(grade.prompts.len(), 2);
        assert_eq!(grade.prompts[1].verdict, Verdict::Unknown);
        assert_eq!(
            grade.prompts[1].concepts,
            vec![("rust.b".to_string(), Verdict::Unknown)]
        );
    }

    #[test]
    fn line_mode_folds_one_concept_across_lines() {
        let snippet = Snippet::parse(
            r#"
id = "t-3"
language = "rust"
title = "lines"
asks = "lines"
code = '''
let a = 1;
let b = 2;
'''

[[blocks]]
lines = [1, 2]
ask = "the bindings"
concepts = ["rust.binding"]
"#,
        )
        .unwrap();
        let prompts = snippet.prompts(Asks::Lines);
        assert_eq!(prompts.len(), 2);
        let grade = grade(
            &prompts,
            r#"{"prompts":[
                {"prompt":1,"verdict":"correct","concepts":{"rust.binding":"correct"}},
                {"prompt":2,"verdict":"wrong","concepts":{"rust.binding":"wrong"}}
            ]}"#,
        );
        assert_eq!(
            grade.concept_verdicts(),
            vec![("rust.binding".to_string(), Verdict::Partial)]
        );
    }
}
