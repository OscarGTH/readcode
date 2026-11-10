//! Renders one TUI frame to JSON so `scripts/render_frame.py` can screenshot it.
//!
//! This exists because the interactive view is drawn with cursor addressing, so a
//! capture of its byte stream cannot be replayed into a picture. Feeding the same
//! `draw` call through ratatui's test backend gives the exact cells a terminal
//! would show — no hand-drawn mock-up.
//!
//!   cargo run --quiet --example frame -- annotate /tmp/frame.json
//!   cargo run --quiet --example frame -- review   /tmp/frame.json 32
//!
//! `review` grades for real through the configured reviewer (ACP or BYOK), so it
//! takes as long as an agent does and needs `--model`/`READCODE_MODEL`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use readcode::corpus::Corpus;
use readcode::review::Reviewer;
use readcode::review::acp::AcpReviewer;
use readcode::review::api::ApiReviewer;
use readcode::snippet::Asks;
use readcode::tui;

const WIDTH: u16 = 104;
const HEIGHT: u16 = 24;

/// Answers a real sitting produced, so the frame shows an honest mid-sitting state
/// rather than invented prose.
fn answers_for(snippet: &str, asks: Asks) -> (&'static [&'static str], &'static str) {
    if snippet.starts_with("rust-006") && asks == Asks::Blocks {
        return (
            &[
                "defines a main function, which is where the program starts",
                "binds apples to 3 and pears to 2",
                "adds the two bindings into a new one called total",
            ],
            "prints the total, capturing it by name from the surrounding scope",
        );
    }
    if snippet.starts_with("rust-006") {
        (
            &[
                "defines a main function which is the entry point for the program",
                "defines a variable apples and assigns the number 3 to it",
                "same as line 2 but for pears, and it is 2",
            ],
            "sums the two bindings into a new one called total",
        )
    } else {
        (
            &[
                "brings HashMap into scope",
                "the returned map borrows from text, so its lifetime is tied to the input",
                "counts has to be mutable because the loop inserts into it",
                "for drives the iterator and split_whitespace yields each word",
            ],
            "entry looks the key up, or_insert puts 0 in when it",
        )
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("annotate");
    let out = args
        .get(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/frame.json"));

    let corpus = Corpus::builtin()?;
    let snippet = corpus.find(args.get(4).map(String::as_str).unwrap_or("005"))?;
    let asks = match args.get(5).map(String::as_str) {
        Some("blocks") => Asks::Blocks,
        _ => Asks::Lines,
    };
    let prompts = snippet.prompts(asks);
    let mut app = tui::App::new(snippet, prompts, asks, reviewer_label());

    let (answers, typing) = answers_for(&snippet.id, asks);
    for answer in answers {
        type_text(&mut app, answer);
        app.on_key(tui::Key::Enter);
    }
    type_text(&mut app, typing);

    if mode == "review" {
        // Finish the sitting, so the review covers every prompt.
        app.on_key(tui::Key::Enter);

        let reviewer = build_reviewer()?;
        let grade = reviewer
            .grade(snippet, &app.prompts, &app.annotations())
            .context("grading the canned sitting")?;
        app.finish(grade);
    }

    let height: u16 = args
        .get(3)
        .and_then(|value| value.parse().ok())
        .unwrap_or(HEIGHT);
    let cells = render(&app, height)?;
    std::fs::write(&out, cells)?;
    println!("{}", out.display());
    Ok(())
}

fn type_text(app: &mut tui::App, text: &str) {
    for char_ in text.chars() {
        app.on_key(tui::Key::Char(char_));
    }
}

fn reviewer_label() -> String {
    let model = std::env::var("READCODE_MODEL").ok();
    match model {
        Some(model) => format!("acp:opencode ({model})"),
        None => "acp:opencode".to_string(),
    }
}

fn build_reviewer() -> Result<Box<dyn Reviewer>> {
    let cwd = std::env::temp_dir().join("readcode-frame-agent");
    std::fs::create_dir_all(&cwd).ok();
    if std::env::var("READCODE_API_KEY").is_ok() {
        let mut reviewer = ApiReviewer::from_env().context("cannot build the api reviewer")?;
        if let Ok(model) = std::env::var("READCODE_MODEL") {
            reviewer.model = model;
        }
        return Ok(Box::new(reviewer));
    }
    if std::env::var("READCODE_AGENT").is_err() {
        anyhow::bail!(
            "set READCODE_AGENT (e.g. `opencode acp`) or READCODE_API_KEY to grade for real"
        );
    }
    let mut reviewer = AcpReviewer::from_env(cwd);
    if let Ok(model) = std::env::var("READCODE_MODEL") {
        reviewer.model = Some(model);
    }
    Ok(Box::new(reviewer))
}

fn render(app: &tui::App, height: u16) -> Result<String> {
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, height))?;
    terminal.draw(|frame| tui::draw(frame, app))?;
    let buffer: &Buffer = terminal.backend().buffer();
    Ok(to_json(buffer))
}

fn to_json(buffer: &Buffer) -> String {
    let mut rows = Vec::new();
    for y in 0..buffer.area.height {
        let mut runs: Vec<String> = Vec::new();
        let mut current = String::new();
        let mut current_style = String::new();
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            let style = style_json(cell.fg, cell.bg, cell.modifier);
            let symbol = if cell.symbol() == " " {
                " "
            } else {
                cell.symbol()
            };
            let escaped = json_escape(symbol);
            if style != current_style && !current.is_empty() {
                runs.push(format!("{{\"t\":\"{current}\"{current_style}}}"));
                current.clear();
            }
            current_style = style;
            current.push_str(&escaped);
        }
        if !current.is_empty() {
            runs.push(format!("{{\"t\":\"{current}\"{current_style}}}"));
        }
        rows.push(format!("[{}]", runs.join(",")));
    }
    format!(
        "{{\"width\":{},\"height\":{},\"rows\":[{}]}}",
        buffer.area.width,
        buffer.area.height,
        rows.join(",")
    )
}

fn style_json(fg: Color, bg: Color, modifier: Modifier) -> String {
    let mut parts = String::new();
    if let Some(hex) = hex(fg) {
        parts.push_str(&format!(",\"fg\":\"{hex}\""));
    }
    if let Some(hex) = hex(bg) {
        parts.push_str(&format!(",\"bg\":\"{hex}\""));
    }
    if modifier.contains(Modifier::BOLD) {
        parts.push_str(",\"b\":true");
    }
    if modifier.contains(Modifier::ITALIC) {
        parts.push_str(",\"i\":true");
    }
    if modifier.contains(Modifier::DIM) {
        parts.push_str(",\"d\":true");
    }
    parts
}

/// The GitHub-dark palette, so the frame and the command-line screenshots look
/// like the same program.
fn hex(color: Color) -> Option<String> {
    let value = match color {
        Color::Reset => return None,
        Color::Black => "#484f58",
        Color::Red => "#f85149",
        Color::Green => "#3fb950",
        Color::Yellow => "#d29922",
        Color::Blue => "#58a6ff",
        Color::Magenta => "#bc8cff",
        Color::Cyan => "#39c5cf",
        Color::Gray => "#8b949e",
        Color::DarkGray => "#6e7681",
        Color::LightRed => "#ff7b72",
        Color::LightGreen => "#56d364",
        Color::LightYellow => "#e3b341",
        Color::LightBlue => "#79c0ff",
        Color::LightMagenta => "#d2a8ff",
        Color::LightCyan => "#56d4dd",
        Color::White => "#f0f6fc",
        Color::Rgb(r, g, b) => return Some(format!("#{r:02x}{g:02x}{b:02x}")),
        Color::Indexed(_) => "#c9d1d9",
    };
    Some(value.to_string())
}

fn json_escape(text: &str) -> String {
    text.chars()
        .map(|char_| match char_ {
            '"' => "\\\"".to_string(),
            '\\' => "\\\\".to_string(),
            '\n' => " ".to_string(),
            char_ => char_.to_string(),
        })
        .collect()
}
