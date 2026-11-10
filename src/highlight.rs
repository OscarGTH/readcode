//! Rust syntax highlighting for the code view.
//!
//! Hand-rolled on purpose: a lexer that colours keywords, types, strings, numbers,
//! comments and macros is ~200 lines, while pulling in a fully general highlighter
//! would add seconds to every build on a small machine. It is a *display* pass —
//! nothing here is used for grading.

use crate::ui::Style;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Normal,
    BlockComment,
}

const TYPES: &[&str] = &[
    "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32",
    "f64", "bool", "char", "str",
];

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use",
    "where", "while", "yield",
];

/// Highlights a run of source lines. Block comments are threaded across lines, so
/// a `/* … */` that spans them is dimmed as one comment.
pub fn highlight(lines: &[&str], style: Style) -> Vec<String> {
    let mut state = State::Normal;
    lines
        .iter()
        .map(|line| highlight_line(line, style, &mut state))
        .collect()
}

fn highlight_line(line: &str, style: Style, state: &mut State) -> String {
    let mut out = String::new();
    let mut index = 0usize;

    while index < line.len() {
        let rest = &line[index..];

        if *state == State::BlockComment {
            match rest.find("*/") {
                Some(offset) => {
                    out.push_str(&style.dim(&rest[..offset + 2]));
                    index += offset + 2;
                    *state = State::Normal;
                }
                None => {
                    out.push_str(&style.dim(rest));
                    index = line.len();
                }
            }
            continue;
        }

        if rest.starts_with("//") {
            out.push_str(&style.dim(rest));
            break;
        }
        if rest.starts_with("/*") {
            *state = State::BlockComment;
            continue;
        }

        // Attributes: #[derive(Debug)] and #![allow(..)]
        if rest.starts_with('#') {
            if let Some(close) = rest.find(']') {
                out.push_str(&style.magenta(&rest[..close + 1]));
                index += close + 1;
                continue;
            }
        }

        // Raw strings first: r"…", r#"…"#, br#"…"#
        if let Some(len) = raw_string_len(rest) {
            out.push_str(&style.green(&rest[..len]));
            index += len;
            continue;
        }

        // Strings and chars.
        if rest.starts_with('"') {
            let len = quoted_len(rest, '"');
            out.push_str(&style.green(&rest[..len]));
            index += len;
            continue;
        }
        if rest.starts_with('\'') {
            if let Some(len) = char_literal_len(rest) {
                out.push_str(&style.green(&rest[..len]));
                index += len;
                continue;
            }
            // Otherwise a lifetime: 'static, 'a
            let body = rest.strip_prefix('\'').unwrap_or(rest);
            let offset = body
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(body.len());
            let len = 1 + offset;
            out.push_str(&style.yellow(&rest[..len]));
            index += len;
            continue;
        }

        // Numbers, including suffixes and floats: 0, 1_000, 3.5f32, 0xFF
        let first = rest.chars().next().unwrap_or(' ');
        if first.is_ascii_digit() {
            let len = rest
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
                .unwrap_or(rest.len());
            out.push_str(&style.yellow(&rest[..len]));
            index += len;
            continue;
        }

        if first.is_alphanumeric() || first == '_' {
            let len = rest
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            let word = &rest[..len];
            let after = &rest[len..];
            out.push_str(&classify(word, after, style));
            index += len;
            continue;
        }

        let mut chars = rest.chars();
        if let Some(char_) = chars.next() {
            out.push(char_);
            index += char_.len_utf8();
        } else {
            break;
        }
    }

    out
}

fn classify(word: &str, after: &str, style: Style) -> String {
    if KEYWORDS.contains(&word) || word == "Self" {
        // `self` in a receiver is a value, but colouring it like a keyword reads
        // correctly either way.
        return style.magenta(word);
    }
    if after.starts_with('!') {
        return style.magenta(word); // macro: println!, vec!, matches!
    }
    if TYPES.contains(&word) || word.chars().next().is_some_and(char::is_uppercase) {
        return style.cyan(word); // primitive, type, enum variant, trait
    }
    if after.starts_with('(') {
        return style.blue(word); // function or method call
    }
    if word == "mut" || word == "dyn" {
        return style.magenta(word);
    }
    word.to_string()
}

/// `"…"` with escapes; returns the byte length including both quotes.
fn quoted_len(text: &str, quote: char) -> usize {
    let mut chars = text.char_indices();
    chars.next(); // opening quote
    while let Some((index, char_)) = chars.next() {
        match char_ {
            '\\' => {
                chars.next();
            }
            c if c == quote => return index + c.len_utf8(),
            _ => {}
        }
    }
    text.len() // unterminated: colour the rest
}

/// `'a'`, `'\n'`, `'\u{1F600}'` — but not `'static`.
fn char_literal_len(text: &str) -> Option<usize> {
    let mut chars = text.char_indices();
    chars.next()?; // opening tick
    let (_, first) = chars.next()?;
    if first == '\\' {
        let (_, escaped) = chars.next()?;
        let width = match escaped {
            'u' => text.find('}').map(|offset| offset + 1)?,
            _ => text[2..].char_indices().next().map(|(i, _)| i + 2)?,
        };
        return text[width..].starts_with('\'').then_some(width + 1);
    }
    let end = first.len_utf8() + 1;
    text[end..].starts_with('\'').then_some(end + 1)
}

/// `r"…"`, `r#"…"#`, `br##"…"##`.
fn raw_string_len(text: &str) -> Option<usize> {
    let mut rest = text;
    let mut hashes = 0usize;
    if let Some(stripped) = rest.strip_prefix('b') {
        rest = stripped;
    }
    if !rest.starts_with('r') {
        return None;
    }
    rest = &rest[1..];
    let consumed = text.len() - rest.len();
    while rest.starts_with('#') {
        hashes += 1;
        rest = &rest[1..];
    }
    if !rest.starts_with('"') {
        return None;
    }
    let body_start = consumed + hashes + 1;
    let terminator = format!("\"{}", "#".repeat(hashes));
    match text[body_start..].find(&terminator) {
        Some(offset) => Some(body_start + offset + terminator.len()),
        None => Some(text.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paint(line: &str) -> String {
        highlight(&[line], Style::new(true)).remove(0)
    }

    fn plain(line: &str) -> String {
        highlight(&[line], Style::new(false)).remove(0)
    }

    #[test]
    fn keywords_types_and_calls_get_their_own_colours() {
        let painted = paint("fn hit(&mut self) -> bool { Counter::new() }");
        assert!(painted.contains("\x1b[35mfn\x1b[0m"), "{painted}");
        assert!(painted.contains("\x1b[35mmut\x1b[0m"), "{painted}");
        assert!(painted.contains("\x1b[36mCounter\x1b[0m"), "{painted}");
    }

    #[test]
    fn strings_comments_and_numbers_are_coloured() {
        let painted = paint(r#"let n = 42; let s = "a\"b"; // note"#);
        assert!(painted.contains("\x1b[33m42\x1b[0m"), "{painted}");
        assert!(painted.contains("\x1b[32m\"a\\\"b\"\x1b[0m"), "{painted}");
        assert!(painted.contains("\x1b[2m// note\x1b[0m"), "{painted}");
    }

    #[test]
    fn macros_and_attributes_are_coloured() {
        let painted = paint(r#"#[derive(Debug)] struct Log; println!("{}", 1);"#);
        assert!(
            painted.contains("\x1b[35m#[derive(Debug)]\x1b[0m"),
            "{painted}"
        );
        assert!(painted.contains("\x1b[35mprintln\x1b[0m"), "{painted}");
    }

    #[test]
    fn a_lifetime_is_not_a_char_literal() {
        let lifetime = paint("struct LogLine<'a> { level: &'static str }");
        assert!(lifetime.contains("\x1b[33m'a\x1b[0m"), "{lifetime}");
        assert!(lifetime.contains("\x1b[33m'static\x1b[0m"), "{lifetime}");

        let char_ = paint("text.push('!');");
        assert!(char_.contains("\x1b[32m'!'\x1b[0m"), "{char_}");
    }

    #[test]
    fn a_block_comment_spanning_lines_stays_a_comment() {
        let lines = highlight(
            &["let a = 1; /* start", "still", "end */ let b = 2;"],
            Style::new(true),
        );
        assert!(lines[1].contains("\x1b[2mstill\x1b[0m"), "{}", lines[1]);
        assert!(lines[2].starts_with("\x1b[2mend */\x1b[0m"), "{}", lines[2]);
    }

    #[test]
    fn raw_strings_keep_their_hashes() {
        let painted = paint(r##"let q = r#"say "hi""#;"##);
        assert!(
            painted.contains("\x1b[32mr#\"say \"hi\"\"#\x1b[0m"),
            "{painted}"
        );
    }

    #[test]
    fn colour_off_returns_the_line_unchanged() {
        for line in [
            "fn main() { let x = 1; }",
            r#"println!("{x}");"#,
            "#[derive(Default)]",
            "// just a comment",
            "let c = 'a';",
        ] {
            assert_eq!(plain(line), line);
        }
    }

    #[test]
    fn an_unterminated_string_does_not_eat_the_next_line() {
        let lines = highlight(
            &[r#"let s = "unterminated"#, "let n = 1;"],
            Style::new(true),
        );
        assert!(lines[1].contains("\x1b[33m1\x1b[0m"), "{}", lines[1]);
    }
}
