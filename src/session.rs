//! One sitting: show a snippet, collect an annotation per block, grade it, and
//! fold the verdicts back into mastery. I/O is injected so the whole loop runs
//! in tests without a terminal.

use std::io::{BufRead, Write};

use anyhow::{Context, Result};

use crate::corpus::Corpus;
use crate::review::{Annotation, Grade, Reviewer};
use crate::snippet::Snippet;
use crate::srs::ConceptState;
use crate::store::{Event, Progress};
use crate::ui::{self, Style};

pub struct Outcome {
    pub grade: Grade,
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

pub fn run(
    snippet: &Snippet,
    reviewer: &dyn Reviewer,
    progress: &mut Progress,
    now: u64,
    style: Style,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<Option<Outcome>> {
    ui::print_snippet(out, style, snippet)?;
    writeln!(out)?;
    writeln!(
        out,
        "{}",
        style.dim(&format!("reviewer: {}", reviewer.name()))
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "{}",
        style.dim("explain each block in your own words · `.` on its own line finishes · `:unknown` if you have no idea · `:quit` stops")
    )?;

    let mut annotations = Vec::with_capacity(snippet.blocks.len());
    for (index, block) in snippet.blocks.iter().enumerate() {
        let (start, end) = (block.lines[0], block.lines[1]);
        writeln!(out)?;
        writeln!(
            out,
            "{}",
            style.bold(&format!("block {} · lines {start}-{end}", index + 1))
        )?;
        writeln!(out, "{}", block.ask)?;
        match read_annotation(input, out, style)? {
            Some(text) => annotations.push(Annotation::new(index, text)),
            None => {
                writeln!(out, "{}", style.dim("stopped — this snippet was not saved"))?;
                return Ok(None);
            }
        }
    }

    writeln!(out)?;
    writeln!(out, "{}", style.dim("grading…"))?;
    let grade = reviewer
        .grade(snippet, &annotations)
        .context("grading failed; nothing was saved")?;

    writeln!(out)?;
    ui::print_grade(out, style, snippet, &grade)?;

    let verdicts = grade.concept_verdicts();
    for (concept, verdict) in &verdicts {
        progress.concept_mut(concept).apply(*verdict, now);
    }
    for block_grade in &grade.blocks {
        progress.record(Event {
            at: now,
            snippet: snippet.id.clone(),
            block: block_grade.block,
            verdict: block_grade.verdict,
        });
    }

    if progress.created_at == 0 {
        progress.created_at = now;
    }
    progress.updated_at = now;
    progress.sessions += 1;
    progress.last_snippet = Some(snippet.id.clone());
    if !progress.seen(&snippet.id) {
        progress.seen_snippets.push(snippet.id.clone());
    }

    let due_now = progress
        .concepts
        .values()
        .filter(|state| state.due_at != 0 && state.due_at <= now)
        .count();

    writeln!(out)?;
    writeln!(
        out,
        "{}",
        style.dim(&format!(
            "saved — {} concept{} updated · {due_now} due for review now · `readcode status` for mastery",
            verdicts.len(),
            if verdicts.len() == 1 { "" } else { "s" }
        ))
    )?;

    Ok(Some(Outcome {
        grade,
        concepts_touched: verdicts.len(),
        due_now,
    }))
}

/// Reads a multi-line annotation until a line that is exactly `.`.
/// Returns `None` when the learner asked to stop.
fn read_annotation(
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    style: Style,
) -> Result<Option<String>> {
    let mut buffer = String::new();
    loop {
        write!(out, "{} ", style.cyan(">"))?;
        out.flush()?;

        let mut line = String::new();
        let read = input.read_line(&mut line)?;
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

/// Small xorshift so snippet choice varies between runs but is reproducible from
/// a seed, which keeps `pick` testable.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// splitmix64: a small seed (an early epoch second, a test counter) still
    /// produces a well-mixed stream, which a bare xorshift does not.
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
    use crate::review::local::LocalReviewer;

    fn corpus() -> Corpus {
        Corpus::builtin().unwrap()
    }

    #[test]
    fn a_full_sitting_saves_progress_and_schedules_concepts() {
        let corpus = corpus();
        let snippet = corpus.find("002").unwrap();
        let mut progress = Progress::default();

        // Answer every block with the rubric's own words, then `.`.
        let mut stdin = String::new();
        for block in &snippet.blocks {
            for expect in &block.expects {
                stdin.push_str(&expect.keywords.join(", "));
                stdin.push('\n');
            }
            stdin.push_str(".\n");
        }

        let mut input = std::io::Cursor::new(stdin);
        let mut out: Vec<u8> = Vec::new();
        let outcome = run(
            snippet,
            &LocalReviewer,
            &mut progress,
            1_000_000,
            Style::new(false),
            &mut input,
            &mut out,
        )
        .unwrap()
        .expect("a finished sitting");

        assert_eq!(progress.sessions, 1);
        assert!(!progress.concepts.is_empty());
        assert_eq!(outcome.grade.blocks.len(), snippet.blocks.len());
        assert!(progress.seen(&snippet.id));
        assert_eq!(progress.last_snippet.as_deref(), Some(snippet.id.as_str()));
        // Correct answers push every concept a day out, so nothing is due now.
        assert!(
            progress.concepts.values().all(|c| c.due_at > 1_000_000),
            "{:?}",
            progress
                .concepts
                .values()
                .map(|c| c.due_at)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn unknown_answers_come_back_within_the_sitting() {
        let corpus = corpus();
        let snippet = corpus.find("002").unwrap();
        let mut progress = Progress::default();
        let mut stdin = String::new();
        for _ in &snippet.blocks {
            stdin.push_str(":unknown\n");
        }
        let mut input = std::io::Cursor::new(stdin);
        let mut out: Vec<u8> = Vec::new();
        run(
            snippet,
            &LocalReviewer,
            &mut progress,
            1_000_000,
            Style::new(false),
            &mut input,
            &mut out,
        )
        .unwrap();

        assert!(
            progress
                .concepts
                .values()
                .all(|c| c.due_at == 1_000_000 + 900),
            "{:?}",
            progress
                .concepts
                .values()
                .map(|c| c.due_at)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn quitting_mid_sitting_saves_nothing() {
        let corpus = corpus();
        let snippet = corpus.find("001").unwrap();
        let mut progress = Progress::default();
        let mut input = std::io::Cursor::new(":quit\n");
        let mut out: Vec<u8> = Vec::new();
        let outcome = run(
            snippet,
            &LocalReviewer,
            &mut progress,
            1_000_000,
            Style::new(false),
            &mut input,
            &mut out,
        )
        .unwrap();
        assert!(outcome.is_none());
        assert_eq!(progress.sessions, 0);
        assert!(progress.concepts.is_empty());
    }

    #[test]
    fn multiline_annotations_keep_their_lines() {
        let style = Style::new(false);
        let mut out: Vec<u8> = Vec::new();
        let mut input = std::io::Cursor::new("first line\nsecond line\n.\n");
        let text = read_annotation(&mut input, &mut out, style)
            .unwrap()
            .unwrap();
        assert_eq!(text, "first line\nsecond line\n");
    }

    #[test]
    fn eof_ends_the_annotation_instead_of_looping() {
        let style = Style::new(false);
        let mut out: Vec<u8> = Vec::new();
        let mut input = std::io::Cursor::new("only line\n");
        let text = read_annotation(&mut input, &mut out, style)
            .unwrap()
            .unwrap();
        assert_eq!(text, "only line\n");
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
        // Make every concept solid except one that lives in rust-003.
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
}
