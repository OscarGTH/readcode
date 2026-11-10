# readcode

Learn a language by annotating real code, then get graded on it.

`readcode` shows you a short piece of production-shaped code — sometimes block by block,
sometimes line by line — and asks you to explain each part in your own words. A reviewer
then marks every concept you were supposed to name: correct, partial, wrong, or unknown.
Concepts you missed come back sooner. Duolingo, aimed at *reading* code instead of
writing toy apps nobody needs.

```
┌ line 6 · rust.hashmap.entry, rust.deref.assign
 6 │         *counts.entry(word).or_insert(0) += 1;
Unpack this line: the `*`, the `entry`, and the `or_insert`.
> entry looks the key up and inserts a default 0 when it is missing, and the star
> dereferences the mutable reference so we can add one
> .
```

```
review · acp:opencode

  prompt 5 · line 6 ✓ correct
    rust.hashmap.entry                 ✓ correct
    rust.deref.assign                  ✓ correct
    You named the insert-on-missing behaviour and the deref behind the `*`.
```

## Install

macOS or Linux, via Homebrew. It builds from source, so it takes about a minute and
pulls in nothing you keep afterwards:

```sh
brew install OscarGTH/tap/readcode
```

Anywhere with a Rust toolchain:

```sh
cargo install --git https://github.com/OscarGTH/readcode
```

Or from a clone: `cargo build --release`, then run `target/release/readcode`.

The snippet corpus is compiled into the binary, so a single executable is the whole
install — no data files, no network at runtime unless you choose a model or agent.

## Quick start

Point it at a reviewer once (see [Choosing a reviewer](#choosing-a-reviewer)):

```sh
export READCODE_AGENT="opencode acp"   # or: export READCODE_API_KEY=sk-...
```

Then everyday:

```sh
readcode            # annotate the snippet that is most due
readcode status     # what you have mastered, what is due
readcode list       # the snippets available
```

Progress is saved between runs, so `readcode` is the same command every day.

## Using it

Answer each question in your own words. A line containing only `.` finishes the answer;
`:unknown` says you have no idea (that is a useful answer, not a failure); `:quit` stops
without saving anything.

```sh
readcode --snippet 002     # a specific snippet (id, or any unambiguous suffix)
readcode --lines           # force line-by-line; --blocks forces the other way
readcode check             # validate your own snippet files
readcode reset --yes       # wipe progress and start over
```

### Blocks or lines

Comprehension and attention to detail are different skills, so the app asks both:

- **block by block** — one question per semantic block: "what does `shout` take, and what
  does it give back?"
- **line by line** — one question per significant line, with closing braces skipped and
  the whole snippet kept on screen so a single line has its surroundings.

You do not choose: the first visit uses whatever the snippet is authored for, and the
next visit to the same snippet **alternates** to the other mode. Same code, different
question. `--lines` / `--blocks` override it for one sitting.

### Where progress lives

`$XDG_DATA_HOME/readcode/progress.json`, or `~/.local/share/readcode/progress.json`.
Override with `--data-dir <dir>` — handy for trying the app without touching your real
progress.

## Choosing a reviewer

Grading needs a model, so the first run needs one of two things set up:

| `--reviewer` | Needs | What you get |
|---|---|---|
| `acp` | an ACP agent on your machine | your own agent and subscription grade; no API key |
| `api` | one API key | a model grades over one HTTP call |

Auto-detection uses a configured agent first, then a key. With neither, `readcode`
stops and tells you which exports to set — `status`, `list` and `check` work either way.
`READCODE_REVIEWER=acp` makes the choice permanent.

**With an API key** (OpenAI- or Anthropic-compatible, including local servers):

```sh
export READCODE_API_KEY=sk-...
export READCODE_MODEL=gpt-4o-mini                     # optional
export READCODE_API_BASE=http://localhost:11434/v1    # optional
export READCODE_API_STYLE=openai                      # openai | anthropic
```

**With your own agent** (anything that speaks ACP):

```sh
export READCODE_AGENT="opencode acp"     # or "copilot --acp", an adapter, ...
readcode --reviewer acp
```

`READCODE_MODEL` works for both backends. If the agent opens on a free shared-queue
model, `readcode` switches to the first paid model it offers and tells you; your explicit
`READCODE_MODEL` always wins.

## Adding your own corpus

The built-in snippets are compiled into the binary, so plain `readcode` works anywhere.
To use your own, point it at a directory of TOML files:

```sh
readcode --corpus ~/my-snippets
readcode --corpus ~/my-snippets check      # validate before you study
```

Files are found recursively, so `~/my-snippets/rust/*.toml` and
`~/my-snippets/python/*.toml` both work. One snippet per file; the `id` must be unique.

### The smallest snippet that works

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

That is a complete snippet: the code, one block covering lines 1-4, and the two concepts you
have to name. `readcode check` accepts it, block mode asks "explain what this block does",
and a reviewer grades the answer.

### Sharpening it — all optional

Two fields make grading more specific. Add them where they earn it, skip them where they
do not.

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
  shown these, so it grades against your rubric instead of guessing at the code. One line,
  only for the concepts that need it.

### Asking about single lines

In line mode every significant line is asked on its own (blank lines and lone braces are
skipped). To give one line its own question and note:

```toml
  [[blocks.line]]
  at = 2
  ask = "Why is `mut` on the parameter, and who does it help?"
  concepts = ["rust.mut.param"]
  notes = { "rust.mut.param" = "`mut text` makes the binding writable inside this function only." }
```

- `at` is a single line inside the owning block. `ask`, `concepts` and `notes` all default to
  the block's, so `at` alone is a valid entry.
- A line without one still gets "What does line N do?", graded against the block's concepts.

### Field reference

Snippet:

- `id` — unique; `--snippet` matches it, or its title. Prefix by language: `rust-001`.
- `language`, `title` — shown in the header.
- `code` — the code, in a TOML literal string (`'''`). Line numbers are 1-based.
- `source`, `intro` — optional: provenance, and one line of framing.
- `asks` — optional: `"blocks"`, `"lines"` or `"auto"` (default: lines if the snippet has
  `blocks.line` entries, otherwise blocks).

Block:

- `lines = [start, end]` — 1-based, inclusive, in order, no gaps.
- `concepts` — what the learner must name. This is the unit of progress, not the snippet:
  mastery and scheduling are per concept, so reuse the same id wherever the idea appears.
  Namespace them (`rust.ownership.move`).
- `ask` — optional question for the block.
- `notes` — optional map of concept → ground truth for that concept.
- `line` — optional `[[blocks.line]]` entries, as above.

### Rules `readcode check` enforces

- every code line belongs to exactly one block, in order, no gaps or overlaps
- every block has at least one concept, and no empty ids
- a `notes` key has to be a concept declared on that block (or line) — this is what catches
  a typo in a concept id
- line questions sit inside their block, are not duplicated, and only use the block's concepts
- ids are unique across the corpus

When a range is wrong, `readcode check` prints the snippet with line numbers, so you never
count lines by hand to find the off-by-one.

### Tips for snippets worth studying

- **Use code you actually had to read.** A file from a repo you maintain beats a textbook
  example, because that is the skill you are training.
- **6–15 lines, 2–4 blocks.** Long enough to be real, short enough to finish in one
  sitting.
- **One idea per block.** If a block needs two unrelated concepts, it is two blocks.
- **Write the note as the correction.** If you cannot state what the learner should have
  said in one sentence, the concept is too vague to grade — and a concept with no note is
  graded from the code alone, which is looser.
- **Reuse concept ids.** `rust.question_mark` across five snippets teaches the scheduler
  that this is one skill; five one-off ids teach it nothing.
- **A generic line inherits the block's concepts.** If a line does not serve them (a lone
  `Ok(())`, a plain `}`), give it its own `blocks.line` entry, or it gets asked a question
  its block's concepts cannot answer.

## Evals

Grading can regress without any Rust test failing, so it has its own suite:

```sh
cargo run -- eval                 # deterministic mock: proves the harness itself
cargo run -- eval --target acp    # your own agent (~10 s per case)
cargo run -- eval --target api    # BYOK
```

Cases live in `evals/cases/` as one TOML file per behaviour — a misconception must never
be credited, "I don't know" must never be marked wrong, a correct idea in clumsy words
must be credited. Runs land in `evals/runs/<stamp>/` with a trace per case. See
[`evals/README.md`](evals/README.md).

## Contributing

```sh
cargo test                        # 84 tests, no network, no agent
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo run -- eval --target mock   # what CI also runs
```

CI (`.github/workflows/ci.yml`) runs those four steps on **every push, on any branch**,
on pull requests, and on demand; superseded runs on the same ref are cancelled so a
quick fixup does not spend the minutes twice. Model-backed evals stay manual, because
they spend tokens.

## Roadmap

- [x] Line-by-line annotation alongside block mode
- [ ] A ratatui TUI: syntax highlighting, arrow keys instead of prompts
- [ ] More corpora: Python, TypeScript, Go, and "read your own repo" mode
- [ ] A targeted follow-up question when a concept is unknown, not just a note
- [ ] An MCP server so your agent can look up weak concepts while you work

## Licence

MIT.
