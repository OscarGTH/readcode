//! readcode — learn a language by annotating real code, then get graded on it.
//!
//! The crate is split so each piece is testable without a terminal, a network
//! connection or an agent process:
//!
//! * [`snippet`] — the corpus format and its validation rules.
//! * [`srs`] — per-concept mastery and scheduling ("what should I see next").
//! * [`store`] — the progress file.
//! * [`review`] — the grading contract and its three backends: a deterministic
//!   offline rubric, a BYOK HTTP client, and an ACP-hosted agent.
//! * [`session`] — one run: pick a snippet, collect annotations, grade, schedule.
//! * [`ui`] — terminal rendering.

pub mod corpus;
pub mod eval;
pub mod review;
pub mod session;
pub mod snippet;
pub mod srs;
pub mod store;
pub mod ui;
