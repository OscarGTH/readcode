//! Painting the app with ratatui.
//!
//! The whole screen is derived from [`App`]: the code with the notes written on
//! it, the question, the input line, the review. No state lives here.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::highlight::{self, Token};
use crate::snippet::Snippet;
use crate::srs::Verdict;
use crate::tui::app::{App, Phase};

/// Colours in one place, so the screenshots and the terminal agree.
pub struct Theme;

impl Theme {
    fn keyword() -> Style {
        Style::default().fg(Color::Magenta)
    }
    fn type_() -> Style {
        Style::default().fg(Color::Cyan)
    }
    fn function() -> Style {
        Style::default().fg(Color::LightBlue)
    }
    fn string() -> Style {
        Style::default().fg(Color::Green)
    }
    fn number() -> Style {
        Style::default().fg(Color::Yellow)
    }
    fn comment() -> Style {
        Style::default().fg(Color::DarkGray)
    }
    /// The learner's own words, which should look like theirs, not like source.
    fn note() -> Style {
        Style::default()
            .fg(Color::LightGreen)
            .add_modifier(Modifier::ITALIC)
    }
    fn gutter() -> Style {
        Style::default().fg(Color::DarkGray)
    }
    fn current_bg() -> Style {
        Style::default().bg(Color::Rgb(38, 42, 52))
    }
    fn title() -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }
    fn dim() -> Style {
        Style::default().fg(Color::Gray)
    }
    fn hint() -> Style {
        Style::default().fg(Color::DarkGray)
    }
    fn focus() -> Style {
        Style::default().fg(Color::Black).bg(Color::LightGreen)
    }
    fn verdict(verdict: Verdict) -> Style {
        match verdict {
            Verdict::Correct => Style::default().fg(Color::Green),
            Verdict::Partial => Style::default().fg(Color::Yellow),
            Verdict::Wrong => Style::default().fg(Color::Red),
            Verdict::Unknown => Style::default().fg(Color::Magenta),
        }
    }
}

fn token_style(token: Token) -> Style {
    match token {
        Token::Keyword | Token::Macro | Token::Attribute => Theme::keyword(),
        Token::Type => Theme::type_(),
        Token::Function => Theme::function(),
        Token::String => Theme::string(),
        Token::Number | Token::Lifetime => Theme::number(),
        Token::Comment => Theme::comment(),
        Token::Plain => Style::default(),
    }
}

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    // Below this the code, the question and the input cannot all be legible, and
    // a half-drawn screen is worse than an honest message.
    if area.width < 46 || area.height < 14 {
        let message = format!(
            " readcode needs at least 46×14 (this terminal is {}×{}) · Ctrl-C quits",
            area.width, area.height
        );
        frame.render_widget(
            Paragraph::new(message)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(Color::Yellow)),
            area,
        );
        return;
    }

    let [header, body, ask, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(if app.phase == Phase::Review { 1 } else { 7 }),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, app, header);

    match app.phase {
        Phase::Annotate => {
            draw_code(frame, app, body, true);
            draw_ask(frame, app, ask);
        }
        Phase::Grading => {
            draw_code(frame, app, body, false);
            let waiting = Paragraph::new(Text::from(vec![
                Line::from(Span::styled("  grading · ", Theme::dim())),
                Line::raw(""),
                Line::from(Span::styled(
                    format!("  {} is reading your notes", app.reviewer),
                    Theme::dim(),
                )),
            ]));
            frame.render_widget(waiting, ask);
        }
        Phase::Review => {
            draw_review(frame, app, body);
        }
    }

    draw_footer(frame, app, footer);
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let [left, right] =
        Layout::horizontal([Constraint::Min(20), Constraint::Length(34)]).areas(area);

    let mode = match app.mode {
        crate::snippet::Asks::Lines => "line by line",
        crate::snippet::Asks::Blocks => "blocks",
        crate::snippet::Asks::Auto => "auto",
    };
    let title = Line::from(vec![
        Span::styled(" readcode", Theme::title()),
        Span::styled(" · ", Theme::hint()),
        Span::styled(app.snippet.title.clone(), Theme::dim()),
    ]);
    frame.render_widget(Paragraph::new(title), left);

    let progress = match app.phase {
        Phase::Review => String::new(),
        _ => format!("{} · {}/{}", mode, app.index + 1, app.prompts.len()),
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{progress} "),
            Theme::hint(),
        )))
        .alignment(Alignment::Right),
        right,
    );
}

/// The code, with each answered line carrying its note above it, the way you
/// would annotate a file.
fn draw_code(frame: &mut Frame, app: &App, area: Rect, follow: bool) {
    let snippet: &Snippet = app.snippet;
    let lines: Vec<String> = snippet.code.lines().map(str::to_string).collect();
    let notes = app.notes();
    let width = area.width.saturating_sub(2) as usize;
    let gutter = app.snippet.line_count().to_string().len().max(2);
    let text_width = width.saturating_sub(gutter + 3);

    let focus = app.current().map(|prompt| (prompt.start, prompt.end));
    let tokens = highlight::tokens(&lines.iter().map(String::as_str).collect::<Vec<_>>());

    let mut rows: Vec<Line> = Vec::new();
    let mut focus_row = 0usize;
    for (index, _line) in lines.iter().enumerate() {
        let number = index + 1;

        // A note about this line alone goes above it, where a comment about a
        // line belongs.
        if let Some((end, note)) = notes.get(&number)
            && *end == number
        {
            for piece in wrap(note, text_width) {
                rows.push(Line::from(vec![
                    Span::styled(format!("{:>gutter$} │ ", ""), Theme::gutter()),
                    Span::styled(format!("// {piece}"), Theme::note()),
                ]));
            }
        }

        if focus.is_some_and(|(start, _)| number == start) {
            focus_row = rows.len();
        }
        let marked = focus.is_some_and(|(start, end)| number >= start && number <= end);
        let marker = if marked { ">" } else { " " };
        let mut spans = vec![Span::styled(
            format!("{marker}{number:>gutter$} │ "),
            if marked {
                Theme::gutter().add_modifier(Modifier::BOLD)
            } else {
                Theme::gutter()
            },
        )];
        for (token, text) in &tokens[index] {
            let mut style = token_style(*token);
            if marked {
                style = style.patch(Theme::current_bg());
            }
            spans.push(Span::styled(text.clone(), style));
        }
        if marked {
            spans[0] = Span::styled(
                format!("{marker}{number:>gutter$} │ "),
                Theme::current_bg().add_modifier(Modifier::BOLD),
            );
        }
        rows.push(Line::from(spans));

        // A note about a whole block goes after the block's last line, labelled
        // with the span, so it never reads as a comment on its first line.
        for (start, end, note) in notes
            .iter()
            .filter(|(start, (end, _))| *end == number && **start < *end)
            .map(|(start, (end, note))| (*start, *end, note))
        {
            let label = format!("// on lines {start}-{end}: ");
            let mut first = true;
            for piece in wrap(note, text_width.saturating_sub(label.len())) {
                rows.push(Line::from(vec![
                    Span::styled(format!("{:>gutter$} │ ", ""), Theme::gutter()),
                    Span::styled(
                        format!(
                            "{}{piece}",
                            if first {
                                label.clone()
                            } else {
                                "   ".to_string()
                            }
                        ),
                        Theme::note(),
                    ),
                ]));
                first = false;
            }
        }
    }

    let height = area.height as usize;
    let mut scroll = app.scroll;
    if follow {
        // Keep the prompt being answered on screen, with a little air below it.
        if focus_row < scroll {
            scroll = focus_row;
        } else if focus_row + 3 >= scroll + height {
            scroll = (focus_row + 3).saturating_sub(height);
        }
    }
    let scroll = scroll.min(rows.len().saturating_sub(1)) as u16;

    let paragraph = Paragraph::new(Text::from(rows))
        .block(Block::default().borders(Borders::NONE))
        .scroll((scroll, 0));
    frame.render_widget(paragraph, area);
}

fn draw_ask(frame: &mut Frame, app: &App, area: Rect) {
    let Some(prompt) = app.current() else {
        return;
    };
    let concepts = prompt.concepts.join("  ");
    let title = format!(
        " {} {} · {} ",
        if prompt.is_line() { "line" } else { "lines" },
        prompt
            .span_label()
            .trim_start_matches("line ")
            .trim_start_matches("lines "),
        concepts
    );
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Theme::hint())
        .title(Span::styled(title, Theme::hint()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [question, input] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(3)]).areas(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {}", prompt.ask),
            Theme::title(),
        )))
        .wrap(Wrap { trim: false }),
        question,
    );

    let text = cursor_text(app);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .style(Style::default()),
        input,
    );
}

/// The typed answer with a block cursor at the caret.
fn cursor_text(app: &App) -> Text<'static> {
    let chars = app.input_chars();
    let mut lines: Vec<Line> = Vec::new();
    let mut current: Vec<Span> = vec![Span::raw(" ")];
    for (index, char_) in chars.iter().enumerate() {
        if index == app.cursor {
            current.push(Span::styled(" ", Theme::focus()));
        }
        if *char_ == '\n' {
            if index == app.cursor {
                // cursor already drawn
            }
            lines.push(Line::from(std::mem::take(&mut current)));
            current.push(Span::raw(" "));
        } else {
            current.push(Span::raw(char_.to_string()));
        }
    }
    if app.cursor >= chars.len() {
        current.push(Span::styled(" ", Theme::focus()));
    }
    lines.push(Line::from(current));
    Text::from(lines)
}

/// The review: for each prompt, the code it was about, what the learner wrote,
/// the verdict, and — when there is something to learn — the explanation.
///
/// The code is here on purpose. Reading a list of verdicts without the lines
/// beside them is asking the learner to remember what they were looking at, and
/// the moment they most want to compare is the moment they get the answer.
fn draw_review(frame: &mut Frame, app: &App, area: Rect) {
    if let Some(error) = &app.error {
        let text = Text::from(vec![
            Line::from(Span::styled(
                " grading failed",
                Theme::verdict(Verdict::Wrong),
            )),
            Line::raw(""),
            Line::from(Span::styled(
                format!(" {error}"),
                Style::default().fg(Color::Gray),
            )),
            Line::raw(""),
            Line::from(vec![
                Span::styled(" your answers are still here · ", Theme::dim()),
                Span::styled("r", Theme::note()),
                Span::styled(" tries the reviewer again · ", Theme::dim()),
                Span::styled("Enter", Theme::dim()),
                Span::styled(" gives up", Theme::dim()),
            ]),
        ]);
        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), area);
        return;
    }
    let Some(grade) = &app.grade else {
        return;
    };

    let width = area.width as usize;
    let code: Vec<String> = app.snippet.code.lines().map(str::to_string).collect();
    let tokens = highlight::tokens(&code.iter().map(String::as_str).collect::<Vec<_>>());
    let gutter = app.snippet.line_count().to_string().len().max(2);

    let mut rows: Vec<Line> = Vec::new();
    let mut counts = [0usize; 4];
    for graded in &grade.prompts {
        counts[match graded.verdict {
            Verdict::Correct => 0,
            Verdict::Partial => 1,
            Verdict::Wrong => 2,
            Verdict::Unknown => 3,
        }] += 1;
    }
    // Only the verdicts that happened: "0 wrong 0 unknown" is not information.
    let mut summary = vec![
        Span::styled(" review · ", Theme::title()),
        Span::styled(grade.reviewer.clone(), Theme::dim()),
    ];
    for (index, verdict) in [
        Verdict::Correct,
        Verdict::Partial,
        Verdict::Wrong,
        Verdict::Unknown,
    ]
    .iter()
    .enumerate()
    {
        if counts[index] > 0 {
            summary.push(Span::styled(" · ", Theme::hint()));
            summary.push(Span::styled(
                format!("{} {}", counts[index], verdict_name(*verdict)),
                Theme::verdict(*verdict),
            ));
        }
    }
    rows.push(Line::from(summary));

    for graded in &grade.prompts {
        let Some(prompt) = app.prompts.get(graded.prompt) else {
            continue;
        };
        rows.push(Line::raw(""));
        // The verdict sits on the first code row, so one prompt is one block
        // rather than a header followed by the code it describes.
        code_rows(
            &mut rows,
            &tokens,
            gutter,
            prompt.start,
            prompt.end,
            verdict_mark(graded.verdict),
            graded.verdict,
        );

        // The question belongs here: without it "you" is an answer to
        // something the learner has to remember from before the grading.
        labelled(
            &mut rows,
            "ask",
            &prompt.ask,
            Theme::hint(),
            Theme::dim(),
            width,
        );

        let answer = app
            .answers
            .get(graded.prompt)
            .map(|text| text.trim())
            .filter(|text| !text.is_empty())
            .unwrap_or("(left blank)");
        labelled(&mut rows, "you", answer, Theme::dim(), Theme::note(), width);

        // A correct answer needs no review and no lesson: the praise is noise,
        // and the lesson is for the lines that did not land.
        if graded.verdict == Verdict::Correct {
            continue;
        }
        let nuanced = graded
            .concepts
            .iter()
            .any(|(_, verdict)| *verdict != graded.verdict);
        if nuanced {
            for (concept, verdict) in &graded.concepts {
                rows.push(Line::from(vec![
                    Span::styled("      ", Theme::hint()),
                    Span::styled(format!("{concept:<30}"), Theme::dim()),
                    Span::styled(verdict_name(*verdict).to_string(), Theme::verdict(*verdict)),
                ]));
            }
        }
        if !graded.feedback.trim().is_empty() {
            labelled(
                &mut rows,
                "review",
                graded.feedback.trim(),
                Theme::dim(),
                Style::default(),
                width,
            );
        }
        if !graded.missing.is_empty() {
            labelled(
                &mut rows,
                "missed",
                &graded.missing.join(" · "),
                Theme::verdict(Verdict::Partial),
                Theme::dim(),
                width,
            );
        }
        if let Some(teach) = &graded.teach {
            labelled(
                &mut rows,
                "why",
                teach,
                Theme::type_(),
                Style::default(),
                width,
            );
        }
    }

    // Reading, once for the whole sitting rather than twice under every prompt.
    let mut seen: Vec<crate::docs::Doc> = Vec::new();
    for prompt in &app.prompts {
        for doc in reading(app, prompt) {
            if !seen.iter().any(|existing| existing.url == doc.url) {
                seen.push(doc);
            }
        }
    }
    if !seen.is_empty() {
        rows.push(Line::raw(""));
        rows.push(Line::from(Span::styled(" read more", Theme::dim())));
        for doc in seen.iter().take(6) {
            rows.push(Line::from(vec![
                Span::styled("   ", Theme::hint()),
                Span::styled(doc.title.clone(), Theme::dim()),
            ]));
            rows.push(Line::from(vec![
                Span::styled("   ", Theme::hint()),
                Span::styled(doc.url.clone(), Theme::hint()),
            ]));
        }
    }

    let height = area.height as usize;
    let scroll = app.scroll.min(rows.len().saturating_sub(height)) as u16;
    frame.render_widget(
        Paragraph::new(Text::from(rows))
            .block(Block::default().borders(Borders::NONE))
            .scroll((scroll, 0)),
        area,
    );
}

/// Where to read more: whatever the snippet author added, then one page per
/// distinct topic among the prompt's concepts.
fn reading(app: &App, prompt: &crate::snippet::Prompt) -> Vec<crate::docs::Doc> {
    let mut docs: Vec<crate::docs::Doc> = app
        .snippet
        .docs
        .iter()
        .map(|doc| crate::docs::Doc {
            title: doc.title.clone(),
            url: doc.url.clone(),
        })
        .collect();
    for doc in crate::docs::for_concepts(prompt.concepts.iter().map(String::as_str)) {
        if !docs.iter().any(|existing| existing.url == doc.url) {
            docs.push(doc);
        }
    }
    docs
}

/// How many code lines a review entry shows before it starts abbreviating. The
/// review is for recognising the code, not for re-reading all of it.
const REVIEW_CODE_LINES: usize = 6;

/// The prompted span's code, syntax-coloured and indented under the verdict.
fn code_rows(
    rows: &mut Vec<Line>,
    tokens: &[Vec<(Token, String)>],
    gutter: usize,
    start: usize,
    end: usize,
    mark: &str,
    verdict: Verdict,
) {
    let shown: Vec<usize> = if end - start < REVIEW_CODE_LINES {
        (start..=end).collect()
    } else {
        let mut shown: Vec<usize> = (start..start + 3).collect();
        shown.extend((end - 1)..=end);
        shown
    };
    let mut previous: Option<usize> = None;
    for number in shown {
        if let Some(previous) = previous
            && number > previous + 1
        {
            rows.push(Line::from(Span::styled(
                format!("     ⋯ {} more lines", number - previous - 1),
                Theme::hint(),
            )));
        }
        let first = previous.is_none();
        previous = Some(number);
        // The mark belongs to the prompt, so it rides the first row of its code.
        let lead = if first {
            mark.to_string()
        } else {
            " ".to_string()
        };
        let mut spans = vec![Span::styled(
            format!("  {lead}{number:>gutter$} │ "),
            if first {
                Theme::verdict(verdict)
            } else {
                Theme::gutter()
            },
        )];
        for (token, text) in tokens.get(number - 1).into_iter().flatten() {
            spans.push(Span::styled(text.clone(), token_style(*token)));
        }
        rows.push(Line::from(spans));
    }
}

/// One labelled paragraph: a dim label in a fixed column, then wrapped text.
fn labelled(
    rows: &mut Vec<Line>,
    label: &str,
    text: &str,
    label_style: Style,
    text_style: Style,
    width: usize,
) {
    let indent = 13usize;
    for (index, piece) in wrap(text, width.saturating_sub(indent).max(20))
        .into_iter()
        .enumerate()
    {
        let lead = if index == 0 {
            format!("    {label:<8}")
        } else {
            " ".repeat(indent)
        };
        rows.push(Line::from(vec![
            Span::styled(
                lead,
                if index == 0 {
                    label_style
                } else {
                    Theme::hint()
                },
            ),
            Span::styled(piece, text_style),
        ]));
    }
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let hint = match app.phase {
        Phase::Annotate => {
            " Enter next · Ctrl-J newline · Ctrl-B back · Enter on empty skips · Esc quits"
        }
        Phase::Grading => " grading…",
        Phase::Review if app.error.is_some() => " r retries grading · Enter gives up",
        Phase::Review => " j/k scroll · Enter finishes · Esc discards",
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(hint, Theme::hint()))),
        area,
    );
}

fn verdict_mark(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Correct => "✓",
        Verdict::Partial => "~",
        Verdict::Wrong => "✗",
        Verdict::Unknown => "?",
    }
}

fn verdict_name(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Correct => "correct",
        Verdict::Partial => "partial",
        Verdict::Wrong => "wrong",
        Verdict::Unknown => "unknown",
    }
}

/// Greedy word wrap. ratatui would wrap a paragraph for us, but notes and
/// feedback need to be measured to lay out the rows around them.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(16);
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if line.is_empty() {
                line.push_str(word);
            } else if line.chars().count() + 1 + word.chars().count() <= width {
                line.push(' ');
                line.push_str(word);
            } else {
                out.push(std::mem::take(&mut line));
                line.push_str(word);
            }
            while line.chars().count() > width {
                let mut cut = line
                    .char_indices()
                    .nth(width)
                    .map(|(index, _)| index)
                    .unwrap_or(line.len());
                if !line.is_char_boundary(cut) {
                    cut = line.len();
                }
                let rest = line.split_off(cut);
                out.push(std::mem::take(&mut line));
                line = rest;
            }
        }
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::Corpus;
    use crate::review::{Grade, PromptGrade};
    use crate::snippet::Asks;
    use crate::tui::app::{App, Key};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn screen(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn app_with_answer<'a>(snippet: &'a Snippet) -> App<'a> {
        let prompts = snippet.prompts(Asks::Lines);
        let mut app = App::new(snippet, prompts, Asks::Lines, "acp:opencode".into());
        app.on_key(Key::Char('a'));
        app.on_key(Key::Enter);
        app
    }

    #[test]
    fn the_code_is_shown_with_the_current_line_marked() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap();
        let app = app_with_answer(snippet);
        let screen = screen(&app, 90, 24);

        assert!(screen.contains("readcode"), "{screen}");
        assert!(
            screen.contains("use std::collections::HashMap;"),
            "{screen}"
        );
        assert!(
            screen.contains("> 3 │"),
            "the second prompt is marked: {screen}"
        );
        assert!(screen.contains("fn count_words"), "{screen}");
    }

    #[test]
    fn an_answer_shows_above_its_line_as_a_note() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap();
        let mut app = App::new(
            snippet,
            snippet.prompts(Asks::Lines),
            Asks::Lines,
            "acp:opencode".into(),
        );
        for char_ in "brings HashMap into scope".chars() {
            app.on_key(Key::Char(char_));
        }
        app.on_key(Key::Enter);
        let screen = screen(&app, 90, 24);

        let note = screen
            .find("// brings HashMap into scope")
            .expect("note shown");
        let code = screen
            .find("use std::collections::HashMap;")
            .expect("line 1 shown");
        assert!(note < code, "the note sits above its line:\n{screen}");
    }

    #[test]
    fn a_block_note_sits_after_the_block_and_says_which_lines_it_covers() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("004").unwrap();
        let prompts = snippet.prompts(crate::snippet::Asks::Blocks);
        let mut app = App::new(
            snippet,
            prompts,
            crate::snippet::Asks::Blocks,
            "acp:opencode".into(),
        );
        for char_ in "the counter starts at zero".chars() {
            app.on_key(Key::Char(char_));
        }
        app.on_key(Key::Enter);
        let screen = screen(&app, 96, 30);

        let note = screen
            .find("// on lines 1-5: the counter starts at zero")
            .unwrap_or_else(|| panic!("block note labelled with its span:\n{screen}"));
        let block_start = screen.find(" 1 │").expect("line 1");
        let block_end = screen.find(" 5 │").expect("line 5");
        assert!(note > block_end, "the note follows the block:\n{screen}");
        assert!(note > block_start, "{screen}");
    }

    #[test]
    fn a_line_note_stays_above_its_own_line() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap();
        let mut app = App::new(
            snippet,
            snippet.prompts(crate::snippet::Asks::Lines),
            crate::snippet::Asks::Lines,
            "acp:opencode".into(),
        );
        for char_ in "imports the map type".chars() {
            app.on_key(Key::Char(char_));
        }
        app.on_key(Key::Enter);
        let screen = screen(&app, 96, 30);
        let note = screen.find("// imports the map type").expect("note");
        let line = screen.find("use std::collections::HashMap;").expect("line");
        assert!(note < line, "a one-line note stays above it:\n{screen}");
    }

    #[test]
    fn the_question_and_the_typed_answer_are_visible() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap();
        let mut app = app_with_answer(snippet);
        for char_ in "grows the map".chars() {
            app.on_key(Key::Char(char_));
        }
        let screen = screen(&app, 90, 24);

        // Whatever the corpus asks for this prompt, it must be on screen.
        let prompt = &app.prompts[app.index];
        assert!(screen.contains(&prompt.concepts[0]), "{screen}");
        // The question is wrapped, so compare its opening rather than the whole.
        assert!(
            screen.contains(&prompt.ask[..prompt.ask.len().min(24)]),
            "{screen}"
        );
        assert!(screen.contains("grows the map"), "{screen}");
    }

    #[test]
    fn a_long_note_wraps_instead_of_being_cut_off() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap();
        let mut app = App::new(
            snippet,
            snippet.prompts(Asks::Lines),
            Asks::Lines,
            "acp:opencode".into(),
        );
        for char_ in "counts has to be mutable because the loop inserts into it and an immutable binding would be a compile error"
            .chars()
        {
            app.on_key(Key::Char(char_));
        }
        app.on_key(Key::Enter);
        let screen = screen(&app, 70, 26);
        assert!(screen.contains("// counts has to be mutable"), "{screen}");
        assert!(
            screen.contains("compile error"),
            "the tail survives:\n{screen}"
        );
    }

    #[test]
    fn the_review_shows_the_code_the_answer_and_the_lesson() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap();
        let mut app = app_with_answer(snippet);
        let grade = Grade {
            reviewer: "acp:opencode".into(),
            summary: String::new(),
            prompts: vec![
                PromptGrade {
                    prompt: 0,
                    verdict: Verdict::Correct,
                    concepts: vec![("rust.use.import".into(), Verdict::Correct)],
                    feedback: "You correctly identify that the import brings HashMap into scope."
                        .into(),
                    missing: vec![],
                    teach: None,
                },
                PromptGrade {
                    prompt: 1,
                    verdict: Verdict::Partial,
                    concepts: vec![("rust.lifetime.elision".into(), Verdict::Partial)],
                    feedback: "The lifetime is mentioned but not why it is needed.".into(),
                    missing: vec!["why the map cannot outlive the input".into()],
                    teach: Some(
                        "The map stores borrows of the input text, so it cannot outlive it.".into(),
                    ),
                },
            ],
        };
        app.finish(grade);
        let screen = screen(&app, 96, 34);

        assert!(screen.contains("review · acp:opencode"), "{screen}");
        // Only the verdicts that happened; "0 wrong 0 unknown" is not information.
        assert!(screen.contains("1 correct · 1 partial"), "{screen}");
        assert!(!screen.contains("0 wrong"), "{screen}");
        assert!(!screen.contains("0 unknown"), "{screen}");
        // The verdict rides the code row, so one prompt is one block.
        assert!(
            screen.contains("✓ 1 │ use std::collections::HashMap;"),
            "{screen}"
        );
        // The question, so the answer is not judged against a memory.
        assert!(screen.contains("ask"), "{screen}");
        assert!(
            screen.contains(&app.prompts[1].ask[..24]),
            "the second question should be on screen:\n{screen}"
        );
        assert!(screen.contains("~ 3 │ fn count_words"), "{screen}");
        // The learner's own words, and the lesson when the answer fell short.
        assert!(screen.contains("you"), "{screen}");
        assert!(screen.contains("missed"), "{screen}");
        assert!(screen.contains("why the map"), "{screen}");
        assert!(
            screen.contains("The map stores borrows of the input text"),
            "{screen}"
        );
        // Reading, once for the sitting.
        assert_eq!(screen.matches("read more").count(), 1, "{screen}");
        assert!(screen.contains("doc.rust-lang.org"), "{screen}");
        assert!(screen.contains("ch10-03"), "the lifetime chapter: {screen}");
    }

    #[test]
    fn a_correct_answer_is_just_the_line_and_what_was_said() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("006").unwrap();
        let mut app = app_with_answer(snippet);
        let prompt = app.prompts[0].clone();
        let grade = Grade {
            reviewer: "acp:opencode".into(),
            summary: String::new(),
            prompts: vec![PromptGrade {
                prompt: 0,
                verdict: Verdict::Correct,
                concepts: vec![(prompt.concepts[0].clone(), Verdict::Correct)],
                feedback: "Spot on — `main` is the entry point where execution begins.".into(),
                missing: vec![],
                teach: Some("`main` is where the program starts.".into()),
            }],
        };
        app.finish(grade);
        let screen = screen(&app, 96, 26);

        assert!(screen.contains("✓ 1 │ fn main() {"), "{screen}");
        // Even a correct line shows what was asked.
        assert!(screen.contains("ask     "), "{screen}");
        assert!(screen.contains(&app.prompts[0].ask), "{screen}");
        // Praise and a lesson on a correct line are clutter, so neither appears.
        assert!(!screen.contains("Spot on"), "{screen}");
        assert!(!screen.contains("why"), "{screen}");
        assert!(!screen.contains("review  "), "{screen}");
    }

    #[test]
    fn a_shared_topic_is_read_more_once() {
        let corpus = Corpus::builtin().unwrap();
        // 006's lines 2 and 3 both teach mutability, so both map to one page.
        let snippet = corpus.find("006").unwrap();
        let mut app = app_with_answer(snippet);
        app.on_key(Key::Char('x'));
        app.on_key(Key::Enter);
        let grade = Grade {
            reviewer: "acp:opencode".into(),
            summary: String::new(),
            prompts: vec![
                PromptGrade {
                    prompt: 0,
                    verdict: Verdict::Correct,
                    concepts: vec![(app.prompts[0].concepts[0].clone(), Verdict::Correct)],
                    feedback: String::new(),
                    missing: vec![],
                    teach: None,
                },
                PromptGrade {
                    prompt: 1,
                    verdict: Verdict::Partial,
                    concepts: vec![(app.prompts[1].concepts[0].clone(), Verdict::Partial)],
                    feedback: String::new(),
                    missing: vec![],
                    teach: None,
                },
            ],
        };
        app.finish(grade);
        let screen = screen(&app, 96, 26);
        assert_eq!(
            screen.matches("ch03-01-variables-and-mutability").count(),
            1,
            "the page for `let` should be listed once:\n{screen}"
        );
    }

    #[test]
    fn a_long_block_shows_the_head_and_tail_of_its_code() {
        let corpus = Corpus::builtin().unwrap();
        // rust-004's third block is seven lines: longer than the review shows.
        let snippet = corpus.find("004").unwrap();
        let prompts = snippet.prompts(crate::snippet::Asks::Blocks);
        let mut app = App::new(
            snippet,
            prompts,
            crate::snippet::Asks::Blocks,
            "acp:opencode".into(),
        );
        let prompt = app.prompts.get(2).cloned().expect("a third prompt");
        let grade = Grade {
            reviewer: "acp:opencode".into(),
            summary: String::new(),
            prompts: vec![PromptGrade {
                prompt: 2,
                verdict: Verdict::Partial,
                concepts: prompt
                    .concepts
                    .iter()
                    .map(|concept| (concept.clone(), Verdict::Partial))
                    .collect(),
                feedback: "Partly there.".into(),
                missing: vec![],
                teach: Some("Think about who owns the string.".into()),
            }],
        };
        app.finish(grade);
        let screen = screen(&app, 96, 30);
        // The block is longer than the cap, so it is abbreviated rather than
        // pushing every other prompt out of reach.
        assert!(screen.contains("more lines"), "{screen}");
    }

    #[test]
    fn a_failed_grade_says_so_and_offers_another_go() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap();
        let mut app = app_with_answer(snippet);
        app.fail("connection refused".into());
        let screen = screen(&app, 90, 20);
        assert!(screen.contains("grading failed"), "{screen}");
        assert!(screen.contains("connection refused"), "{screen}");
        // The point of the retry: the answers were never thrown away.
        assert!(screen.contains("your answers are still here"), "{screen}");
        assert!(screen.contains("tries the reviewer again"), "{screen}");
    }

    #[test]
    fn a_tiny_terminal_gets_a_message_instead_of_a_broken_layout() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap();
        let app = app_with_answer(snippet);
        let screen = screen(&app, 30, 8);
        assert!(screen.contains("at least 46×14"), "{screen}");
        assert!(screen.contains("30×8"), "{screen}");
    }

    /// The thing that most easily goes quietly wrong: an answer landing above the
    /// line next to the one it was asked about. Checked for every prompt of every
    /// built-in snippet, because one snippet's numbering will not cover them all.
    #[test]
    fn every_note_sits_above_the_line_it_answers() {
        let corpus = Corpus::builtin().unwrap();
        for snippet in corpus.snippets() {
            let prompts = snippet.prompts(crate::snippet::Asks::Lines);
            if prompts.is_empty() {
                continue;
            }
            let mut app = App::new(
                snippet,
                prompts.clone(),
                crate::snippet::Asks::Lines,
                "check".into(),
            );
            for (index, prompt) in prompts.iter().enumerate() {
                for char_ in format!("note{}-{index}", prompt.start).chars() {
                    app.on_key(Key::Char(char_));
                }
                app.on_key(Key::Enter);
            }

            // Tall enough that nothing is scrolled out of the way.
            let screen = screen(&app, 120, 90);
            let rows: Vec<&str> = screen.lines().collect();
            let gutter = snippet.line_count().to_string().len().max(2);

            for (index, prompt) in prompts.iter().enumerate() {
                let note = format!("// note{}-{index}", prompt.start);
                let note_row = rows
                    .iter()
                    .position(|row| row.contains(&note))
                    .unwrap_or_else(|| panic!("{}: no note for line {}", snippet.id, prompt.start));
                let code_row = rows
                    .iter()
                    .position(|row| {
                        !row.contains("//") && row.contains(&format!("{:>gutter$} │", prompt.start))
                    })
                    .unwrap_or_else(|| panic!("{}: no code row {}", snippet.id, prompt.start));
                assert_eq!(
                    note_row + 1,
                    code_row,
                    "{}: the note for line {} is not directly above it
{}",
                    snippet.id,
                    prompt.start,
                    screen
                );
            }
        }
    }

    #[test]
    fn wrapping_never_splits_a_word_that_fits() {
        let wrapped = wrap("the map borrows from the input text", 18);
        assert!(
            wrapped.iter().all(|line| line.chars().count() <= 18),
            "{wrapped:?}"
        );
        assert_eq!(wrapped.join(" "), "the map borrows from the input text");
    }

    #[test]
    fn a_blank_line_in_a_note_is_kept() {
        let wrapped = wrap("first\n\nsecond", 40);
        assert_eq!(wrapped, vec!["first", "", "second"]);
    }
}
