//! The first-run wizard: who grades, and with which model.
//!
//! Someone who has just installed `readcode` has no idea that grading needs an
//! agent command, a model id, or two environment variables. Finding out by having
//! a sitting die at grading time is the worst possible introduction, so the first
//! run asks, writes the answer down, and says what else the tool can do.

use anyhow::Result;

use crate::config::Config;
use crate::corpus::Corpus;
use crate::session::{Terminal, ask_choice};

/// Agents this tool has actually been run against, with the command that speaks
/// the protocol. Anything else can be typed in by hand.
const KNOWN_AGENTS: &[(&str, &str)] = &[("opencode", "opencode acp")];

pub fn run(term: &mut Terminal<'_>, corpus: &Corpus, data_dir: &std::path::Path) -> Result<Config> {
    writeln!(term.out)?;
    writeln!(
        term.out,
        "{}",
        term.style.bold("First, how should your answers be graded?")
    )?;
    writeln!(
        term.out,
        "{}",
        term.style.dim(
            "readcode asks a model to mark what you wrote. It can run through an agent you\nalready have, or call an API with your own key."
        )
    )?;

    let found: Vec<&str> = KNOWN_AGENTS
        .iter()
        .filter(|(binary, _)| on_path(binary))
        .map(|(_, command)| *command)
        .collect();
    let has_key = std::env::var("READCODE_API_KEY").is_ok()
        || std::env::var("OPENAI_API_KEY").is_ok()
        || std::env::var("ANTHROPIC_API_KEY").is_ok();

    let mut options: Vec<String> = Vec::new();
    for command in &found {
        options.push(format!("an agent already on this machine ({command})"));
    }
    if found.is_empty() {
        options.push("an agent command I will type".to_string());
    }
    options.push(if has_key {
        "an API key from the environment".to_string()
    } else {
        "an API key (set READCODE_API_KEY afterwards)".to_string()
    });

    let choice = ask_choice(term, "reviewer", &options, 0)?;
    let use_api = if found.is_empty() {
        choice == options.len() - 1
    } else {
        choice >= found.len()
    };

    let mut config = Config {
        reviewer: Some(if use_api { "api" } else { "acp" }.to_string()),
        ..Config::default()
    };

    if use_api {
        writeln!(term.out)?;
        writeln!(
            term.out,
            "{}",
            term.style.dim(
                "The key is read from READCODE_API_KEY (or OPENAI_API_KEY / ANTHROPIC_API_KEY).\nIt is not written to disk — this file is copied into dotfiles and pasted into issues."
            )
        )?;
        let model = ask_text(term, "model id", Some("gpt-4o-mini"))?;
        config.model = Some(model);
        if !has_key {
            writeln!(
                term.out,
                "{}",
                term.style.yellow(
                    "No key found in this shell: export READCODE_API_KEY=… before a sitting."
                )
            )?;
        }
    } else {
        let agent = if found.len() == 1 && choice == 0 {
            found[0].to_string()
        } else {
            ask_text(term, "agent command", found.first().copied())?
        };
        config.agent = Some(agent.clone());

        writeln!(term.out)?;
        writeln!(
            term.out,
            "{}",
            term.style.dim("Asking that agent which models it offers…")
        )?;
        let models = list_models(data_dir, &agent);
        match models {
            Ok((current, offered)) if !offered.is_empty() => {
                let mut options: Vec<String> = offered
                    .iter()
                    .map(|(value, name)| {
                        if name.is_empty() || name == value {
                            value.clone()
                        } else {
                            format!("{value}  {name}")
                        }
                    })
                    .collect();
                // A free shared-queue model is a fine default to *see*, not a fine
                // thing to grade with unnoticed.
                let default = offered
                    .iter()
                    .position(|(value, _)| *value == current)
                    .unwrap_or(0);
                let pick = ask_choice(term, "model", &options, default)?;
                config.model = Some(offered[pick].0.clone());
                options.clear();
            }
            Ok(_) => {
                let model = ask_text(term, "model id", None)?;
                config.model = Some(model);
            }
            Err(error) => {
                writeln!(
                    term.out,
                    "{}",
                    term.style
                        .yellow(&format!("could not list models ({error:#})"))
                )?;
                let model = ask_text(term, "model id", None)?;
                config.model = Some(model);
            }
        }
    }

    config.save(data_dir)?;
    writeln!(term.out)?;
    writeln!(
        term.out,
        "{}",
        term.style
            .bold(&format!("saved · {}", Config::path(data_dir).display()))
    )?;
    if let Some(model) = &config.model {
        writeln!(term.out, "{}", term.style.dim(&format!("model · {model}")))?;
    }
    writeln!(
        term.out,
        "{}",
        term.style.dim("change it any time with `readcode setup`")
    )?;

    teach(term, corpus)?;
    Ok(config)
}

/// The bit that makes the tool discoverable. Six commands, one line each, in the
/// order someone actually needs them.
fn teach(term: &mut Terminal<'_>, corpus: &Corpus) -> Result<()> {
    writeln!(term.out)?;
    writeln!(term.out, "{}", term.style.bold("What you can run"))?;
    let lines = [
        (
            "readcode",
            "one sitting: read the code, explain it, get marked",
        ),
        (
            "readcode status",
            "where you are, what is due, what is next",
        ),
        (
            "readcode lang",
            "the languages in the corpus, and how far you are in each",
        ),
        ("readcode level", "the rungs, with yours marked"),
        ("readcode list", "every snippet, with its level"),
        (
            "readcode history",
            "past sittings, and the grading page for each",
        ),
        ("readcode setup", "change reviewer, agent or model"),
    ];
    for (command, what) in lines {
        writeln!(
            term.out,
            "  {} {:<16} {}",
            term.style.cyan("·"),
            command,
            term.style.dim(what)
        )?;
    }
    writeln!(
        term.out,
        "{}",
        term.style.dim(&format!(
            "\n{} snippets, {} languages. Enter submits an answer, Ctrl-J adds a line, Ctrl-B goes back.",
            corpus.snippets().len(),
            corpus.languages().len()
        ))
    )?;
    Ok(())
}

/// `(current, options)` from the agent, without spending a turn on it.
fn list_models(data_dir: &std::path::Path, agent: &str) -> Result<(String, Vec<(String, String)>)> {
    let cwd = data_dir.join("agent");
    std::fs::create_dir_all(&cwd).ok();
    let mut reviewer = crate::review::acp::AcpReviewer::from_env(cwd);
    reviewer.set_command(agent);
    let models = reviewer.list_models()?;
    Ok((
        models.current.clone(),
        models
            .options
            .iter()
            .map(|option| (option.value.clone(), option.name.clone()))
            .collect(),
    ))
}

fn on_path(binary: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let candidate = dir.join(binary);
                candidate.is_file() || dir.join(format!("{binary}.exe")).is_file()
            })
        })
        .unwrap_or(false)
}

/// A free-text answer, with `default` offered by pressing Enter.
fn ask_text(term: &mut Terminal<'_>, label: &str, default: Option<&str>) -> Result<String> {
    loop {
        match default {
            Some(default) => write!(
                term.out,
                "{} ",
                term.style.cyan(&format!("{label} [{default}]:"))
            )?,
            None => write!(term.out, "{} ", term.style.cyan(&format!("{label}:")))?,
        }
        term.out.flush()?;
        let mut line = String::new();
        let read = term.input.read_line(&mut line)?;
        let answer = line.trim();
        if read == 0 {
            // Piped input that ran out: fall back rather than loop forever.
            return Ok(default.unwrap_or_default().to_string());
        }
        if answer.is_empty() {
            if let Some(default) = default {
                return Ok(default.to_string());
            }
            writeln!(term.out, "{}", term.style.yellow("needed"))?;
            continue;
        }
        return Ok(answer.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_we_tested_is_the_one_that_is_offered() {
        // If this list ever grows, each entry has to have been run end to end.
        assert_eq!(KNOWN_AGENTS.len(), 1);
        assert_eq!(KNOWN_AGENTS[0].1, "opencode acp");
    }
}
