//! Terminal rendering. Colour is opt-out (`--no-color` or `NO_COLOR`), and
//! every layout degrades to plain text when it is off.

use std::collections::BTreeMap;
use std::io::{self, Write};

use crate::review::Grade;
use crate::snippet::{Prompt, Snippet};
use crate::srs::{ConceptState, Verdict, describe_due};

#[derive(Debug, Clone, Copy)]
pub struct Style {
    color: bool,
}

impl Style {
    pub fn new(color: bool) -> Self {
        Self { color }
    }

    pub fn from_env(disabled: bool) -> Self {
        Self {
            color: !disabled && std::env::var_os("NO_COLOR").is_none(),
        }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    pub fn bold(&self, text: &str) -> String {
        self.paint("1", text)
    }

    pub fn dim(&self, text: &str) -> String {
        self.paint("2", text)
    }

    pub fn green(&self, text: &str) -> String {
        self.paint("32", text)
    }

    pub fn red(&self, text: &str) -> String {
        self.paint("31", text)
    }

    pub fn yellow(&self, text: &str) -> String {
        self.paint("33", text)
    }

    pub fn cyan(&self, text: &str) -> String {
        self.paint("36", text)
    }

    pub fn magenta(&self, text: &str) -> String {
        self.paint("35", text)
    }

    pub fn blue(&self, text: &str) -> String {
        self.paint("34", text)
    }
}

/// The snippet's source, syntax-highlighted once so every view shares it.
fn highlighted(snippet: &Snippet, style: Style) -> Vec<String> {
    let lines = snippet.code_lines();
    crate::highlight::highlight(&lines, style)
}

pub fn verdict_label(style: Style, verdict: Verdict) -> String {
    match verdict {
        Verdict::Correct => style.green("✓ correct"),
        Verdict::Partial => style.yellow("~ partial"),
        Verdict::Wrong => style.red("✗ wrong"),
        Verdict::Unknown => style.magenta("? unknown"),
    }
}

/// An eight-cell mastery bar plus the percentage.
pub fn bar(style: Style, strength: f64) -> String {
    let strength = strength.clamp(0.0, 1.0);
    let filled = (strength * 8.0).round() as usize;
    let cells = format!("{}{}", "█".repeat(filled), "░".repeat(8 - filled));
    let painted = if strength >= 0.85 {
        style.green(&cells)
    } else if strength >= 0.5 {
        style.yellow(&cells)
    } else {
        style.red(&cells)
    };
    format!("{painted} {:>3.0}%", strength * 100.0)
}

/// Number width for the gutter, so lines stay aligned down a snippet.
pub fn gutter_width(snippet: &Snippet) -> usize {
    snippet.line_count().to_string().len().max(2)
}

pub fn print_header(out: &mut dyn Write, style: Style, snippet: &Snippet) -> io::Result<()> {
    writeln!(
        out,
        "{}",
        style.bold(&format!("{} · {}", snippet.language, snippet.title))
    )?;
    writeln!(out, "{}", style.dim(&snippet.id))?;
    if let Some(intro) = &snippet.intro {
        writeln!(out, "{}", style.dim(intro))?;
    }
    if let Some(source) = &snippet.source {
        writeln!(out, "{}", style.dim(&format!("source: {source}")))?;
    }
    Ok(())
}

/// The whole snippet, grouped by block. Line mode shows this once, so the
/// learner has the surrounding code while answering one line at a time.
pub fn print_code(out: &mut dyn Write, style: Style, snippet: &Snippet) -> io::Result<()> {
    let lines = highlighted(snippet, style);
    let width = gutter_width(snippet);
    for (index, block) in snippet.blocks.iter().enumerate() {
        let (start, end) = (block.lines[0], block.lines[1]);
        writeln!(out)?;
        writeln!(
            out,
            "{}",
            style.cyan(&format!("── block {} · lines {start}-{end}", index + 1))
        )?;
        for number in start..=end {
            if let Some(line) = lines.get(number - 1) {
                writeln!(
                    out,
                    "{} {}",
                    style.dim(&format!("{number:>width$} │")),
                    line
                )?;
            }
        }
    }
    Ok(())
}

/// One block with its lines and question: the block-mode unit.
pub fn print_block(
    out: &mut dyn Write,
    style: Style,
    snippet: &Snippet,
    block_index: usize,
) -> io::Result<()> {
    let Some(block) = snippet.blocks.get(block_index) else {
        return Ok(());
    };
    let (start, end) = (block.lines[0], block.lines[1]);
    let lines = highlighted(snippet, style);
    let width = gutter_width(snippet);
    writeln!(
        out,
        "{}",
        style.cyan(&format!(
            "┌ block {} · lines {start}-{end}",
            block_index + 1
        ))
    )?;
    for number in start..=end {
        if let Some(line) = lines.get(number - 1) {
            writeln!(
                out,
                "{} {}",
                style.dim(&format!("{number:>width$} │")),
                line
            )?;
        }
    }
    writeln!(out, "{}", style.dim(&format!("└ {}", block.ask)))?;
    Ok(())
}

/// Context around the line being asked, so a single line is not asked in a vacuum.
const WINDOW: usize = 2;

/// One line-mode question, answered the way you would annotate a file: the
/// surrounding code, your notes already sitting above the lines they belong to,
/// and the line in question marked.
pub fn print_line_prompt(
    out: &mut dyn Write,
    style: Style,
    snippet: &Snippet,
    notes: &BTreeMap<usize, String>,
    number: usize,
    concepts: &[String],
    ask: &str,
) -> io::Result<()> {
    let concepts = concepts.join(", ");
    let header = if concepts.is_empty() {
        format!("┌ line {number}")
    } else {
        format!("┌ line {number} · {concepts}")
    };
    writeln!(out, "{}", style.cyan(&header))?;

    let first = number.saturating_sub(WINDOW).max(1);
    let last = (number + WINDOW).min(snippet.line_count());
    let lines = snippet.code_lines();
    let painted = crate::highlight::highlight(&lines[first - 1..last], style);
    let width = gutter_width(snippet);

    for (offset, code) in painted.iter().enumerate() {
        let current = first + offset;
        if let Some(note) = notes.get(&current) {
            for line in note.lines() {
                writeln!(
                    out,
                    "{} {}",
                    // One column wider than the code gutter: the marker column.
                    style.dim(&format!("{:>pad$} │", "", pad = width + 1)),
                    style.dim(&format!("// {line}"))
                )?;
            }
        }
        // ASCII on purpose: a fancy marker falls back to a proportional font in
        // many terminals and knocks the gutter out of alignment.
        let marker = if current == number { ">" } else { " " };
        let gutter = format!("{marker}{current:>width$} │");
        if current == number {
            writeln!(out, "{} {}", style.cyan(&gutter), code)?;
        } else {
            writeln!(out, "{} {}", style.dim(&gutter), code)?;
        }
    }
    writeln!(out, "{}", style.bold(ask))?;
    Ok(())
}

/// The whole snippet with your notes above the lines they belong to: the file you
/// would have written if you had annotated it by hand.
pub fn print_annotated(
    out: &mut dyn Write,
    style: Style,
    snippet: &Snippet,
    notes: &BTreeMap<usize, String>,
) -> io::Result<()> {
    writeln!(
        out,
        "{}",
        style.bold(&format!("annotated · {}", snippet.id))
    )?;
    let lines = snippet.code_lines();
    let painted = crate::highlight::highlight(&lines, style);
    let width = gutter_width(snippet);

    for (index, code) in painted.iter().enumerate() {
        let number = index + 1;
        if let Some(note) = notes.get(&number) {
            for line in note.lines() {
                writeln!(
                    out,
                    "{} {}",
                    style.dim(&format!("{:>width$} │", "")),
                    style.dim(&format!("// {line}"))
                )?;
            }
        }
        writeln!(
            out,
            "{} {}",
            style.dim(&format!("{number:>width$} │")),
            code
        )?;
    }
    Ok(())
}

pub fn print_grade(
    out: &mut dyn Write,
    style: Style,
    prompts: &[Prompt],
    grade: &Grade,
) -> io::Result<()> {
    writeln!(
        out,
        "{}",
        style.bold(&format!("review · {}", grade.reviewer))
    )?;

    for graded in &grade.prompts {
        let span = prompts
            .get(graded.prompt)
            .map(Prompt::span_label)
            .unwrap_or_default();
        writeln!(out)?;
        writeln!(
            out,
            "  {} {}",
            style.bold(&format!("prompt {} · {span}", graded.prompt + 1)),
            verdict_label(style, graded.verdict)
        )?;
        for (concept, verdict) in &graded.concepts {
            writeln!(
                out,
                "    {:<34} {}",
                concept,
                verdict_label(style, *verdict)
            )?;
        }
        if let Some(question) = prompts.get(graded.prompt) {
            writeln!(
                out,
                "    {} {}",
                style.dim("ask ·"),
                style.dim(&question.ask)
            )?;
        }
        if !graded.feedback.is_empty() {
            writeln!(out, "    {}", graded.feedback)?;
        }
        for missing in &graded.missing {
            writeln!(
                out,
                "    {} {}",
                style.yellow("missed"),
                style.dim(&format!("· {missing}"))
            )?;
        }
        if let Some(teach) = &graded.teach {
            writeln!(out)?;
            for line in teach.lines() {
                writeln!(out, "    {} {}", style.cyan("why"), line)?;
            }
        }
        if let Some(prompt) = prompts.get(graded.prompt) {
            for doc in crate::docs::for_concepts(prompt.concepts.iter().map(String::as_str)) {
                writeln!(out, "    {} {}", style.dim("docs ·"), style.dim(&doc.title))?;
                writeln!(out, "           {}", style.dim(&doc.url))?;
            }
        }
    }

    if !grade.summary.is_empty() {
        writeln!(out)?;
        writeln!(out, "  {}", grade.summary)?;
    }
    Ok(())
}

/// Groups stay contiguous and the weakest group comes first: a plain
/// urgency sort interleaves groups and repeats their headers.
/// Everything `readcode status` needs to answer three questions: where am I, what
/// is due, and what should I do next. Built by the caller, which is the only place
/// that has both the corpus and the progress.
pub struct ProgressView<'a> {
    pub level: Option<u8>,
    pub level_name: &'a str,
    /// Concepts in snippets at or below the learner's level, and how many are solid.
    pub solid_at_level: usize,
    pub total_at_level: usize,
    /// Summed strength of the concepts at this level, for a bar that moves.
    pub strength_at_level: f64,
    pub snippets_seen: usize,
    pub snippets_total: usize,
    pub new_at_level: usize,
    pub due_now: usize,
    pub next_due: Option<u64>,
    pub needs_work: Vec<ConceptState>,
    pub solid: usize,
}

pub fn print_progress(
    out: &mut dyn Write,
    style: Style,
    view: &ProgressView<'_>,
    now: u64,
) -> io::Result<()> {
    match view.level {
        Some(level) => writeln!(
            out,
            "{}",
            style.bold(&format!("level {level} · {}", view.level_name))
        )?,
        None => writeln!(
            out,
            "{}",
            style.dim("no level set — `readcode level <1-5>` or just run `readcode`")
        )?,
    }

    if view.total_at_level > 0 {
        // Average strength, not the count of mastered concepts: a first correct
        // answer is real progress, and a bar that stays at 0% for three sittings
        // tells the learner nothing moved.
        let ratio = view.strength_at_level / view.total_at_level as f64;
        writeln!(
            out,
            "  {}  {}/{} solid at this level",
            bar(style, ratio),
            view.solid_at_level,
            view.total_at_level
        )?;
    }
    if view.snippets_seen == 0 {
        writeln!(out)?;
        writeln!(
            out,
            "{}",
            style.dim("nothing annotated yet — `readcode` starts you on the first snippet")
        )?;
        return Ok(());
    }

    writeln!(out)?;
    writeln!(
        out,
        "  {} of {} snippets seen · {}",
        view.snippets_seen,
        view.snippets_total,
        match view.new_at_level {
            0 => "nothing new at your level".to_string(),
            1 => "1 new at your level".to_string(),
            many => format!("{many} new at your level"),
        }
    )?;
    writeln!(
        out,
        "  {}",
        match (view.due_now, view.next_due) {
            (0, Some(next)) => format!("nothing due now · next review {}", describe_due(now, next)),
            (0, None) => "nothing due — every concept is scheduled".to_string(),
            (many, _) => format!("{many} due now"),
        }
    )?;

    writeln!(out)?;
    if view.needs_work.is_empty() {
        writeln!(
            out,
            "{}",
            style.dim("nothing needs work: every concept is solid and scheduled")
        )?;
        return Ok(());
    }

    writeln!(out, "{}", style.bold("needs work"))?;
    const SHOWN: usize = 8;
    for state in view.needs_work.iter().take(SHOWN) {
        let (group, leaf) = match state.id.rsplit_once('.') {
            Some((group, leaf)) => (group, leaf),
            None => ("", state.id.as_str()),
        };
        writeln!(
            out,
            "  {:<32} {}  {}",
            format!("{group}.{leaf}"),
            bar(style, state.strength),
            describe_due(now, state.due_at)
        )?;
    }
    if view.needs_work.len() > SHOWN {
        writeln!(
            out,
            "  {}",
            style.dim(&format!(
                "and {} more — `readcode` works through them as they come due",
                view.needs_work.len() - SHOWN
            ))
        )?;
    }
    writeln!(
        out,
        "  {}",
        style.dim(&format!(
            "{} concept{} solid in total",
            view.solid,
            plural(view.solid)
        ))
    )?;
    Ok(())
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

pub fn order_for_status(states: Vec<ConceptState>, now: u64) -> Vec<ConceptState> {
    let mut groups: std::collections::BTreeMap<String, Vec<ConceptState>> =
        std::collections::BTreeMap::new();
    for state in states {
        let group = state
            .id
            .rsplit_once('.')
            .map(|(group, _)| group.to_string())
            .unwrap_or_default();
        groups.entry(group).or_default().push(state);
    }

    let mut ranked: Vec<(f64, Vec<ConceptState>)> = groups
        .into_values()
        .map(|mut group| {
            group.sort_by(|a, b| b.urgency(now).total_cmp(&a.urgency(now)));
            let peak = group.first().map(|state| state.urgency(now)).unwrap_or(0.0);
            (peak, group)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    ranked.into_iter().flat_map(|(_, group)| group).collect()
}

pub fn print_status(
    out: &mut dyn Write,
    style: Style,
    states: &[ConceptState],
    now: u64,
) -> io::Result<()> {
    if states.is_empty() {
        writeln!(
            out,
            "{}",
            style.dim("nothing tracked yet — run `readcode` to annotate your first snippet")
        )?;
        return Ok(());
    }

    writeln!(out, "{}", style.bold("mastery"))?;
    let mut group_shown = String::new();
    for state in states {
        let (group, leaf) = match state.id.rsplit_once('.') {
            Some((group, leaf)) => (group.to_string(), leaf.to_string()),
            None => (String::new(), state.id.clone()),
        };
        if group != group_shown {
            writeln!(out)?;
            writeln!(out, "  {}", style.cyan(&group))?;
            group_shown = group;
        }
        writeln!(
            out,
            "    {:<26} {}  {}",
            leaf,
            bar(style, state.strength),
            describe_due(now, state.due_at)
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(view: &ProgressView<'_>) -> String {
        let mut out: Vec<u8> = Vec::new();
        print_progress(&mut out, Style::new(false), view, 1_000).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn view<'a>(_states: &'a [ConceptState], needs_work: Vec<ConceptState>) -> ProgressView<'a> {
        ProgressView {
            level: Some(2),
            level_name: "small functions",
            solid_at_level: 2,
            total_at_level: 8,
            strength_at_level: 3.2,
            snippets_seen: 3,
            snippets_total: 9,
            new_at_level: 4,
            due_now: 1,
            next_due: Some(5_000),
            needs_work,
            solid: 2,
        }
    }

    #[test]
    fn progress_leads_with_the_level_the_bar_and_what_is_due() {
        let mut state = ConceptState::new("rust.binding.mut");
        state.strength = 0.4;
        state.due_at = 900;
        let states = vec![state.clone()];
        let screen = rendered(&view(&states, vec![state]));

        assert!(screen.contains("level 2 · small functions"), "{screen}");
        assert!(screen.contains("2/8 solid at this level"), "{screen}");
        // 3.2 of 8 is 40%, so the bar is not empty even though nothing is mastered.
        assert!(screen.contains("40%"), "{screen}");
        assert!(screen.contains("3 of 9 snippets seen"), "{screen}");
        assert!(screen.contains("4 new at your level"), "{screen}");
        assert!(screen.contains("1 due now"), "{screen}");
        assert!(screen.contains("rust.binding.mut"), "{screen}");
        assert!(screen.contains("2 concepts solid in total"), "{screen}");
    }

    #[test]
    fn a_silent_rung_says_so_instead_of_showing_an_empty_table() {
        let screen = rendered(&view(&[], Vec::new()));
        assert!(screen.contains("nothing needs work"), "{screen}");
    }

    #[test]
    fn a_fresh_profile_is_told_what_to_do() {
        let empty = ProgressView {
            level: Some(1),
            level_name: "just started",
            solid_at_level: 0,
            total_at_level: 6,
            strength_at_level: 0.0,
            snippets_seen: 0,
            snippets_total: 9,
            new_at_level: 2,
            due_now: 0,
            next_due: None,
            needs_work: Vec::new(),
            solid: 0,
        };
        let screen = rendered(&empty);
        assert!(screen.contains("nothing annotated yet"), "{screen}");
        assert!(!screen.contains("needs work"), "{screen}");
    }

    #[test]
    fn a_long_needs_work_list_is_capped() {
        let states: Vec<ConceptState> = (0..12)
            .map(|index| {
                let mut state = ConceptState::new(&format!("rust.thing.{index}"));
                state.strength = 0.2;
                state.due_at = 500;
                state
            })
            .collect();
        let screen = rendered(&view(&states, states.clone()));
        assert!(screen.contains("and 4 more"), "{screen}");
    }
    use crate::corpus::Corpus;
    use crate::snippet::Asks;

    #[test]
    fn status_order_keeps_groups_contiguous() {
        let now = 1_000;
        let mut states = Vec::new();
        for (id, strength) in [
            ("rust.a.weak", 0.0),
            ("rust.b.strong", 1.0),
            ("rust.a.strong", 1.0),
            ("rust.b.weak", 0.1),
        ] {
            let mut state = ConceptState::new(id);
            state.strength = strength;
            state.reps = 1;
            states.push(state);
        }
        let ordered: Vec<String> = order_for_status(states, now)
            .into_iter()
            .map(|state| state.id)
            .collect();
        // rust.a has the weakest concept, so it comes first, and its two
        // concepts stay together.
        assert_eq!(
            ordered,
            vec![
                "rust.a.weak",
                "rust.a.strong",
                "rust.b.weak",
                "rust.b.strong"
            ]
        );
    }

    #[test]
    fn plain_output_has_no_escape_codes() {
        let style = Style::new(false);
        assert_eq!(style.bold("x"), "x");
        assert_eq!(verdict_label(style, Verdict::Correct), "✓ correct");
    }

    #[test]
    fn a_bar_saturates_at_both_ends() {
        let style = Style::new(false);
        assert!(bar(style, 0.0).contains("░░░░░░░░"));
        assert!(bar(style, 1.0).contains("████████"));
        // Out-of-range input cannot produce a bar of the wrong width.
        let wide = bar(style, 4.0);
        assert_eq!(wide.matches('█').count(), 8);
    }

    fn line_fixture() -> (crate::snippet::Snippet, Vec<Prompt>) {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap().clone();
        let prompts = snippet.prompts(Asks::Lines);
        (snippet, prompts)
    }

    #[test]
    fn a_line_prompt_shows_a_window_and_marks_the_line() {
        let (snippet, prompts) = line_fixture();
        let prompt = prompts
            .iter()
            .find(|prompt| prompt.start == 6)
            .expect("line 6 is asked");
        let mut out: Vec<u8> = Vec::new();
        print_line_prompt(
            &mut out,
            Style::new(false),
            &snippet,
            &BTreeMap::new(),
            6,
            &prompt.concepts,
            &prompt.ask,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();

        assert!(text.contains("┌ line 6 · rust."), "{text}");
        assert!(text.contains("> 6 │"), "the line is marked: {text}");
        assert!(text.contains(&prompt.ask), "{text}");
        // Context, not the whole snippet.
        assert!(text.contains(" 4 │"), "context above: {text}");
        assert!(
            !text.contains(" 1 │"),
            "line 1 is out of the window: {text}"
        );
        assert!(
            !text.contains(" 9 │"),
            "line 9 is out of the window: {text}"
        );
    }

    #[test]
    fn an_existing_note_sits_above_its_line() {
        let (snippet, prompts) = line_fixture();
        let mut notes = BTreeMap::new();
        notes.insert(4usize, "counts has to be mutable".to_string());
        let prompt = prompts.iter().find(|p| p.start == 6).unwrap();
        let mut out: Vec<u8> = Vec::new();
        print_line_prompt(
            &mut out,
            Style::new(false),
            &snippet,
            &notes,
            6,
            &prompt.concepts,
            &prompt.ask,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        let note_at = text
            .find("// counts has to be mutable")
            .expect("note shown");
        let line_at = text.find(" 4 │     let mut counts").expect("line 4 shown");
        assert!(note_at < line_at, "note precedes its line:\n{text}");
    }

    #[test]
    fn the_annotated_listing_covers_the_whole_snippet() {
        let (snippet, _) = line_fixture();
        let mut notes = BTreeMap::new();
        notes.insert(4usize, "counts is mutable".to_string());
        notes.insert(6usize, "entry + or_insert\nsecond line".to_string());
        let mut out: Vec<u8> = Vec::new();
        print_annotated(&mut out, Style::new(false), &snippet, &notes).unwrap();
        let text = String::from_utf8(out).unwrap();

        assert!(text.contains("annotated · rust-005"), "{text}");
        assert!(text.contains("// counts is mutable"), "{text}");
        // A multi-line note stays multi-line, each line commented.
        assert!(text.contains("// entry + or_insert"), "{text}");
        assert!(text.contains("// second line"), "{text}");
        // Every code line is present, including ones nobody asked about.
        assert!(
            text.contains(" 1 │ use std::collections::HashMap;"),
            "{text}"
        );
        assert!(text.contains(" 9 │ }"), "{text}");
    }

    #[test]
    fn the_whole_snippet_is_shown_with_block_boundaries() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("003").unwrap();
        let mut out: Vec<u8> = Vec::new();
        print_code(&mut out, Style::new(false), snippet).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("── block 1 · lines 1-6"), "{text}");
        assert!(text.contains("── block 3 · lines 11-13"), "{text}");
        assert!(text.contains(".filter("), "{text}");
    }
}
