# Building a corpus

`readcode` ships with 29 snippets compiled into the binary, so plain `readcode` works
anywhere. Your own live as TOML files in a directory:

```sh
readcode --corpus ~/my-snippets
readcode --corpus ~/my-snippets check     # validate before you study
```

Files are found recursively, so `~/my-snippets/rust/*.toml` and `~/my-snippets/python/*.toml`
both work. One snippet per file, and the `id` must be unique. The directory names are a
convention, not a rule — `language` inside the file is what counts, and a language the corpus
has never seen simply becomes a new entry in `readcode lang`.

## The smallest snippet that works

```toml
id = "rust-001"
language = "rust"
title = "Ownership: moving a String into a function"

code = '''
fn shout(mut text: String) -> String {
    text.push('!');
    text
}
'''

[[blocks]]
lines = [1, 4]
concepts = ["rust.ownership.param", "rust.ownership.return"]
```

That is complete: the code, one block covering lines 1–4, and the two concepts you have to
name. `check` accepts it, block mode asks "explain what this block does", and a reviewer
grades the answer.

## Sharpening it — all optional

Two fields make grading more specific. Add them where they earn it.

```toml
source = "adapted from The Rust Book, ch. 4.1"
intro = "Say what each block does to the data."

[[blocks]]
lines = [1, 4]
ask = "What does `shout` take, and what does it give back?"
concepts = ["rust.ownership.param", "rust.mut.param", "rust.ownership.return"]
notes = { "rust.ownership.param" = "`String` is not `Copy`: passing it by value hands ownership to `shout`.", "rust.mut.param" = "`mut text` makes the binding writable inside this function only.", "rust.ownership.return" = "The tail expression returns `text`, moving ownership back to the caller." }
```

- `ask` — the question the learner sees. Without it they get a generic one.
- `notes` — the ground truth per concept: what a correct answer has to say. The reviewer is
  shown these, so it grades against your rubric instead of guessing from the code.

## Asking about single lines

To give one line its own question and note:

```toml
  [[blocks.line]]
  at = 2
  ask = "Why is `mut` on the parameter, and who does it help?"
  concepts = ["rust.mut.param"]
  notes = { "rust.mut.param" = "`mut text` makes the binding writable inside this function only." }
```

- `at` is a single line inside the owning block. `ask`, `concepts` and `notes` all default to
  the block's, so `at` alone is a valid entry.
- **A line only inherits its block's concepts when the block is that one line.** In a
  multi-line block the idea may live on a different line, so a line without its own entry is
  asked as a reading check — "what does line N do?" — and grades no concept.
- **Line mode needs authored lines.** A snippet with no `blocks.line` entries at all is asked
  block by block even when you pass `--lines`, because asking every line of an un-authored
  block is how a correct answer about an `if` gets marked down for not mentioning the
  `append` on the next line.

## Field reference

Snippet:

- `id` — unique; `--snippet` matches it, or its title. Prefix by language: `rust-001`.
- `language` — `rust`, `go`, or any name of your own. It decides which selectors offer the
  snippet, which progress it counts towards, and which vocabulary the highlighter uses; a
  language the lexer does not know falls back to Rust's colours rather than none.
- `title` — shown in the header, and matched by `--snippet` alongside the id.
- `code` — the code, in a TOML literal string (`'''`). Line numbers are 1-based.
- `source`, `intro` — optional: where the code came from, and one line saying what it *is*.
- `asks` — optional: `"blocks"`, `"lines"` or `"auto"` (default: lines when the snippet has
  `blocks.line` entries, otherwise blocks).
- `docs = [{ title, url }]` — optional: reading for this snippet specifically, shown in the
  review above the per-concept links.
- `level` — optional: the rung this snippet is written for, 1 to 5.
- `levels = [from, to]` — optional: a band, for a simple idea shown in code that is harder to
  read than the idea. `level` and `levels` are mutually exclusive.

Block:

- `lines = [start, end]` — 1-based, inclusive, in order, no gaps.
- `concepts` — what the learner must name. This is the unit of progress, not the snippet:
  mastery and scheduling are per concept, so reuse the same id wherever the idea appears,
  and make each one a rule of this language rather than of programming in general.
- `ask` — optional question for the block.
- `notes` — optional map of concept → ground truth.
- `line` — optional `[[blocks.line]]` entries, as above.

## What `readcode check` enforces

- every code line belongs to exactly one block, in order, no gaps or overlaps
- every block has at least one concept, and no empty ids
- a `notes` key has to be a concept declared on that block or line — this catches a typo in a
  concept id
- line questions sit inside their block, are not duplicated, and only use the block's concepts
- ids are unique across the corpus

When a range is wrong, `check` prints the snippet with line numbers, so you never count lines
by hand to find the off-by-one.

## Drafting one with your reviewer

Choosing blocks and writing the ground truth are the slow parts, so let the model draft it and
keep the judging for yourself:

```sh
readcode draft src/parser.rs                    # print a draft
readcode draft src/parser.rs --span 41-58       # just that function
readcode draft src/parser.rs --level 3 --write  # save it under ./snippets/rust/
```

It reads the file, asks your reviewer for one snippet in the corpus format, then fixes the id,
the language and the level itself, and runs the draft through the same rules `check` enforces.
Expect a header with:

- `# fixed: block 2 extended from line 5 to 7 to cover the gap` — models think in ideas, not
  partitions, so they leave blank lines uncovered. That is unambiguous, so it is repaired and
  reported; an **overlap** is refused, because it means the reply is confused.
- `# check: block 1 asks more than one question — split it in two` — the failure that hurts
  grading most, flagged by counting interrogatives.
- Concept ids reused where they already exist, so progress carries across snippets.

It will not tell you whether the *questions* are good. Read the draft: the asks are the
product and the notes are what the grader judges against.

## Tips for snippets worth studying

- **Teach one idea in more than one place.** The scheduler revisits whatever is weakest, and
  being handed the same file a third time teaches the file, not the rule. Two or three
  snippets per important concept, with different code, is what makes repetition useful.
- **Ask about the language, not about programming.** "What does `%` do" and "what is a
  `while` loop" teach nothing about Rust or Go; "why may this borrow end before the next
  line" and "what does `&mut self` force on the caller" do.
- **Use code you actually had to read.** A file from a repo you maintain beats a textbook
  example, because that is the skill you are training.
- **6–15 lines, 2–4 blocks.** Long enough to be real, short enough to finish in one sitting.
- **Say which rung it is.** An unlevelled snippet is offered to everybody, which in practice
  means it lands on a beginner and stalls them.
- **One idea per block.** If a block needs two unrelated concepts, it is two blocks.
- **Write the note as the correction.** If you cannot state what the learner should have said
  in one sentence, the concept is too vague to grade.
- **Reuse concept ids.** `rust.question_mark` across five snippets teaches the scheduler that
  this is one skill; five one-off ids teach it nothing.
- **One question per prompt.** If the ask needs "and" to join two unrelated ideas, it is two
  blocks, or one block plus a `blocks.line` entry — a bundled question gets one answer
  covering two things and is graded twice against two rubrics that each see half of it.
- **Keep the question short and in plain words.** "Where does the value the caller gets come
  from?" has to be parsed before it can be answered.
