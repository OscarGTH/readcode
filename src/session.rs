//! One sitting: show a snippet, collect an annotation per prompt (a block or a
//! single line), grade it, and fold the verdicts back into mastery. I/O is
//! injected so the whole loop runs in tests without a terminal.

use std::io::{BufRead, Write};

use anyhow::{Context, Result, bail};

use crate::corpus::Corpus;
use crate::review::{Annotation, Grade, Reviewer};
use crate::snippet::{Asks, Snippet};
use crate::srs::ConceptState;
use crate::store::{Event, Progress};
use crate::ui::{self, Style};

const HELP: &str = "explain in your own words · `.` on its own line finishes · `:unknown` if you have no idea · `:quit` stops";

/// Where a sitting reads and writes, bundled so a session takes one terminal
/// instead of three separate handles.
pub struct Terminal<'a> {
    pub input: &'a mut dyn BufRead,
    pub out: &'a mut dyn Write,
    pub style: Style,
}

pub struct Outcome {
    pub grade: Grade,
    /// The granularity that was actually used, after `auto` resolved.
    pub mode: Asks,
    /// Distinct concepts the grade touched.
    pub concepts_touched: usize,
    /// Concepts due for review right now, after this sitting.
    pub due_now: usize,
}

/// The snippet that most wants to be seen: overdue and weak concepts first,
/// snippets never seen get a nudge, and the last snippet is skipped when there
/// is anywhere else to go.
pub fn pick<'a>(corpus: &'a Corpus, progress: &Progress, now: u64) -> Option<&'a Snippet> {
    if corpus.is_empty() {
        return None;
    }

    let weighted: Vec<(f64, &Snippet)> = corpus
        .snippets()
        .iter()
        .map(|snippet| {
            let concepts = snippet.concepts();
            let total: f64 = concepts
                .iter()
                .map(|concept| match progress.concepts.get(concept) {
                    Some(state) => state.urgency(now),
                    None => ConceptState::new(concept).urgency(now),
                })
                .sum();
            let average = total / concepts.len().max(1) as f64;
            let novelty = if progress.seen(&snippet.id) {
                0.85
            } else {
                1.15
            };
            (average * novelty, snippet)
        })
        .collect();

    let avoid = progress.last_snippet.as_deref();
    let mut candidates: Vec<(f64, &Snippet)> = weighted
        .iter()
        .filter(|(_, snippet)| Some(snippet.id.as_str()) != avoid)
        .map(|(weight, snippet)| (*weight, *snippet))
        .collect();
    if candidates.is_empty() {
        candidates = weighted;
    }

    let total: f64 = candidates.iter().map(|(weight, _)| *weight).sum();
    if total <= 0.0 {
        return candidates.first().map(|(_, snippet)| *snippet);
    }

    let mut rng = Rng::new(now);
    let mut ticket = rng.next_f64() * total;
    for (weight, snippet) in &candidates {
        ticket -= weight;
        if ticket <= 0.0 {
            return Some(snippet);
        }
    }
    candidates.last().map(|(_, snippet)| *snippet)
}

/// Which granularity to use for a snippet, given what came before.
/// A repeat visit alternates: blocks test comprehension, lines test detail, and
/// the same code is worth meeting both ways.
pub fn choose_mode(snippet: &Snippet, progress: &Progress) -> Asks {
    let preferred = snippet.resolve_mode(snippet.asks);
    match progress.last_mode(&snippet.id) {
        Some(previous) => previous.flipped(),
        None => preferred,
    }
}

pub fn run(
    snippet: &Snippet,
    mode: Asks,
    reviewer: &dyn Reviewer,
    progress: &mut Progress,
    now: u64,
    term: &mut Terminal<'_>,
) -> Result<Option<Outcome>> {
    let style = term.style;
    let mode = snippet.resolve_mode(mode);
    let prompts = snippet.prompts(mode);
    if prompts.is_empty() {
        bail!("snippet `{}` produced no prompts", snippet.id);
    }

    ui::print_header(term.out, style, snippet)?;
    writeln!(term.out)?;
    writeln!(
        term.out,
        "{}",
        style.dim(&format!(
            "asked {} · {} prompt{} · reviewer: {}",
            if mode == Asks::Lines {
                "line by line"
            } else {
                "block by block"
            },
            prompts.len(),
            if prompts.len() == 1 { "" } else { "s" },
            reviewer.name()
        ))
    )?;

    let line_mode = mode == Asks::Lines;
    if line_mode {
        // In line mode the whole snippet is shown once: you need the
        // surrounding code to explain a single line.
        ui::print_code(term.out, style, snippet)?;
    }

    writeln!(term.out)?;
    writeln!(term.out, "{}", style.dim(HELP))?;

    let mut annotations = Vec::with_capacity(prompts.len());
    for (index, prompt) in prompts.iter().enumerate() {
        writeln!(term.out)?;
        if line_mode {
            ui::print_line_prompt(
                term.out,
                style,
                ui::gutter_width(snippet),
                prompt.start,
                prompt,
            )?;
        } else {
            ui::print_block(term.out, style, snippet, prompt.block)?;
        }
        match read_annotation(term)? {
            Some(text) => annotations.push(Annotation::new(index, text)),
            None => {
                writeln!(
                    term.out,
                    "{}",
                    style.dim("stopped — this snippet was not saved")
                )?;
                return Ok(None);
            }
        }
    }

    writeln!(term.out)?;
    writeln!(term.out, "{}", style.dim("grading…"))?;
    let grade = reviewer
        .grade(snippet, &prompts, &annotations)
        .context("grading failed; nothing was saved")?;

    writeln!(term.out)?;
    ui::print_grade(term.out, style, &prompts, &grade)?;

    let verdicts = grade.concept_verdicts();
    for (concept, verdict) in &verdicts {
        progress.concept_mut(concept).apply(*verdict, now);
    }
    for graded in &grade.prompts {
        progress.record(Event {
            at: now,
            snippet: snippet.id.clone(),
            prompt: graded.prompt,
            verdict: graded.verdict,
        });
    }

    if progress.created_at == 0 {
        progress.created_at = now;
    }
    progress.updated_at = now;
    progress.sessions += 1;
    progress.last_snippet = Some(snippet.id.clone());
    progress.set_last_mode(&snippet.id, mode);
    if !progress.seen(&snippet.id) {
        progress.seen_snippets.push(snippet.id.clone());
    }

    let due_now = progress
        .concepts
        .values()
        .filter(|state| state.due_at != 0 && state.due_at <= now)
        .count();

    writeln!(term.out)?;
    writeln!(
        term.out,
        "{}",
        style.dim(&format!(
            "saved — {} concept{} updated · {} prompt{} graded · {due_now} due for review now · `readcode status` for mastery",
            verdicts.len(),
            if verdicts.len() == 1 { "" } else { "s" },
            grade.prompts.len(),
            if grade.prompts.len() == 1 { "" } else { "s" },
        ))
    )?;

    Ok(Some(Outcome {
        grade,
        mode,
        concepts_touched: verdicts.len(),
        due_now,
    }))
}

/// Reads a multi-line annotation until a line that is exactly `.`.
/// Returns `None` when the learner asked to stop.
fn read_annotation(term: &mut Terminal<'_>) -> Result<Option<String>> {
    let mut buffer = String::new();
    loop {
        write!(term.out, "{} ", term.style.cyan(">"))?;
        term.out.flush()?;

        let mut line = String::new();
        let read = term.input.read_line(&mut line)?;
        if read == 0 {
            // EOF (piped input ended): take what we have.
            return Ok(Some(buffer));
        }
        let trimmed = line.trim_end_matches(['\n', '\r']);

        match trimmed.trim() {
            "." => return Ok(Some(buffer)),
            ":quit" | ":q" | ":stop" => return Ok(None),
            ":unknown" => {
                buffer.push_str("I don't know.");
                return Ok(Some(buffer));
            }
            "" => {
                if !buffer.is_empty() && !buffer.ends_with('\n') {
                    buffer.push('\n');
                }
            }
            _ => {
                buffer.push_str(trimmed);
                buffer.push('\n');
            }
        }
    }
}

/// splitmix64, so a small seed (an early epoch second, a test counter) still
/// produces a well-mixed stream — which is what keeps `pick` testable.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::{Grade, PromptGrade};
    use crate::srs::Verdict;

    /// Grades like a cooperative model: anything the learner actually explains
    /// is correct, "I don't know" and blanks are unknown. That is all these
    /// tests need — the real grading quality is measured by `evals/`.
    struct EchoReviewer;

    impl Reviewer for EchoReviewer {
        fn name(&self) -> String {
            "echo".to_string()
        }

        fn grade(
            &self,
            _snippet: &Snippet,
            prompts: &[crate::snippet::Prompt],
            annotations: &[Annotation],
        ) -> Result<Grade> {
            let graded = prompts
                .iter()
                .enumerate()
                .map(|(index, prompt)| {
                    let said = annotations
                        .iter()
                        .find(|annotation| annotation.prompt == index)
                        .map(|annotation| annotation.text.to_lowercase())
                        .unwrap_or_default();
                    let verdict = if said.trim().is_empty() || said.contains("don't know") {
                        Verdict::Unknown
                    } else {
                        Verdict::Correct
                    };
                    PromptGrade {
                        prompt: index,
                        verdict,
                        feedback: String::new(),
                        missing: Vec::new(),
                        concepts: prompt
                            .concepts
                            .iter()
                            .map(|concept| (concept.clone(), verdict))
                            .collect(),
                    }
                })
                .collect();
            Ok(Grade {
                reviewer: self.name(),
                prompts: graded,
                summary: String::new(),
            })
        }
    }

    fn corpus() -> Corpus {
        Corpus::builtin().unwrap()
    }

    /// Runs a sitting over piped input; returns the outcome and the screen.
    fn sitting(
        snippet: &Snippet,
        mode: Asks,
        progress: &mut Progress,
        now: u64,
        stdin: String,
    ) -> (Result<Option<Outcome>>, String) {
        let mut input = std::io::Cursor::new(stdin);
        let mut out: Vec<u8> = Vec::new();
        let mut term = Terminal {
            input: &mut input,
            out: &mut out,
            style: Style::new(false),
        };
        let outcome = run(snippet, mode, &EchoReviewer, progress, now, &mut term);
        (outcome, String::from_utf8_lossy(&out).to_string())
    }

    /// One short answer per prompt, each finished with `.`.
    fn answers(snippet: &Snippet, mode: Asks, text: &str) -> String {
        format!("{text}\n.\n").repeat(snippet.prompts(mode).len())
    }

    #[test]
    fn a_full_sitting_saves_progress_and_schedules_concepts() {
        let corpus = corpus();
        let snippet = corpus.find("002").unwrap();
        let prompts = snippet.prompts(Asks::Blocks);
        let mut progress = Progress::default();
        let stdin = answers(snippet, Asks::Blocks, "an explanation");

        let (result, _) = sitting(snippet, Asks::Blocks, &mut progress, 1_000_000, stdin);
        let outcome = result.unwrap().expect("a finished sitting");

        assert_eq!(progress.sessions, 1);
        assert!(!progress.concepts.is_empty());
        assert_eq!(outcome.grade.prompts.len(), prompts.len());
        assert_eq!(outcome.mode, Asks::Blocks);
        assert!(progress.seen(&snippet.id));
        assert_eq!(progress.last_snippet.as_deref(), Some(snippet.id.as_str()));
        assert_eq!(progress.last_mode(&snippet.id), Some(Asks::Blocks));
        // Correct answers push every concept a day out, so nothing is due now.
        assert!(
            progress
                .concepts
                .values()
                .all(|concept| concept.due_at > 1_000_000),
            "{:?}",
            progress
                .concepts
                .values()
                .map(|concept| concept.due_at)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn line_mode_asks_more_questions_and_shows_the_whole_snippet() {
        let corpus = corpus();
        let snippet = corpus.find("005").unwrap();
        let block_prompts = snippet.prompts(Asks::Blocks);
        let line_prompts = snippet.prompts(Asks::Lines);
        assert!(
            line_prompts.len() > block_prompts.len(),
            "line mode should ask about more than {} things",
            block_prompts.len()
        );

        let mut progress = Progress::default();
        let stdin = answers(snippet, Asks::Lines, "an explanation");
        let (result, screen) = sitting(snippet, Asks::Lines, &mut progress, 2_000_000, stdin);
        let outcome = result.unwrap().expect("a finished sitting");

        assert_eq!(outcome.mode, Asks::Lines);
        assert_eq!(progress.events.len(), line_prompts.len());
        assert_eq!(progress.last_mode(&snippet.id), Some(Asks::Lines));
        // The whole snippet is shown once, then each significant line is asked.
        assert!(screen.contains("asked line by line"), "{screen}");
        assert!(screen.contains("── block 4 · lines 6-7"), "{screen}");
        assert!(screen.contains("┌ line 6 · "), "{screen}");
    }

    #[test]
    fn unknown_answers_come_back_within_the_sitting() {
        let corpus = corpus();
        let snippet = corpus.find("002").unwrap();
        let prompts = snippet.prompts(Asks::Blocks);
        let mut progress = Progress::default();
        let stdin = ":unknown\n".repeat(prompts.len());
        sitting(snippet, Asks::Blocks, &mut progress, 1_000_000, stdin)
            .0
            .unwrap();

        assert!(
            progress
                .concepts
                .values()
                .all(|concept| concept.due_at == 1_000_000 + 900),
            "{:?}",
            progress
                .concepts
                .values()
                .map(|concept| concept.due_at)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn quitting_mid_sitting_saves_nothing() {
        let corpus = corpus();
        let snippet = corpus.find("001").unwrap();
        let mut progress = Progress::default();
        let (result, _) = sitting(
            snippet,
            Asks::Blocks,
            &mut progress,
            1_000_000,
            ":quit\n".to_string(),
        );
        assert!(result.unwrap().is_none());
        assert_eq!(progress.sessions, 0);
        assert!(progress.concepts.is_empty());
    }

    #[test]
    fn multiline_annotations_keep_their_lines() {
        let mut input = std::io::Cursor::new("first line\nsecond line\n.\n");
        let mut out: Vec<u8> = Vec::new();
        let mut term = Terminal {
            input: &mut input,
            out: &mut out,
            style: Style::new(false),
        };
        let text = read_annotation(&mut term).unwrap().unwrap();
        assert_eq!(text, "first line\nsecond line\n");
    }

    #[test]
    fn eof_ends_the_annotation_instead_of_looping() {
        let mut input = std::io::Cursor::new("only line\n");
        let mut out: Vec<u8> = Vec::new();
        let mut term = Terminal {
            input: &mut input,
            out: &mut out,
            style: Style::new(false),
        };
        let text = read_annotation(&mut term).unwrap().unwrap();
        assert_eq!(text, "only line\n");
    }

    #[test]
    fn choose_mode_alternates_on_a_repeat_visit() {
        let corpus = corpus();
        let snippet = corpus.find("001").unwrap();
        let mut progress = Progress::default();
        // First visit: the mode the corpus is authored for.
        assert_eq!(choose_mode(snippet, &progress), Asks::Blocks);
        progress.set_last_mode(&snippet.id, Asks::Blocks);
        // Second visit: the other granularity.
        assert_eq!(choose_mode(snippet, &progress), Asks::Lines);
        progress.set_last_mode(&snippet.id, Asks::Lines);
        assert_eq!(choose_mode(snippet, &progress), Asks::Blocks);
    }

    #[test]
    fn choose_mode_prefers_lines_for_a_line_authored_snippet() {
        let corpus = corpus();
        let snippet = corpus.find("005").unwrap();
        let progress = Progress::default();
        assert_eq!(choose_mode(snippet, &progress), Asks::Lines);
    }

    #[test]
    fn pick_avoids_the_last_snippet_when_it_can() {
        let corpus = corpus();
        let mut progress = Progress::default();
        for snippet in corpus.snippets() {
            progress.last_snippet = Some(snippet.id.clone());
            let picked = pick(&corpus, &progress, 5_000).unwrap();
            assert_ne!(picked.id, snippet.id);
        }
    }

    #[test]
    fn pick_prefers_the_weaker_concept() {
        let corpus = corpus();
        let mut progress = Progress::default();
        // Make every concept solid except the ones in rust-003.
        for snippet in corpus.snippets() {
            for concept in snippet.concepts() {
                let state = progress.concept_mut(&concept);
                state.strength = 1.0;
                state.reps = 5;
                state.interval_days = 30.0;
                state.due_at = 9_000_000;
            }
        }
        let target = corpus.find("003").unwrap();
        for concept in target.concepts() {
            let state = progress.concept_mut(&concept);
            state.strength = 0.0;
            state.reps = 1;
            state.due_at = 0;
        }

        let mut hits = 0;
        for seed in 0..40u64 {
            if pick(&corpus, &progress, seed).unwrap().id == target.id {
                hits += 1;
            }
        }
        assert!(hits > 20, "weak snippet was picked {hits}/40 times");
    }

    #[test]
    fn a_line_sitting_schedules_every_concept_like_a_block_sitting() {
        let corpus = corpus();
        let snippet = corpus.find("005").unwrap();
        let prompts = snippet.prompts(Asks::Lines);
        let mut progress = Progress::default();
        let stdin = ":unknown\n".repeat(prompts.len());
        sitting(snippet, Asks::Lines, &mut progress, 3_000_000, stdin)
            .0
            .unwrap();

        assert!(!progress.concepts.is_empty());
        assert!(
            progress
                .concepts
                .values()
                .all(|concept| concept.due_at == 3_000_000 + 900),
            "every concept in the snippet should be scheduled inside the sitting"
        );
        assert_eq!(progress.last_mode(&snippet.id), Some(Asks::Lines));
    }
}
