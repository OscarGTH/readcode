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

| `--reviewer` | Needs | What you get |
|---|---|---|
| `local` | nothing | offline keyword matching; instant and free, but it cannot credit an idea phrased differently |
| `api` | one API key | a model grades against the same rubric, one HTTP call |
| `acp` | an ACP agent on your machine | your own agent and subscription do the grading; no API key |

Auto-detection prefers a configured agent, then an API key, then the offline reviewer.
`READCODE_REVIEWER=acp` sets it permanently.

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

### A minimal snippet

```toml
id = "rust-001"
language = "rust"
title = "Ownership: moving a String into a function"
source = "adapted from The Rust Book, ch. 4.1"
intro = "Say what each block does to the data."

code = '''
fn shout(mut text: String) -> String {
    text.push('!');
    text
}
'''

[[blocks]]
lines = [1, 4]
ask = "What does `shout` take, and what does it give back?"
concepts = ["rust.ownership.param", "rust.mut.param", "rust.ownership.return"]
  [[blocks.expects]]
  concept = "rust.ownership.param"
  keywords = ["takes ownership", "moved", "by value", "consumes"]
  note = "`String` is not `Copy`: passing it by value hands ownership to `shout`."

  [[blocks.expects]]
  concept = "rust.mut.param"
  keywords = ["mut", "mutable", "modify"]
  note = "`mut text` makes the binding writable inside this function only."

  [[blocks.expects]]
  concept = "rust.ownership.return"
  keywords = ["returns", "hands back", "tail"]
  note = "The tail expression returns `text`, moving ownership back to the caller."
```

### Fields

Snippet:

- `id` — unique, and what `--snippet` matches. Prefix by language: `rust-001`.
- `language`, `title` — shown in the header. `title` is also searched by `--snippet`.
- `code` — the snippet itself, in a TOML literal string (`'''`). Line numbers are 1-based.
- `source`, `intro` — optional. Where the code came from, and one line of framing.
- `asks` — optional: `"blocks"`, `"lines"`, or `"auto"` (default). `auto` asks lines if
  the snippet has line-level questions below, otherwise blocks.

Blocks — every line of `code` must belong to exactly one block:

- `lines = [start, end]` — 1-based, inclusive, in order, no gaps.
- `ask` — the question the learner sees for that block.
- `concepts = [...]` — what the learner must name. This is the unit of progress, not the
  snippet: mastery and scheduling are tracked per concept, so reuse the same id whenever
  the same idea appears in another snippet. Namespace them (`rust.ownership.move`).
- `expects` — one entry per concept: `keywords` for the offline reviewer and `note`, the
  correction shown when the concept is missed. Write the note as the thing you would say
  to a colleague; the model reviewers use it as the ground truth too.

Line-level questions — optional, and what makes a snippet good in line mode:

```toml
  [[blocks.line]]
  at = 2
  ask = "Why is `mut` on the parameter, and who does it help?"
  concepts = ["rust.mut.param"]
    [[blocks.line.expects]]
    concept = "rust.mut.param"
    keywords = ["mut", "mutable", "modify"]
    note = "`mut text` makes the binding writable inside this function only."
```

- `at` is a single line inside the owning block. `ask` and `concepts` default to the
  block's; `expects` defaults to the block's rubric filtered to those concepts.
- Lines without a `blocks.line` entry still get a generic "what does line N do?" in line
  mode, using the block's concepts. Blank lines and lone braces are never asked.

### Rules `readcode check` enforces

- every code line belongs to exactly one block, in order, no gaps or overlaps
- every block has an `ask` and at least one concept, and every concept has an `expects`
  entry with at least one keyword
- line questions sit inside their block, are not duplicated, and only use concepts the
  block declares
- ids are unique across the corpus

### Tips for snippets worth studying

- **Use code you actually had to read.** A file from a repo you maintain beats a textbook
  example, because that is the skill you are training.
- **6–15 lines, 2–4 blocks.** Long enough to be real, short enough to finish in one
  sitting.
- **One idea per block.** If a block needs two unrelated concepts, it is two blocks.
- **Keywords name ideas, not syntax.** `"takes ownership"`, `"early return"`,
  `"allocates on the heap"` — not `"String"` or `"?"`, which appear everywhere and credit
  nothing.
- **Write the note as the correction.** If you cannot state what the learner should have
  said in one sentence, the concept is too vague to grade.
- **Reuse concept ids.** `rust.question_mark` across five snippets teaches the scheduler
  that this is one skill; five one-off ids teach it nothing.
- **A generic line inherits the block's concepts.** If a line does not serve them (a lone
  `Ok(())`, a plain `}`), give it its own `blocks.line` entry, or it will be asked a
  question its block's rubric cannot answer.

## Evals

Grading can regress without any Rust test failing, so it has its own suite:

```sh
cargo run -- eval                     # offline rubric, deterministic
cargo run -- eval --target mock       # prove the harness itself
cargo run -- eval --target acp        # your own agent (~10 s per case)
cargo run -- eval --target api        # BYOK
```

Cases live in `evals/cases/` as one TOML file per behaviour — a misconception must never
be credited, "I don't know" must never be marked wrong, a correct idea in clumsy words
must be credited. Runs land in `evals/runs/<stamp>/` with a trace per case. See
[`evals/README.md`](evals/README.md).

## Contributing

```sh
cargo test                        # 89 tests, no network, no agent
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
