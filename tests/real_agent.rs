//! Spends the user's tokens and needs an ACP agent on PATH, so this is ignored by
//! default. Run it deliberately after changing the ACP client:
//!
//! ```sh
//! cargo test --test real_agent -- --ignored --nocapture
//! READCODE_AGENT="copilot --acp" cargo test --test real_agent -- --ignored --nocapture
//! ```

use readcode::corpus::Corpus;
use readcode::review::{Annotation, Reviewer, acp::AcpReviewer};

#[test]
#[ignore = "spawns a real ACP agent and spends tokens"]
fn grades_through_the_real_agent() {
    let corpus = Corpus::builtin().unwrap();
    let snippet = corpus.find("004").unwrap();
    let annotations: Vec<Annotation> = snippet
        .blocks
        .iter()
        .enumerate()
        .map(|(index, _)| Annotation::new(index, "I don't know."))
        .collect();

    let cwd = std::env::temp_dir().join("readcode-real-agent");
    std::fs::create_dir_all(&cwd).unwrap();

    let reviewer = AcpReviewer::from_env(cwd);
    println!("spawning: {} {:?}", reviewer.program, reviewer.args);
    let grade = reviewer.grade(snippet, &annotations).unwrap();

    println!("reviewer: {}", grade.reviewer);
    println!("summary: {}", grade.summary);
    for block in &grade.blocks {
        println!(
            "block {} => {:?} | {}",
            block.block + 1,
            block.verdict,
            block.feedback
        );
    }

    // A reviewer that answered for every block is the contract we care about.
    assert_eq!(grade.blocks.len(), snippet.blocks.len());
    assert!(!grade.summary.is_empty());
}
