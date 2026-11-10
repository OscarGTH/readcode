use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};

use readcode::corpus::Corpus;
use readcode::review::{Reviewer, acp::AcpReviewer, api::ApiReviewer, local::LocalReviewer};
use readcode::session;
use readcode::srs::{self, ConceptState};
use readcode::store::{self, Progress};
use readcode::ui::{self, Style};

#[derive(Parser, Debug)]
#[command(
    name = "readcode",
    version,
    about = "Learn a language by annotating real code, then get graded on it.",
    long_about = "readcode shows you real code block by block, asks you to explain each block, and \
grades your annotations. Concepts you miss come back sooner. Grading runs offline (keyword \
rubric), against a BYOK API key, or through your own coding agent over ACP."
)]
struct Cli {
    /// Grade offline with the keyword rubric, even when a key or agent exists.
    #[arg(long, global = true)]
    offline: bool,

    /// Which reviewer grades you (default: auto-detect).
    #[arg(long, global = true, value_enum)]
    reviewer: Option<Backend>,

    /// Model to ask the reviewer for (api and acp backends).
    #[arg(long, global = true)]
    model: Option<String>,

    /// Progress directory (default: $XDG_DATA_HOME/readcode).
    #[arg(long, global = true, value_name = "DIR")]
    data_dir: Option<PathBuf>,

    /// Load snippets from this directory instead of the built-in corpus.
    #[arg(long, global = true, value_name = "DIR")]
    corpus: Option<PathBuf>,

    /// Annotate a specific snippet (id, or an unambiguous suffix like `002`).
    #[arg(long, global = true, value_name = "ID")]
    snippet: Option<String>,

    /// Disable ANSI colour.
    #[arg(long, global = true)]
    no_color: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Backend {
    /// Deterministic keyword rubric, no model, no network.
    Local,
    /// OpenAI- or Anthropic-compatible endpoint with your own key (BYOK).
    Api,
    /// Your own coding agent over the Agent Client Protocol (`opencode acp`).
    Acp,
}

#[derive(Clone, Debug, Subcommand)]
enum Command {
    /// Annotate the snippet that is most due (default).
    Next,
    /// Show concept mastery and what is due.
    Status,
    /// List the snippets in the corpus.
    List,
    /// Validate the corpus: every line covered, ids unique, rubrics complete.
    Check,
    /// Delete saved progress.
    Reset {
        /// Required: this is irreversible.
        #[arg(long)]
        yes: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let style = Style::from_env(cli.no_color);
    let corpus = Corpus::load(cli.corpus.as_deref()).context("cannot load the snippet corpus")?;

    let data_dir = match &cli.data_dir {
        Some(dir) => dir.clone(),
        None => store::default_data_dir()?,
    };
    let progress_path = store::progress_path(&data_dir);
    let mut progress = Progress::load(&progress_path)?;
    let now = srs::now();

    match cli.command.clone().unwrap_or(Command::Next) {
        Command::Next => {
            let chosen = match &cli.snippet {
                Some(needle) => corpus.find(needle)?,
                None => session::pick(&corpus, &progress, now)
                    .context("the corpus is empty — `--corpus <dir>` to load some")?,
            };

            let backend = choose_backend(&cli);
            if backend == Backend::Local {
                eprintln!(
                    "{}",
                    style.dim(
                        "grading offline with the keyword rubric · `--reviewer acp` grades with your agent · `--reviewer api` with a key"
                    )
                );
            }

            let agent_cwd = data_dir.join("agent");
            std::fs::create_dir_all(&agent_cwd).ok();
            let reviewer = build_reviewer(&cli, backend, agent_cwd)?;

            let mut input = io::stdin().lock();
            let mut out = io::stdout();
            let outcome = session::run(
                chosen,
                reviewer.as_ref(),
                &mut progress,
                now,
                style,
                &mut input,
                &mut out,
            )?;
            if outcome.is_some() {
                progress.save(&progress_path)?;
            }
            Ok(())
        }

        Command::Status => {
            let mut out = io::stdout();
            let states: Vec<ConceptState> = progress.concepts.values().cloned().collect();
            let states = ui::order_for_status(states, now);
            ui::print_status(&mut out, style, &states, now)?;

            let mastered = states.iter().filter(|state| state.is_mastered()).count();
            let due = states
                .iter()
                .filter(|state| state.due_at != 0 && state.due_at <= now)
                .count();
            writeln!(out)?;
            writeln!(
                out,
                "{}",
                style.dim(&format!(
                    "{} sessions · {} of {} snippets seen · {} concepts, {mastered} mastered · {due} due now",
                    progress.sessions,
                    progress.seen_snippets.len(),
                    corpus.snippets().len(),
                    states.len()
                ))
            )?;
            Ok(())
        }

        Command::List => {
            let mut out = io::stdout();
            for snippet in corpus.snippets() {
                let seen = if progress.seen(&snippet.id) {
                    "seen"
                } else {
                    "new"
                };
                writeln!(
                    out,
                    "{}  {:<5} {:<52} {} blocks · {} concepts · {seen}",
                    style.bold(&snippet.id),
                    snippet.language,
                    snippet.title,
                    snippet.blocks.len(),
                    snippet.concepts().len()
                )?;
            }
            Ok(())
        }

        Command::Check => {
            let problems = corpus.problems();
            if problems.is_empty() {
                println!(
                    "{}",
                    style.green(&format!(
                        "ok — {} snippets, every line accounted for",
                        corpus.snippets().len()
                    ))
                );
                return Ok(());
            }
            for problem in &problems {
                println!("{} {problem}", style.red("✗"));
            }
            bail!("{} problem(s) in the corpus", problems.len());
        }

        Command::Reset { yes } => {
            if !yes {
                bail!(
                    "this deletes {} — re-run with --yes",
                    progress_path.display()
                );
            }
            match std::fs::remove_file(&progress_path) {
                Ok(()) => println!("progress cleared: {}", progress_path.display()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    println!("nothing to clear at {}", progress_path.display())
                }
                Err(error) => return Err(error).context("cannot delete progress"),
            }
            Ok(())
        }
    }
}

/// Explicit flag, then `READCODE_REVIEWER`, then auto-detect: a configured agent
/// beats an API key, because that is the choice the user already made once.
fn choose_backend(cli: &Cli) -> Backend {
    if cli.offline {
        return Backend::Local;
    }
    if let Some(requested) = cli.reviewer {
        return requested;
    }
    match std::env::var("READCODE_REVIEWER")
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("local") | Some("offline") => return Backend::Local,
        Some("api") | Some("byok") => return Backend::Api,
        Some("acp") | Some("agent") => return Backend::Acp,
        Some(other) if !other.is_empty() => {
            eprintln!("readcode: unknown READCODE_REVIEWER `{other}`, choosing automatically")
        }
        _ => {}
    }
    let agent_configured = std::env::var("READCODE_AGENT")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    if agent_configured {
        return Backend::Acp;
    }
    if ApiReviewer::from_env().is_some() {
        return Backend::Api;
    }
    Backend::Local
}

fn build_reviewer(cli: &Cli, backend: Backend, agent_cwd: PathBuf) -> Result<Box<dyn Reviewer>> {
    match backend {
        Backend::Local => Ok(Box::new(LocalReviewer)),
        Backend::Api => {
            let mut reviewer = ApiReviewer::from_env().context(
                "no API key found — set READCODE_API_KEY (or OPENAI_API_KEY / ANTHROPIC_API_KEY), or use --reviewer acp",
            )?;
            if let Some(model) = &cli.model {
                reviewer.model = model.clone();
            }
            Ok(Box::new(reviewer))
        }
        Backend::Acp => {
            let mut reviewer = AcpReviewer::from_env(agent_cwd);
            if let Some(model) = &cli.model {
                reviewer.model = Some(model.clone());
            }
            Ok(Box::new(reviewer))
        }
    }
}
