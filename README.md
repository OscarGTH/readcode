# readcode

Learn a language by annotating real code, then get graded on it.

`readcode` shows you a short piece of production-shaped code, block by block, and asks
you to explain what each block does. When you are done, a reviewer marks every concept
you were supposed to name: correct, partial, wrong, or unknown. Concepts you missed come
back sooner — the same idea as Duolingo, aimed at *reading* code instead of writing it.

The premise: you do not want to learn Rust by writing a toy app nobody needs. You want to
understand the code you are already reviewing, reading and inheriting.

```
block 1 · lines 1-3
What does `use` do, and why is the error type a `Box<dyn Error>` rather than a concrete error?
> use brings fs into scope so we can write fs:: ; Box<dyn Error> hides which concrete error type it is
> .
```

```
review · acp:opencode

  block 1 · lines 1-3 ~ partial
    rust.use.import                    ✓ correct
    rust.error.dynamic                 ~ partial
    You correctly explain that `use` brings `fs` into scope and that `Box<dyn Error>` hides the
    concrete type, but you don't say *why* it's erased — so `io::Error` and `ParseIntError` can
    share one return type.
    ! why both io::Error and ParseIntError can fit the same return type
```

## Install

```sh
cargo install --git https://github.com/OscarGTH/readcode
```

or from a clone: `cargo build --release` and run `target/release/readcode`.

## Use

```sh
readcode                     # annotate the snippet that is most due
readcode --snippet 002       # a specific one
readcode status              # mastery per concept, what is due
readcode list                # the corpus
readcode check               # validate your own snippets
readcode reset --yes         # start over
```

While answering: `.` on its own line finishes a block, `:unknown` says you have no idea,
`:quit` stops without saving.

Progress lives in `$XDG_DATA_HOME/readcode/progress.json` (override with `--data-dir`).

## Three reviewers

| Backend | Needs | Grading quality |
|---|---|---|
| `local` (default fallback) | nothing | keyword rubric: deterministic, offline, cannot credit phrasing it does not know |
| `api` | one API key | model grades against the rubric, one HTTP call |
| `acp` | an ACP agent on your machine | your own agent and subscription grade it; no API key at all |

Pick with `--reviewer local|api|acp`, or set `READCODE_REVIEWER`. Auto-detection prefers
a configured agent, then an API key, then the offline rubric.

```sh
# BYOK — OpenAI-compatible or Anthropic-compatible
export READCODE_API_KEY=sk-...          # or OPENAI_API_KEY / ANTHROPIC_API_KEY
export READCODE_MODEL=gpt-4o-mini       # optional
export READCODE_API_BASE=https://api.openai.com/v1   # optional, for any compatible endpoint
export READCODE_API_STYLE=openai        # openai | anthropic

# your own agent over ACP
export READCODE_AGENT="opencode acp"    # any ACP agent: `copilot --acp`, an adapter, ...
readcode --reviewer acp

# offline
readcode --offline
```

### Why ACP and not just an API key

Many people cannot get an API key (work laptops, no card) but already pay for a coding
agent. ACP is JSON-RPC over the agent's stdin/stdout, so `readcode` hosts the agent you
already have instead of selling you a second subscription. The client is hand-written
(~600 lines, no SDK) and tested against a fixture agent, so it cannot drift with a crate
version.

Three things that are easy to get wrong, and how they are handled here:

* **ACP has no system prompt.** `session/new` accepts `cwd` and `mcpServers`, nothing else.
  The grading contract is therefore prepended to the first prompt of the session
  (`review/contract.rs`), and the same constant is the only text that defines grading for
  all three backends.
* **There is no approval UI in a CLI.** Permission requests are answered with the agent's
  own reject option, and refusals are reported in the summary. An unanswered request
  hangs the run forever, which looks exactly like a slow agent.
* **`session/prompt` can resolve before the last `session/update`.** The turn is drained
  until quiet before the JSON is parsed, because the alternative passes locally and loses
  the closing text under load.
* **The agent's default model is not your model.** `opencode` opens on a free
  shared-queue model (`opencode/big-pickle`); on a session that did not pin a model,
  `readcode` moves to the first paid model the agent advertises and says so. `READCODE_MODEL`
  always wins.

## Writing your own snippets

`snippets/rust/*.toml` is the corpus, and it is compiled into the binary. Point
`--corpus <dir>` at your own directory to load TOML files instead.

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
concepts = ["rust.ownership.param", "rust.ownership.return"]
  [[blocks.expects]]
  concept = "rust.ownership.param"
  keywords = ["takes ownership", "moved", "by value", "consumes"]
  note = "`String` is not `Copy`: passing it by value hands ownership to `shout`."
```

* `lines` are 1-based and inclusive; every line must belong to exactly one block
  (`readcode check` enforces it).
* `concepts` are what the learner must name. They are the scheduling unit: mastery is
  tracked per concept, not per snippet.
* `expects` is the keyword rubric and the ground truth handed to model-backed reviewers,
  so all three backends grade the same thing. Write the `note` as the correction you would
  give a colleague — it is shown verbatim when the concept is missed.

## How the schedule works

Each concept carries a strength in `0..1` and a due time:

* **correct** — strength `+ (1 - strength) * 0.40`, interval `×2.2` (1 day, capped at 120)
* **partial** — `+15%` of the gap, interval `×1.3`
* **wrong** — strength `×0.45`, back in 6 hours
* **unknown** — strength `×0.75`, back inside the same sitting (15 minutes)

The next snippet is picked by summed urgency (overdue and weak wins) with a nudge towards
snippets you have not seen, skipping the one you just did when there is anywhere else to
go.

## Layout

```
src/
  snippet.rs   corpus format + validation (uncovered lines are a build error)
  srs.rs       mastery, scheduling, verdict folding
  store.rs     atomic progress file
  review/
    contract.rs  the one grading contract, shared by all backends
    local.rs     keyword rubric
    api.rs       BYOK HTTP (OpenAI / Anthropic shapes)
    acp.rs       hand-written ACP client + reviewer
  session.rs   one sitting: pick, annotate, grade, schedule
  ui.rs        terminal rendering
```

`cargo test` runs 60+ tests with no network and no agent: the ACP client is driven over a
socketpair fixture, and one test is ignored because it spawns a real agent and spends
tokens (`cargo test -- --ignored`).

## Roadmap

- [ ] A ratatui TUI: per-line annotation instead of per-block, syntax highlighting
- [ ] More corpora: Python, TypeScript, Go, and "read your own repo" mode
- [ ] Ask the reviewer for a *targeted* follow-up when a concept is unknown, not just a note
- [ ] An MCP server so the same agent can look up your weak concepts while you work
- [ ] Shareable corpora with a manifest and a difficulty signal

## Licence

MIT.
