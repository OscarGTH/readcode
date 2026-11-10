# readcode evals

A regression suite for the thing that can silently rot: **grading**. The snippet
corpus and the scheduler are covered by `cargo test`; whether a reviewer still
credits a correct idea, refuses to credit a misconception, and treats "I don't
know" as `unknown` is behaviour no unit test pins down — a prompt edit or a model
swap can change it without touching a line of Rust.

```sh
cargo run -- eval                      # offline rubric, deterministic, free
cargo run -- eval --target mock         # harness self-check
cargo run -- eval --target acp          # your own agent, ~10 s per case
cargo run -- eval --target api --model gpt-4o-mini
cargo run -- eval --filter line-level   # one case or tag
```

Exit code is non-zero when a `must_pass` case fails or the score is below
`--min-score`, so it works as a CI gate.

## What a case is

One TOML file per case in `evals/cases/`, named after the behaviour, not the
snippet. A case carries the annotations to feed the reviewer and what the
reviewer must say about them.

```toml
id = "never-credits-a-misconception"
description = "A confidently false annotation must not be graded correct."
snippet = "rust-001"          # corpus id, or an unambiguous suffix
mode = "blocks"               # blocks | lines | leave out for the corpus default
must_pass = true              # a failure here is a broken promise, not a noisy model
tags = ["safety"]

[[annotations]]
prompt = 3
text = "the second call makes a shallow copy because String implements Copy"

[[expect]]
prompt = 3
concept = "rust.ownership.move"   # omit for the prompt's overall verdict
not = ["correct"]                 # or: verdict = "wrong"
```

Assertions are exact (`verdict = "..."`) or prohibitive (`not = ["correct"]`).
Prohibitions are what catch the expensive failure: an annotation that sounds
plausible, states something false, and gets a green tick.

## Cases worth having

The suite is small on purpose — one case per bug that would otherwise ship:

- `credits-a-paraphrase` — a correct idea in the learner's own words is correct.
- `never-credits-a-misconception` — confident and wrong must not pass.
- `says-i-dont-know` / `blank-is-not-wrong` — not knowing is `unknown`, never
  `wrong`; the scheduler must resurface it, not punish it.
- `off-topic-is-wrong` — explaining something else collects nothing.
- `line-level-entry-api` — line mode grades a single line on its own rubric.

Add a case when a reviewer gets something wrong in real use, not to raise the
count. Write it from the transcript that failed.

## Targets

- `mock` — answers from the case's own expectations. It proves the harness: if
  this is red, the runner is broken, not the model. The unit test
  `a_contrarian_mock_fails_every_case` flips every mock answer and requires the
  whole suite to go red, so "all green" cannot be a scoring bug.
- `local` — the offline keyword rubric. Deterministic, so it is a real
  regression guard; `cargo test` asserts it stays above its baseline.
- `api` / `acp` — a real reviewer. Costs tokens and minutes: run it when you
  change the contract, the rubric, or the model.

## Baselines

Measured on this corpus, 2026-11-10, one trial each:

- `local`: **0.83** — 5/6 cases, 10/15 assertions. It fails `credits-a-paraphrase`
  by design: keyword matching cannot credit wording it does not know. Everything
  in `must_pass` passes.
- `acp` (opencode 1.19.5, `deepseek/deepseek-v4-pro`): **1.00** — 6/6 cases,
  15/15 assertions, 9–12 s per case. Each case spawns one agent process, so cost
  is per case; batch into fewer cases before making the suite much bigger.

Quote the model with the number, or the number means nothing. Re-run both after
a contract change and compare case by case, not by the average.

## Runs

`evals/runs/<utc-stamp>/` (gitignored) holds:

- `manifest.json` — target, model-free metadata, score, must-pass failures
- `results.jsonl` — one line per case per trial
- `summary.md` — the table
- `traces/<case>-t<trial>.json` — the prompt, the annotation, every verdict and
  the reviewer's own feedback

Read the trace before the assertion message: the assertion says what disagreed,
the trace says what the reviewer actually saw.
