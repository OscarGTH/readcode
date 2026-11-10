//! Running a case against a reviewer, scoring it, and writing the run to disk.
//!
//! Three targets share this runner:
//!  * `mock` — answers from the case's own expectations. It proves the harness
//!    and the scoring arithmetic, and it is the only eval cheap enough for CI.
//!  * `local` — the offline rubric. Deterministic, so it is a regression guard.
//!  * `api` / `acp` — a real reviewer. Costs tokens, run it deliberately.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::case::{Case, Expectation};
use crate::corpus::Corpus;
use crate::review::{Annotation, Grade, PromptGrade, Reviewer};
use crate::snippet::{Asks, Prompt, Snippet};
use crate::srs::Verdict;

#[derive(Debug, Clone)]
pub struct CaseResult {
    pub id: String,
    pub description: String,
    pub snippet: String,
    pub mode: Asks,
    pub trial: u32,
    pub passed: bool,
    pub must_pass: bool,
    pub assertions_passed: usize,
    pub assertions_total: usize,
    pub failures: Vec<String>,
    pub duration_ms: u128,
    pub reviewer: String,
    pub summary: String,
    pub error: Option<String>,
    pub trace: Value,
}

impl CaseResult {
    fn failed(id: &str, trial: u32, error: String, duration_ms: u128) -> Self {
        Self {
            id: id.to_string(),
            description: String::new(),
            snippet: String::new(),
            mode: Asks::Auto,
            trial,
            passed: false,
            must_pass: false,
            assertions_passed: 0,
            assertions_total: 0,
            failures: vec![error.clone()],
            duration_ms,
            reviewer: String::new(),
            summary: String::new(),
            error: Some(error),
            trace: Value::Null,
        }
    }
}

pub struct RunSummary {
    pub run_id: String,
    pub target: String,
    pub started: u64,
    pub finished: u64,
    pub trials: u32,
    pub results: Vec<CaseResult>,
    pub dir: Option<PathBuf>,
}

impl RunSummary {
    /// One line per case, judged by majority across trials.
    pub fn passes(&self) -> Vec<(&str, bool, bool, usize, usize, u128)> {
        let mut by_case: BTreeMap<&str, Vec<&CaseResult>> = BTreeMap::new();
        for result in &self.results {
            by_case.entry(result.id.as_str()).or_default().push(result);
        }
        by_case
            .into_iter()
            .map(|(id, trials)| {
                let won = trials.iter().filter(|result| result.passed).count();
                let passed = won * 2 > trials.len();
                let must_pass = trials.iter().any(|result| result.must_pass);
                let assertions_total = trials
                    .iter()
                    .map(|result| result.assertions_total)
                    .max()
                    .unwrap_or(0);
                let assertions_passed = trials
                    .iter()
                    .filter(|result| result.passed)
                    .map(|result| result.assertions_passed)
                    .max()
                    .unwrap_or(0);
                let duration_ms = trials
                    .iter()
                    .map(|result| result.duration_ms)
                    .max()
                    .unwrap_or(0);
                (
                    id,
                    passed,
                    must_pass,
                    assertions_passed,
                    assertions_total,
                    duration_ms,
                )
            })
            .collect()
    }

    pub fn case_count(&self) -> usize {
        self.passes().len()
    }

    pub fn passed_count(&self) -> usize {
        self.passes()
            .iter()
            .filter(|(_, passed, ..)| *passed)
            .count()
    }

    pub fn score(&self) -> f64 {
        let total = self.case_count();
        if total == 0 {
            return 0.0;
        }
        self.passed_count() as f64 / total as f64
    }

    pub fn assertion_rate(&self) -> (usize, usize) {
        self.passes().iter().fold((0, 0), |(passed, total), case| {
            (passed + case.3, total + case.4)
        })
    }

    pub fn must_pass_failures(&self) -> Vec<String> {
        self.passes()
            .into_iter()
            .filter(|(_, passed, must_pass, ..)| *must_pass && !passed)
            .map(|(id, ..)| id.to_string())
            .collect()
    }

    /// The terminal report: per case, then the totals.
    pub fn report(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "eval · {} · {} cases · {} trial{}\n",
            self.target,
            self.case_count(),
            self.trials,
            if self.trials == 1 { "" } else { "s" }
        ));

        for (id, passed, must_pass, assertions_passed, assertions_total, duration_ms) in
            self.passes()
        {
            let mark = if passed { "✓" } else { "✗" };
            let flag = if must_pass { " must-pass" } else { "" };
            out.push_str(&format!(
                "  {mark} {:<38} {}/{} assertions{:>9}ms{flag}\n",
                id, assertions_passed, assertions_total, duration_ms
            ));
            if !passed {
                for result in self.results.iter().filter(|result| result.id == id) {
                    for failure in &result.failures {
                        out.push_str(&format!("      {failure}\n"));
                    }
                }
            }
        }

        let (assertions_passed, assertions_total) = self.assertion_rate();
        out.push_str(&format!(
            "\nscore {:.2} ({}/{} cases, {}/{} assertions)",
            self.score(),
            self.passed_count(),
            self.case_count(),
            assertions_passed,
            assertions_total
        ));
        let must_fail = self.must_pass_failures();
        if must_fail.is_empty() {
            out.push_str("\nmust-pass: all green");
        } else {
            out.push_str(&format!("\nmust-pass failures: {}", must_fail.join(", ")));
        }
        if let Some(dir) = &self.dir {
            out.push_str(&format!("\nrun: {}", dir.display()));
        }
        out
    }
}

/// Runs every case `trials` times.
/// Runs the suite with the deterministic mock: the reviewer answers from each
/// case's own expectations, so a red case here means the runner is broken.
/// `contrarian` flips every answer, which must turn the whole suite red.
pub fn run_suite_mock(
    corpus: &Corpus,
    cases: &[Case],
    trials: u32,
    contrarian: bool,
) -> Result<RunSummary> {
    let started = crate::srs::now();
    let mut results = Vec::new();
    for trial in 1..=trials.max(1) {
        for case in cases {
            let mock = if contrarian {
                MockReviewer::contrarian(case)
            } else {
                MockReviewer::from_case(case)
            };
            results.push(run_case(corpus, case, &mock, trial)?);
        }
    }
    let finished = crate::srs::now();
    Ok(RunSummary {
        run_id: run_id(started),
        target: "mock".to_string(),
        started,
        finished,
        trials: trials.max(1),
        results,
        dir: None,
    })
}

pub fn run_suite(
    corpus: &Corpus,
    cases: &[Case],
    reviewer: &dyn Reviewer,
    trials: u32,
) -> Result<RunSummary> {
    let started = crate::srs::now();
    let mut results = Vec::new();
    for trial in 1..=trials.max(1) {
        for case in cases {
            results.push(run_case(corpus, case, reviewer, trial)?);
        }
    }
    let finished = crate::srs::now();
    Ok(RunSummary {
        run_id: run_id(started),
        target: reviewer.name(),
        started,
        finished,
        trials: trials.max(1),
        results,
        dir: None,
    })
}

pub fn run_case(
    corpus: &Corpus,
    case: &Case,
    reviewer: &dyn Reviewer,
    trial: u32,
) -> Result<CaseResult> {
    let snippet = corpus
        .find(&case.snippet)
        .with_context(|| format!("case `{}` names an unknown snippet", case.id))?;
    let mode = case
        .mode
        .unwrap_or_else(|| snippet.resolve_mode(snippet.asks));
    let prompts = snippet.prompts(mode);
    if prompts.is_empty() {
        bail!(
            "case `{}`: snippet `{}` produced no prompts",
            case.id,
            snippet.id
        );
    }
    check_expectations(case, &prompts)?;

    let annotations = annotations_for(case, prompts.len());
    let started = Instant::now();
    let graded = reviewer.grade(snippet, &prompts, &annotations);
    let duration_ms = started.elapsed().as_millis();

    let grade = match graded {
        Ok(grade) => grade,
        Err(error) => {
            let mut result = CaseResult::failed(
                &case.id,
                trial,
                format!("grading failed: {error:#}"),
                duration_ms,
            );
            result.description = case.description.clone();
            result.must_pass = case.must_pass;
            result.mode = mode;
            result.snippet = snippet.id.clone();
            result.reviewer = reviewer.name();
            return Ok(result);
        }
    };

    let mut failures = Vec::new();
    let mut passed_assertions = 0;
    for expectation in &case.expect {
        match judge(expectation, &grade, &prompts) {
            Ok(true) => passed_assertions += 1,
            Ok(false) => failures.push(format!(
                "{} — got `{}`",
                expectation.describe(),
                observed(expectation, &grade)
            )),
            Err(error) => failures.push(format!("{} — {error}", expectation.describe())),
        }
    }

    let passed = failures.is_empty();
    Ok(CaseResult {
        id: case.id.clone(),
        description: case.description.clone(),
        snippet: snippet.id.clone(),
        mode,
        trial,
        passed,
        must_pass: case.must_pass,
        assertions_passed: passed_assertions,
        assertions_total: case.expect.len(),
        failures,
        duration_ms,
        reviewer: grade.reviewer.clone(),
        summary: grade.summary.clone(),
        error: None,
        trace: trace(&case.id, trial, snippet, &prompts, &annotations, &grade),
    })
}

fn check_expectations(case: &Case, prompts: &[Prompt]) -> Result<()> {
    for expectation in &case.expect {
        let prompt = prompts.get(expectation.prompt - 1).with_context(|| {
            format!(
                "case `{}`: expectation names prompt {} but the sitting has {}",
                case.id,
                expectation.prompt,
                prompts.len()
            )
        })?;
        if let Some(concept) = &expectation.concept {
            if !prompt.concepts.contains(concept) {
                bail!(
                    "case `{}`: prompt {} does not carry concept `{concept}` (it has {:?})",
                    case.id,
                    expectation.prompt,
                    prompt.concepts
                );
            }
        }
    }
    Ok(())
}

fn annotations_for(case: &Case, count: usize) -> Vec<Annotation> {
    let mut texts = vec![String::new(); count];
    for annotation in &case.annotations {
        if let Some(slot) = texts.get_mut(annotation.prompt - 1) {
            *slot = annotation.text.clone();
        }
    }
    texts
        .into_iter()
        .enumerate()
        .map(|(index, text)| Annotation::new(index, text))
        .collect()
}

fn observed(expectation: &Expectation, grade: &Grade) -> String {
    let Some(prompt) = grade.prompts.get(expectation.prompt - 1) else {
        return "nothing".to_string();
    };
    match &expectation.concept {
        Some(concept) => prompt
            .concepts
            .iter()
            .find(|(id, _)| id == concept)
            .map(|(_, verdict)| verdict.as_str().to_string())
            .unwrap_or_else(|| "not graded".to_string()),
        None => prompt.verdict.as_str().to_string(),
    }
}

fn judge(expectation: &Expectation, grade: &Grade, prompts: &[Prompt]) -> Result<bool> {
    let prompt_number = expectation.prompt - 1;
    let Some(prompt) = grade.prompts.get(prompt_number) else {
        bail!(
            "the reviewer returned no grade for prompt {}",
            expectation.prompt
        );
    };
    let actual = match &expectation.concept {
        Some(concept) => prompt
            .concepts
            .iter()
            .find(|(id, _)| id == concept)
            .map(|(_, verdict)| *verdict)
            .with_context(|| {
                format!(
                    "the reviewer did not grade `{concept}` on prompt {} (the snippet expects {:?})",
                    expectation.prompt,
                    prompts[prompt_number].concepts
                )
            })?,
        None => prompt.verdict,
    };
    match expectation.verdict.as_deref() {
        Some(_) => Ok(actual == expectation.required()?),
        None => Ok(!expectation.forbidden()?.contains(&actual)),
    }
}

fn trace(
    id: &str,
    trial: u32,
    snippet: &Snippet,
    prompts: &[Prompt],
    annotations: &[Annotation],
    grade: &Grade,
) -> Value {
    json!({
        "case": id,
        "trial": trial,
        "snippet": snippet.id,
        "reviewer": grade.reviewer,
        "summary": grade.summary,
        "prompts": prompts.iter().enumerate().map(|(index, prompt)| json!({
            "prompt": index + 1,
            "span": prompt.span_label(),
            "ask": prompt.ask,
            "concepts": prompt.concepts,
            "annotation": annotations.get(index).map(|annotation| annotation.text.clone()).unwrap_or_default(),
            "verdict": grade.prompts.get(index).map(|graded| graded.verdict.as_str()),
            "feedback": grade.prompts.get(index).map(|graded| graded.feedback.clone()).unwrap_or_default(),
            "per_concept": grade.prompts.get(index).map(|graded| graded.concepts.iter()
                .map(|(concept, verdict)| (concept.clone(), verdict.as_str().to_string()))
                .collect::<BTreeMap<String, String>>()),
        })).collect::<Vec<Value>>(),
    })
}

/// Writes `manifest.json`, `results.jsonl`, `summary.md` and one trace per case.
pub fn write_run(summary: &RunSummary, runs_dir: &Path) -> Result<PathBuf> {
    let dir = runs_dir.join(&summary.run_id);
    fs::create_dir_all(dir.join("traces"))
        .with_context(|| format!("cannot create {}", dir.display()))?;

    let manifest = json!({
        "run_id": summary.run_id,
        "target": summary.target,
        "trials": summary.trials,
        "started": iso8601(summary.started),
        "finished": iso8601(summary.finished),
        "cases": summary.case_count(),
        "passed": summary.passed_count(),
        "score": summary.score(),
        "assertions": {
            "passed": summary.assertion_rate().0,
            "total": summary.assertion_rate().1,
        },
        "must_pass_failures": summary.must_pass_failures(),
    });
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)?,
    )?;

    let mut lines = String::new();
    for result in &summary.results {
        let line = json!({
            "case": result.id,
            "trial": result.trial,
            "snippet": result.snippet,
            "mode": result.mode.label(),
            "passed": result.passed,
            "must_pass": result.must_pass,
            "assertions": {"passed": result.assertions_passed, "total": result.assertions_total},
            "duration_ms": result.duration_ms,
            "failures": result.failures,
            "error": result.error,
        });
        lines.push_str(&serde_json::to_string(&line)?);
        lines.push('\n');
    }
    fs::write(dir.join("results.jsonl"), lines)?;

    let mut markdown = String::from("# readcode eval run\n\n");
    markdown.push_str(&format!(
        "- target: `{}`\n- trials: {}\n- started: {}\n- score: {:.2} ({}/{} cases)\n\n",
        summary.target,
        summary.trials,
        iso8601(summary.started),
        summary.score(),
        summary.passed_count(),
        summary.case_count()
    ));
    markdown.push_str("| case | result | assertions | ms |\n|---|---|---|---|\n");
    for (id, passed, must_pass, passed_assertions, total_assertions, duration_ms) in
        summary.passes()
    {
        markdown.push_str(&format!(
            "| `{id}` | {} | {passed_assertions}/{total_assertions} | {duration_ms} |\n",
            if passed {
                "pass"
            } else if must_pass {
                "FAIL (must-pass)"
            } else {
                "fail"
            }
        ));
    }
    fs::write(dir.join("summary.md"), markdown)?;

    for result in &summary.results {
        let name = format!("{}-t{}", result.id, result.trial);
        fs::write(
            dir.join("traces").join(format!("{name}.json")),
            serde_json::to_string_pretty(&result.trace)?,
        )?;
    }

    Ok(dir)
}

/// A reviewer that answers from the case's own expectations. It exists to prove
/// the harness: with it, a red run means the runner is broken, not the model.
pub struct MockReviewer {
    label: String,
    /// `(prompt, concept)` → verdict.
    concepts: BTreeMap<(usize, String), Verdict>,
    /// prompt → verdict.
    prompts: BTreeMap<usize, Verdict>,
    contrarian: bool,
}

impl MockReviewer {
    pub fn from_case(case: &Case) -> Self {
        let mut reviewer = Self {
            label: format!("mock:{}", case.id),
            concepts: BTreeMap::new(),
            prompts: BTreeMap::new(),
            contrarian: false,
        };
        for expectation in &case.expect {
            let verdict = satisfying(expectation);
            match &expectation.concept {
                Some(concept) => {
                    reviewer
                        .concepts
                        .insert((expectation.prompt, concept.clone()), verdict);
                }
                None => {
                    reviewer.prompts.insert(expectation.prompt, verdict);
                }
            }
        }
        reviewer
    }

    /// Every answer flipped, so every case must fail: the harness self-test.
    pub fn contrarian(case: &Case) -> Self {
        let mut reviewer = Self::from_case(case);
        reviewer.label = format!("mock:{}:contrarian", case.id);
        reviewer.contrarian = true;
        reviewer
    }

    fn answer(&self, verdict: Verdict) -> Verdict {
        if !self.contrarian {
            return verdict;
        }
        match verdict {
            Verdict::Correct => Verdict::Wrong,
            _ => Verdict::Correct,
        }
    }
}

fn satisfying(expectation: &Expectation) -> Verdict {
    if let Some(verdict) = expectation.verdict.as_deref() {
        return Verdict::from_label(verdict);
    }
    let forbidden: Vec<Verdict> = expectation
        .not
        .iter()
        .map(|label| Verdict::from_label(label))
        .collect();
    [
        Verdict::Wrong,
        Verdict::Partial,
        Verdict::Unknown,
        Verdict::Correct,
    ]
    .into_iter()
    .find(|verdict| !forbidden.contains(verdict))
    .unwrap_or(Verdict::Unknown)
}

impl Reviewer for MockReviewer {
    fn name(&self) -> String {
        self.label.clone()
    }

    fn grade(
        &self,
        _snippet: &Snippet,
        prompts: &[Prompt],
        _annotations: &[Annotation],
    ) -> Result<Grade> {
        let graded = prompts
            .iter()
            .enumerate()
            .map(|(index, prompt)| {
                let number = index + 1;
                let fallback = self
                    .prompts
                    .get(&number)
                    .copied()
                    .map(|verdict| self.answer(verdict))
                    .unwrap_or(Verdict::Correct);
                let concepts: Vec<(String, Verdict)> = prompt
                    .concepts
                    .iter()
                    .map(|concept| {
                        let verdict = self
                            .concepts
                            .get(&(number, concept.clone()))
                            .copied()
                            .map(|verdict| self.answer(verdict))
                            .unwrap_or(fallback);
                        (concept.clone(), verdict)
                    })
                    .collect();
                PromptGrade {
                    prompt: index,
                    verdict: fallback,
                    feedback: String::new(),
                    missing: Vec::new(),
                    concepts,
                }
            })
            .collect();

        Ok(Grade {
            reviewer: self.name(),
            prompts: graded,
            summary: "mock reviewer".to_string(),
        })
    }
}

fn run_id(now: u64) -> String {
    iso8601(now)
        .replace(['-', ':'], "")
        .replace('T', "-")
        .replace('Z', "")
}

/// UTC timestamp without pulling in a date crate.
pub fn iso8601(now: u64) -> String {
    let days = (now / 86_400) as i64;
    let seconds = now % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { y + 1 } else { y }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::case::Case;
    use crate::review::local::LocalReviewer;

    const CASES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/evals/cases");

    fn corpus() -> Corpus {
        Corpus::builtin().unwrap()
    }

    fn cases() -> Vec<Case> {
        super::super::case::load_dir(Path::new(CASES)).unwrap()
    }

    #[test]
    fn timestamps_are_iso8601_utc() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_700_000_000), "2023-11-14T22:13:20Z");
        // A leap day, which is where naive date arithmetic breaks.
        assert_eq!(iso8601(1_709_164_800), "2024-02-29T00:00:00Z");
    }

    #[test]
    fn the_mock_target_passes_every_case() {
        let corpus = corpus();
        for case in cases() {
            let mock = MockReviewer::from_case(&case);
            let result = run_case(&corpus, &case, &mock, 1).unwrap();
            assert!(result.passed, "{}: {:?}", case.id, result.failures);
        }
    }

    #[test]
    fn a_contrarian_mock_fails_every_case() {
        // Proves the harness can actually report red.
        let corpus = corpus();
        for case in cases() {
            let mock = MockReviewer::contrarian(&case);
            let result = run_case(&corpus, &case, &mock, 1).unwrap();
            assert!(
                !result.passed,
                "case `{}` passed against a reviewer that flipped every answer",
                case.id
            );
        }
    }

    #[test]
    fn the_offline_rubric_meets_its_recorded_baseline() {
        // The offline rubric cannot credit a paraphrase, so this is a floor,
        // not a target: it exists to catch a regression in the rubric itself.
        let corpus = corpus();
        let cases = cases();
        let summary = run_suite(&corpus, &cases, &LocalReviewer, 1).unwrap();
        let score = summary.score();
        assert!(
            score >= 0.6,
            "offline reviewer scored {score:.2}:\n{}",
            summary.report()
        );
    }

    #[test]
    fn a_case_naming_an_unknown_snippet_is_an_error() {
        let text =
            std::fs::read_to_string(Path::new(CASES).join("credits-a-paraphrase.toml")).unwrap();
        let case = Case::parse(&text.replace("rust-001", "rust-999")).unwrap();
        let error = run_case(&corpus(), &case, &LocalReviewer, 1)
            .unwrap_err()
            .to_string();
        assert!(error.contains("unknown snippet"), "{error}");
    }

    #[test]
    fn a_run_writes_its_artifacts() {
        let corpus = corpus();
        let case = cases().into_iter().next().unwrap();
        let mock = MockReviewer::from_case(&case);
        let mut summary = run_suite(&corpus, std::slice::from_ref(&case), &mock, 1).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let run_dir = write_run(&summary, dir.path()).unwrap();
        summary.dir = Some(run_dir.clone());

        assert!(run_dir.join("manifest.json").is_file());
        assert!(run_dir.join("results.jsonl").is_file());
        assert!(run_dir.join("summary.md").is_file());
        assert!(
            run_dir
                .join("traces")
                .join(format!("{}-t1.json", case.id))
                .is_file()
        );
        let manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(run_dir.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["cases"], 1);
        assert_eq!(manifest["target"], mock.name());
        assert!(summary.report().contains("must-pass"));
    }
}
