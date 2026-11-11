//! The ACP wire protocol, checked against a fake agent.
//!
//! This exists because model selection broke silently once: `--model` was parsed
//! and shown in the review header but never sent, so every sitting ran on the
//! agent's default while the UI claimed otherwise. A unit test cannot see that —
//! the defect is in what goes over the pipe — so this drives the real client
//! against a script that speaks the protocol and records what it is asked.

use readcode::corpus::Corpus;
use readcode::review::{Annotation, Reviewer, acp::AcpReviewer};
use readcode::snippet::Asks;

/// A minimal ACP agent: answers the handshake, logs every method it receives to
/// the path in argv, and replies to a prompt with a grade.
const FAKE_AGENT: &str = r#"
import json, sys

log = open(sys.argv[1], "a", buffering=1)

for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    if not method:
        continue
    log.write(method + " " + json.dumps(message.get("params", {})) + "\n")

    if method == "initialize":
        result = {
            "protocolVersion": 1,
            "agentCapabilities": {"loadSession": False},
            "agentInfo": {"name": "fake", "version": "1.0"},
        }
    elif method == "session/new":
        result = {
            "sessionId": "s1",
            "configOptions": [{
                "id": "model",
                "category": "model",
                "currentValue": "default/one",
                "options": [
                    {"value": "default/one", "name": "Default One"},
                    {"value": "chosen/two", "name": "Chosen Two"},
                ],
            }],
        }
    elif method == "session/prompt":
        grade = {
            "prompts": [{
                "prompt": 1,
                "verdict": "correct",
                "concepts": {"rust.fn.main": "correct"},
                "feedback": "fine",
                "teach": "a binding names a value",
            }],
            "summary": "fine",
        }
        sys.stdout.write(json.dumps({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {"sessionId": "s1", "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": json.dumps(grade)},
            }},
        }) + "\n")
        sys.stdout.flush()
        result = {"stopReason": "end_turn"}
    else:
        result = {}

    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}) + "\n")
    sys.stdout.flush()
"#;

/// A reviewer wired to the fake agent, with the model set explicitly (the env var
/// is process-wide, and these tests run in parallel).
fn fake_reviewer(dir: &std::path::Path, model: Option<&str>) -> (AcpReviewer, std::path::PathBuf) {
    let agent = dir.join("fake_agent.py");
    std::fs::write(&agent, FAKE_AGENT).unwrap();
    let log = dir.join("methods.log");
    std::fs::write(&log, "").unwrap();

    let mut reviewer = AcpReviewer::from_env(dir.to_path_buf());
    reviewer.program = "python3".to_string();
    reviewer.args = vec![
        agent.to_string_lossy().to_string(),
        log.to_string_lossy().to_string(),
    ];
    reviewer.model = model.map(str::to_string);
    (reviewer, log)
}

fn grade_once(reviewer: &AcpReviewer) {
    let corpus = Corpus::builtin().unwrap();
    let snippet = corpus.find("rust-006").unwrap();
    let prompts = snippet.prompts(Asks::Blocks);
    let annotations: Vec<Annotation> = prompts
        .iter()
        .enumerate()
        .map(|(index, _)| Annotation::new(index, "a binding names a value"))
        .collect();
    reviewer.grade(snippet, &prompts, &annotations).unwrap();
}

#[test]
fn the_pinned_model_is_sent_to_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let (reviewer, log) = fake_reviewer(dir.path(), Some("chosen/two"));
    grade_once(&reviewer);

    let recorded = std::fs::read_to_string(&log).unwrap();
    assert!(
        recorded.contains("session/set_config_option"),
        "the model was never sent — the sitting would run on the agent's default:\n{recorded}"
    );
    assert!(
        recorded.contains("chosen/two"),
        "the wrong model was sent:\n{recorded}"
    );
}

#[test]
fn a_model_the_agent_does_not_offer_fails_before_any_answer() {
    let dir = tempfile::tempdir().unwrap();
    let (reviewer, _log) = fake_reviewer(dir.path(), Some("not/listed"));

    // The pre-flight the CLI runs before the first question.
    let error = reviewer.check().unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains("not a model this agent offers"),
        "{message}"
    );
    assert!(message.contains("chosen/two"), "{message}");
}

#[test]
fn no_model_at_all_is_refused_without_spawning_anything() {
    let dir = tempfile::tempdir().unwrap();
    let (reviewer, log) = fake_reviewer(dir.path(), None);

    let error = reviewer.check().unwrap_err();
    assert!(
        format!("{error:#}").contains("no model pinned"),
        "{error:#}"
    );
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "");
}

#[test]
fn a_turn_error_about_the_model_explains_itself() {
    // The failure that was reported: the agent accepted the id, then the turn died
    // with a JSON-RPC internal error that says nothing about what to do.
    let hint = readcode::review::acp::turn_hint(
        "the agent failed the turn: Internal error: The requested model is not supported. (code -32603)",
        Some("github-copilot/gpt-6-luna"),
    )
    .unwrap();
    assert!(hint.contains("github-copilot/gpt-6-luna"), "{hint}");
    assert!(hint.contains("readcode models"), "{hint}");
}
