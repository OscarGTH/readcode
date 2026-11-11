//! One sitting: show a snippet, collect an annotation per prompt (a block or a
//! single line), grade it, and fold the verdicts back into mastery. I/O is
//! injected so the whole loop runs in tests without a terminal.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Write};

use anyhow::{Context, Result, bail};

use crate::corpus::Corpus;
use crate::review::{Annotation, Grade, Reviewer};
use crate::snippet::{Asks, LEVELS, MAX_LEVEL, Snippet};
use crate::srs::ConceptState;
use crate::store::{Event, Progress};
use crate::tui::{self, Action};
use crate::ui::{self, Style};

const HELP: &str = "your own words · `.` finishes · `:unknown` = no idea · `:quit` stops";

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
/// The language in play. Before one is chosen, that is the corpus's main
/// language — the one with the most material — rather than "all of them at
/// once", which would mix two ladders into one menu.
pub fn current_language(corpus: &Corpus, progress: &Progress) -> Option<String> {
    progress
        .active_language()
        .map(str::to_string)
        .or_else(|| corpus.languages().first().cloned())
}

pub fn pick<'a>(corpus: &'a Corpus, progress: &Progress, now: u64) -> Option<&'a Snippet> {
    if corpus.is_empty() {
        return None;
    }

    // One language at a time: a sitting in Go must not be decided by what you
    // have read in Rust.
    let language = current_language(corpus, progress);
    let in_language: Vec<&Snippet> = corpus
        .snippets()
        .iter()
        .filter(|snippet| {
            language
                .as_deref()
                .is_none_or(|language| snippet.language == language)
        })
        .collect();
    if in_language.is_empty() {
        return None;
    }
    let level = language
        .as_deref()
        .and_then(|language| progress.level_of(language));

    // A declared level limits the menu to what the learner can actually read,
    // plus one rung of stretch. No declared level means no filter.
    let mut pool: Vec<&Snippet> = match level {
        Some(level) => {
            let reachable: Vec<&Snippet> = in_language
                .iter()
                .copied()
                .filter(|snippet| snippet.levels().reachable(level))
                .collect();
            if reachable.is_empty() {
                in_language.clone()
            } else {
                reachable
            }
        }
        None => in_language.clone(),
    };

    // Start at the bottom of your own level: while there is still something to
    // learn at or below it, the rung above waits. That is the difference between
    // "you may read this" and "this is what you are reading next".
    if let Some(level) = level {
        let at_or_below: Vec<&Snippet> = pool
            .iter()
            .copied()
            .filter(|snippet| snippet.levels().from <= level)
            .collect();
        // Stay on the rung while it still has something to do: a snippet you have
        // not met, or a concept that has come due again. Requiring everything at
        // this level to be *mastered* before moving on pinned a beginner to the
        // two level-1 snippets for days — the same snippets, again and again,
        // after they had been answered correctly.
        let unseen = at_or_below
            .iter()
            .any(|snippet| !progress.seen(&snippet.id));
        let due = at_or_below
            .iter()
            .flat_map(|snippet| snippet.concepts())
            .any(|concept| {
                progress
                    .concepts
                    .get(&concept)
                    .is_none_or(|state| state.due_at <= now)
            });
        if !at_or_below.is_empty() && (unseen || due) {
            pool = at_or_below;
        }
    }

    let weighted: Vec<(f64, &Snippet)> = pool
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
            // Content at the learner's level beats content far from it, so a
            // beginner meets `let` before they meet an iterator chain.
            let fit = match level {
                Some(level) => 1.0 / (1.0 + snippet.levels().distance(level)),
                None => 1.0,
            };
            (average * novelty * fit, *snippet)
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

/// A nudge upwards once everything at the learner's level is solid. The level is
/// theirs to set; this only says when the current rung has stopped teaching.
pub fn progress_view<'a>(corpus: &Corpus, progress: &Progress, now: u64) -> ui::ProgressView<'a> {
    // Before a language is chosen, the corpus's main one is what will be offered,
    // so that is what the numbers should describe — not two ladders added together.
    let active = current_language(corpus, progress);
    let in_language = |snippet: &crate::snippet::Snippet| match active.as_deref() {
        Some(language) => snippet.language == language,
        None => true,
    };
    let language_name = active.clone().unwrap_or_default();
    let level = active
        .as_deref()
        .and_then(|language| progress.level_of(language));

    // Everything below counts only this language: reading Go must not move the
    // numbers for Rust.
    let snippets: Vec<&crate::snippet::Snippet> = corpus
        .snippets()
        .iter()
        .filter(|snippet| in_language(snippet))
        .collect();

    let mut at_level: BTreeSet<String> = BTreeSet::new();
    let mut new_at_level = 0usize;
    for snippet in &snippets {
        if level.is_none_or(|level| snippet.levels().from <= level) {
            at_level.extend(snippet.concepts());
            if !progress.seen(&snippet.id) {
                new_at_level += 1;
            }
        }
    }

    let mut ladder: Vec<ui::LevelProgress> = Vec::new();
    for rung in 1..=crate::snippet::MAX_LEVEL {
        let concepts: BTreeSet<String> = snippets
            .iter()
            .filter(|snippet| snippet.levels().from == rung)
            .flat_map(|snippet| snippet.concepts())
            .collect();
        if concepts.is_empty() {
            continue;
        }
        let strength: f64 = concepts
            .iter()
            .map(|concept| {
                progress
                    .concepts
                    .get(concept)
                    .map(|state| state.strength)
                    .unwrap_or(0.0)
            })
            .sum();
        let solid = concepts
            .iter()
            .filter(|concept| {
                progress
                    .concepts
                    .get(*concept)
                    .is_some_and(|state| state.is_mastered())
            })
            .count();
        ladder.push(ui::LevelProgress {
            level: rung,
            strength: strength / concepts.len() as f64,
            solid,
            total: concepts.len(),
        });
    }

    let this_language: BTreeSet<String> = snippets
        .iter()
        .flat_map(|snippet| snippet.concepts())
        .collect();
    let mut needs_work: Vec<ConceptState> = progress
        .concepts
        .iter()
        .filter(|(concept, state)| this_language.contains(*concept) && !state.is_mastered())
        .map(|(_, state)| state.clone())
        .collect();
    needs_work.sort_by(|a, b| b.urgency(now).total_cmp(&a.urgency(now)));

    let others: Vec<(String, usize, usize)> = corpus
        .languages()
        .into_iter()
        .filter(|language| Some(language) != active.as_ref())
        .map(|language| {
            let all: Vec<&crate::snippet::Snippet> = corpus
                .snippets()
                .iter()
                .filter(|snippet| snippet.language == language)
                .collect();
            let seen = all
                .iter()
                .filter(|snippet| progress.seen(&snippet.id))
                .count();
            (language, seen, all.len())
        })
        .collect();

    ui::ProgressView {
        language: language_name,
        others,
        level,
        level_name: level
            .map(|level| crate::snippet::LEVELS[level as usize - 1])
            .unwrap_or(""),
        ladder,
        snippets_seen: snippets
            .iter()
            .filter(|snippet| progress.seen(&snippet.id))
            .count(),
        snippets_total: snippets.len(),
        new_at_level,
        started: this_language
            .iter()
            .filter(|concept| {
                progress
                    .concepts
                    .get(*concept)
                    .is_some_and(|state| state.reps > 0)
            })
            .count(),
        solid: this_language
            .iter()
            .filter(|concept| {
                progress
                    .concepts
                    .get(*concept)
                    .is_some_and(|state| state.is_mastered())
            })
            .count(),
        untouched: at_level
            .iter()
            .filter(|concept| !progress.concepts.contains_key(*concept))
            .count(),
        needs_work,
    }
}

pub fn level_advice(corpus: &Corpus, progress: &Progress) -> Option<String> {
    let language = current_language(corpus, progress);
    let level = progress.level_of(language.as_deref()?)?;
    let language = language.as_deref();
    if level >= MAX_LEVEL {
        return None;
    }
    let mut seen = 0usize;
    let mut mastered = 0usize;
    for snippet in corpus.snippets() {
        if language.is_some_and(|language| snippet.language != language) {
            continue;
        }
        if snippet.levels().to > level {
            continue;
        }
        for concept in snippet.concepts() {
            let Some(state) = progress.concepts.get(&concept) else {
                continue;
            };
            seen += 1;
            if state.is_mastered() {
                mastered += 1;
            }
        }
    }
    if seen >= 3 && mastered * 4 >= seen * 3 {
        return Some(format!(
            "you have cleared level {level} ({mastered} of {seen} concepts solid) — `readcode level {}` opens up harder code",
            level + 1
        ));
    }
    None
}

/// `go` → `Go`, `rust` → `Rust`: a language name in a sentence, not an id.
fn display_name(language: &str) -> String {
    match language {
        "rust" => "Rust".to_string(),
        "go" => "Go".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

/// The one-time question about which language to work in, asked when the corpus
/// carries more than one.
pub fn ask_language(term: &mut Terminal<'_>, languages: &[String]) -> Result<String> {
    writeln!(term.out)?;
    writeln!(
        term.out,
        "{}",
        term.style.bold("Which language are you reading?")
    )?;
    for (index, language) in languages.iter().enumerate() {
        writeln!(
            term.out,
            "  {} {}",
            term.style.cyan(&format!("{} ·", index + 1)),
            language
        )?;
    }
    writeln!(
        term.out,
        "{}",
        term.style
            .dim("each language keeps its own level and progress; `readcode lang <name>` switches")
    )?;

    for attempt in 0..3 {
        write!(
            term.out,
            "{} ",
            term.style
                .cyan(&format!("language 1-{} [1]:", languages.len()))
        )?;
        term.out.flush()?;

        let mut line = String::new();
        let read = term.input.read_line(&mut line)?;
        let answer = line.trim().to_lowercase();
        if read == 0 || answer.is_empty() {
            return Ok(languages[0].clone());
        }
        if let Ok(number) = answer.parse::<usize>() {
            if let Some(language) = languages.get(number.wrapping_sub(1)) {
                return Ok(language.clone());
            }
        }
        if let Some(language) = languages
            .iter()
            .find(|language| language.to_lowercase() == answer)
        {
            return Ok(language.clone());
        }
        if attempt < 2 {
            writeln!(
                term.out,
                "{}",
                term.style.yellow("pick one of the names or numbers above")
            )?;
        }
    }
    Ok(languages[0].clone())
}

/// The one-time question, asked on the normal screen before the sitting so it
/// works the same in the full-screen view and the plain one.
pub fn ask_level(term: &mut Terminal<'_>, language: &str) -> Result<u8> {
    writeln!(term.out)?;
    writeln!(
        term.out,
        "{}",
        term.style.bold(&format!(
            "How much {} can you read right now?",
            display_name(language)
        ))
    )?;
    for (index, description) in LEVELS.iter().enumerate() {
        writeln!(
            term.out,
            "  {} {}",
            term.style.cyan(&format!("{} ·", index + 1)),
            description
        )?;
    }
    writeln!(
        term.out,
        "{}",
        term.style
            .dim("this decides what you are offered first; `readcode level <n>` changes it")
    )?;

    for attempt in 0..3 {
        write!(
            term.out,
            "{} ",
            term.style.cyan(&format!("level 1-{MAX_LEVEL} [1]:"))
        )?;
        term.out.flush()?;

        let mut line = String::new();
        let read = term.input.read_line(&mut line)?;
        let answer = line.trim();
        if read == 0 || answer.is_empty() {
            // Piped input, or enter: first rung. Nothing is lost by starting low.
            return Ok(1);
        }
        match answer.parse::<u8>() {
            Ok(level) if (1..=MAX_LEVEL).contains(&level) => return Ok(level),
            _ => {
                if attempt < 2 {
                    writeln!(
                        term.out,
                        "{}",
                        term.style
                            .yellow(&format!("pick a number from 1 to {MAX_LEVEL}"))
                    )?;
                }
            }
        }
    }
    writeln!(
        term.out,
        "{}",
        term.style.dim("no answer — starting at level 1")
    )?;
    Ok(1)
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
    // Notes keyed by line, so each question shows the ones already given.
    let mut notes: BTreeMap<usize, String> = BTreeMap::new();

    writeln!(term.out)?;
    writeln!(term.out, "{}", style.dim(HELP))?;

    let mut annotations = Vec::with_capacity(prompts.len());
    for (index, prompt) in prompts.iter().enumerate() {
        writeln!(term.out)?;
        if line_mode {
            ui::print_line_prompt(
                term.out,
                style,
                snippet,
                &notes,
                prompt.start,
                &prompt.concepts,
                &prompt.ask,
            )?;
        } else {
            ui::print_block(term.out, style, snippet, prompt.block)?;
        }
        match read_annotation(term)? {
            Some(text) => {
                if line_mode && !text.trim().is_empty() {
                    notes.insert(prompt.start, text.trim().to_string());
                }
                annotations.push(Annotation::new(index, text));
            }
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

    if line_mode {
        writeln!(term.out)?;
        ui::print_annotated(term.out, style, snippet, &notes)?;
    }

    writeln!(term.out)?;
    writeln!(term.out, "{}", style.dim("grading…"))?;
    let grade = match reviewer.grade(snippet, &prompts, &annotations) {
        Ok(grade) => grade,
        // A reply that would not parse is usually the agent's formatting, not the
        // learner's answers: ask once more rather than lose the sitting.
        Err(error) if is_parse_failure(&error) => {
            writeln!(
                term.out,
                "{}",
                style.dim("the reviewer's reply did not parse — asking once more")
            )?;
            reviewer
                .grade(snippet, &prompts, &annotations)
                .context("grading failed; nothing was saved")?
        }
        Err(error) => return Err(error).context("grading failed; nothing was saved"),
    };

    writeln!(term.out)?;
    ui::print_grade(term.out, style, &prompts, &grade)?;

    let (concepts_touched, due_now) = commit(snippet, mode, &grade, progress, now, term)?;

    Ok(Some(Outcome {
        grade,
        mode,
        concepts_touched,
        due_now,
    }))
}

/// The interactive sitting: the code on screen with your notes written on it,
/// one prompt at a time. Falls back to [`run`] when there is no terminal to
/// drive.
pub fn run_tui(
    snippet: &Snippet,
    mode: Asks,
    reviewer: &dyn Reviewer,
    progress: &mut Progress,
    now: u64,
    term: &mut Terminal<'_>,
) -> Result<Option<Outcome>> {
    let mode = snippet.resolve_mode(mode);
    let prompts = snippet.prompts(mode);
    if prompts.is_empty() {
        bail!("snippet `{}` produced no prompts", snippet.id);
    }

    let mut app = tui::App::new(snippet, prompts, mode, reviewer.name());
    // The guard restores the terminal when this block ends, including when
    // grading returns an error.
    let action = {
        let mut guard = tui::Guard::enter()?;
        loop {
            guard.terminal().draw(|frame| tui::draw(frame, &app))?;
            match app.on_key(guard.next_key()?) {
                Action::Abort => break Action::Abort,
                Action::Finish => break Action::Finish,
                Action::None => {}
                Action::Retry => {
                    app.retry();
                    guard.terminal().draw(|frame| tui::draw(frame, &app))?;
                    match reviewer.grade(snippet, &app.prompts, &app.annotations()) {
                        Ok(grade) => app.finish(grade),
                        Err(error) => app.fail(format!("{error:#}")),
                    }
                }
                Action::Grade => {
                    // Drawn before the call, because grading blocks for as long
                    // as the agent takes.
                    guard.terminal().draw(|frame| tui::draw(frame, &app))?;
                    match reviewer.grade(snippet, &app.prompts, &app.annotations()) {
                        Ok(grade) => app.finish(grade),
                        Err(error) => app.fail(format!("{error:#}")),
                    }
                }
            }
        }
    };

    if action == Action::Abort {
        writeln!(term.out)?;
        writeln!(
            term.out,
            "{}",
            term.style.dim("stopped — this snippet was not saved")
        )?;
        return Ok(None);
    }

    let Some(grade) = app.grade.take() else {
        bail!(
            "grading failed; nothing was saved: {}",
            app.error.unwrap_or_else(|| "no grade returned".to_string())
        );
    };

    let (concepts_touched, due_now) = commit(snippet, mode, &grade, progress, now, term)?;

    Ok(Some(Outcome {
        grade,
        mode,
        concepts_touched,
        due_now,
    }))
}

/// Whether a grading error was the reviewer failing to produce parseable JSON,
/// rather than the request never reaching it. Those are worth one more try.
pub fn is_parse_failure(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}").to_lowercase();
    text.contains("json") || text.contains("did not return a grade")
}

/// Folds a grade into progress and reports what changed. Shared by both views,
/// so a sitting recorded through the TUI is indistinguishable from one recorded
/// on the command line.
fn commit(
    snippet: &Snippet,
    mode: Asks,
    grade: &Grade,
    progress: &mut Progress,
    now: u64,
    term: &mut Terminal<'_>,
) -> Result<(usize, usize)> {
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
        term.style.dim(&format!(
            "saved — {} concept{} updated · {} prompt{} graded · {due_now} due for review now · `readcode status` for mastery",
            verdicts.len(),
            if verdicts.len() == 1 { "" } else { "s" },
            grade.prompts.len(),
            if grade.prompts.len() == 1 { "" } else { "s" },
        ))
    )?;

    Ok((verdicts.len(), due_now))
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

    /// Fails the first call with a parse error, then behaves. A reply that will
    /// not parse is the agent's formatting, not the learner's answers, so the
    /// sitting should survive one bad reply.
    struct FlakyReviewer(std::cell::Cell<u32>);

    impl Reviewer for FlakyReviewer {
        fn name(&self) -> String {
            "flaky".to_string()
        }

        fn grade(
            &self,
            snippet: &Snippet,
            prompts: &[crate::snippet::Prompt],
            annotations: &[Annotation],
        ) -> Result<Grade> {
            let calls = self.0.get() + 1;
            self.0.set(calls);
            if calls == 1 {
                anyhow::bail!("the reviewer's JSON could not be parsed (expected value)");
            }
            EchoReviewer.grade(snippet, prompts, annotations)
        }
    }

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
                        teach: None,
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

    fn terminal<'a>(input: &'a mut std::io::Cursor<String>, out: &'a mut Vec<u8>) -> Terminal<'a> {
        Terminal {
            input,
            out,
            style: Style::new(false),
        }
    }

    #[test]
    fn the_first_run_question_takes_a_level() {
        for (stdin, want) in [("3\n", 3), ("5\n", 5), ("\n", 1), ("", 1)] {
            let mut input = std::io::Cursor::new(stdin.to_string());
            let mut out: Vec<u8> = Vec::new();
            let level = ask_level(&mut terminal(&mut input, &mut out), "rust").unwrap();
            assert_eq!(level, want, "stdin {stdin:?}");
            let screen = String::from_utf8_lossy(&out).to_string();
            assert!(screen.contains("How much Rust can you read"), "{screen}");
            for description in LEVELS {
                assert!(screen.contains(description), "{screen}");
            }
        }
    }

    #[test]
    fn a_wrong_answer_is_asked_again_and_then_accepted() {
        let mut input = std::io::Cursor::new("9\nyes\n2\n".to_string());
        let mut out: Vec<u8> = Vec::new();
        let level = ask_level(&mut terminal(&mut input, &mut out), "rust").unwrap();
        assert_eq!(level, 2);
        let screen = String::from_utf8_lossy(&out).to_string();
        assert_eq!(screen.matches("pick a number").count(), 2, "{screen}");
    }

    #[test]
    fn nonsense_answers_fall_back_to_the_first_rung() {
        let mut input = std::io::Cursor::new("nope\nnope\nnope\n".to_string());
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            ask_level(&mut terminal(&mut input, &mut out), "rust").unwrap(),
            1
        );
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
    fn the_declared_level_limits_what_is_offered() {
        let corpus = corpus();
        // One rung of stretch above a declared level 1 is allowed; a level 3
        // snippet is not, however due it might be.
        for seed in 0..200u64 {
            let mut progress = Progress {
                level: Some(1),
                ..Progress::default()
            };
            progress.last_snippet = None;
            let picked = pick(&corpus, &progress, seed).expect("a snippet");
            assert!(
                picked.levels().from <= 2,
                "level 1 learner offered {:?} ({})",
                picked.id,
                picked.levels().label()
            );
        }
    }

    #[test]
    fn a_beginner_is_started_on_the_simplest_snippet() {
        let corpus = corpus();
        let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
        for seed in 0..300u64 {
            let progress = Progress {
                level: Some(1),
                ..Progress::default()
            };
            let picked = pick(&corpus, &progress, seed).expect("a snippet");
            *counts.entry(picked.levels().from.to_string()).or_default() += 1;
        }
        let easiest = counts.get("1").copied().unwrap_or(0);
        let next = counts.get("2").copied().unwrap_or(0);
        assert!(
            easiest > 0,
            "nothing at level 1 was ever offered: {counts:?}"
        );
        assert!(
            easiest > next,
            "level 1 should dominate at level 1: {counts:?}"
        );
    }

    #[test]
    fn a_declared_level_above_the_corpus_still_offers_something() {
        let corpus = corpus();
        let progress = Progress {
            level: Some(5),
            ..Progress::default()
        };
        assert!(pick(&corpus, &progress, 7).is_some());
    }

    #[test]
    fn no_declared_level_means_no_filter() {
        let corpus = corpus();
        let mut seen_high = false;
        for seed in 0..200u64 {
            let progress = Progress::default();
            let picked = pick(&corpus, &progress, seed).expect("a snippet");
            if picked.levels().from >= 3 {
                seen_high = true;
            }
        }
        assert!(
            seen_high,
            "an unset level should not hide the harder snippets"
        );
    }

    #[test]
    fn a_answered_snippet_is_not_offered_again_while_the_rung_has_new_work() {
        let corpus = corpus();
        let now = 5_000_000u64;
        let mut progress = Progress {
            level: Some(1),
            ..Progress::default()
        };
        // Answer every level-1 snippet once, correctly: seen, and scheduled ahead.
        let level_one: Vec<_> = corpus
            .snippets()
            .iter()
            .filter(|snippet| snippet.levels().from <= 1)
            .cloned()
            .collect();
        for snippet in &level_one {
            progress.seen_snippets.push(snippet.id.clone());
            for concept in snippet.concepts() {
                progress.concept_mut(&concept).apply(Verdict::Correct, now);
            }
        }

        // Level 1 is now answered and not due, so the next rung is fair game even
        // though nothing is "mastered" yet.
        let mut offered: Vec<String> = Vec::new();
        for seed in 0..40u64 {
            let picked = pick(&corpus, &progress, seed).unwrap();
            offered.push(picked.id.clone());
        }
        offered.sort();
        offered.dedup();
        assert!(
            offered
                .iter()
                .any(|id| !level_one.iter().any(|s| &s.id == id)),
            "only finished level-1 snippets were offered: {offered:?}"
        );
    }

    #[test]
    fn a_concept_that_came_due_pulls_its_snippet_back() {
        let corpus = corpus();
        let now = 9_000_000u64;
        let mut progress = Progress {
            level: Some(1),
            ..Progress::default()
        };
        // One language's worth: `pick` never leaves the language you are in, so a
        // Go snippet in this list would never be offered back.
        let level_one: Vec<_> = corpus
            .snippets()
            .iter()
            .filter(|snippet| snippet.language == "rust" && snippet.levels().from <= 1)
            .cloned()
            .collect();
        for snippet in &level_one {
            progress.seen_snippets.push(snippet.id.clone());
            for concept in snippet.concepts() {
                progress.concept_mut(&concept).apply(Verdict::Correct, now);
            }
        }
        // One concept from the first snippet falls due long before the rest.
        let first = &level_one[0];
        let concept = first.concepts().into_iter().next().unwrap();
        progress.concept_mut(&concept).due_at = now - 1;

        // `pick` reads the clock as well as seeding its draw, so ask it about a
        // time after the due date, not about the epoch.
        let mut hits = 0;
        let mut seen_ids = std::collections::BTreeSet::new();
        for seed in 0..40u64 {
            let picked = pick(&corpus, &progress, now + seed * 7).unwrap();
            seen_ids.insert(picked.id.clone());
            if picked.id == first.id {
                hits += 1;
            }
        }
        assert!(
            hits > 0,
            "the due snippet never came back; picked {seen_ids:?}"
        );
    }

    #[test]
    fn the_advice_appears_once_the_rung_is_solid() {
        let corpus = corpus();
        let mut progress = Progress {
            level: Some(1),
            ..Progress::default()
        };
        assert!(level_advice(&corpus, &progress).is_none());

        let at_level: Vec<String> = corpus
            .snippets()
            .iter()
            .filter(|snippet| snippet.levels().to <= 1)
            .flat_map(|snippet| snippet.concepts().into_iter())
            .collect();
        assert!(!at_level.is_empty(), "the corpus needs level-1 content");

        for concept in &at_level {
            for _ in 0..4 {
                progress.concept_mut(concept).apply(Verdict::Correct, 1_000);
            }
        }
        let advice = level_advice(&corpus, &progress).expect("advice once the rung is solid");
        assert!(advice.contains("cleared level 1"), "{advice}");
        assert!(advice.contains("readcode level 2"), "{advice}");
    }

    #[test]
    fn one_unparseable_reply_does_not_lose_the_sitting() {
        let corpus = corpus();
        let snippet = corpus.find("rust-002").unwrap();
        let mut progress = Progress::default();
        let stdin = answers(snippet, Asks::Blocks, "an explanation");
        let mut input = std::io::Cursor::new(stdin);
        let mut out: Vec<u8> = Vec::new();
        let mut term = Terminal {
            input: &mut input,
            out: &mut out,
            style: Style::new(false),
        };
        let reviewer = FlakyReviewer(std::cell::Cell::new(0));

        let outcome = run(
            snippet,
            Asks::Blocks,
            &reviewer,
            &mut progress,
            2_000,
            &mut term,
        )
        .unwrap();

        assert!(outcome.is_some(), "the sitting was lost");
        assert_eq!(reviewer.0.get(), 2, "it should have asked twice");
        let screen = String::from_utf8_lossy(&out).to_string();
        assert!(screen.contains("did not parse"), "{screen}");
        assert!(screen.contains("saved —"), "{screen}");
    }

    #[test]
    fn a_full_sitting_saves_progress_and_schedules_concepts() {
        let corpus = corpus();
        let snippet = corpus.find("rust-002").unwrap();
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
        let snippet = corpus.find("rust-005").unwrap();
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
        // Each question brings its own window, and the answers end up as
        // comments in the annotated listing.
        assert!(screen.contains("asked line by line"), "{screen}");
        assert!(screen.contains("┌ line 6 · "), "{screen}");
        assert!(screen.contains("annotated · rust-005"), "{screen}");
        assert!(screen.contains("// an explanation"), "{screen}");
    }

    #[test]
    fn unknown_answers_come_back_within_the_sitting() {
        let corpus = corpus();
        let snippet = corpus.find("rust-002").unwrap();
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
        let snippet = corpus.find("rust-001").unwrap();
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
        let snippet = corpus.find("rust-001").unwrap();
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
        let snippet = corpus.find("rust-005").unwrap();
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
        let target = corpus.find("rust-003").unwrap();
        for concept in target.concepts() {
            let state = progress.concept_mut(&concept);
            state.strength = 0.0;
            state.reps = 1;
            state.due_at = 0;
        }

        // Concepts are shared between snippets, so zeroing rust-003's concepts
        // also weakens every snippet that teaches the same ids. The assertion is
        // therefore about the weak concepts, not about one id.
        let weak: std::collections::BTreeSet<String> = target.concepts().into_iter().collect();
        let mut hits = 0;
        for seed in 0..40u64 {
            let picked = pick(&corpus, &progress, seed).unwrap();
            let overlaps = picked.concepts().intersection(&weak).next().is_some();
            assert!(
                overlaps,
                "picked {} which teaches none of the weak concepts",
                picked.id
            );
            if picked.id == target.id {
                hits += 1;
            }
        }
        assert!(hits > 5, "weak snippet was picked {hits}/40 times");
    }

    #[test]
    fn a_line_sitting_schedules_every_concept_like_a_block_sitting() {
        let corpus = corpus();
        let snippet = corpus.find("rust-005").unwrap();
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

    // ---- languages -------------------------------------------------------

    fn a_progress_in(language: Option<&str>) -> Progress {
        Progress {
            language: language.map(str::to_string),
            ..Progress::default()
        }
    }

    #[test]
    fn a_sitting_stays_in_the_language_you_chose() {
        let corpus = Corpus::builtin().unwrap();
        let progress = a_progress_in(Some("go"));
        let now = 1_700_000_000;

        for _ in 0..40 {
            let picked = pick(&corpus, &progress, now).unwrap();
            assert_eq!(picked.language, "go", "{}", picked.id);
        }
    }

    #[test]
    fn the_level_is_kept_per_language() {
        let mut progress = Progress::default();
        progress.set_level("rust", 4);
        progress.set_level("go", 1);
        assert_eq!(progress.level_of("rust"), Some(4));
        assert_eq!(progress.level_of("go"), Some(1));
        // A language that was never set has no level, so it gets asked about.
        assert_eq!(progress.level_of("python"), None);
    }

    #[test]
    fn a_level_from_a_single_language_file_still_applies_to_rust() {
        // Progress written before languages existed: one `level`, no `levels`.
        let progress = Progress {
            level: Some(3),
            ..Progress::default()
        };
        assert_eq!(progress.level_of("rust"), Some(3));
        // It must not leak into a language the file never mentioned.
        assert_eq!(progress.level_of("go"), None);
    }

    #[test]
    fn a_level_narrows_the_go_menu_too() {
        let corpus = Corpus::builtin().unwrap();
        let mut progress = a_progress_in(Some("go"));
        progress.set_level("go", 1);

        for _ in 0..40 {
            let picked = pick(&corpus, &progress, 1_700_000_000).unwrap();
            assert_eq!(picked.language, "go");
            assert!(
                picked.levels().reachable(1),
                "{} is above level 1",
                picked.id
            );
        }
    }

    #[test]
    fn progress_in_one_language_does_not_count_in_another() {
        let corpus = Corpus::builtin().unwrap();
        let mut progress = a_progress_in(Some("go"));
        progress.set_level("go", 1);

        // Read two Go snippets and meet one Go concept.
        let go: Vec<String> = corpus
            .snippets()
            .iter()
            .filter(|snippet| snippet.language == "go")
            .take(2)
            .map(|snippet| snippet.id.clone())
            .collect();
        for id in &go {
            progress.seen_snippets.push(id.clone());
        }

        let view = progress_view(&corpus, &progress, 1_700_000_000);
        assert_eq!(view.language, "go");
        assert_eq!(view.snippets_seen, 2);
        let go_total = corpus
            .snippets()
            .iter()
            .filter(|snippet| snippet.language == "go")
            .count();
        assert_eq!(view.snippets_total, go_total);
        assert!(
            view.others
                .iter()
                .any(|(language, _, _)| language == "rust"),
            "the other language should still be offered: {:?}",
            view.others
        );
    }

    #[test]
    fn the_ladder_only_shows_rungs_this_language_reaches() {
        let corpus = Corpus::builtin().unwrap();
        let mut progress = a_progress_in(Some("go"));
        progress.set_level("go", 1);

        let view = progress_view(&corpus, &progress, 1_700_000_000);
        let go_top = corpus
            .snippets()
            .iter()
            .filter(|snippet| snippet.language == "go")
            .map(|snippet| snippet.levels().to)
            .max()
            .unwrap();
        assert_eq!(view.ladder.last().map(|rung| rung.level), Some(go_top));
    }
}
