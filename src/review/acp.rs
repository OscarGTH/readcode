//! Host the user's own coding agent over the Agent Client Protocol.
//!
//! ACP is JSON-RPC 2.0 over the agent's stdin/stdout; readcode is the client.
//! The client is written by hand rather than pulled from an SDK, so it stays
//! testable against a fixture and does not drift with a crate version.
//!
//! Three details matter for a headless grader:
//!  * ACP has no system prompt — the grading contract is prepended to the first
//!    prompt ([`super::contract::prime`]).
//!  * There is no approval UI, so permission requests are refused
//!    deterministically instead of hanging the run.
//!  * The `session/prompt` response can arrive before the last
//!    `session/update`, so the turn is drained until quiet before parsing.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::{Annotation, Grade, Reviewer, contract, parse_json_object, truncate};
use crate::snippet::{Prompt, Snippet};

pub const PROTOCOL_VERSION: u32 = 1;

/// How long to keep reading after `session/prompt` resolved, to catch updates
/// that were still in flight.
const DRAIN_IDLE: Duration = Duration::from_millis(400);

enum Incoming {
    Response {
        id: i64,
        result: std::result::Result<Value, String>,
    },
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Closed,
}

pub struct AcpClient {
    writer: Option<Box<dyn Write + Send>>,
    rx: Receiver<Incoming>,
    reader: Option<JoinHandle<()>>,
    child: Option<Child>,
    next_id: i64,
    timeout: Duration,
    session_id: Option<String>,
    /// What the agent lets the client switch for this session, from `session/new`.
    pub config_options: Value,
    pub agent_name: String,
    pub agent_version: String,
    /// Permission requests we refused, for the summary line.
    pub refusals: Vec<String>,
    stderr_log: Arc<Mutex<Vec<String>>>,
}

impl AcpClient {
    pub fn spawn(program: &str, args: &[String], cwd: &Path, timeout: Duration) -> Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "cannot start `{program} {}` — is it installed and on PATH?",
                    args.join(" ")
                )
            })?;

        let stdin = child.stdin.take().context("the agent has no stdin")?;
        let stdout = child.stdout.take().context("the agent has no stdout")?;
        let stderr = child.stderr.take();

        let (tx, rx) = channel();
        let reader = spawn_reader(stdout, tx);
        let stderr_log = Arc::new(Mutex::new(Vec::new()));
        if let Some(stderr) = stderr {
            spawn_stderr_reader(stderr, Arc::clone(&stderr_log));
        }

        Ok(Self {
            writer: Some(Box::new(stdin)),
            rx,
            reader: Some(reader),
            child: Some(child),
            next_id: 0,
            timeout,
            session_id: None,
            config_options: Value::Null,
            agent_name: String::new(),
            agent_version: String::new(),
            refusals: Vec::new(),
            stderr_log,
        })
    }

    /// For tests: drive the client over any pair of streams.
    pub fn from_parts<R, W>(reader: R, writer: W, timeout: Duration) -> Self
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        let (tx, rx) = channel();
        let reader = spawn_reader(reader, tx);
        Self {
            writer: Some(Box::new(writer)),
            rx,
            reader: Some(reader),
            child: None,
            next_id: 0,
            timeout,
            session_id: None,
            config_options: Value::Null,
            agent_name: String::new(),
            agent_version: String::new(),
            refusals: Vec::new(),
            stderr_log: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn initialize(&mut self) -> Result<()> {
        let result = self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "clientCapabilities": {
                    "fs": {"readTextFile": false, "writeTextFile": false},
                    "terminal": false
                },
                "clientInfo": {
                    "name": "readcode",
                    "title": "readcode",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )?;

        let version = result["protocolVersion"].as_u64().unwrap_or(0);
        if version != PROTOCOL_VERSION as u64 {
            bail!("the agent speaks ACP v{version}, readcode implements v{PROTOCOL_VERSION}");
        }
        self.agent_name = result["agentInfo"]["name"]
            .as_str()
            .unwrap_or("agent")
            .to_string();
        self.agent_version = result["agentInfo"]["version"]
            .as_str()
            .unwrap_or("?")
            .to_string();
        Ok(())
    }

    pub fn new_session(&mut self, cwd: &Path) -> Result<&str> {
        let result = self.request(
            "session/new",
            json!({
                "cwd": cwd.to_string_lossy(),
                "mcpServers": []
            }),
        )?;
        let id = result["sessionId"]
            .as_str()
            .context("`session/new` returned no sessionId")?
            .to_string();
        self.config_options = result.get("configOptions").cloned().unwrap_or(Value::Null);
        self.session_id = Some(id);
        Ok(self.session_id.as_deref().unwrap_or_default())
    }

    /// The model this session is on, from `session/new`'s `configOptions`.
    pub fn current_model(&self) -> Option<String> {
        model_options(&self.config_options)
            .map(|(current, _)| current)
            .filter(|current| !current.is_empty())
    }

    pub fn set_config_option(&mut self, config_id: &str, value: &str) -> Result<()> {
        let session = self.session_id.clone().context("no ACP session is open")?;
        self.request(
            "session/set_config_option",
            json!({"sessionId": session, "configId": config_id, "value": value}),
        )?;
        Ok(())
    }

    /// Sends one turn and returns the agent's text. Updates that arrive after the
    /// prompt resolved are drained, not dropped.
    pub fn prompt(&mut self, text: &str) -> Result<String> {
        let session = self.session_id.clone().context("no ACP session is open")?;
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": text}]}
        }))
        .context("cannot send the prompt to the agent")?;

        let deadline = Instant::now() + self.timeout;
        let mut output = String::new();
        let stop_reason: String;

        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!(
                    "the agent did not finish the turn within {:?}",
                    self.timeout
                );
            }
            match self.rx.recv_timeout(left) {
                Ok(Incoming::Notification { method, params }) => {
                    observe(&method, &params, &mut output)
                }
                Ok(Incoming::Request { id, method, params }) => {
                    self.answer_request(id, &method, &params)?
                }
                Ok(Incoming::Response { id: got, result }) if got == id => {
                    match result {
                        Ok(value) => {
                            stop_reason = value["stopReason"]
                                .as_str()
                                .unwrap_or("end_turn")
                                .to_string();
                        }
                        Err(error) => bail!("the agent failed the turn: {error}"),
                    }
                    break;
                }
                Ok(Incoming::Response { .. }) => continue,
                Ok(Incoming::Closed) => bail!("the agent exited mid-turn{}", self.stderr_hint()),
                Err(RecvTimeoutError::Timeout) => {
                    bail!(
                        "the agent did not finish the turn within {:?}",
                        self.timeout
                    )
                }
                Err(RecvTimeoutError::Disconnected) => {
                    bail!("the agent connection closed mid-turn{}", self.stderr_hint())
                }
            }
        }

        // A last chunk can arrive after the response; keep reading until quiet.
        loop {
            match self.rx.recv_timeout(DRAIN_IDLE) {
                Ok(Incoming::Notification { method, params }) => {
                    observe(&method, &params, &mut output)
                }
                Ok(Incoming::Request { id, method, params }) => {
                    self.answer_request(id, &method, &params)?
                }
                Ok(Incoming::Response { .. }) => continue,
                Ok(Incoming::Closed) => break,
                Err(_) => break,
            }
        }

        if stop_reason == "cancelled" {
            bail!("the agent cancelled the turn");
        }
        if output.trim().is_empty() {
            bail!("the agent returned no text (stop reason: {stop_reason})");
        }
        Ok(output)
    }

    fn send(&mut self, message: Value) -> Result<()> {
        let writer = self
            .writer
            .as_mut()
            .context("the agent connection is already closed")?;
        let line = serde_json::to_string(&message)?;
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        Ok(())
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .with_context(|| format!("cannot send `{method}` to the agent"))?;

        let deadline = Instant::now() + self.timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!(
                    "the agent did not answer `{method}` within {:?}{}",
                    self.timeout,
                    self.stderr_hint()
                );
            }
            match self.rx.recv_timeout(left) {
                Ok(Incoming::Response { id: got, result }) if got == id => {
                    return result.map_err(|error| anyhow::anyhow!("`{method}` failed: {error}"));
                }
                Ok(Incoming::Response { .. }) => continue,
                Ok(Incoming::Request { id, method, params }) => {
                    self.answer_request(id, &method, &params)?
                }
                Ok(Incoming::Notification { .. }) => continue,
                Ok(Incoming::Closed) => {
                    bail!(
                        "the agent exited while answering `{method}`{}",
                        self.stderr_hint()
                    )
                }
                Err(RecvTimeoutError::Timeout) => bail!(
                    "the agent did not answer `{method}` within {:?}{}",
                    self.timeout,
                    self.stderr_hint()
                ),
                Err(RecvTimeoutError::Disconnected) => {
                    bail!(
                        "the agent connection closed during `{method}`{}",
                        self.stderr_hint()
                    )
                }
            }
        }
    }

    /// There is no approval UI here, so every permission request is refused with
    /// the agent's own reject option. Anything else is a method we never
    /// advertised.
    fn answer_request(&mut self, id: Value, method: &str, params: &Value) -> Result<()> {
        let reply = match method {
            "session/request_permission" => {
                let options = params["options"].as_array().cloned().unwrap_or_default();
                let reject = options
                    .iter()
                    .find(|option| option["kind"].as_str().unwrap_or("").starts_with("reject"));
                match reject {
                    Some(option) => {
                        let option_id = option["optionId"]
                            .as_str()
                            .unwrap_or("reject-once")
                            .to_string();
                        let title = params["toolCall"]["title"]
                            .as_str()
                            .unwrap_or("a tool call")
                            .to_string();
                        self.refusals.push(title);
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {"outcome": {"outcome": "selected", "optionId": option_id}}
                        })
                    }
                    None => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {"outcome": {"outcome": "cancelled"}}
                    }),
                }
            }
            other => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": format!("readcode does not implement `{other}`")}
            }),
        };
        self.send(reply)
    }

    fn stderr_hint(&self) -> String {
        let log = match self.stderr_log.lock() {
            Ok(log) => log.clone(),
            Err(_) => return String::new(),
        };
        if log.is_empty() {
            return String::new();
        }
        let tail = log
            .iter()
            .rev()
            .take(5)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join(" | ");
        format!(". Agent stderr: {}", truncate(&tail, 400))
    }
}

impl Drop for AcpClient {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Dropping the writer closes the agent's stdin, which ends its stdout.
        self.writer = None;
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn spawn_reader<R: Read + Send + 'static>(
    source: R,
    tx: std::sync::mpsc::Sender<Incoming>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let reader = BufReader::new(source);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                // Agents sometimes print a banner before the protocol starts.
                continue;
            };
            if let Some(message) = classify(value) {
                if tx.send(message).is_err() {
                    return;
                }
            }
        }
        let _ = tx.send(Incoming::Closed);
    })
}

fn spawn_stderr_reader<R: Read + Send + 'static>(source: R, log: Arc<Mutex<Vec<String>>>) {
    thread::spawn(move || {
        let reader = BufReader::new(source);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(mut log) = log.lock() {
                log.push(line);
                if log.len() > 50 {
                    log.remove(0);
                }
            }
        }
    });
}

fn classify(value: Value) -> Option<Incoming> {
    if let Some(method) = value.get("method").and_then(|m| m.as_str()) {
        let method = method.to_string();
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        return match value.get("id").filter(|id| !id.is_null()) {
            Some(id) => Some(Incoming::Request {
                id: id.clone(),
                method,
                params,
            }),
            None => Some(Incoming::Notification { method, params }),
        };
    }

    let id = value.get("id").and_then(|id| id.as_i64())?;
    if let Some(error) = value.get("error") {
        let message = error["message"]
            .as_str()
            .unwrap_or("unknown error")
            .to_string();
        let code = error["code"].as_i64().unwrap_or(0);
        return Some(Incoming::Response {
            id,
            result: Err(format!("{message} (code {code})")),
        });
    }
    Some(Incoming::Response {
        id,
        result: Ok(value.get("result").cloned().unwrap_or(Value::Null)),
    })
}

fn observe(method: &str, params: &Value, output: &mut String) {
    if method != "session/update" {
        return;
    }
    let update = &params["update"];
    if update["sessionUpdate"].as_str() == Some("agent_message_chunk") {
        if let Some(text) = update["content"]["text"].as_str() {
            output.push_str(text);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOption {
    pub value: String,
    pub name: String,
}

/// Reads the `model` select out of `session/new`'s `configOptions`, flattening
/// grouped options. Returns `(current value, every option)`.
pub fn model_options(config_options: &Value) -> Option<(String, Vec<ModelOption>)> {
    let options = config_options
        .as_array()
        .or_else(|| config_options.get("configOptions")?.as_array())?;
    let select = options.iter().find(|option| {
        option["id"].as_str() == Some("model") || option["category"].as_str() == Some("model")
    })?;
    let current = select["currentValue"].as_str().unwrap_or("").to_string();

    let mut flat = Vec::new();
    for option in select["options"].as_array()? {
        match option.get("options").and_then(|group| group.as_array()) {
            Some(group) => {
                for inner in group {
                    push_option(&mut flat, inner);
                }
            }
            None => push_option(&mut flat, option),
        }
    }
    Some((current, flat))
}

fn push_option(flat: &mut Vec<ModelOption>, option: &Value) {
    if let (Some(value), Some(name)) = (option["value"].as_str(), option["name"].as_str()) {
        flat.push(ModelOption {
            value: value.to_string(),
            name: name.to_string(),
        });
    }
}

/// Free tiers are recognised by identifier first, then by name. Anything
/// unrecognised is left alone: guessing a provider's prices is worse than
/// inheriting a default.
pub fn is_free_model(value: &str, name: &str) -> bool {
    let value = value.to_lowercase();
    let name = name.to_lowercase();
    [":free", "-free", "/free", "big-pickle", "grok-code"]
        .iter()
        .any(|marker| value.contains(marker))
        || ["(free)", "free tier", "free)"]
            .iter()
            .any(|marker| name.contains(marker))
}

/// Grades through the user's own agent. One agent process per grade: the CLI
/// grades a single snippet per run, so the ~2 s handshake is paid once.
pub struct AcpReviewer {
    pub program: String,
    pub args: Vec<String>,
    /// An explicit model choice wins over the free-tier rule.
    pub model: Option<String>,
    pub timeout: Duration,
    pub cwd: std::path::PathBuf,
}

impl AcpReviewer {
    /// `READCODE_AGENT` holds the full command (`opencode acp`), so any ACP
    /// agent works without a new flag.
    pub fn from_env(cwd: std::path::PathBuf) -> Self {
        let command = std::env::var("READCODE_AGENT")
            .ok()
            .filter(|c| !c.trim().is_empty())
            .unwrap_or_else(|| "opencode acp".to_string());
        let mut parts: Vec<String> = command.split_whitespace().map(str::to_string).collect();
        let program = if parts.is_empty() {
            "opencode".to_string()
        } else {
            parts.remove(0)
        };
        Self {
            program,
            args: parts,
            model: std::env::var("READCODE_MODEL")
                .ok()
                .filter(|m| !m.trim().is_empty()),
            timeout: Duration::from_secs(180),
            cwd,
        }
    }

    fn connect(&self) -> Result<AcpClient> {
        let mut client = AcpClient::spawn(&self.program, &self.args, &self.cwd, self.timeout)?;
        client.initialize()?;
        client.new_session(&self.cwd)?;

        match &self.model {
            Some(model) => {
                // An explicit choice is the only way a model is selected. If the
                // agent refuses it, say so and stop rather than grading on
                // something the user did not ask for.
                client
                    .set_config_option("model", model)
                    .with_context(|| format!("the agent refused the model `{model}`"))?;
            }
            None => {
                // Never switch silently. Name what it will use and how to pin it.
                if let Some((current, options)) = model_options(&client.config_options) {
                    let name = options
                        .iter()
                        .find(|option| option.value == current)
                        .map(|option| option.name.clone())
                        .unwrap_or_else(|| current.clone());
                    if is_free_model(&current, &name) {
                        eprintln!(
                            "readcode: grading with `{current}` — a free shared-queue model, so expect slow, variable answers. Pin one with `--model <id>` or READCODE_MODEL; `readcode models` lists what this agent offers."
                        );
                    }
                }
            }
        }

        Ok(client)
    }

    /// Opens a session and reports the models this agent offers. No prompt is
    /// sent, so it spends no tokens.
    pub fn list_models(&self) -> Result<AgentModels> {
        let mut client = AcpClient::spawn(&self.program, &self.args, &self.cwd, self.timeout)?;
        client.initialize()?;
        client.new_session(&self.cwd)?;
        let (current, options) = model_options(&client.config_options)
            .context("this agent does not offer a model choice for the session")?;
        Ok(AgentModels {
            agent: format!("{} {}", client.agent_name, client.agent_version),
            current,
            options,
        })
    }
}

/// What an agent will let you choose from.
pub struct AgentModels {
    pub agent: String,
    pub current: String,
    pub options: Vec<ModelOption>,
}

impl AcpReviewer {
    /// `acp:opencode (deepseek/deepseek-v4-pro)`: the agent, plus whatever model
    /// actually answered. Kept in one place so the header and the review agree.
    fn label(&self, model: Option<&str>) -> String {
        match model.filter(|model| !model.is_empty()) {
            Some(model) => format!("acp:{} ({model})", self.program),
            None => format!("acp:{}", self.program),
        }
    }
}

impl Reviewer for AcpReviewer {
    fn name(&self) -> String {
        self.label(self.model.as_deref())
    }

    fn grade(
        &self,
        snippet: &Snippet,
        prompts: &[Prompt],
        annotations: &[Annotation],
    ) -> Result<Grade> {
        let mut client = self.connect()?;
        let prompt = contract::prime(&contract::task(snippet, prompts, annotations));
        let text = client.prompt(&prompt)?;
        let raw = parse_json_object(&text)
            .with_context(|| format!("the agent ({}) did not return a grade", client.agent_name))?;
        let mut grade = Grade::from_raw(prompts, raw, &self.name())?;
        // When nothing was pinned, name the model the session reports it is on.
        // When one was pinned, keep it: `current_model` reflects the session's
        // opening config, not the option we set.
        if self.model.is_none() {
            grade.reviewer = self.label(client.current_model().as_deref());
        }
        if !client.refusals.is_empty() {
            grade.summary = format!(
                "{} (refused tool use: {})",
                grade.summary,
                client.refusals.join(", ")
            );
        }
        Ok(grade)
    }

    fn ask(&self, task: &str) -> Result<String> {
        let mut client = self.connect()?;
        client.prompt(&contract::prime(task))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNIPPET: &str = r#"
id = "t-acp"
language = "rust"
title = "fixture"
code = '''
let x = 1;
'''

[[blocks]]
lines = [1, 1]
ask = "what is x?"
concepts = ["rust.binding"]
"#;

    const CHUNKS: [&str; 3] = [
        "{\"blocks\":[{\"block\":1,",
        "\"verdict\":\"correct\"",
        ",\"concepts\":{\"rust.binding\":\"correct\"},\"feedback\":\"yes\"}],\"summary\":\"fine\"}",
    ];

    #[test]
    fn grading_carries_the_contract_without_a_system_prompt() {
        let snippet = Snippet::parse(SNIPPET).unwrap();
        let prompts = snippet.prompts(crate::snippet::Asks::Blocks);
        let prompt = contract::prime(&contract::task(
            &snippet,
            &prompts,
            &[Annotation::new(0, "x is bound")],
        ));
        assert!(prompt.starts_with(contract::CONTRACT));
        assert!(prompt.contains("\"verdict\""));
    }

    #[test]
    fn classifies_responses_requests_and_notifications() {
        let response = classify(json!({"jsonrpc":"2.0","id":3,"result":{"ok":true}}));
        assert!(matches!(response, Some(Incoming::Response { id: 3, .. })));

        let request = classify(
            json!({"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{}}),
        );
        assert!(matches!(request, Some(Incoming::Request { .. })));

        let note = classify(json!({"jsonrpc":"2.0","method":"session/update","params":{}}));
        assert!(matches!(note, Some(Incoming::Notification { .. })));

        let error =
            classify(json!({"jsonrpc":"2.0","id":4,"error":{"code":-32602,"message":"bad"}}));
        match error {
            Some(Incoming::Response {
                result: Err(message),
                ..
            }) => {
                assert!(message.contains("bad") && message.contains("-32602"))
            }
            other => panic!("expected an error response, got {:?}", other.is_some()),
        }
    }

    #[test]
    fn the_reviewer_name_names_the_model_only_when_one_was_asked_for() {
        let mut reviewer = AcpReviewer::from_env(std::path::PathBuf::from("/tmp"));
        reviewer.program = "opencode".to_string();
        reviewer.model = None;
        assert_eq!(reviewer.name(), "acp:opencode");

        reviewer.model = Some("deepseek/deepseek-v4-pro".to_string());
        assert_eq!(reviewer.name(), "acp:opencode (deepseek/deepseek-v4-pro)");
    }

    #[test]
    fn free_detection_covers_ids_and_names() {
        assert!(is_free_model("opencode/big-pickle", "Big Pickle"));
        assert!(is_free_model("x/y:free", "Y"));
        assert!(is_free_model("acme/small", "Small (free)"));
        assert!(!is_free_model("acme/opus", "Opus"));
    }

    #[test]
    fn model_options_flatten_groups() {
        let config = json!([{
            "id": "model",
            "currentValue": "paid/a",
            "options": [
                {"value": "paid/a", "name": "A"},
                {"group": "free", "name": "Free", "options": [{"value": "free/b", "name": "B (free)"}]}
            ]
        }]);
        let (current, options) = model_options(&config).unwrap();
        assert_eq!(current, "paid/a");
        assert_eq!(options.len(), 2);
        assert_eq!(options[1].value, "free/b");
    }

    fn snippet() -> Snippet {
        Snippet::parse(SNIPPET).unwrap()
    }

    fn fixture_reply(writer: &mut impl Write, id: Value, result: Value) {
        let line = json!({"jsonrpc": "2.0", "id": id, "result": result});
        writeln!(writer, "{line}").unwrap();
        writer.flush().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn completes_a_turn_against_a_fixture_agent() {
        use std::os::unix::net::UnixStream;

        let (client_side, agent_side) = UnixStream::pair().unwrap();
        let agent = thread::spawn(move || {
            let mut reader = BufReader::new(agent_side.try_clone().unwrap());
            let mut writer = agent_side;
            let mut seen: Vec<Value> = Vec::new();
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
                    line.clear();
                    continue;
                };
                seen.push(value.clone());
                let id = value["id"].clone();
                match value["method"].as_str().unwrap_or("") {
                    "initialize" => fixture_reply(
                        &mut writer,
                        id,
                        json!({"protocolVersion":1,"agentCapabilities":{},"agentInfo":{"name":"Fixture","version":"0.0.1"}}),
                    ),
                    "session/new" => fixture_reply(
                        &mut writer,
                        id,
                        json!({"sessionId":"ses_1","configOptions":[{"id":"model","currentValue":"free/big-pickle","options":[
                            {"value":"free/big-pickle","name":"Big Pickle (free)"},
                            {"value":"paid/model","name":"Paid Model"}]}]}),
                    ),
                    "session/set_config_option" => fixture_reply(&mut writer, id, json!({})),
                    "session/prompt" => {
                        // The agent asks permission mid-turn: we must refuse, not hang.
                        let permission = json!({"jsonrpc":"2.0","id":900,"method":"session/request_permission","params":{
                            "sessionId":"ses_1",
                            "toolCall":{"toolCallId":"c1","title":"write to disk","kind":"edit"},
                            "options":[{"optionId":"allow-once","name":"Allow","kind":"allow_once"},
                                       {"optionId":"reject-once","name":"Reject","kind":"reject_once"}]}});
                        writeln!(writer, "{permission}").unwrap();
                        writer.flush().unwrap();
                        let mut answer = String::new();
                        reader.read_line(&mut answer).unwrap();
                        seen.push(serde_json::from_str(answer.trim()).unwrap());
                        // Stream the grade in three chunks.
                        for chunk in CHUNKS {
                            let update = json!({"jsonrpc":"2.0","method":"session/update","params":{
                                "sessionId":"ses_1","update":{"sessionUpdate":"agent_message_chunk",
                                "content":{"type":"text","text":chunk}}}});
                            writeln!(writer, "{update}").unwrap();
                        }
                        writer.flush().unwrap();
                        fixture_reply(&mut writer, id, json!({"stopReason":"end_turn"}));
                        break;
                    }
                    other => panic!("unexpected method {other}"),
                }
                line.clear();
            }
            seen
        });

        let mut client = AcpClient::from_parts(
            client_side.try_clone().unwrap(),
            client_side,
            Duration::from_secs(5),
        );
        client.initialize().unwrap();
        assert_eq!(client.agent_name, "Fixture");
        client.new_session(Path::new("/tmp")).unwrap();
        assert_eq!(client.session_id.as_deref(), Some("ses_1"));

        // The session reports what it is on; nothing switches behind the user's back.
        assert_eq!(client.current_model().as_deref(), Some("free/big-pickle"));
        let (_, options) = model_options(&client.config_options).unwrap();
        assert!(options.iter().any(|option| option.value == "paid/model"));

        let text = client.prompt("grade this").unwrap();
        let raw = parse_json_object(&text).unwrap();
        let prompts = snippet().prompts(crate::snippet::Asks::Blocks);
        let grade = Grade::from_raw(&prompts, raw, "acp:fixture").unwrap();
        assert_eq!(grade.prompts[0].verdict, crate::srs::Verdict::Correct);
        assert_eq!(client.refusals, vec!["write to disk".to_string()]);

        let seen = agent.join().unwrap();
        let permission_answer = seen
            .iter()
            .find(|message| message["id"] == json!(900))
            .expect("the client answered the permission request");
        assert_eq!(
            permission_answer["result"]["outcome"]["optionId"],
            json!("reject-once")
        );
        let prompt = seen
            .iter()
            .find(|message| message["method"] == json!("session/prompt"))
            .expect("the prompt was sent");
        assert_eq!(prompt["params"]["prompt"][0]["type"], json!("text"));
        assert_eq!(prompt["params"]["prompt"][0]["text"], json!("grade this"));
    }

    #[cfg(unix)]
    #[test]
    fn an_agent_that_dies_mid_turn_is_an_error_not_a_hang() {
        use std::os::unix::net::UnixStream;

        let (client_side, agent_side) = UnixStream::pair().unwrap();
        let agent = thread::spawn(move || {
            let mut reader = BufReader::new(agent_side.try_clone().unwrap());
            let mut writer = agent_side;
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                let value: Value = serde_json::from_str(line.trim()).unwrap();
                match value["method"].as_str().unwrap_or("") {
                    "initialize" => fixture_reply(
                        &mut writer,
                        value["id"].clone(),
                        json!({"protocolVersion":1,"agentInfo":{"name":"Dying"}}),
                    ),
                    "session/new" => {
                        fixture_reply(&mut writer, value["id"].clone(), json!({"sessionId":"s"}))
                    }
                    // Read the prompt, then vanish without answering.
                    _ => break,
                }
                line.clear();
            }
        });

        let mut client = AcpClient::from_parts(
            client_side.try_clone().unwrap(),
            client_side,
            Duration::from_secs(5),
        );
        client.initialize().unwrap();
        client.new_session(Path::new("/tmp")).unwrap();
        let error = client.prompt("grade this").unwrap_err().to_string();
        assert!(error.contains("exited"), "{error}");
        agent.join().unwrap();
    }
}
