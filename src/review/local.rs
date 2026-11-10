//! The offline reviewer. No model, no network: it checks the annotation against
//! the keywords each concept carries. Coarser than a model, but deterministic,
//! instant, free, and always available — which is what makes `readcode` usable
//! before any key or agent is configured.

use anyhow::Result;

use super::{Annotation, Grade, PromptGrade, Reviewer};
use crate::snippet::{Prompt, Snippet};
use crate::srs::{Verdict, combine};

pub struct LocalReviewer;

impl Reviewer for LocalReviewer {
    fn name(&self) -> String {
        "local".to_string()
    }

    fn grade(
        &self,
        _snippet: &Snippet,
        prompts: &[Prompt],
        annotations: &[Annotation],
    ) -> Result<Grade> {
        let mut graded = Vec::with_capacity(prompts.len());

        for (index, prompt) in prompts.iter().enumerate() {
            let text = annotations
                .iter()
                .find(|annotation| annotation.prompt == index)
                .map(|annotation| annotation.text.as_str())
                .unwrap_or("");

            let blank = text.trim().is_empty();
            let unsure = !blank && is_unsure(text);

            let mut concepts = Vec::new();
            let mut missing = Vec::new();
            for concept in &prompt.concepts {
                let verdict = if blank || unsure {
                    Verdict::Unknown
                } else {
                    match_concept(prompt, concept, text)
                };
                concepts.push((concept.clone(), verdict));
                if verdict != Verdict::Correct {
                    // The note is the correction: it says what the learner
                    // should have seen. Sending the raw concept id would not.
                    if let Some(note) = prompt
                        .expects
                        .iter()
                        .find(|expect| &expect.concept == concept)
                        .map(|expect| expect.note.clone())
                        .filter(|note| !note.is_empty())
                    {
                        missing.push(note);
                    }
                }
            }

            let verdicts: Vec<Verdict> = concepts.iter().map(|(_, verdict)| *verdict).collect();
            let verdict = combine(&verdicts);
            let feedback = if blank {
                "Left blank — treated as not known yet.".to_string()
            } else if unsure {
                "Marked as unknown, so this comes back soon.".to_string()
            } else {
                String::new()
            };

            graded.push(PromptGrade {
                prompt: index,
                verdict,
                feedback,
                missing,
                concepts,
            });
        }

        let correct = graded
            .iter()
            .filter(|prompt| prompt.verdict == Verdict::Correct)
            .count();
        let summary = format!(
            "{correct}/{} prompts matched the rubric. The offline reviewer compares keywords, so it cannot credit an idea you phrased differently — use a model-backed reviewer for that.",
            graded.len()
        );

        Ok(Grade {
            reviewer: self.name(),
            prompts: graded,
            summary,
        })
    }
}

/// Any hit is evidence; two hits, or a third of the rubric, is a pass.
fn match_concept(prompt: &Prompt, concept: &str, text: &str) -> Verdict {
    let Some(expect) = prompt
        .expects
        .iter()
        .find(|expect| expect.concept == concept)
    else {
        return Verdict::Unknown;
    };
    let lower = text.to_lowercase();
    let hits = expect
        .keywords
        .iter()
        .filter(|keyword| {
            let keyword = keyword.to_lowercase();
            !keyword.is_empty() && lower.contains(&keyword)
        })
        .count();
    let total = expect.keywords.len().max(1);

    if hits == 0 {
        Verdict::Wrong
    } else if hits >= 2 || hits * 3 >= total {
        Verdict::Correct
    } else {
        Verdict::Partial
    }
}

fn is_unsure(text: &str) -> bool {
    let lower = text.to_lowercase();
    if lower.trim().trim_end_matches(['!', '.', ',']).trim() == "?" {
        return true;
    }
    [
        "don't know",
        "dont know",
        "do not know",
        "no idea",
        "no clue",
        "not sure",
        "not certain",
        "not confident",
        "idk",
        "unknown",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::Corpus;
    use crate::snippet::Asks;

    fn grade_prompt(needle: &str, asks: Asks, target: usize, text: &str) -> PromptGrade {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find(needle).unwrap();
        let prompts = snippet.prompts(asks);
        let annotations: Vec<Annotation> = prompts
            .iter()
            .enumerate()
            .map(|(index, _)| {
                Annotation::new(
                    index,
                    if index == target {
                        text.to_string()
                    } else {
                        String::new()
                    },
                )
            })
            .collect();
        let grade = LocalReviewer
            .grade(snippet, &prompts, &annotations)
            .unwrap();
        grade.prompts[target].clone()
    }

    fn block_grade(needle: &str, block: usize, text: &str) -> PromptGrade {
        grade_prompt(needle, Asks::Blocks, block, text)
    }

    #[test]
    fn a_correct_annotation_is_credited() {
        let block = block_grade(
            "004",
            1,
            "impl attaches methods to the type; &mut self makes the receiver mutable so the method can change hits; the last expression returns a bool from the comparison; += adds one and writes it back",
        );
        assert_eq!(block.verdict, Verdict::Correct, "{:?}", block);
    }

    #[test]
    fn a_blank_annotation_is_unknown_and_carries_the_correction() {
        let block = block_grade("004", 1, "   ");
        assert_eq!(block.verdict, Verdict::Unknown);
        assert!(!block.missing.is_empty());
        assert!(block.missing.iter().any(|note| note.contains("&mut self")));
    }

    #[test]
    fn saying_you_do_not_know_is_never_graded_as_wrong() {
        for phrase in ["I don't know", "no idea", "idk", "?", "not sure"] {
            let block = block_grade("001", 0, phrase);
            assert_eq!(block.verdict, Verdict::Unknown, "{phrase}");
        }
    }

    #[test]
    fn an_unrelated_annotation_is_wrong() {
        let block = block_grade("001", 0, "this line prints a number to the terminal");
        assert_eq!(block.verdict, Verdict::Wrong);
    }

    #[test]
    fn one_keyword_of_many_is_only_partial() {
        // "move" is one of the ownership keywords; the rubric wants more.
        let block = block_grade("001", 0, "it is a move");
        assert_ne!(block.verdict, Verdict::Correct);
    }

    #[test]
    fn every_snippet_can_be_fully_credited() {
        let corpus = Corpus::builtin().unwrap();
        for snippet in corpus.snippets() {
            for asks in [Asks::Blocks, Asks::Lines] {
                let prompts = snippet.prompts(asks);
                for (index, prompt) in prompts.iter().enumerate() {
                    // Build an annotation out of the rubric itself: if this does
                    // not reach `correct`, the keywords and the matcher disagree.
                    let text: Vec<String> = prompt
                        .expects
                        .iter()
                        .flat_map(|expect| expect.keywords.iter().cloned())
                        .collect();
                    let grade = grade_prompt(&snippet.id, asks, index, &text.join(", "));
                    assert_eq!(
                        grade.verdict,
                        Verdict::Correct,
                        "{} {} prompt {} with rubric text {:?}",
                        snippet.id,
                        asks.label(),
                        index + 1,
                        grade.concepts
                    );
                }
            }
        }
    }

    #[test]
    fn line_mode_grades_each_line_on_its_own_rubric() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("005").unwrap();
        let prompts = snippet.prompts(Asks::Lines);
        // The authored `entry(...).or_insert(0)` line is asked with its own rubric.
        let index = prompts
            .iter()
            .position(|prompt| prompt.start == 6)
            .expect("an authored line prompt for line 6");
        assert!(
            prompts[index].ask.contains("Unpack"),
            "{:?}",
            prompts[index].ask
        );
        let good = grade_prompt(
            "005",
            Asks::Lines,
            index,
            "entry looks the key up and inserts a default 0 if it is missing, and the star dereferences the mutable reference so we can add one",
        );
        assert_eq!(good.verdict, Verdict::Correct, "{:?}", good);

        let bad = grade_prompt("005", Asks::Lines, index, "it prints the word count");
        assert_eq!(bad.verdict, Verdict::Wrong);
    }

    #[test]
    fn the_grade_records_which_concepts_were_missed() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("002").unwrap();
        let prompts = snippet.prompts(Asks::Blocks);
        let annotations: Vec<Annotation> = prompts
            .iter()
            .enumerate()
            .map(|(index, _)| {
                Annotation::new(index, "the ? propagates the error with an early return")
            })
            .collect();
        let grade = LocalReviewer
            .grade(snippet, &prompts, &annotations)
            .unwrap();
        let verdicts = grade.concept_verdicts();
        assert!(
            verdicts
                .iter()
                .any(|(concept, verdict)| concept == "rust.question_mark"
                    && *verdict == Verdict::Correct)
        );
        assert!(
            verdicts
                .iter()
                .any(|(concept, verdict)| concept == "rust.error.dynamic"
                    && *verdict != Verdict::Correct)
        );
    }
}
