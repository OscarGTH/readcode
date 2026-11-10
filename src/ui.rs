//! Terminal rendering. Colour is opt-out (`--no-color` or `NO_COLOR`), and
//! every layout degrades to plain text when it is off.

use std::io::{self, Write};

use crate::review::Grade;
use crate::snippet::Snippet;
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

pub fn print_snippet(out: &mut dyn Write, style: Style, snippet: &Snippet) -> io::Result<()> {
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

    let lines = snippet.code_lines();
    let width = snippet.line_count().to_string().len().max(2);

    for (index, block) in snippet.blocks.iter().enumerate() {
        let (start, end) = (block.lines[0], block.lines[1]);
        writeln!(out)?;
        writeln!(
            out,
            "{}",
            style.cyan(&format!("┌ block {} · lines {start}-{end}", index + 1))
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
    }
    Ok(())
}

pub fn print_grade(
    out: &mut dyn Write,
    style: Style,
    snippet: &Snippet,
    grade: &Grade,
) -> io::Result<()> {
    writeln!(
        out,
        "{}",
        style.bold(&format!("review · {}", grade.reviewer))
    )?;

    for block in &grade.blocks {
        let (start, end) = snippet
            .blocks
            .get(block.block)
            .map(|b| (b.lines[0], b.lines[1]))
            .unwrap_or((0, 0));
        writeln!(out)?;
        writeln!(
            out,
            "  {} {}",
            style.bold(&format!("block {} · lines {start}-{end}", block.block + 1)),
            verdict_label(style, block.verdict)
        )?;
        for (concept, verdict) in &block.concepts {
            writeln!(
                out,
                "    {:<34} {}",
                concept,
                verdict_label(style, *verdict)
            )?;
        }
        if !block.feedback.is_empty() {
            writeln!(out, "    {}", block.feedback)?;
        }
        for missing in &block.missing {
            writeln!(out, "    {} {}", style.yellow("!"), style.dim(missing))?;
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
}
