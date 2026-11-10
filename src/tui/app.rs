//! The interactive state machine behind the TUI.
//!
//! Everything here is plain data and pure methods — no terminal, no clock, no
//! I/O — so the behaviour that matters (what a keystroke does, when a sitting is
//! finished, what the notes look like) is testable without a pty.

use std::collections::BTreeMap;

use crate::review::Grade;
use crate::snippet::{Asks, Prompt, Snippet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Answering the prompts.
    Annotate,
    /// Answers are in, the reviewer is working.
    Grading,
    /// Verdicts are in and scrollable.
    Review,
}

/// What the event loop should do after a keystroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    /// The sitting was abandoned; nothing should be saved.
    Abort,
    /// The last answer is in — go and grade.
    Grade,
    /// The review was dismissed; the answers are ready to be scored and saved.
    Finish,
    /// Grading failed and the learner wants another go: the answers are still here.
    Retry,
}

pub struct App<'a> {
    pub snippet: &'a Snippet,
    pub prompts: Vec<Prompt>,
    pub mode: Asks,
    pub reviewer: String,
    pub phase: Phase,
    /// One answer per prompt, filled as they are submitted.
    pub answers: Vec<String>,
    /// The prompt being answered.
    pub index: usize,
    /// The answer being typed, as chars so the cursor is never inside a
    /// multi-byte character.
    input: Vec<char>,
    pub cursor: usize,
    /// First visible row of the body pane.
    pub scroll: usize,
    pub grade: Option<Grade>,
    pub error: Option<String>,
}

impl<'a> App<'a> {
    pub fn new(snippet: &'a Snippet, prompts: Vec<Prompt>, mode: Asks, reviewer: String) -> Self {
        let answers = vec![String::new(); prompts.len()];
        Self {
            snippet,
            prompts,
            mode,
            reviewer,
            phase: Phase::Annotate,
            answers,
            index: 0,
            input: Vec::new(),
            cursor: 0,
            scroll: 0,
            grade: None,
            error: None,
        }
    }

    pub fn current(&self) -> Option<&Prompt> {
        self.prompts.get(self.index)
    }

    pub fn input_text(&self) -> String {
        self.input.iter().collect()
    }

    pub fn input_chars(&self) -> &[char] {
        &self.input
    }

    /// Notes keyed by the first line they cover, with the last line too so the
    /// code pane can tell a line's note from a block's: a note about one line
    /// sits above it, a note about a span sits after it, labelled with the span.
    pub fn notes(&self) -> BTreeMap<usize, (usize, String)> {
        let mut notes = BTreeMap::new();
        for (prompt, answer) in self.prompts.iter().zip(&self.answers) {
            if answer.trim().is_empty() {
                continue;
            }
            notes.insert(prompt.start, (prompt.end, answer.trim().to_string()));
        }
        notes
    }

    /// Progress through the sitting, for the header.
    pub fn answered(&self) -> usize {
        self.answers.iter().filter(|a| !a.trim().is_empty()).count()
    }

    fn insert_char(&mut self, char_: char) {
        self.input.insert(self.cursor, char_);
        self.cursor += 1;
    }

    fn newline(&mut self) {
        self.insert_char('\n');
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.input.remove(self.cursor);
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.input.len() {
            self.input.remove(self.cursor);
        }
    }

    fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn right(&mut self) {
        if self.cursor < self.input.len() {
            self.cursor += 1;
        }
    }

    fn home(&mut self) {
        self.cursor = 0;
    }

    fn end(&mut self) {
        self.cursor = self.input.len();
    }

    fn clear(&mut self) {
        self.input.clear();
        self.cursor = 0;
    }

    /// Stores the typed answer and moves on. Returns `Action::Grade` when there
    /// is nothing left to ask.
    fn submit(&mut self) -> Action {
        let typed = self.input_text().trim().to_string();
        if let Some(answer) = self.answers.get_mut(self.index) {
            *answer = typed;
        }
        self.clear();
        self.scroll = 0;
        if self.index + 1 >= self.prompts.len() {
            self.phase = Phase::Grading;
            Action::Grade
        } else {
            self.index += 1;
            Action::None
        }
    }

    /// Skips the current prompt without answering it.
    fn skip(&mut self) -> Action {
        self.clear();
        if let Some(answer) = self.answers.get_mut(self.index) {
            answer.clear();
        }
        self.advance()
    }

    fn advance(&mut self) -> Action {
        if self.index + 1 >= self.prompts.len() {
            self.phase = Phase::Grading;
            Action::Grade
        } else {
            self.index += 1;
            Action::None
        }
    }

    fn back(&mut self) {
        if self.index > 0 {
            self.index -= 1;
            self.input = self.answers[self.index].chars().collect();
            self.cursor = self.input.len();
        }
    }

    pub fn finish(&mut self, grade: Grade) {
        self.grade = Some(grade);
        self.phase = Phase::Review;
        self.scroll = 0;
    }

    pub fn fail(&mut self, error: String) {
        self.error = Some(error);
        self.phase = Phase::Review;
        self.scroll = 0;
    }

    /// Grading failed; keep the answers and try again.
    pub fn retry(&mut self) {
        self.error = None;
        self.grade = None;
        self.phase = Phase::Grading;
        self.scroll = 0;
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let next = self.scroll as isize + delta;
        self.scroll = next.max(0) as usize;
    }

    pub fn annotations(&self) -> Vec<crate::review::Annotation> {
        self.answers
            .iter()
            .enumerate()
            .map(|(index, text)| crate::review::Annotation::new(index, text.clone()))
            .collect()
    }

    /// The whole keystroke mapping, in one place.
    pub fn on_key(&mut self, key: Key) -> Action {
        match self.phase {
            Phase::Annotate => self.on_key_annotate(key),
            Phase::Grading => match key {
                Key::CtrlC => Action::Abort,
                _ => Action::None,
            },
            Phase::Review => self.on_key_review(key),
        }
    }

    fn on_key_annotate(&mut self, key: Key) -> Action {
        match key {
            Key::Char(char_) => {
                self.insert_char(char_);
                Action::None
            }
            Key::Enter => {
                let typed = self.input_text();
                if typed.trim().is_empty() {
                    // Nothing typed: Enter just skips this prompt, so a sitting
                    // of six lines does not become twelve keystrokes.
                    self.skip()
                } else {
                    self.submit()
                }
            }
            Key::Newline => {
                self.newline();
                Action::None
            }
            Key::Backspace => {
                self.backspace();
                Action::None
            }
            Key::Delete => {
                self.delete();
                Action::None
            }
            Key::Left => {
                self.left();
                Action::None
            }
            Key::Right => {
                self.right();
                Action::None
            }
            Key::Home => {
                self.home();
                Action::None
            }
            Key::End => {
                self.end();
                Action::None
            }
            Key::Clear => {
                self.clear();
                Action::None
            }
            Key::Back => {
                self.back();
                Action::None
            }
            Key::Escape | Key::CtrlC => Action::Abort,
            Key::Other | Key::Up | Key::Down | Key::PageUp | Key::PageDown => Action::None,
        }
    }

    fn on_key_review(&mut self, key: Key) -> Action {
        match key {
            // Only meaningful after a failure, and then it matters: the sitting's
            // answers are the only thing of value here, and they are not lost yet.
            Key::Char('r') if self.error.is_some() => Action::Retry,
            Key::Char('q') | Key::Escape | Key::CtrlC => Action::Abort,
            Key::Enter => Action::Finish,
            Key::Char('j') | Key::Down => {
                self.scroll_by(1);
                Action::None
            }
            Key::Char('k') | Key::Up => {
                self.scroll_by(-1);
                Action::None
            }
            Key::PageDown => {
                self.scroll_by(10);
                Action::None
            }
            Key::PageUp => {
                self.scroll_by(-10);
                Action::None
            }
            _ => Action::None,
        }
    }
}

/// A terminal-independent key, so the state machine does not depend on
/// crossterm's event types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Newline,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Clear,
    Back,
    Escape,
    CtrlC,
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::Corpus;
    use crate::snippet::Asks;

    fn app<'a>(snippet: &'a Snippet) -> App<'a> {
        let prompts = snippet.prompts(Asks::Lines);
        App::new(snippet, prompts, Asks::Lines, "acp:opencode".into())
    }

    fn type_text(app: &mut App, text: &str) {
        for char_ in text.chars() {
            let key = if char_ == '\n' {
                Key::Newline
            } else {
                Key::Char(char_)
            };
            app.on_key(key);
        }
    }

    #[test]
    fn typing_then_enter_stores_the_answer_and_moves_on() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap().clone();
        let mut app = app(&snippet);
        let first_line = app.prompts[0].start;

        type_text(&mut app, "brings HashMap into scope");
        assert_eq!(app.on_key(Key::Enter), Action::None);

        assert_eq!(app.index, 1);
        assert_eq!(app.answers[0], "brings HashMap into scope");
        assert_eq!(app.input_text(), "");
        assert_eq!(
            app.notes().get(&first_line).map(|(_, note)| note.as_str()),
            Some("brings HashMap into scope")
        );
    }

    #[test]
    fn enter_on_an_empty_answer_skips_the_prompt() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap().clone();
        let mut app = app(&snippet);
        app.on_key(Key::Enter);
        assert_eq!(app.index, 1);
        assert_eq!(app.answers[0], "");
        assert!(app.notes().is_empty());
    }

    #[test]
    fn the_last_answer_asks_for_grading() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap().clone();
        let mut app = app(&snippet);
        for _ in 0..app.prompts.len() {
            type_text(&mut app, "something");
            let action = app.on_key(Key::Enter);
            if action == Action::Grade {
                assert_eq!(app.phase, Phase::Grading);
                return;
            }
        }
        panic!("the sitting never reached grading");
    }

    #[test]
    fn backspace_deletes_before_the_cursor_only() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap().clone();
        let mut app = app(&snippet);
        type_text(&mut app, "abcd");
        app.on_key(Key::Left);
        app.on_key(Key::Left);
        // Cursor sits between `b` and `c`, so backspace takes `b`.
        app.on_key(Key::Backspace);
        assert_eq!(app.input_text(), "acd");
        // Delete takes what is under the cursor instead.
        app.on_key(Key::Delete);
        assert_eq!(app.input_text(), "ad");
    }

    #[test]
    fn ctrl_j_inserts_a_newline_and_enter_submits() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap().clone();
        let mut app = app(&snippet);
        type_text(&mut app, "first");
        app.on_key(Key::Newline);
        type_text(&mut app, "second");
        assert_eq!(app.input_text(), "first\nsecond");
        app.on_key(Key::Enter);
        assert_eq!(app.answers[0], "first\nsecond");
    }

    #[test]
    fn back_reopens_the_previous_answer() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap().clone();
        let mut app = app(&snippet);
        type_text(&mut app, "one");
        app.on_key(Key::Enter);
        app.on_key(Key::Back);
        assert_eq!(app.index, 0);
        assert_eq!(app.input_text(), "one");
        assert_eq!(app.cursor, 3);
    }

    #[test]
    fn typing_a_multi_byte_character_keeps_the_cursor_on_boundaries() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap().clone();
        let mut app = app(&snippet);
        type_text(&mut app, "ä");
        type_text(&mut app, "ö");
        assert_eq!(app.cursor, 2);
        app.on_key(Key::Backspace);
        assert_eq!(app.input_text(), "ä");
    }

    #[test]
    fn escape_abandons_the_sitting() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap().clone();
        let mut app = app(&snippet);
        assert_eq!(app.on_key(Key::Escape), Action::Abort);
    }

    #[test]
    fn retry_is_only_offered_after_a_failure_and_keeps_the_answers() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap().clone();
        let mut app = app(&snippet);
        type_text(&mut app, "an answer worth keeping");
        app.on_key(Key::Enter);

        // A good review: `r` is not a retry key.
        app.phase = Phase::Review;
        assert_eq!(app.on_key(Key::Char('r')), Action::None);

        app.fail("the reviewer's JSON could not be parsed".into());
        assert_eq!(app.on_key(Key::Char('r')), Action::Retry);
        app.retry();
        assert_eq!(app.phase, Phase::Grading);
        assert!(app.error.is_none());
        assert_eq!(app.answers[0], "an answer worth keeping");
    }

    #[test]
    fn the_review_scrolls_and_finishes() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap().clone();
        let mut app = app(&snippet);
        app.phase = Phase::Review;
        app.on_key(Key::Char('j'));
        app.on_key(Key::Char('j'));
        assert_eq!(app.scroll, 2);
        app.on_key(Key::Char('k'));
        assert_eq!(app.scroll, 1);
        app.on_key(Key::PageUp);
        assert_eq!(app.scroll, 0);
        assert_eq!(app.on_key(Key::Enter), Action::Finish);
    }
}
