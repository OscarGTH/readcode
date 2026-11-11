//! readcode — learn a language by annotating real code, then get graded on it.
//!
//! The crate is split so each piece is testable without a terminal, a network
//! connection or an agent process:
//!
//! * [`snippet`] — the corpus format and its validation rules.
//! * [`srs`] — per-concept mastery and scheduling ("what should I see next").
//! * [`store`] — the progress file.
//! * [`review`] — the grading contract and its two backends: a BYOK HTTP client
//!   and an ACP-hosted agent.
//! * [`session`] — one run: pick a snippet, collect annotations, grade, schedule.
//! * [`ui`] — terminal rendering.

pub mod config;
pub mod corpus;
pub mod docs;
pub mod draft;
pub mod eval;
pub mod highlight;
pub mod review;
pub mod session;
pub mod setup;
pub mod snippet;
pub mod srs;
pub mod store;
pub mod tui;
pub mod ui;
