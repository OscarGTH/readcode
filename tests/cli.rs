//! The command-line wiring, run as a real binary.
//!
//! The per-language filtering lives in library functions with their own tests, but
//! three of the four ways to choose a language are in `main`: `--language`, the
//! first-run question, and `readcode lang`. A library test cannot see whether
//! `status` honoured the flag it was given — this is the test that can.

use std::path::Path;
use std::process::Command;

fn readcode(data_dir: &Path, args: &[&str]) -> (String, String, bool) {
    let output = Command::new(env!("CARGO_BIN_EXE_readcode"))
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("running readcode");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

#[test]
fn the_corpus_ships_more_than_one_language() {
    let dir = tempfile::tempdir().unwrap();
    let (out, err, ok) = readcode(dir.path(), &["lang"]);
    assert!(ok, "{err}");
    assert!(out.contains("rust"), "{out}");
    assert!(out.contains("go"), "{out}");
}

#[test]
fn the_language_flag_decides_what_status_counts() {
    let dir = tempfile::tempdir().unwrap();
    // Nothing is stored yet, so the flag is the only thing choosing.
    let (go, err, ok) = readcode(dir.path(), &["--language", "go", "status"]);
    assert!(ok, "{err}");
    assert!(go.contains("go · "), "{go}");
    assert!(!go.contains("rust · "), "{go}");

    // A run with `status` must not remember it: the flag is for this run.
    let (after, err, ok) = readcode(dir.path(), &["status"]);
    assert!(ok, "{err}");
    assert!(after.contains("rust · "), "{after}");
    let saved = std::fs::read_to_string(dir.path().join("progress.json")).unwrap_or_default();
    assert!(
        !saved.contains("\"language\":"),
        "status saved a language it was only shown: {saved}"
    );
}

#[test]
fn an_unknown_language_is_refused_with_the_ones_that_exist() {
    let dir = tempfile::tempdir().unwrap();
    let (out, err, ok) = readcode(dir.path(), &["--language", "cobol", "status"]);
    assert!(!ok, "{out}{err}");
    let message = format!("{out}{err}");
    assert!(message.contains("no `cobol`"), "{message}");
    assert!(message.contains("rust"), "{message}");
    assert!(message.contains("go"), "{message}");
}

#[test]
fn lang_switches_and_stays_switched() {
    let dir = tempfile::tempdir().unwrap();
    let (_, err, ok) = readcode(dir.path(), &["lang", "go"]);
    assert!(ok, "{err}");

    let (out, err, ok) = readcode(dir.path(), &["lang"]);
    assert!(ok, "{err}");
    let go = out
        .lines()
        .find(|line| line.contains("go"))
        .unwrap_or_default()
        .to_string();
    assert!(go.contains('→'), "go should be marked as current: {out}");
}
