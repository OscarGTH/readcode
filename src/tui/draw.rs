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
        Phase::Review => " review".to_string(),
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

        // The note goes above the line it explains.
        if let Some(note) = notes.get(&number) {
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

fn draw_review(frame: &mut Frame, app: &App, area: Rect) {
    if let Some(error) = &app.error {
        frame.render_widget(
            Paragraph::new(format!(" grading failed · {error}"))
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(Color::Red)),
            area,
        );
        return;
    }
    let Some(grade) = &app.grade else {
        return;
    };

    let width = area.width.saturating_sub(6) as usize;
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
    rows.push(Line::from(vec![
        Span::styled(" review · ", Theme::title()),
        Span::styled(grade.reviewer.clone(), Theme::dim()),
        Span::styled(" · ", Theme::hint()),
        Span::styled(
            format!("{} correct ", counts[0]),
            Theme::verdict(Verdict::Correct),
        ),
        Span::styled(
            format!("{} partial ", counts[1]),
            Theme::verdict(Verdict::Partial),
        ),
        Span::styled(
            format!("{} wrong ", counts[2]),
            Theme::verdict(Verdict::Wrong),
        ),
        Span::styled(
            format!("{} unknown", counts[3]),
            Theme::verdict(Verdict::Unknown),
        ),
    ]));
    rows.push(Line::raw(""));

    for graded in &grade.prompts {
        let span = app
            .prompts
            .get(graded.prompt)
            .map(|prompt| prompt.span_label())
            .unwrap_or_default();
        rows.push(Line::from(vec![
            Span::styled(
                format!(" {} ", verdict_mark(graded.verdict)),
                Theme::verdict(graded.verdict),
            ),
            Span::styled(
                format!("prompt {} · {span}", graded.prompt + 1),
                Theme::title(),
            ),
            Span::styled(
                format!("  {}", verdict_name(graded.verdict)),
                Theme::verdict(graded.verdict),
            ),
        ]));
        for (concept, verdict) in &graded.concepts {
            rows.push(Line::from(vec![
                Span::styled("    ", Theme::hint()),
                Span::styled(format!("{concept:<30}"), Theme::dim()),
                Span::styled(verdict_name(*verdict).to_string(), Theme::verdict(*verdict)),
            ]));
        }
        for piece in wrap(&graded.feedback, width.saturating_sub(4)) {
            rows.push(Line::from(vec![
                Span::styled("    ", Theme::hint()),
                Span::styled(piece, Style::default()),
            ]));
        }
        for missing in &graded.missing {
            for (index, piece) in wrap(missing, width.saturating_sub(16))
                .into_iter()
                .enumerate()
            {
                let lead = if index == 0 {
                    "    missed · "
                } else {
                    "             "
                };
                rows.push(Line::from(vec![
                    Span::styled(lead.to_string(), Theme::verdict(Verdict::Partial)),
                    Span::styled(piece, Theme::dim()),
                ]));
            }
        }
        rows.push(Line::raw(""));
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

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let hint = match app.phase {
        Phase::Annotate => {
            " Enter next · Ctrl-J newline · Ctrl-B back · Enter on empty skips · Esc quits"
        }
        Phase::Grading => " grading…",
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
    fn the_review_lists_verdicts_and_what_was_missed() {
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
                },
                PromptGrade {
                    prompt: 1,
                    verdict: Verdict::Partial,
                    concepts: vec![("rust.lifetime".into(), Verdict::Partial)],
                    feedback: "The lifetime is mentioned but not why it is needed.".into(),
                    missing: vec!["why the map cannot outlive the input".into()],
                },
            ],
        };
        app.finish(grade);
        let screen = screen(&app, 90, 30);

        assert!(screen.contains("review · acp:opencode"), "{screen}");
        assert!(screen.contains("1 correct"), "{screen}");
        assert!(screen.contains("1 partial"), "{screen}");
        assert!(screen.contains("rust.use.import"), "{screen}");
        assert!(screen.contains("missed · why the map"), "{screen}");
    }

    #[test]
    fn a_failed_grade_says_so_instead_of_showing_an_empty_review() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap();
        let mut app = app_with_answer(snippet);
        app.fail("connection refused".into());
        let screen = screen(&app, 90, 20);
        assert!(screen.contains("grading failed"), "{screen}");
        assert!(screen.contains("connection refused"), "{screen}");
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
