# Evals

Grading can regress without a single Rust test failing: the parser is fine, the contract is
fine, and the model simply starts crediting something it used to catch. So grading has its own
suite, separate from `cargo test`.

```sh
cargo run -- eval                 # deterministic mock: proves the harness itself
cargo run -- eval --target acp    # your own agent (~10-20 s per case)
cargo run -- eval --target api    # BYOK
```

Cases live in `evals/cases/`, one TOML file per behaviour:

- a misconception must never be credited
- "I don't know" is `unknown`, never `wrong`
- a correct idea in clumsy words is `correct`
- an answer that names a type correctly is complete — `u32` already means unsigned
- a line prompt with no concepts is a reading check, not a place to attach the block's ideas

Each case carries the snippet, the mode, the annotations, and the verdicts or concepts that
must come back. `must_pass = true` means a failure is a release blocker; the rest are quality
cases that measure the reviewer rather than gate it.

Runs land in `evals/runs/<stamp>/` with a trace per case, so a regression can be read rather
than guessed at. The suite that runs in CI is the mock target, because it costs nothing;
model-backed runs stay manual because they spend tokens.

## Adding a case

```toml
id = "credits-a-named-type"
description = "Naming a type correctly is the answer."
snippet = "rust-009"
mode = "blocks"
tags = ["quality", "model-expected"]

[[annotations]]
prompt = 1
text = "it is an unsigned 32 bit integer"

[[expect]]
prompt = 1
concept = "rust.type.u32"
verdict = "correct"
```

Cases are compiled into the binary (`include_str!`), so a Homebrew install can run them
without a checkout. A test asserts every case on disk is embedded, and the count in
`src/eval/case.rs` is checked too — add the file, add the line, bump the number.
