//! The one text that defines grading. Both model-backed reviewers use it, so
//! the offline rubric, the API client and the ACP agent cannot drift apart.
//!
//! ACP has no system-prompt field, so this contract is prepended to the first
//! prompt of a session — see [`prime`].

use crate::review::Annotation;
use crate::snippet::Snippet;

pub const CONTRACT: &str = r#"You are the reviewer inside readcode, a tool that teaches a programming language by having the learner annotate real code, block by block, and then grading what they wrote.

Rules:
- Grade the learner's annotation for each block against that block's expected concepts. Do not rewrite the code, do not give a tutorial, do not ask questions.
- Mark a concept "correct" only if the annotation actually says it — in its own words, with the right meaning. Restating the code is not explaining it.
- "partial" means the idea is there but incomplete or imprecise. "wrong" means the annotation says something false or explains a different thing. "unknown" means the learner said they do not know, or left it blank.
- Keep feedback to one or two sentences per block. Name the misconception, not the syntax.
- Reply with TEXT ONLY. Do not call tools, do not create or edit files, do not run commands, do not request permissions.
- Reply with a single JSON object and nothing else: no prose before or after it, no markdown code fences.

The JSON object has exactly this shape:
{"blocks":[{"block":1,"verdict":"correct|partial|wrong|unknown","concepts":{"<concept id>":"correct|partial|wrong|unknown"},"missing":["short phrase"],"feedback":"one or two sentences"}],"summary":"one sentence for the whole snippet"}

Emit one entry per block, in block order, using the block numbers given to you. `concepts` must use the exact concept ids given. `missing` lists what the learner failed to notice, as short phrases; leave it empty when nothing is missing.
"#;

/// The grading request for one snippet. Everything a reviewer needs is here:
/// the numbered code, the blocks with their expected concepts, and what the
/// learner wrote.
pub fn task(snippet: &Snippet, annotations: &[Annotation]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "SNIPPET {} ({}): {}\n",
        snippet.id, snippet.language, snippet.title
    ));
    if let Some(intro) = &snippet.intro {
        out.push_str(&format!("Framing: {intro}\n"));
    }

    out.push_str("\nCODE, WITH LINE NUMBERS:\n");
    for (index, line) in snippet.code_lines().iter().enumerate() {
        out.push_str(&format!("{:>3} | {}\n", index + 1, line));
    }

    out.push_str("\nBLOCKS, AND THE CONCEPTS EACH ONE MUST BE EXPLAINED WITH:\n");
    for (index, block) in snippet.blocks.iter().enumerate() {
        let (start, end) = (block.lines[0], block.lines[1]);
        out.push_str(&format!(
            "\nblock {} (lines {start}-{end})\n  question: {}\n",
            index + 1,
            block.ask
        ));
        for concept in &block.concepts {
            match block.expects.iter().find(|e| &e.concept == concept) {
                Some(expect) if !expect.note.is_empty() => {
                    out.push_str(&format!("  - {concept}: {}\n", expect.note))
                }
                _ => out.push_str(&format!("  - {concept}\n")),
            }
        }
    }

    out.push_str("\nTHE LEARNER'S ANNOTATIONS:\n");
    for annotation in annotations {
        let text = annotation.text.trim();
        let shown = if text.is_empty() {
            "(left blank)"
        } else {
            text
        };
        out.push_str(&format!("\n[block {}]\n{shown}\n", annotation.block + 1));
    }

    out.push_str(
        "\nGrade every block above. Be strict about meaning and generous about wording: a correct idea in clumsy words is \"correct\".",
    );
    out
}

/// ACP has no system prompt, so the contract travels with the first prompt.
pub fn prime(task: &str) -> String {
    format!("{CONTRACT}\n\n---\n\n{task}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::Corpus;

    fn annotation(block: usize, text: &str) -> Annotation {
        Annotation::new(block, text)
    }

    #[test]
    fn the_contract_states_the_shape_it_expects() {
        for needle in [
            "\"blocks\"",
            "\"verdict\"",
            "\"concepts\"",
            "\"missing\"",
            "\"summary\"",
            "TEXT ONLY",
            "no markdown code fences",
        ] {
            assert!(CONTRACT.contains(needle), "contract is missing {needle}");
        }
    }

    #[test]
    fn the_task_carries_code_blocks_and_annotations() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("001").unwrap();
        let notes: Vec<Annotation> = snippet
            .blocks
            .iter()
            .enumerate()
            .map(|(i, _)| annotation(i, &format!("annotation number {i}")))
            .collect();
        let text = task(snippet, &notes);

        assert!(text.contains("1 | fn shout(mut text: String) -> String {"));
        for concept in snippet.concepts() {
            assert!(text.contains(&concept), "task is missing concept {concept}");
        }
        for (i, _) in snippet.blocks.iter().enumerate() {
            assert!(text.contains(&format!("annotation number {i}")));
            assert!(text.contains(&format!("block {} (lines ", i + 1)));
        }
    }

    #[test]
    fn blank_annotations_are_labelled_not_dropped() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("004").unwrap();
        let notes: Vec<Annotation> = (0..snippet.blocks.len())
            .map(|i| annotation(i, "   "))
            .collect();
        let text = task(snippet, &notes);
        assert_eq!(text.matches("(left blank)").count(), snippet.blocks.len());
    }

    #[test]
    fn primed_prompts_start_with_the_contract() {
        let primed = prime("do the thing");
        assert!(primed.starts_with(CONTRACT));
        assert!(primed.ends_with("do the thing"));
    }
}
