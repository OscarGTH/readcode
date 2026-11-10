//! The one text that defines grading. Both reviewers use it, so the API client
//! and the ACP agent cannot grade differently.
//!
//! ACP has no system-prompt field, so this contract is prepended to the first
//! prompt of a session — see [`prime`].

use crate::review::Annotation;
use crate::snippet::{Prompt, Snippet};

pub const CONTRACT: &str = r#"You are the reviewer inside readcode, a tool that teaches a programming language by having the learner explain real code and then grading what they wrote.

Rules:
- The learner is given PROMPTS. A prompt is either a block of lines or a single line. Grade the learner's annotation for each prompt against that prompt's expected concepts. Do not rewrite the code, do not give a tutorial, do not ask questions.
- Mark a concept "correct" only if the annotation actually says it — in its own words, with the right meaning. Restating the code is not explaining it.
- "partial" means the idea is there but incomplete or imprecise. "wrong" means the annotation says something false or explains a different thing. "unknown" means the learner said they do not know, or left it blank.
- A short, accurate answer for a single line is complete. Do not ask for more detail than the line needs.
- Grade only the concepts the annotation actually addresses. If a prompt lists several concepts but the answer speaks to one of them, return an entry for that one and OMIT the others — an omitted concept means "not asked of this prompt", not "wrong". Use "unknown" only when the learner says they do not know, or leaves the answer blank.
- Keep feedback to one or two sentences per prompt. Name the misconception, not the syntax.
- Reply with TEXT ONLY. Do not call tools, do not create or edit files, do not run commands, do not request permissions.
- Reply with a single JSON object and nothing else: no prose before or after it, no markdown code fences.

The JSON object has exactly this shape:
{"prompts":[{"prompt":1,"verdict":"correct|partial|wrong|unknown","concepts":{"<concept id>":"correct|partial|wrong|unknown"},"missing":["short phrase"],"feedback":"one or two sentences"}],"summary":"one sentence for the whole snippet"}

Emit one entry per prompt, in prompt order, using the prompt numbers given to you. `concepts` must use the exact concept ids given, and may contain fewer ids than the prompt lists. `missing` lists what the learner failed to notice, as short phrases — not the concept's definition, and not a restatement of the rubric; leave it empty when nothing is missing.
"#;

/// The grading request for one sitting. Everything a reviewer needs is here:
/// the numbered code, the prompts with their expected concepts, and what the
/// learner wrote.
pub fn task(snippet: &Snippet, prompts: &[Prompt], annotations: &[Annotation]) -> String {
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

    out.push_str("\nPROMPTS, AND THE CONCEPTS EACH ONE MUST BE EXPLAINED WITH:\n");
    for (index, prompt) in prompts.iter().enumerate() {
        out.push_str(&format!(
            "\nprompt {} ({}, block {})\n  question: {}\n",
            index + 1,
            prompt.span_label(),
            prompt.block + 1,
            prompt.ask
        ));
        for concept in &prompt.concepts {
            match prompt.notes.get(concept) {
                Some(note) if !note.is_empty() => out.push_str(&format!("  - {concept}: {note}\n")),
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
        out.push_str(&format!("\n[prompt {}]\n{shown}\n", annotation.prompt + 1));
    }

    out.push_str(
        "\nGrade every prompt above. Be strict about meaning and generous about wording: a correct idea in clumsy words is \"correct\", and a line that is fully explained in one clause is not incomplete.",
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
    use crate::snippet::Asks;

    #[test]
    fn the_contract_states_the_shape_it_expects() {
        for needle in [
            "\"prompts\"",
            "\"prompt\"",
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
    fn the_task_carries_code_prompts_and_annotations() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("001").unwrap();
        let prompts = snippet.prompts(Asks::Blocks);
        let notes: Vec<Annotation> = prompts
            .iter()
            .enumerate()
            .map(|(index, _)| Annotation::new(index, format!("annotation number {index}")))
            .collect();
        let text = task(snippet, &prompts, &notes);

        assert!(text.contains("1 | fn shout(mut text: String) -> String {"));
        for concept in snippet.concepts() {
            assert!(text.contains(&concept), "task is missing concept {concept}");
        }
        for (index, prompt) in prompts.iter().enumerate() {
            assert!(text.contains(&format!("annotation number {index}")));
            assert!(text.contains(&format!("prompt {} (", index + 1)));
            assert!(text.contains(&prompt.ask));
        }
    }

    #[test]
    fn line_mode_prompts_are_labelled_by_line() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap();
        let prompts = snippet.prompts(Asks::Lines);
        let notes: Vec<Annotation> = prompts
            .iter()
            .enumerate()
            .map(|(index, _)| Annotation::new(index, "x"))
            .collect();
        let text = task(snippet, &prompts, &notes);
        assert!(text.contains("prompt 1 (line "), "{text}");
        assert!(!text.contains("(lines 1-5, block 1)\nprompt 2 (line"));
    }

    #[test]
    fn blank_annotations_are_labelled_not_dropped() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("004").unwrap();
        let prompts = snippet.prompts(Asks::Blocks);
        let notes: Vec<Annotation> = (0..prompts.len())
            .map(|index| Annotation::new(index, "   "))
            .collect();
        let text = task(snippet, &prompts, &notes);
        assert_eq!(text.matches("(left blank)").count(), prompts.len());
    }

    #[test]
    fn primed_prompts_start_with_the_contract() {
        let primed = prime("do the thing");
        assert!(primed.starts_with(CONTRACT));
        assert!(primed.ends_with("do the thing"));
    }
}
