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
    /// The key that does the thing the screen is waiting for.
    fn accent() -> Style {
        Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD)
    }
    fn hint() -> Style {
        Style::default().fg(Color::DarkGray)
    }
    /// Every other entry gets this behind it. Subtle on purpose: it separates
    /// blocks while scrolling, it does not decorate them.
    fn band() -> Color {
        Color::Rgb(21, 25, 33)
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

    // The question sits beside the answer, so its height is part of the layout:
    // a fixed two rows used to cut long questions off mid-sentence.
    let question_rows = app
        .current()
        .map(|prompt| wrap(&prompt.ask, area.width.saturating_sub(4) as usize).len())
        .unwrap_or(1)
        .clamp(1, 4) as u16;
    let input_height = match app.phase {
        Phase::Review => 1,
        Phase::Confirm => 6,
        Phase::Annotate | Phase::Grading => question_rows + 5,
    };
    let header_height = if app.reason.is_empty() || app.phase == Phase::Review {
        1
    } else {
        2
    };
    let [header, body, ask, footer] = Layout::vertical([
        Constraint::Length(header_height),
        Constraint::Min(5),
        Constraint::Length(input_height),
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
            // A clock, because a still screen for forty seconds reads as a hang
            // and people kill it — losing the sitting they just answered.
            let elapsed = app
                .grading_for
                .map(|elapsed| format!(" · {}s", elapsed.as_secs()))
                .unwrap_or_default();
            let waiting = Paragraph::new(Text::from(vec![
                Line::from(Span::styled(format!("  grading{elapsed} · "), Theme::dim())),
                Line::raw(""),
                Line::from(Span::styled(
                    format!("  {} is reading your notes", app.reviewer),
                    Theme::dim(),
                )),
                Line::from(Span::styled(
                    "  this usually takes 15–40 s · the answers are safe",
                    Theme::hint(),
                )),
            ]));
            frame.render_widget(waiting, ask);
        }
        Phase::Confirm => {
            // Same view as answering, with the code and every note on it.
            draw_code(frame, app, body, true);
            draw_check(frame, app, ask);
        }
        Phase::Review => {
            draw_review(frame, app, body);
        }
    }

    draw_footer(frame, app, footer);
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let [title_row, why_row] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(area.height.saturating_sub(1)),
    ])
    .areas(area);
    let area = title_row;
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

    // The language rides with the mode, not with the title: both say what kind
    // of sitting this is, while the title says what the code is about.
    let language = app.snippet.language.clone();
    let progress = match app.phase {
        Phase::Confirm => format!("ready to grade · {language}"),
        Phase::Review => language,
        _ => format!(
            "{language} · {mode} · {}/{}",
            app.index + 1,
            app.prompts.len()
        ),
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{progress} "),
            Theme::hint(),
        )))
        .alignment(Alignment::Right),
        right,
    );

    // Why this snippet: one quiet line under the title, gone during the review
    // where the question no longer matters.
    if why_row.height > 0 && !app.reason.is_empty() && app.phase != Phase::Review {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" why this · ", Theme::gutter()),
                Span::styled(app.reason.clone(), Theme::hint()),
            ]))
            .wrap(Wrap { trim: true }),
            why_row,
        );
    }
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
    let tokens = highlight::tokens_in(
        &snippet.language,
        &lines.iter().map(String::as_str).collect::<Vec<_>>(),
    );

    let mut rows: Vec<Line> = Vec::new();
    let mut focus_row = 0usize;
    // Row of the focused span's last line, so follow-scroll keeps a whole block in
    // view instead of only its first line.
    let mut focus_end = 0usize;
    for (index, _line) in lines.iter().enumerate() {
        let number = index + 1;

        // A note goes above the span it is about — one line or five — because a
        // comment above the code is how you annotate a file. A note about several
        // lines is labelled with its span so it cannot be mistaken for a remark
        // on the first line alone.
        if let Some(note) = notes.get(&number) {
            let label = if note.end > number {
                Some(format!("on lines {number}-{}: ", note.end))
            } else {
                None
            };
            push_note(&mut rows, gutter, text_width, note, label.as_deref());
        }

        if focus.is_some_and(|(start, _)| number == start) {
            focus_row = rows.len();
        }
        if focus.is_some_and(|(_, end)| number == end) {
            focus_end = rows.len();
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
        // Keep the prompt being answered on screen — the code, the question under
        // it, and a little air below.
        let bottom = focus_end.max(focus_row) + 1;
        if focus_row < scroll {
            scroll = focus_row;
        } else if bottom >= scroll + height {
            scroll = bottom.saturating_sub(height) + 1;
        }
    }
    let scroll = scroll.min(rows.len().saturating_sub(1)) as u16;

    let paragraph = Paragraph::new(Text::from(rows))
        .block(Block::default().borders(Borders::NONE))
        .scroll((scroll, 0));
    frame.render_widget(paragraph, area);
}

/// A note as it appears on the code: the question, then the answer.
///
/// The question is written with the answer rather than only in the input panel,
/// because once the next prompt arrives it is gone from the screen and the note
/// alone no longer says what it was answering.
fn push_note(
    rows: &mut Vec<Line<'static>>,
    gutter: usize,
    width: usize,
    note: &crate::tui::app::Note,
    label: Option<&str>,
) {
    let label = label.unwrap_or("");
    let indent = label.len();
    let mut first = true;
    for piece in wrap(&note.ask, width.saturating_sub(indent + 5)) {
        rows.push(Line::from(vec![
            Span::styled(
                format!("{:>pad$} │ ", "", pad = gutter + 1),
                Theme::gutter(),
            ),
            Span::styled(
                format!(
                    "{}{piece}",
                    if first {
                        format!("// ? {label}")
                    } else {
                        "//   ".to_string()
                    }
                ),
                Theme::hint(),
            ),
        ]));
        first = false;
    }
    let mut first = true;
    for piece in wrap(&note.answer, width.saturating_sub(indent + 5)) {
        rows.push(Line::from(vec![
            Span::styled(
                format!("{:>pad$} │ ", "", pad = gutter + 1),
                Theme::gutter(),
            ),
            Span::styled(
                format!(
                    "{}{piece}",
                    if first {
                        "// ".to_string()
                    } else {
                        "//   ".to_string()
                    }
                ),
                Theme::note(),
            ),
        ]));
        first = false;
    }
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

    let question_rows = wrap(&prompt.ask, inner.width.saturating_sub(2) as usize)
        .len()
        .clamp(1, 4) as u16;
    let [question, hint, input] = Layout::vertical([
        Constraint::Length(question_rows),
        Constraint::Length(1),
        Constraint::Min(2),
    ])
    .areas(inner);

    // Asked here, beside the answer, and nowhere else while it is being asked.
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {}", prompt.ask),
            Theme::title(),
        )))
        .wrap(Wrap { trim: false }),
        question,
    );
    // Quiet print, not competing with the question above it: the question is
    // bold and this is the darkest thing on screen.
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " your words · Enter submits · Ctrl-J newline · Ctrl-B back",
            Theme::hint(),
        ))),
        hint,
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

/// The check: the code with every note on it, plus the answer you are looking at.
///
/// A separate list screen meant leaving the code to re-read what you wrote, which
/// is backwards — the notes sitting above their lines *are* the answers, and the
/// thing worth checking is whether each note says the right thing about the code
/// it sits on.
fn draw_check(frame: &mut Frame, app: &App, area: Rect) {
    let Some(prompt) = app.current() else {
        return;
    };
    let total = app.prompts.len();
    let title = format!(
        " answer {} of {total} · {} · {} ",
        app.confirm_index + 1,
        prompt.span_label(),
        prompt.concepts.join("  ")
    );
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Theme::hint())
        .title(Span::styled(title, Theme::hint()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [call, answer, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(2),
        Constraint::Length(1),
    ])
    .areas(inner);

    // The one thing the learner cannot guess: that the sitting is waiting on
    // them, and that Ctrl-S is what sends it.
    let answered = app.answers.iter().filter(|a| !a.trim().is_empty()).count();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(
                    " {answered} of {} answered — review them, then ",
                    app.prompts.len()
                ),
                Theme::dim(),
            ),
            Span::styled("Ctrl-S", Theme::accent()),
            Span::styled(" sends them for grading", Theme::dim()),
        ]))
        .wrap(Wrap { trim: true }),
        call,
    );

    let text = app
        .answers
        .get(app.confirm_index)
        .map(String::as_str)
        .unwrap_or("");
    let body = if text.trim().is_empty() {
        Line::from(Span::styled(
            " — not answered; Enter opens it —",
            Theme::dim(),
        ))
    } else {
        Line::from(Span::styled(format!(" \"{text}\""), Theme::note()))
    };
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), answer);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " j/k moves · Enter reopens the selected answer",
            Theme::hint(),
        ))),
        hint,
    );
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
    let tokens = highlight::tokens_in(
        &app.snippet.language,
        &code.iter().map(String::as_str).collect::<Vec<_>>(),
    );
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

    for (entry, graded) in grade.prompts.iter().enumerate() {
        let Some(prompt) = app.prompts.get(graded.prompt) else {
            continue;
        };
        let band_start = rows.len();
        if entry > 0 {
            // A line, not just a shade: it reads in a colourless terminal too, and
            // it says "this entry ends here" without asking the eye to compare two
            // backgrounds.
            rows.push(Line::from(Span::styled("─".repeat(width), Theme::hint())));
        } else {
            rows.push(Line::raw(""));
        }
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

        // The question and the answer come first, then the code they are about
        // with the verdict on its first row: the same order as the notes written
        // during the sitting, so the page reads the way the sitting did.
        code_rows(
            &mut rows,
            &tokens,
            gutter,
            prompt.start,
            prompt.end,
            verdict_mark(graded.verdict),
            graded.verdict,
        );

        {
            // Even a correct answer gets its explanation: it may have been a guess,
            // and the lesson is the part worth keeping.
            let nuanced = graded
                .concepts
                .iter()
                .any(|(_, verdict)| *verdict != graded.verdict);
            if nuanced && graded.verdict != Verdict::Correct {
                for (concept, verdict) in &graded.concepts {
                    rows.push(Line::from(vec![
                        Span::styled(" ".repeat(LABEL_INDENT), Theme::hint()),
                        Span::styled(format!("{concept:<30}"), Theme::dim()),
                        Span::styled(verdict_name(*verdict).to_string(), Theme::verdict(*verdict)),
                    ]));
                }
            }
            // On a correct answer the reviewer's feedback is usually praise; the
            // lesson is the part that teaches, so `review` and `missed` are for the
            // answers that fell short.
            if graded.verdict != Verdict::Correct && !graded.feedback.trim().is_empty() {
                labelled(
                    &mut rows,
                    "review",
                    graded.feedback.trim(),
                    Theme::dim(),
                    Style::default(),
                    width,
                );
            }
            if graded.verdict != Verdict::Correct && !graded.missing.is_empty() {
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
            // Reading under the entry it belongs to: a list at the end made the
            // learner match links back to prompts.
            for doc in reading(app, prompt) {
                labelled(
                    &mut rows,
                    "docs",
                    &doc.title,
                    Theme::dim(),
                    Theme::dim(),
                    width,
                );
                labelled(&mut rows, "", &doc.url, Theme::hint(), Theme::hint(), width);
            }
        }
        if entry % 2 == 0 {
            band(&mut rows[band_start..], width);
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

/// Give a run of rows one background, all the way to the right edge — a band is
/// only readable if it reaches the edge, and a Paragraph only paints the cells a
/// span covers.
fn band(rows: &mut [Line], width: usize) {
    for line in rows {
        let used: usize = line
            .spans
            .iter()
            .map(|span| span.content.chars().count())
            .sum();
        line.style = line.style.bg(Theme::band());
        if used < width {
            line.spans.push(Span::styled(
                " ".repeat(width - used),
                Style::default().bg(Theme::band()),
            ));
        }
    }
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
/// Where the text after a label starts: 4 spaces of margin plus an 8-wide label
/// column. Shared by every row that lines up under the labels, so nothing sits a
/// stray column away from its neighbours.
const LABEL_INDENT: usize = 12;

fn labelled(
    rows: &mut Vec<Line>,
    label: &str,
    text: &str,
    label_style: Style,
    text_style: Style,
    width: usize,
) {
    let indent = LABEL_INDENT;
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
        // The check strip already carries these keys; repeating them here is the
        // kind of footnote that teaches nobody anything.
        // The strip above already says what happens to an unanswered sitting.
        Phase::Confirm => "",
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
        let snippet = corpus.find("rust-005").unwrap();
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
        let snippet = corpus.find("rust-005").unwrap();
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
    fn a_block_note_sits_above_the_block_and_says_which_lines_it_covers() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("rust-004").unwrap();
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
            .find("// ? on lines 1-5: ")
            .unwrap_or_else(|| panic!("block note labelled with its span:\n{screen}"));
        assert!(
            screen.contains("// the counter starts at zero"),
            "the answer follows its question:\n{screen}"
        );
        // Above the first line of the span, like a comment on it — not parked
        // after the closing brace where it reads as a remark on the last line.
        let block_start = screen.find(" 1 │").expect("line 1");
        assert!(note < block_start, "the note precedes the block:\n{screen}");
        let block_end = screen.find(" 5 │").expect("line 5");
        assert!(note < block_end, "{screen}");
    }

    #[test]
    fn a_line_note_stays_above_its_own_line() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("rust-005").unwrap();
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
        let snippet = corpus.find("rust-005").unwrap();
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
        let snippet = corpus.find("rust-005").unwrap();
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
        let snippet = corpus.find("rust-005").unwrap();
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
        // Reading sits under the entry it belongs to, not in a list at the end.
        assert!(screen.contains("docs"), "{screen}");
        assert!(screen.contains("doc.rust-lang.org"), "{screen}");
        assert!(screen.contains("ch10-03"), "the lifetime chapter: {screen}");
    }

    #[test]
    fn a_correct_answer_still_gets_the_explanation() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("rust-006").unwrap();
        let mut app = app_with_answer(snippet);
        let prompt = app.prompts[0].clone();
        let grade = Grade {
            reviewer: "acp:opencode".into(),
            summary: String::new(),
            prompts: vec![PromptGrade {
                prompt: 0,
                verdict: Verdict::Correct,
                concepts: vec![(prompt.concepts[0].clone(), Verdict::Correct)],
                feedback: "Spot on — `let` binds without `mut`, so the name cannot be reassigned."
                    .into(),
                missing: vec![],
                teach: Some("`let` binds a name that cannot be reassigned.".into()),
            }],
        };
        app.finish(grade);
        let screen = screen(&app, 96, 26);

        assert!(screen.contains("✓ 1 │ fn main() {"), "{screen}");
        // Even a correct line shows what was asked.
        assert!(screen.contains("ask     "), "{screen}");
        assert!(screen.contains(&app.prompts[0].ask), "{screen}");
        // And it still gets the lesson: a right answer may have been a guess.
        assert!(screen.contains("why"), "{screen}");
        assert!(
            screen.contains("`let` binds a name that cannot be reassigned"),
            "{screen}"
        );
        // With its reading beside it.
        assert!(screen.contains("docs"), "{screen}");
        assert!(
            screen.contains("ch03-01-variables-and-mutability"),
            "{screen}"
        );
    }

    #[test]
    fn every_other_entry_is_banded_right_to_the_edge() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("rust-006").unwrap();
        let mut app = app_with_answer(snippet);
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
                    verdict: Verdict::Correct,
                    concepts: vec![(app.prompts[1].concepts[0].clone(), Verdict::Correct)],
                    feedback: String::new(),
                    missing: vec![],
                    teach: None,
                },
            ],
        };
        app.finish(grade);

        let width = 96u16;
        let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer().clone();

        // Find the two code rows, then read the background of their last cell.
        let mut code_rows = Vec::new();
        for y in 0..buffer.area.height {
            let mut row = String::new();
            for x in 0..buffer.area.width {
                row.push_str(buffer[(x, y)].symbol());
            }
            if row.contains("│ fn main()") || row.contains("│     let") {
                code_rows.push(y);
            }
        }
        assert_eq!(code_rows.len(), 2, "expected two entries");
        let first = buffer[(width - 1, code_rows[0])].bg;
        let second = buffer[(width - 1, code_rows[1])].bg;
        assert_eq!(first, Theme::band(), "the first entry is banded");
        assert_eq!(second, Color::Reset, "the second is not");
    }

    #[test]
    fn a_shared_topic_is_read_more_once() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("rust-006").unwrap();
        let mut app = app_with_answer(snippet);
        // Two concepts that point at the same page: both are about borrowing, and
        // the review must send the learner to that page once, not twice.
        app.prompts[0].concepts = vec![
            "rust.borrow".to_string(),
            "rust.borrow.str_param".to_string(),
        ];
        app.on_key(Key::Char('x'));
        app.on_key(Key::Enter);
        let grade = Grade {
            reviewer: "acp:opencode".into(),
            summary: String::new(),
            prompts: vec![PromptGrade {
                prompt: 0,
                verdict: Verdict::Correct,
                concepts: vec![("rust.borrow".to_string(), Verdict::Correct)],
                feedback: String::new(),
                missing: vec![],
                teach: None,
            }],
        };
        app.finish(grade);
        let screen = screen(&app, 96, 26);
        assert_eq!(
            screen.matches("ch04-02-references-and-borrowing").count(),
            1,
            "one prompt, two concepts, one page:\n{screen}"
        );
    }

    #[test]
    fn a_long_block_shows_the_head_and_tail_of_its_code() {
        let corpus = Corpus::builtin().unwrap();
        // rust-004's third block is seven lines: longer than the review shows.
        let snippet = corpus.find("rust-004").unwrap();
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
        let snippet = corpus.find("rust-005").unwrap();
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
        let snippet = corpus.find("rust-005").unwrap();
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

            // The last answer opens the confirmation; step back into the code to
            // read the notes, which is what this test is about.
            if app.phase == Phase::Confirm {
                app.edit(prompts.len() - 1);
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

    #[test]
    fn the_question_is_asked_beside_the_input_and_not_on_the_code() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("rust-006").unwrap();
        let app = App::new(
            snippet,
            snippet.prompts(Asks::Blocks),
            Asks::Blocks,
            "check".into(),
        );
        let screen = screen(&app, 96, 24);
        let rows: Vec<&str> = screen.lines().collect();

        let opening: String = app.prompts[0]
            .ask
            .split_whitespace()
            .take(3)
            .collect::<Vec<_>>()
            .join(" ");
        let ask_row = rows
            .iter()
            .position(|row| row.contains(&opening))
            .unwrap_or_else(|| panic!("the question is not on screen:\n{screen}"));
        // Below the code, in the panel the answer is typed into.
        let last_code = rows
            .iter()
            .rposition(|row| row.contains("│ ") && !row.contains("//"))
            .unwrap_or(0);
        assert!(
            ask_row > last_code,
            "the question belongs beside the input, not on the code:\n{screen}"
        );
        assert!(
            !screen.contains('└'),
            "nothing is drawn onto the code:\n{screen}"
        );
    }

    #[test]
    fn an_answered_question_is_written_onto_the_code_with_the_answer() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("rust-006").unwrap();
        let mut app = App::new(
            snippet,
            snippet.prompts(Asks::Blocks),
            Asks::Blocks,
            "check".into(),
        );
        for char_ in "both are i32".chars() {
            app.on_key(Key::Char(char_));
        }
        app.on_key(Key::Enter);
        let screen = screen(&app, 96, 30);

        // The question you answered, and your answer under it, as one comment.
        assert!(
            screen.contains("// ? on lines 1-3: What type is `apples`"),
            "{screen}"
        );
        assert!(screen.contains("// both are i32"), "{screen}");
        // The panel has moved on to the next question, so the answer you gave is
        // now only findable on the code — which is why the question is written
        // there with it.
        assert!(screen.contains("2/3"), "the sitting moved on:\n{screen}");
    }

    #[test]
    fn a_long_question_is_not_clipped() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("rust-004").unwrap();
        let app = App::new(
            snippet,
            snippet.prompts(Asks::Blocks),
            Asks::Blocks,
            "check".into(),
        );
        let screen = screen(&app, 80, 30);
        let ask = app.prompts[0].ask.as_str();
        // The last words of the question have to be there: a fixed two-row area
        // used to cut long questions off mid-sentence.
        let tail: String = ask
            .chars()
            .rev()
            .take(18)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        assert!(
            screen.contains(&tail),
            "the question was clipped:\n{screen}"
        );
    }

    #[test]
    fn the_confirmation_shows_the_code_and_the_answer_being_checked() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("rust-006").unwrap();
        let mut app = App::new(
            snippet,
            snippet.prompts(Asks::Blocks),
            Asks::Blocks,
            "check".into(),
        );
        for index in 0..app.prompts.len() {
            for char_ in format!("answer {index}").chars() {
                app.on_key(Key::Char(char_));
            }
            app.on_key(Key::Enter);
        }
        assert_eq!(app.phase, Phase::Confirm);

        let screen = screen(&app, 96, 26);
        // The code is still there, with the notes written on it: those notes are
        // the answers, so checking them means seeing the code too.
        assert!(screen.contains("fn main() {"), "{screen}");
        // The first answer, written into the code as a note over its span,
        // with the question it was answering above it.
        assert!(screen.contains("// ? on lines 1-3: "), "{screen}");
        assert!(screen.contains("// answer 0"), "{screen}");
        // The answer being checked is repeated where the input used to be.
        assert!(screen.contains("answer 1 of "), "{screen}");
        assert!(screen.contains("\"answer 0\""), "{screen}");
        // The state says what it is waiting for, and names the key that sends it.
        assert!(screen.contains("ready to grade"), "{screen}");
        assert!(screen.contains("Ctrl-S"), "{screen}");
        assert!(screen.contains("sends them for grading"), "{screen}");
    }

    #[test]
    fn the_screen_says_why_this_snippet() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("rust-006").unwrap();
        let mut app = App::new(
            snippet,
            snippet.prompts(Asks::Blocks),
            Asks::Blocks,
            "check".into(),
        );
        app.reason = "due here: `rust.type.default_int`".to_string();
        let screen = screen(&app, 96, 26);
        assert!(screen.contains("why this"), "{screen}");
        assert!(screen.contains("rust.type.default_int"), "{screen}");
        // It sits above the code, not in the panel where the answer is typed.
        let why = screen.find("why this").unwrap();
        let code = screen.find("fn main()").unwrap();
        assert!(why < code, "{screen}");
    }

    #[test]
    fn grading_shows_a_clock_and_a_reassurance() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("rust-006").unwrap();
        let mut app = App::new(
            snippet,
            snippet.prompts(Asks::Blocks),
            Asks::Blocks,
            "acp:opencode (deepseek)".into(),
        );
        app.phase = Phase::Grading;
        app.grading_for = Some(std::time::Duration::from_secs(12));
        let screen = screen(&app, 96, 26);
        assert!(screen.contains("12s"), "{screen}");
        assert!(screen.contains("the answers are safe"), "{screen}");
    }

    #[test]
    fn note_rows_line_up_with_the_code_gutter() {
        // A note is a comment on the code, so its vertical rule belongs in the
        // same column. The code rows carry a marker column for the `>`; the note
        // rows have to pad for it too, or every note sits one column left and the
        // gutter looks broken.
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("rust-005").unwrap();
        let mut app = App::new(
            snippet,
            snippet.prompts(Asks::Lines),
            Asks::Lines,
            "check".into(),
        );
        for index in 0..2 {
            for char_ in format!("note {index}").chars() {
                app.on_key(Key::Char(char_));
            }
            app.on_key(Key::Enter);
        }
        let screen = screen(&app, 90, 26);
        let columns: Vec<usize> = screen
            .lines()
            .filter(|row| row.contains('│'))
            .map(|row| row.find('│').unwrap_or(0))
            .collect();
        assert!(columns.len() > 3, "{screen}");
        assert!(
            columns.windows(2).all(|pair| pair[0] == pair[1]),
            "the gutter moves between rows: {columns:?}\n{screen}"
        );
    }
}
