//! The grading eval suite: does a reviewer still grade the way we promise?
//!
//! One case per file under `evals/cases/`. Targets are the reviewers this app
//! ships: `mock` (harness self-check), `local` (offline rubric), `api`, `acp`.

pub mod case;
pub mod runner;

pub use case::{Case, CaseAnnotation, Expectation, load, load_dir, load_embedded};
pub use runner::{
    CaseResult, MockReviewer, RunSummary, iso8601, run_case, run_suite, run_suite_mock, write_run,
};
