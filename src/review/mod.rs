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

use anyhow::{Result, bail};
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

    /// A plain request and reply, with the grading contract in front so the
    /// reviewer knows what the tool is for. Used to draft corpus entries.
    fn ask(&self, task: &str) -> Result<String> {
        let _ = task;
        bail!("this reviewer cannot draft — `--reviewer acp` or an API key can (the mock cannot)")
    }
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

/// Reads the grade out of whatever the reviewer said.
///
/// Models wrap JSON in prose, fence it, leave trailing commas, and occasionally
/// put a real newline inside a string. Any of those used to lose a finished
/// sitting, so the parse is now tolerant: every balanced object in the reply is
/// tried, then the same object after the usual repairs. Being forgiving here
/// costs nothing — the grade is checked against the prompts either way.
pub fn parse_json_object(text: &str) -> Result<RawGrade> {
    let candidates = json_candidates(text);
    if candidates.is_empty() {
        bail!(
            "the reviewer did not return a JSON object: {}",
            truncate(text.trim(), 400)
        );
    }

    let mut last_error = String::new();
    for candidate in &candidates {
        for attempt in repairs(candidate) {
            match read_grade(&attempt) {
                Ok(grade) => return Ok(grade),
                Err(error) => last_error = error,
            }
        }
    }
    // Keep the reply somewhere it can be read: this error is impossible to act
    // on from a one-line excerpt, and it is the reviewer's text, not the
    // learner's.
    let saved = std::env::temp_dir().join("readcode-last-reply.txt");
    let where_ = match std::fs::write(&saved, text) {
        Ok(()) => format!(" — the full reply is in {}", saved.display()),
        Err(_) => String::new(),
    };
    bail!(
        "the reviewer's JSON could not be parsed ({last_error}){where_}; it replied: {}",
        truncate(candidates[0].as_str(), 600)
    )
}

/// One candidate string to a grade. A model that emits a key twice (a repeated
/// `feedback`, say) makes serde's derive reject the whole object, so the text
/// round-trips through `Value` first, which collapses duplicates (last wins).
fn read_grade(body: &str) -> std::result::Result<RawGrade, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|error| format!("{error}"))?;
    let collapsed = serde_json::to_string(&value).map_err(|error| format!("{error}"))?;
    serde_json::from_str::<RawGrade>(&collapsed).map_err(|error| format!("{error}"))
}

/// The strict parse first, then the repairs models actually need: trailing
/// commas, raw newlines inside strings, and an object that stops mid-way.
fn repairs(body: &str) -> Vec<String> {
    let mut out = vec![body.to_string()];

    let commas = strip_trailing_commas(body);
    if commas != body {
        out.push(commas.clone());
    }

    let escaped = escape_control_characters(body);
    if escaped != body {
        out.push(escaped.clone());
        let both = strip_trailing_commas(&escaped);
        if both != escaped {
            out.push(both);
        }
    }

    if let Some(closed) = close_open_object(body) {
        out.push(closed.clone());
        out.push(strip_trailing_commas(&closed));
    }
    out
}

/// Every top-level `{…}` span, biggest first, with braces inside strings ignored
/// so a `}` in a code sample does not end the object early.
pub fn json_candidates(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (index, byte) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'"' => in_string = true,
            b'{' => {
                if depth == 0 {
                    start = index;
                }
                depth += 1;
            }
            b'}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    spans.push((start, index + 1));
                }
            }
            _ => {}
        }
    }

    // Nothing balanced: hand back what there is so the closing repair can try.
    if spans.is_empty() {
        match text.find('{') {
            Some(start) => vec![text[start..].trim().to_string()],
            None => Vec::new(),
        }
    } else {
        spans.sort_by_key(|(start, end)| std::cmp::Reverse(end - start));
        spans
            .into_iter()
            .map(|(start, end)| text[start..end].to_string())
            .collect()
    }
}

fn strip_trailing_commas(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let chars: Vec<char> = body.chars().collect();
    let mut in_string = false;
    let mut escaped = false;
    let mut index = 0usize;
    while index < chars.len() {
        let char_ = chars[index];
        if in_string {
            out.push(char_);
            if escaped {
                escaped = false;
            } else if char_ == '\\' {
                escaped = true;
            } else if char_ == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if char_ == '"' {
            in_string = true;
            out.push(char_);
            index += 1;
            continue;
        }
        if char_ == ',' {
            let mut look = index + 1;
            while look < chars.len() && chars[look].is_whitespace() {
                look += 1;
            }
            if look < chars.len() && (chars[look] == '}' || chars[look] == ']') {
                index += 1; // drop the comma
                continue;
            }
        }
        out.push(char_);
        index += 1;
    }
    out
}

fn escape_control_characters(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut in_string = false;
    let mut escaped = false;
    for char_ in body.chars() {
        if in_string {
            if escaped {
                escaped = false;
                out.push(char_);
                continue;
            }
            match char_ {
                '\\' => {
                    escaped = true;
                    out.push(char_);
                }
                '"' => {
                    in_string = false;
                    out.push(char_);
                }
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                other if (other as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", other as u32)),
                other => out.push(other),
            }
            continue;
        }
        if char_ == '"' {
            in_string = true;
        }
        out.push(char_);
    }
    out
}

/// A reply cut off mid-object: close the string and the containers the model left
/// open. Only worth trying when the text is unbalanced.
fn close_open_object(body: &str) -> Option<String> {
    let mut closers: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for byte in body.as_bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'"' => in_string = true,
            b'{' => closers.push('}'),
            b'[' => closers.push(']'),
            b'}' | b']' => {
                closers.pop();
            }
            _ => {}
        }
    }
    if closers.is_empty() && !in_string {
        return None;
    }

    let mut out = body.trim_end().to_string();
    // A half-written key or value is worse than a missing one: drop a dangling
    // `"key":` or `"key"` fragment before closing.
    while out.ends_with(':') || out.ends_with(',') {
        out.pop();
        let trimmed = out.trim_end().to_string();
        out = trimmed;
    }
    if in_string {
        out.push('"');
    }
    while let Some(closer) = closers.pop() {
        out.push(closer);
    }
    Some(out)
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
    fn a_trailing_comma_is_forgiven() {
        let text =
            r#"{"prompts":[{"prompt":1,"verdict":"correct","feedback":"good",},],"summary":"ok",}"#;
        let raw = parse_json_object(text).unwrap();
        assert_eq!(raw.prompts[0].verdict, "correct");
    }

    #[test]
    fn a_real_newline_inside_a_string_is_forgiven() {
        // Models write multi-line feedback with real newlines now and then.
        let text = "{\"prompts\":[{\"prompt\":1,\"verdict\":\"partial\",\"feedback\":\"the idea is there\nbut the borrow is wrong\"}],\"summary\":\"ok\"}";
        let raw = parse_json_object(text).unwrap();
        assert!(raw.prompts[0].feedback.contains("the idea is there"));
    }

    #[test]
    fn a_reply_cut_off_mid_object_still_yields_what_was_finished() {
        // Truncated by a token limit: the first prompt is complete, the second is
        // not. Recovering the first is better than losing the sitting.
        let text = r#"{"prompts":[{"prompt":1,"verdict":"correct","concepts":{"a.b":"correct"}},{"prompt":2,"verdict":"part"#;
        let raw = parse_json_object(text).unwrap();
        assert_eq!(raw.prompts.len(), 2);
        assert_eq!(raw.prompts[0].verdict, "correct");
    }

    #[test]
    fn braces_in_prose_before_the_json_do_not_confuse_the_scan() {
        let text = r#"I will use {curly braces} for the ids. Here is the grade:
{"prompts":[{"prompt":1,"verdict":"correct"}]}"#;
        let raw = parse_json_object(text).unwrap();
        assert_eq!(raw.prompts.len(), 1);
    }

    #[test]
    fn a_brace_inside_a_string_does_not_end_the_object() {
        let text = r#"{"prompts":[{"prompt":1,"verdict":"partial","teach":"the body is `if x {}` here"}],"summary":"ok"}"#;
        let raw = parse_json_object(text).unwrap();
        assert_eq!(raw.prompts[0].teach, "the body is `if x {}` here");
        assert_eq!(raw.summary, "ok");
    }

    #[test]
    fn a_missing_object_is_an_error_that_keeps_the_reply() {
        let error = parse_json_object("I cannot grade this, sorry.").unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("did not return a JSON object"),
            "{message}"
        );
        assert!(message.contains("I cannot grade this"), "{message}");
    }

    #[test]
    fn an_unparseable_object_saves_the_whole_reply_for_a_human() {
        let text = "{\"prompts\": [ {\"prompt\": 1, \"verdict\": oops} ] }";
        let error = parse_json_object(text).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("could not be parsed"), "{message}");
        let saved = std::env::temp_dir().join("readcode-last-reply.txt");
        assert!(message.contains("readcode-last-reply.txt"), "{message}");
        assert_eq!(std::fs::read_to_string(saved).unwrap(), text);
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
