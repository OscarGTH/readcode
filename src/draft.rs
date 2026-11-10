//! Drafting a corpus entry from real code.
//!
//! The point is to get from "this function is worth studying" to a snippet that
//! `check` accepts, without hand-writing TOML. The model proposes the blocks, the
//! questions, the concepts and the notes; this module keeps it honest — it fixes
//! the id and language, validates against the same rules `check` enforces, and
//! hands back TOML that is ready to read and edit rather than to trust.

use anyhow::{Context, Result, bail};

use crate::snippet::{LEVELS, MAX_LEVEL, Snippet};

/// What the reviewer is told to produce.
pub fn prompt(
    code: &str,
    language: &str,
    source: Option<&str>,
    level: u8,
    known_concepts: &[String],
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Write one corpus entry for readcode: a short piece of {language} that a learner explains, block by block, in their own words.\n\n"
    ));
    out.push_str("THE CODE:\n");
    for (index, line) in code.lines().enumerate() {
        out.push_str(&format!("{:>3} | {}\n", index + 1, line));
    }
    if let Some(source) = source {
        out.push_str(&format!("\nIt comes from: {source}\n"));
    }

    out.push_str(&format!(
        "\nRULES:\n\
- Every line must belong to exactly one block, in order, with no gaps and no overlaps. Blank lines belong to the block they follow.\n\
- 2 to 4 blocks. If the code is longer than 15 lines, cover it with 3 to 5 blocks rather than dropping lines.\n\
- ONE question per block. If a block holds two unrelated ideas, split it into two blocks. Never join two questions with \"and\".\n\
- 1 or 2 concepts per block, from the list below where one fits. Every concept id must be lowercase and dotted.\n\
- `notes` is the ground truth: one sentence per concept saying what a correct answer has to mention. Prefer the mechanism over the syntax.\n\
- `ask` is a question about the code. `intro` describes what the code IS, in one line, and never instructs the learner.\n\
- level = {level} ({})\n\n",
        LEVELS[level as usize - 1]
    ));

    if !known_concepts.is_empty() {
        out.push_str("CONCEPT IDS ALREADY IN THE CORPUS (reuse one when it means the same thing; invent a new one only for a genuinely new idea):\n");
        for concept in known_concepts {
            out.push_str(&format!("  {concept}\n"));
        }
        out.push('\n');
    }

    out.push_str(
        "SHAPE (change the content, keep the shape):\n\
title = \"...\"\n\
intro = \"...\"\n\
asks = \"blocks\"\n\
code = '''\n\
<the code, verbatim, no line numbers>\n\
'''\n\n\
[[blocks]]\n\
lines = [1, 3]\n\
ask = \"...\"\n\
concepts = [\"lang.something\"]\n\
notes = { \"lang.something\" = \"...\" }\n\n\
Reply with the TOML only — no prose, no code fences, no `id` and no `language`; those are added for you.\n",
    );
    out
}

/// The reply, cleaned up: fences and prose stripped, id and language set, level
/// pinned, and validated against the same rules `readcode check` uses.
pub fn from_reply(reply: &str, id: &str, language: &str, level: Option<u8>) -> Result<String> {
    let body = extract_toml(reply)?;
    let mut value: toml::Value = toml::from_str(&body)
        .with_context(|| format!("the reviewer's TOML did not parse:\n{}\n", trimmed(&body)))?;

    let table = value
        .as_table_mut()
        .context("the reviewer did not reply with a TOML table")?;
    // Ours to set, not theirs.
    table.insert("id".to_string(), toml::Value::String(id.to_string()));
    table.insert(
        "language".to_string(),
        toml::Value::String(language.to_string()),
    );
    table.remove("levels");
    if let Some(level) = level {
        table.insert("level".to_string(), toml::Value::Integer(level.into()));
    }
    if !table.contains_key("asks") {
        table.insert("asks".to_string(), toml::Value::String("blocks".into()));
    }

    let line_count = value
        .get("code")
        .and_then(|code| code.as_str())
        .map(|code| code.lines().count())
        .unwrap_or(0);
    let repairs = repair(&mut value, line_count)?;

    let snippet: Snippet = value
        .clone()
        .try_into()
        .context("the reviewer's TOML is not a snippet")?;
    snippet
        .validate()
        .context("the draft does not pass the corpus rules")?;

    let text = toml::to_string_pretty(&value).context("could not re-serialise the draft")?;
    let mut header =
        format!("# draft {id} — review the asks, the notes and the level before committing\n");
    for note in &repairs {
        header.push_str(&format!("# fixed: {note}\n"));
    }
    for warning in &ask_warnings(&value) {
        header.push_str(&format!("# check: {warning}\n"));
    }
    Ok(format!("{header}{text}"))
}

/// The one structural mistake models make reliably: a block boundary that leaves
/// a blank line or a closing brace uncovered, because they think of blocks as
/// ideas rather than as a partition. Extending the previous block over the gap is
/// always what the author meant, so do it and say so, instead of throwing the
/// draft away. Overlaps and reversed ranges are left alone: those mean the reply
/// is confused and should be read by a person.
pub fn repair(value: &mut toml::Value, line_count: usize) -> Result<Vec<String>> {
    let Some(blocks) = value
        .get_mut("blocks")
        .and_then(|blocks| blocks.as_array_mut())
    else {
        return Ok(Vec::new());
    };
    if blocks.is_empty() || line_count == 0 {
        return Ok(Vec::new());
    }

    let span_of = |block: &toml::Value| -> Option<(usize, usize)> {
        let lines = block.get("lines")?.as_array()?;
        Some((
            lines.first()?.as_integer()? as usize,
            lines.get(1)?.as_integer()? as usize,
        ))
    };
    let mut spans: Vec<(usize, usize)> = Vec::with_capacity(blocks.len());
    for block in blocks.iter() {
        let span =
            span_of(block).with_context(|| "a block has no `lines = [start, end]`".to_string())?;
        spans.push(span);
    }

    let mut repairs = Vec::new();
    for index in 0..spans.len() {
        let (start, mut end) = spans[index];
        if start == 0 || start > end {
            bail!("block {} has a reversed range {start}-{end}", index + 1);
        }
        if let Some((next_start, _)) = spans.get(index + 1).copied() {
            if next_start <= end {
                bail!(
                    "blocks {} and {} overlap (lines {start}-{end} and from {next_start})",
                    index + 1,
                    index + 2
                );
            }
            if next_start > end + 1 {
                repairs.push(format!(
                    "block {} extended from line {end} to {} to cover the gap",
                    index + 1,
                    next_start - 1
                ));
                end = next_start - 1;
            }
        } else if end < line_count {
            repairs.push(format!(
                "block {} extended to the last line ({line_count})",
                index + 1
            ));
            end = line_count;
        }
        spans[index] = (start, end);
    }
    if spans[0].0 > 1 {
        repairs.push(format!(
            "block 1 starts at line 1 instead of {}",
            spans[0].0
        ));
        spans[0].0 = 1;
    }

    for (block, (start, end)) in blocks.iter_mut().zip(spans) {
        if let Some(table) = block.as_table_mut() {
            table.insert(
                "lines".to_string(),
                toml::Value::Array(vec![
                    toml::Value::Integer(start as i64),
                    toml::Value::Integer(end as i64),
                ]),
            );
        }
    }
    Ok(repairs)
}

/// Cheap reading of the asks. A question that holds two interrogatives is two
/// questions joined by "and" — which is what makes one answer cover two things
/// and land on whichever line happened to open the block. Say so in the header
/// rather than accepting it quietly.
pub fn ask_warnings(value: &toml::Value) -> Vec<String> {
    const INTERROGATIVES: [&str; 7] = ["what", "why", "how", "when", "where", "which", "who"];
    let mut out = Vec::new();
    let Some(blocks) = value.get("blocks").and_then(|blocks| blocks.as_array()) else {
        return out;
    };
    for (index, block) in blocks.iter().enumerate() {
        let Some(ask) = block.get("ask").and_then(|ask| ask.as_str()) else {
            continue;
        };
        let words: Vec<String> = ask
            .to_lowercase()
            .split(|char_: char| !char_.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_string)
            .collect();
        // Counted, not deduplicated: "what does X mean, and what does Y do?" is
        // two questions even though it uses one interrogative word.
        let interrogatives = words
            .iter()
            .filter(|token| INTERROGATIVES.contains(&token.as_str()))
            .count();
        if ask.matches('?').count() > 1 || interrogatives > 1 {
            out.push(format!(
                "block {} asks more than one question — split it in two: {ask}",
                index + 1
            ));
        }
    }
    out
}

/// Models fence TOML and add a sentence before it. Take the block.
pub fn extract_toml(reply: &str) -> Result<String> {
    let text = reply.trim();
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        let after = after.strip_prefix("toml").unwrap_or(after);
        let after = after.strip_prefix('\n').unwrap_or(after);
        if let Some(end) = after.find("```") {
            return Ok(after[..end].trim().to_string());
        }
    }
    if text.contains('=') {
        // Prose before the first key is common; cut to the first line that looks
        // like the start of the document.
        let start = text
            .lines()
            .position(|line| {
                let line = line.trim_start();
                line.starts_with("title")
                    || line.starts_with("intro")
                    || line.starts_with("code")
                    || line.starts_with("level")
                    || line.starts_with('#')
            })
            .unwrap_or(0);
        return Ok(text.lines().skip(start).collect::<Vec<_>>().join("\n"));
    }
    bail!("the reviewer did not reply with TOML")
}

/// The next free id for a language: `rust-001`, `rust-002`, …
pub fn next_id(existing: impl IntoIterator<Item = String>, language: &str) -> String {
    let prefix = format!("{language}-");
    let highest = existing
        .into_iter()
        .filter_map(|id| id.strip_prefix(&prefix)?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("{prefix}{:03}", highest + 1)
}

/// A filename-safe slug from the title.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for char_ in title.chars() {
        if char_.is_alphanumeric() {
            out.extend(char_.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "snippet".to_string()
    } else {
        trimmed.chars().take(40).collect()
    }
}

/// `rs` → `rust`, so `readcode draft foo.rs` needs no flag.
pub fn language_for(path: &std::path::Path, corpus_languages: &[String]) -> String {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_lowercase();
    let named = match extension.as_str() {
        "rs" => "rust",
        "py" => "python",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "go" => "go",
        "kt" => "kotlin",
        "swift" => "swift",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_string();
    }
    if corpus_languages.iter().any(|one| one == &extension) {
        return extension;
    }
    if extension.is_empty() {
        "rust".to_string()
    } else {
        extension
    }
}

fn trimmed(text: &str) -> String {
    let limit = 400;
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let cut: String = text.chars().take(limit).collect();
    format!("{cut}…")
}

/// The levels a draft can be authored at, for the CLI's error message.
pub fn level_help() -> String {
    format!("levels run 1 to {MAX_LEVEL}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "fn double(n: i32) -> i32 {\n    n * 2\n}";

    #[test]
    fn the_prompt_carries_the_code_the_level_and_the_known_concepts() {
        let text = prompt(
            CODE,
            "rust",
            Some("src/lib.rs"),
            2,
            &[
                "rust.fn.return".to_string(),
                "rust.operator.arith".to_string(),
            ],
        );
        assert!(text.contains("  3 | }"), "{text}");
        assert!(text.contains("level = 2"), "{text}");
        assert!(text.contains("small functions"), "{text}");
        assert!(text.contains("rust.fn.return"), "{text}");
        assert!(text.contains("src/lib.rs"), "{text}");
        assert!(text.contains("ONE question per block"), "{text}");
    }

    #[test]
    fn a_fenced_reply_is_unwrapped_and_given_an_id() {
        let reply = format!(
            "```toml\ntitle = \"Doubling\"\nintro = \"A function that doubles.\"\ncode = '''\n{CODE}\n'''\n\n[[blocks]]\nlines = [1, 3]\nask = \"What does it return?\"\nconcepts = [\"rust.fn.return\"]\nnotes = {{ \"rust.fn.return\" = \"Twice the argument.\" }}\n```"
        );
        let toml = from_reply(&reply, "rust-010", "rust", Some(2)).unwrap();
        assert!(toml.contains("id = \"rust-010\""), "{toml}");
        assert!(toml.contains("language = \"rust\""), "{toml}");
        assert!(toml.contains("level = 2"), "{toml}");
        assert!(toml.contains("draft rust-010"), "{toml}");
        // And it round-trips as a snippet.
        Snippet::parse(&toml).unwrap().validate().unwrap();
    }

    #[test]
    fn the_reviewer_cannot_choose_the_id() {
        let reply = "id = \"rust-999\"\nlanguage = \"go\"\ntitle = \"t\"\ncode = '''\nlet x = 1;\n'''\n\n[[blocks]]\nlines = [1, 1]\nconcepts = [\"go.binding\"]\n";
        let toml = from_reply(reply, "rust-011", "rust", None).unwrap();
        let parsed = Snippet::parse(&toml).unwrap();
        assert_eq!(parsed.id, "rust-011");
        assert_eq!(parsed.language, "rust");
        assert!(!toml.contains("rust-999"), "{toml}");
    }

    #[test]
    fn a_gap_between_blocks_is_closed_and_reported() {
        // Block 1 covers line 1, block 2 starts at line 3: line 2 is orphaned.
        let reply = "title = \"t\"\ncode = '''\na\nb\nc\n'''\n\n[[blocks]]\nlines = [1, 1]\nconcepts = [\"x.y\"]\n\n[[blocks]]\nlines = [3, 3]\nconcepts = [\"x.z\"]\n";
        let toml = from_reply(reply, "x-001", "rust", None).unwrap();
        assert!(toml.contains("fixed: block 1 extended"), "{toml}");
        let parsed = Snippet::parse(&toml).unwrap();
        parsed.validate().unwrap();
        assert_eq!(parsed.blocks[0].lines, vec![1, 2]);
    }

    #[test]
    fn an_overlap_is_refused_rather_than_guessed_at() {
        let reply = "title = \"t\"\ncode = '''\na\nb\n'''\n\n[[blocks]]\nlines = [1, 2]\nconcepts = [\"x.y\"]\n\n[[blocks]]\nlines = [2, 2]\nconcepts = [\"x.z\"]\n";
        let error = from_reply(reply, "x-001", "rust", None).unwrap_err();
        assert!(format!("{error:#}").contains("overlap"), "{error:#}");
    }

    #[test]
    fn a_trailing_block_is_extended_to_the_last_line() {
        let reply = "title = \"t\"\ncode = '''\na\nb\nc\n'''\n\n[[blocks]]\nlines = [1, 1]\nconcepts = [\"x.y\"]\n\n[[blocks]]\nlines = [2, 2]\nconcepts = [\"x.z\"]\n";
        let toml = from_reply(reply, "x-001", "rust", None).unwrap();
        assert!(toml.contains("extended to the last line"), "{toml}");
        let parsed = Snippet::parse(&toml).unwrap();
        parsed.validate().unwrap();
    }

    #[test]
    fn prose_before_the_document_is_dropped() {
        let reply = "Here is the snippet you asked for:\n\ntitle = \"t\"\ncode = '''\na\n'''\n\n[[blocks]]\nlines = [1, 1]\nconcepts = [\"x.y\"]\n";
        let toml = from_reply(reply, "x-001", "rust", None).unwrap();
        assert!(toml.contains("title = \"t\""), "{toml}");
        assert!(!toml.contains("Here is"), "{toml}");
    }

    #[test]
    fn ids_continue_from_the_highest() {
        let ids = [
            "rust-001".to_string(),
            "rust-007".to_string(),
            "go-003".to_string(),
        ];
        assert_eq!(next_id(ids.to_vec(), "rust"), "rust-008");
        assert_eq!(next_id(ids.to_vec(), "go"), "go-004");
        assert_eq!(next_id(ids.to_vec(), "python"), "python-001");
    }

    #[test]
    fn a_two_part_question_is_flagged_for_the_author() {
        let reply = "title = \"t\"\ncode = '''\na\nb\n'''\n\n[[blocks]]\nlines = [1, 1]\nask = \"What does the parameter mean, and what does the function return?\"\nconcepts = [\"x.y\"]\n\n[[blocks]]\nlines = [2, 2]\nask = \"Why is this borrowed?\"\nconcepts = [\"x.z\"]\n";
        let toml = from_reply(reply, "x-001", "rust", None).unwrap();
        assert!(
            toml.contains("# check: block 1 asks more than one question"),
            "{toml}"
        );
        assert!(!toml.contains("# check: block 2"), "{toml}");
    }

    #[test]
    fn slugs_are_file_names() {
        assert_eq!(
            slug("Ownership: moving a String!"),
            "ownership-moving-a-string"
        );
        assert_eq!(slug("  ...  "), "snippet");
        assert_eq!(slug("UPPER"), "upper");
    }

    #[test]
    fn the_language_comes_from_the_extension() {
        use std::path::Path;
        assert_eq!(language_for(Path::new("a/b.rs"), &[]), "rust");
        assert_eq!(language_for(Path::new("x.py"), &[]), "python");
        assert_eq!(language_for(Path::new("x.unknown"), &[]), "unknown");
        assert_eq!(language_for(Path::new("x"), &[]), "rust");
    }
}
