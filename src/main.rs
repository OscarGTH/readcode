use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};

use readcode::corpus::Corpus;
use readcode::review::{Reviewer, acp::AcpReviewer, api::ApiReviewer};
use readcode::session;
use readcode::snippet::Asks;
use readcode::srs::{self, ConceptState};
use readcode::store::{self, Progress};
use readcode::ui::{self, Style};

#[derive(Parser, Debug)]
#[command(
    name = "readcode",
    version,
    about = "Learn a language by annotating real code, then get graded on it.",
    long_about = "readcode shows you real code block by block or line by line, asks you to explain \
each part, and grades your annotations. Concepts you miss come back sooner. Grading runs on a \
BYOK API key or through your own coding agent over ACP."
)]
struct Cli {
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

    /// Ask block by block, whatever the corpus prefers.
    #[arg(long, global = true, conflicts_with = "lines")]
    blocks: bool,

    /// Ask line by line, whatever the corpus prefers.
    #[arg(long, global = true)]
    lines: bool,

    /// Disable ANSI colour.
    #[arg(long, global = true)]
    no_color: bool,

    /// Use the plain line-by-line view instead of the full-screen one.
    #[arg(long, global = true)]
    no_tui: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Backend {
    /// OpenAI- or Anthropic-compatible endpoint with your own key (BYOK).
    Api,
    /// Your own coding agent over the Agent Client Protocol (`opencode acp`).
    Acp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum EvalTarget {
    /// Deterministic mock: answers from the case's own expectations, so it only
    /// proves the harness.
    Mock,
    /// A BYOK API key.
    Api,
    /// Your own agent over ACP (spends tokens).
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
    /// List the models the configured agent offers, and which one it is on.
    Models,
    /// Run the grading eval suite (see evals/).
    Eval {
        /// Which reviewer to measure (default: the deterministic mock).
        #[arg(long, value_enum, default_value_t = EvalTarget::Mock)]
        target: EvalTarget,
        /// Case directory (default: `evals/cases` here, else the cases compiled in).
        #[arg(long, value_name = "DIR")]
        cases: Option<PathBuf>,
        /// Only run cases whose id or tag contains this.
        #[arg(long, value_name = "NEEDLE")]
        filter: Option<String>,
        /// Trials per case; the majority decides.
        #[arg(long, default_value_t = 1)]
        trials: u32,
        /// Fail when the case score is below this.
        #[arg(long, default_value_t = 0.0)]
        min_score: f64,
        /// Where run artifacts are written.
        #[arg(long, value_name = "DIR", default_value = "evals/runs")]
        runs: PathBuf,
        /// Do not write a run directory.
        #[arg(long)]
        no_save: bool,
    },
    /// Draft a corpus entry from a file, using the reviewer as an author.
    Draft {
        /// The file to turn into a snippet.
        path: PathBuf,
        /// Only this span of the file, 1-based (`--span 12-24`).
        #[arg(long, value_name = "A-B")]
        span: Option<String>,
        /// The level the snippet is written for (1-5); inferred as 2 if omitted.
        #[arg(long, value_name = "N")]
        level: Option<u8>,
        /// Where drafts are written with `--write` (default: ./snippets).
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
        /// Write the file instead of printing it.
        #[arg(long)]
        write: bool,
    },
    /// Show or set the level you are reading at (1 = just started, 5 = fluent).
    Level {
        /// The new level, 1 to 5. Omit to see the current one.
        level: Option<u8>,
    },
    /// Delete saved progress.
    Reset {
        /// Required: this is irreversible.
        #[arg(long)]
        yes: bool,
    },
}

/// The numbers behind `readcode status`, gathered where both the corpus and the
/// progress are in hand.
fn progress_view<'a>(corpus: &Corpus, progress: &Progress, now: u64) -> ui::ProgressView<'a> {
    use std::collections::BTreeSet;

    let level = progress.level;
    let mut at_level: BTreeSet<String> = BTreeSet::new();
    let mut new_at_level = 0usize;
    for snippet in corpus.snippets() {
        let in_level = level.is_none_or(|level| snippet.levels().from <= level);
        if !in_level {
            continue;
        }
        at_level.extend(snippet.concepts());
        if !progress.seen(&snippet.id) {
            new_at_level += 1;
        }
    }

    let solid_at_level = at_level
        .iter()
        .filter(|concept| {
            progress
                .concepts
                .get(*concept)
                .is_some_and(|state| state.is_mastered())
        })
        .count();

    let mut needs_work: Vec<ConceptState> = progress
        .concepts
        .values()
        .filter(|state| !state.is_mastered() || (state.due_at != 0 && state.due_at <= now))
        .cloned()
        .collect();
    needs_work.sort_by(|a, b| b.urgency(now).total_cmp(&a.urgency(now)));

    ui::ProgressView {
        level,
        level_name: level
            .map(|level| readcode::snippet::LEVELS[level as usize - 1])
            .unwrap_or(""),
        solid_at_level,
        total_at_level: at_level.len(),
        strength_at_level: at_level
            .iter()
            .map(|concept| {
                progress
                    .concepts
                    .get(concept)
                    .map(|state| state.strength)
                    .unwrap_or(0.0)
            })
            .sum(),
        snippets_seen: progress.seen_snippets.len(),
        snippets_total: corpus.snippets().len(),
        new_at_level,
        due_now: progress
            .concepts
            .values()
            .filter(|state| state.due_at != 0 && state.due_at <= now)
            .count(),
        next_due: progress
            .concepts
            .values()
            .filter(|state| state.due_at > now)
            .map(|state| state.due_at)
            .min(),
        needs_work,
        solid: progress
            .concepts
            .values()
            .filter(|state| state.is_mastered())
            .count(),
    }
}

/// `--lines 12-24` (or `--lines 12`) as a 1-based slice of a file.
fn slice_lines(source: &str, span: &str) -> Result<String> {
    let (start, end) = match span.split_once('-') {
        Some((start, end)) => (start.trim().parse::<usize>()?, end.trim().parse::<usize>()?),
        None => {
            let only = span.trim().parse::<usize>()?;
            (only, only)
        }
    };
    if start == 0 || start > end {
        bail!("`--lines {span}` is not a range — try `--lines 12-24`");
    }
    let lines: Vec<&str> = source.lines().collect();
    if end > lines.len() {
        bail!(
            "`--lines {span}` runs past the end of the file, which has {} lines",
            lines.len()
        );
    }
    Ok(lines[start - 1..end].join("\n"))
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
            let mut input = io::stdin().lock();
            let mut out = io::stdout();

            // The level decides what the picker may offer, so it is settled
            // before the first snippet is chosen. Naming a snippet yourself
            // answers the question the level would have answered.
            if progress.level.is_none() && cli.snippet.is_none() {
                let level = {
                    let mut term = session::Terminal {
                        input: &mut input,
                        out: &mut out,
                        style,
                    };
                    session::ask_level(&mut term)?
                };
                progress.level = Some(level);
                progress.save(&progress_path)?;
            }

            let chosen = match &cli.snippet {
                Some(needle) => corpus.find(needle)?,
                None => session::pick(&corpus, &progress, now)
                    .context("the corpus is empty — `--corpus <dir>` to load some")?,
            };

            let backend = choose_backend(&cli).ok_or_else(no_reviewer)?;
            let agent_cwd = data_dir.join("agent");
            std::fs::create_dir_all(&agent_cwd).ok();
            let reviewer = build_reviewer(&cli, backend, agent_cwd)?;

            let mode = if cli.lines {
                Asks::Lines
            } else if cli.blocks {
                Asks::Blocks
            } else {
                session::choose_mode(chosen, &progress)
            };

            // Full screen when there is a screen to fill, plain lines when the
            // output is being piped somewhere.
            let interactive =
                !cli.no_tui && io::stdin().is_terminal() && io::stdout().is_terminal();
            let outcome = {
                let mut term = session::Terminal {
                    input: &mut input,
                    out: &mut out,
                    style,
                };
                if interactive {
                    session::run_tui(
                        chosen,
                        mode,
                        reviewer.as_ref(),
                        &mut progress,
                        now,
                        &mut term,
                    )?
                } else {
                    session::run(
                        chosen,
                        mode,
                        reviewer.as_ref(),
                        &mut progress,
                        now,
                        &mut term,
                    )?
                }
            };
            if outcome.is_some() {
                progress.save(&progress_path)?;
                let view = progress_view(&corpus, &progress, now);
                if view.total_at_level > 0 {
                    writeln!(
                        out,
                        "{}",
                        style.dim(&format!(
                            "level {} · {} of {} solid at this level",
                            progress.level.unwrap_or(1),
                            view.solid_at_level,
                            view.total_at_level
                        ))
                    )?;
                }
                if let Some(advice) = session::level_advice(&corpus, &progress) {
                    writeln!(out, "{}", style.dim(&advice))?;
                }
            }
            Ok(())
        }

        Command::Draft {
            path,
            span,
            level,
            out,
            write,
        } => {
            let mut out_writer = io::stdout();
            let source = std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            let code = match &span {
                Some(span) => slice_lines(&source, span)?,
                None => source.clone(),
            };
            if code.trim().is_empty() {
                bail!("{} has nothing to draft from", path.display());
            }

            let language = readcode::draft::language_for(&path, &corpus.languages());
            let level = level.unwrap_or(2);
            if !(1..=readcode::snippet::MAX_LEVEL).contains(&level) {
                bail!(
                    "{} — {level} is not one of them",
                    readcode::draft::level_help()
                );
            }

            let backend = choose_backend(&cli).ok_or_else(no_reviewer)?;
            let agent_cwd = data_dir.join("agent");
            std::fs::create_dir_all(&agent_cwd).ok();
            let reviewer = build_reviewer(&cli, backend, agent_cwd)?;

            let known: Vec<String> = corpus
                .snippets()
                .iter()
                .flat_map(|snippet| snippet.concepts())
                .collect();
            let id = readcode::draft::next_id(
                corpus.snippets().iter().map(|snippet| snippet.id.clone()),
                &language,
            );
            let task = readcode::draft::prompt(&code, &language, path.to_str(), level, &known);

            writeln!(
                out_writer,
                "{}",
                style.dim(&format!(
                    "drafting {id} from {} ({language}, level {level}) with {}",
                    path.display(),
                    reviewer.name()
                ))
            )?;

            let reply = reviewer
                .ask(&task)
                .with_context(|| format!("{} could not draft this snippet", reviewer.name()))?;
            let toml = readcode::draft::from_reply(&reply, &id, &language, Some(level))?;

            if write {
                let root = out.unwrap_or_else(|| PathBuf::from("snippets"));
                // TOML ignores the leading comment, so the draft parses as-is.
                let title = toml::from_str::<toml::Value>(&toml)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("title")
                            .and_then(|title| title.as_str())
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| id.clone());
                let dir = root.join(&language);
                std::fs::create_dir_all(&dir)?;
                let file = dir.join(format!("{id}-{}.toml", readcode::draft::slug(&title)));
                std::fs::write(&file, &toml)?;
                writeln!(
                    out_writer,
                    "{}",
                    style.bold(&format!("wrote {}", file.display()))
                )?;
                writeln!(
                    out_writer,
                    "{}",
                    style.dim(&format!(
                        "read it, fix the asks and notes, then: readcode --corpus {} check",
                        root.display()
                    ))
                )?;
            } else {
                writeln!(out_writer)?;
                writeln!(out_writer, "{toml}")?;
                writeln!(
                    out_writer,
                    "{}",
                    style.dim("add --write --out <dir> to keep it; read it before you trust it")
                )?;
            }
            Ok(())
        }

        Command::Level { level } => {
            let mut out = io::stdout();
            match level {
                Some(level) => {
                    if !(1..=readcode::snippet::MAX_LEVEL).contains(&level) {
                        bail!(
                            "levels run 1 to {} — {level} is not one of them",
                            readcode::snippet::MAX_LEVEL
                        );
                    }
                    progress.level = Some(level);
                    progress.save(&progress_path)?;
                    writeln!(
                        out,
                        "{}",
                        style.bold(&format!(
                            "level {level} · {}",
                            readcode::snippet::LEVELS[level as usize - 1]
                        ))
                    )?;
                    writeln!(
                        out,
                        "{}",
                        style.dim(
                            "`readcode` now offers snippets at this level, plus one rung above"
                        )
                    )?;
                }
                None => {
                    writeln!(out, "{}", style.bold("how much Rust can you read?"))?;
                    for (index, description) in readcode::snippet::LEVELS.iter().enumerate() {
                        let number = index as u8 + 1;
                        let here = progress.level == Some(number);
                        writeln!(
                            out,
                            "  {} {} {}",
                            if here {
                                style.cyan("→")
                            } else {
                                " ".to_string()
                            },
                            style.dim(&format!("{number} ·")),
                            description
                        )?;
                    }
                    writeln!(out)?;
                    match progress.level {
                        Some(level) => writeln!(
                            out,
                            "{}",
                            style.dim(&format!("set to {level} — `readcode level <1-5>` to change it"))
                        )?,
                        None => writeln!(
                            out,
                            "{}",
                            style.dim("not set yet — `readcode level <1-5>` sets it, or just run `readcode`")
                        )?,
                    }
                }
            }
            Ok(())
        }

        Command::Status => {
            let mut out = io::stdout();
            let view = progress_view(&corpus, &progress, now);
            ui::print_progress(&mut out, style, &view, now)?;
            if let Some(advice) = session::level_advice(&corpus, &progress) {
                writeln!(out)?;
                writeln!(out, "{}", style.cyan(&advice))?;
            }
            if let Some(next) = session::pick(&corpus, &progress, now) {
                writeln!(out)?;
                writeln!(
                    out,
                    "{}",
                    style.dim(&format!("next · {} {}", next.id, next.title))
                )?;
            }
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
                let mode = snippet.resolve_mode(snippet.asks);
                writeln!(
                    out,
                    "{}  {:<5} {:<46} {:<9} {} blocks · {} concepts · asks {:<6} · {seen}",
                    style.bold(&snippet.id),
                    snippet.language,
                    snippet.title,
                    snippet.levels().label(),
                    snippet.blocks.len(),
                    snippet.concepts().len(),
                    mode.label()
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

        Command::Models => {
            let agent_cwd = data_dir.join("agent");
            std::fs::create_dir_all(&agent_cwd).ok();
            let reviewer = AcpReviewer::from_env(agent_cwd);
            let models = reviewer
                .list_models()
                .context("cannot ask the agent for its models")?;

            println!("{}", style.bold(&format!("agent · {}", models.agent)));
            println!(
                "{}",
                style.dim(&format!(
                    "current · {}",
                    if models.current.is_empty() {
                        "(none reported)"
                    } else {
                        &models.current
                    }
                ))
            );
            println!();
            for option in &models.options {
                let mark = if option.value == models.current {
                    style.green("*")
                } else {
                    " ".to_string()
                };
                let free = if readcode::review::acp::is_free_model(&option.value, &option.name) {
                    style.dim("  free tier")
                } else {
                    String::new()
                };
                println!("{mark} {:<42} {}{free}", option.value, option.name);
            }
            println!();
            println!(
                "{}",
                style.dim("pin one with --model <id>, or READCODE_MODEL=<id>")
            );
            Ok(())
        }

        Command::Eval {
            target,
            cases,
            filter,
            trials,
            min_score,
            runs,
            no_save,
        } => {
            let mut suite = readcode::eval::load(cases.as_deref())?;
            if let Some(needle) = &filter {
                suite.retain(|case| {
                    case.id.contains(needle.as_str())
                        || case.tags.iter().any(|tag| tag.contains(needle.as_str()))
                });
                if suite.is_empty() {
                    bail!("no case matches `{needle}`");
                }
            }

            let agent_cwd = data_dir.join("agent");
            std::fs::create_dir_all(&agent_cwd).ok();

            let mut summary = match target {
                EvalTarget::Mock => readcode::eval::run_suite_mock(&corpus, &suite, trials, false)?,
                EvalTarget::Api => {
                    let reviewer = build_reviewer(&cli, Backend::Api, agent_cwd)?;
                    readcode::eval::run_suite(&corpus, &suite, reviewer.as_ref(), trials)?
                }
                EvalTarget::Acp => {
                    let reviewer = build_reviewer(&cli, Backend::Acp, agent_cwd)?;
                    readcode::eval::run_suite(&corpus, &suite, reviewer.as_ref(), trials)?
                }
            };

            if !no_save {
                summary.dir = Some(readcode::eval::write_run(&summary, &runs)?);
            }

            println!("{}", summary.report());

            let must_fail = summary.must_pass_failures();
            if !must_fail.is_empty() {
                bail!(
                    "{} must-pass case(s) failed: {}",
                    must_fail.len(),
                    must_fail.join(", ")
                );
            }
            if summary.score() < min_score {
                bail!(
                    "score {:.2} is below --min-score {:.2}",
                    summary.score(),
                    min_score
                );
            }
            Ok(())
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
/// There is no offline fallback — grading needs a model.
fn choose_backend(cli: &Cli) -> Option<Backend> {
    if let Some(requested) = cli.reviewer {
        return Some(requested);
    }
    match std::env::var("READCODE_REVIEWER")
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("api") | Some("byok") | Some("openai") | Some("anthropic") => {
            return Some(Backend::Api);
        }
        Some("acp") | Some("agent") => return Some(Backend::Acp),
        Some(other) if !other.is_empty() => {
            eprintln!("readcode: unknown READCODE_REVIEWER `{other}`, choosing automatically")
        }
        _ => {}
    }
    let agent_configured = std::env::var("READCODE_AGENT")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    if agent_configured {
        return Some(Backend::Acp);
    }
    if ApiReviewer::from_env().is_some() {
        return Some(Backend::Api);
    }
    None
}

/// Neither configured: say exactly what to set, once, and stop. A grader that
/// silently degrades to something worse than a model is not worth shipping.
fn no_reviewer() -> anyhow::Error {
    anyhow::anyhow!(
        "no reviewer configured, and grading needs one.\n\n\
         Your own agent (no API key):   export READCODE_AGENT=\"opencode acp\"\n\
         An OpenAI-compatible key:      export READCODE_API_KEY=sk-...\n\
         An Anthropic-compatible key:   export READCODE_API_KEY=... READCODE_API_STYLE=anthropic\n\n\
         Or pass --reviewer acp / --reviewer api. `status`, `list` and `check` work without one."
    )
}

fn build_reviewer(cli: &Cli, backend: Backend, agent_cwd: PathBuf) -> Result<Box<dyn Reviewer>> {
    match backend {
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
