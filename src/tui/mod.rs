//! The interactive view: a ratatui screen over the code, with the learner's
//! notes written on it.
//!
//! `app` holds the state machine, `draw` paints it, and this module owns the
//! terminal (raw mode, alternate screen, key translation). The event loop lives
//! in `session`, next to the grading it drives.

pub mod app;
pub mod draw;

pub use app::{Action, App, Key, Phase};
pub use draw::draw;

use anyhow::{Context, Result};
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::io::Stdout;

type Screen = Terminal<CrosstermBackend<Stdout>>;

/// Takes over the terminal and gives it back on drop, including when a grading
/// call fails halfway through a sitting.
pub struct Guard {
    terminal: Screen,
}

impl Guard {
    pub fn enter() -> Result<Self> {
        enable_raw_mode().context("cannot put the terminal into raw mode")?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen).context("cannot open the alternate screen")?;
        let terminal = Terminal::with_options(
            CrosstermBackend::new(stdout),
            TerminalOptions {
                // Whole screen, so the code pane can be as tall as the terminal.
                viewport: Viewport::Fullscreen,
            },
        )
        .context("cannot initialise the terminal")?;
        Ok(Self { terminal })
    }

    pub fn terminal(&mut self) -> &mut Screen {
        &mut self.terminal
    }

    /// Waits for a keystroke, translated into the app's own vocabulary.
    pub fn next_key(&mut self) -> Result<Key> {
        loop {
            match ratatui::crossterm::event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => return Ok(translate(key)),
                Event::Key(_) => continue,
                Event::Resize(_, _) => continue,
                _ => continue,
            }
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

/// crossterm key → app key. Control chords are named so the state machine can
/// stay free of modifier logic.
pub fn translate(key: KeyEvent) -> Key {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char(char_) if control => match char_ {
            'c' => Key::CtrlC,
            'j' => Key::Newline,
            'u' => Key::Clear,
            'b' => Key::Back,
            's' => Key::Submit,
            'h' => Key::Backspace,
            _ => Key::Other,
        },
        KeyCode::Char(char_) => Key::Char(char_),
        KeyCode::Enter => Key::Enter,
        KeyCode::Tab => Key::Other,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Esc => Key::Escape,
        _ => Key::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEventState;

    fn event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn control_chords_are_named() {
        assert_eq!(
            translate(event(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Key::CtrlC
        );
        assert_eq!(
            translate(event(KeyCode::Char('j'), KeyModifiers::CONTROL)),
            Key::Newline
        );
        assert_eq!(
            translate(event(KeyCode::Char('u'), KeyModifiers::CONTROL)),
            Key::Clear
        );
        assert_eq!(
            translate(event(KeyCode::Char('b'), KeyModifiers::CONTROL)),
            Key::Back
        );
        assert_eq!(
            translate(event(KeyCode::Char('s'), KeyModifiers::CONTROL)),
            Key::Submit
        );
    }

    #[test]
    fn plain_characters_survive_including_multibyte() {
        assert_eq!(
            translate(event(KeyCode::Char('a'), KeyModifiers::NONE)),
            Key::Char('a')
        );
        assert_eq!(
            translate(event(KeyCode::Char('ä'), KeyModifiers::SHIFT)),
            Key::Char('ä')
        );
        assert_eq!(
            translate(event(KeyCode::Char('7'), KeyModifiers::NONE)),
            Key::Char('7')
        );
    }

    #[test]
    fn editing_keys_are_translated() {
        assert_eq!(
            translate(event(KeyCode::Enter, KeyModifiers::NONE)),
            Key::Enter
        );
        assert_eq!(
            translate(event(KeyCode::Backspace, KeyModifiers::NONE)),
            Key::Backspace
        );
        assert_eq!(
            translate(event(KeyCode::Esc, KeyModifiers::NONE)),
            Key::Escape
        );
        assert_eq!(
            translate(event(KeyCode::PageDown, KeyModifiers::NONE)),
            Key::PageDown
        );
    }
}
