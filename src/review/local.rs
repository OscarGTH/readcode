//! The offline reviewer. No model, no network: it checks the annotation against
//! the keywords each concept carries. Coarser than a model, but deterministic,
//! instant, free, and always available — which is what makes `readcode` usable
//! before any key or agent is configured.

use anyhow::Result;

use super::{Annotation, BlockGrade, Grade, Reviewer};
use crate::snippet::{Block, Snippet};
use crate::srs::{Verdict, combine};

pub struct LocalReviewer;

impl Reviewer for LocalReviewer {
    fn name(&self) -> String {
        "local".to_string()
    }

    fn grade(&self, snippet: &Snippet, annotations: &[Annotation]) -> Result<Grade> {
        let mut blocks = Vec::with_capacity(snippet.blocks.len());

        for (index, block) in snippet.blocks.iter().enumerate() {
            let text = annotations
                .iter()
                .find(|a| a.block == index)
                .map(|a| a.text.as_str())
                .unwrap_or("");

            let blank = text.trim().is_empty();
            let unsure = !blank && is_unsure(text);

            let mut concepts = Vec::new();
            let mut missing = Vec::new();
            for concept in &block.concepts {
                let verdict = if blank || unsure {
                    Verdict::Unknown
                } else {
                    match_concept(block, concept, text)
                };
                concepts.push((concept.clone(), verdict));
                if verdict != Verdict::Correct {
                    // The note is the correction: it says what the learner
                    // should have seen. Sending the raw concept id would not.
                    if let Some(note) = block
                        .expects
                        .iter()
                        .find(|e| &e.concept == concept)
                        .map(|e| e.note.clone())
                        .filter(|n| !n.is_empty())
                    {
                        missing.push(note);
                    }
                }
            }

            let verdicts: Vec<Verdict> = concepts.iter().map(|(_, v)| *v).collect();
            let verdict = combine(&verdicts);
            let feedback = if blank {
                "Left blank — treated as not known yet.".to_string()
            } else if unsure {
                "Marked as unknown, so this comes back soon.".to_string()
            } else {
                String::new()
            };

            blocks.push(BlockGrade {
                block: index,
                verdict,
                feedback,
                missing,
                concepts,
            });
        }

        let correct = blocks
            .iter()
            .filter(|b| b.verdict == Verdict::Correct)
            .count();
        let summary = format!(
            "{correct}/{} blocks matched the rubric. The offline reviewer compares keywords, so it cannot credit an idea you phrased differently — use a model-backed reviewer for that.",
            blocks.len()
        );

        Ok(Grade {
            reviewer: self.name(),
            blocks,
            summary,
        })
    }
}

/// Any hit is evidence; two hits, or a third of the rubric, is a pass.
fn match_concept(block: &Block, concept: &str, text: &str) -> Verdict {
    let Some(expect) = block.expects.iter().find(|e| e.concept == concept) else {
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

    fn grade_block(needle: &str, block: usize, text: &str) -> BlockGrade {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find(needle).unwrap();
        let annotations: Vec<Annotation> = snippet
            .blocks
            .iter()
            .enumerate()
            .map(|(i, _)| {
                Annotation::new(
                    i,
                    if i == block {
                        text.to_string()
                    } else {
                        String::new()
                    },
                )
            })
            .collect();
        let grade = LocalReviewer.grade(snippet, &annotations).unwrap();
        grade.blocks[block].clone()
    }

    #[test]
    fn a_correct_annotation_is_credited() {
        let block = grade_block(
            "004",
            1,
            "impl attaches methods to the type; &mut self makes the receiver mutable so the method can change hits; the last expression returns a bool from the comparison; += adds one and writes it back",
        );
        assert_eq!(block.verdict, Verdict::Correct, "{:?}", block);
    }

    #[test]
    fn a_blank_annotation_is_unknown_and_carries_the_correction() {
        let block = grade_block("004", 1, "   ");
        assert_eq!(block.verdict, Verdict::Unknown);
        assert!(!block.missing.is_empty());
        assert!(block.missing.iter().any(|m| m.contains("&mut self")));
    }

    #[test]
    fn saying_you_do_not_know_is_never_graded_as_wrong() {
        for phrase in ["I don't know", "no idea", "idk", "?", "not sure"] {
            let block = grade_block("001", 0, phrase);
            assert_eq!(block.verdict, Verdict::Unknown, "{phrase}");
        }
    }

    #[test]
    fn an_unrelated_annotation_is_wrong() {
        let block = grade_block("001", 0, "this line prints a number to the terminal");
        assert_eq!(block.verdict, Verdict::Wrong);
    }

    #[test]
    fn one_keyword_of_many_is_only_partial() {
        // "move" is one of the ownership keywords; the rubric wants more.
        let block = grade_block("001", 0, "it is a move");
        assert_ne!(block.verdict, Verdict::Correct);
    }

    #[test]
    fn every_snippet_can_be_fully_credited() {
        let corpus = Corpus::builtin().unwrap();
        for snippet in corpus.snippets() {
            for (index, block) in snippet.blocks.iter().enumerate() {
                // Build an annotation out of the rubric itself: if this does not
                // reach `correct`, the keywords and the matcher disagree.
                let text: Vec<String> = block
                    .expects
                    .iter()
                    .flat_map(|e| e.keywords.iter().cloned())
                    .collect();
                let grade = grade_block(&snippet.id, index, &text.join(", "));
                assert_eq!(
                    grade.verdict,
                    Verdict::Correct,
                    "{} block {} with rubric text {:?}",
                    snippet.id,
                    index + 1,
                    grade.concepts
                );
            }
        }
    }

    #[test]
    fn the_grade_records_which_concepts_were_missed() {
        let corpus = Corpus::builtin().unwrap();
        let snippet = corpus.find("002").unwrap();
        let annotations: Vec<Annotation> = snippet
            .blocks
            .iter()
            .enumerate()
            .map(|(i, _)| Annotation::new(i, "the ? propagates the error with an early return"))
            .collect();
        let grade = LocalReviewer.grade(snippet, &annotations).unwrap();
        let verdicts = grade.concept_verdicts();
        assert!(
            verdicts
                .iter()
                .any(|(c, v)| c == "rust.question_mark" && *v == Verdict::Correct)
        );
        assert!(
            verdicts
                .iter()
                .any(|(c, v)| c == "rust.error.dynamic" && *v != Verdict::Correct)
        );
    }
}
