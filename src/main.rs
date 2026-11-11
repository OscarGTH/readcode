use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};

use readcode::config::Config;
use readcode::corpus::Corpus;
use readcode::review::{Reviewer, acp::AcpReviewer, api::ApiReviewer};
use readcode::session;
use readcode::snippet::Asks;
use readcode::srs;
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

    /// The language to study (the corpus may carry several).
    #[arg(long, global = true, value_name = "LANG")]
    language: Option<String>,

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
    /// Look at the grading results of past sittings.
    History {
        /// Which one to open, counting from the newest (1). Omit to list them.
        index: Option<usize>,
    },
    /// Set up, or change, who grades you: reviewer, agent, model.
    Setup,
    /// Show or switch the language you are reading in.
    Lang {
        /// The language to switch to, e.g. `go`. Omit to see them all.
        language: Option<String>,
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
    let mut config = Config::load(&data_dir);
    let now = srs::now();

    // `--language` is a flag for the whole program, not just for a sitting:
    // `status`, `level` and `pick` all have to agree about which language they
    // are talking about. It applies to this run only — `readcode lang <name>`
    // is what remembers the choice.
    if let Some(language) = &cli.language {
        let languages = corpus.languages();
        if !languages.contains(language) {
            bail!(
                "the corpus has no `{language}` — it has: {}",
                languages.join(", ")
            );
        }
        progress.language = Some(language.clone());
    }

    match cli.command.clone().unwrap_or(Command::Next) {
        Command::History { index } => {
            let mut out = io::stdout();
            let history = readcode::history::History::load(&data_dir);
            if history.sittings.is_empty() {
                writeln!(
                    out,
                    "{}",
                    style.dim("no sittings yet — `readcode` keeps one each time it grades you")
                )?;
                return Ok(());
            }
            match index {
                None => {
                    writeln!(
                        out,
                        "{}",
                        style.bold(&format!("history · {} sittings", history.sittings.len()))
                    )?;
                    for (position, sitting) in history.sittings.iter().enumerate() {
                        writeln!(
                            out,
                            "  {} {}  {}  {}",
                            style.cyan(&format!("{:>2}", position + 1)),
                            style.dim(&readcode::history::when(sitting.at, now)),
                            style.bold(&sitting.snippet),
                            style.dim(&readcode::history::summary(&sitting.grade))
                        )?;
                        writeln!(out, "     {}", style.dim(&sitting.title))?;
                    }
                    writeln!(
                        out,
                        "{}",
                        style.dim("`readcode history <n>` opens the grading page again")
                    )?;
                }
                Some(index) => {
                    let Some(sitting) = index
                        .checked_sub(1)
                        .and_then(|position| history.sittings.get(position))
                    else {
                        bail!(
                            "there is no sitting {index} — `readcode history` lists {} of them",
                            history.sittings.len()
                        );
                    };
                    let snippet = sitting.snippet();
                    let prompts = snippet.prompts(sitting.asks());
                    writeln!(
                        out,
                        "{}",
                        style.dim(&format!(
                            "{} · answered {}",
                            snippet.title,
                            readcode::history::when(sitting.at, now)
                        ))
                    )?;
                    let mut input = io::stdin().lock();
                    let mut term = session::Terminal {
                        input: &mut input,
                        out: &mut out,
                        style,
                    };
                    session::view_grade(
                        &snippet,
                        &prompts,
                        &sitting.grade,
                        &sitting.answers,
                        &mut term,
                    )?;
                }
            }
            Ok(())
        }

        Command::Setup => {
            let mut input = io::stdin().lock();
            let mut out = io::stdout();
            let mut term = session::Terminal {
                input: &mut input,
                out: &mut out,
                style,
            };
            let _ = readcode::setup::run(&mut term, &corpus, &data_dir)?;
            Ok(())
        }
        Command::Next => {
            let mut input = io::stdin().lock();
            let mut out = io::stdout();

            // A sitting left part-answered is worth more than a fresh pick, so it
            // is offered before the language and level questions: continuing is
            // the answer to all of them.
            let mut draft = store::Draft::load(&data_dir).unwrap_or_default();
            let mut resumed: Option<&readcode::snippet::Snippet> = None;
            let interactive_now =
                !cli.no_tui && io::stdin().is_terminal() && io::stdout().is_terminal();
            if interactive_now && !draft.answers.is_empty() {
                match corpus
                    .snippets()
                    .iter()
                    .find(|snippet| snippet.id == draft.snippet)
                {
                    Some(snippet) => {
                        let question = format!(
                            "you left `{}` with {} answer{} saved — continue?",
                            snippet.id,
                            draft.answered(),
                            if draft.answered() == 1 { "" } else { "s" }
                        );
                        let mut term = session::Terminal {
                            input: &mut input,
                            out: &mut out,
                            style,
                        };
                        if session::ask_yes(&mut term, &question)? {
                            resumed = Some(snippet);
                            progress.language = Some(snippet.language.clone());
                        } else {
                            store::Draft::clear(&data_dir);
                            draft = store::Draft::default();
                        }
                    }
                    None => {
                        // The snippet is gone from the corpus: nothing to resume.
                        store::Draft::clear(&data_dir);
                        draft = store::Draft::default();
                    }
                }
            }

            // Nobody reads a README before the first sitting. If there is no
            // reviewer configured and a person is at the keyboard, ask now rather
            // than failing at grading time, after every answer has been typed.
            if !config.grades() && cli.reviewer.is_none() && std::io::stdin().is_terminal() {
                let mut term = session::Terminal {
                    input: &mut input,
                    out: &mut out,
                    style,
                };
                config = readcode::setup::run(&mut term, &corpus, &data_dir)?;
            }

            // Language first — it decides which snippets and which level apply —
            // then that language's level.
            let languages = corpus.languages();
            if progress.language.is_none() {
                if languages.len() > 1 && cli.snippet.is_none() {
                    let mut term = session::Terminal {
                        input: &mut input,
                        out: &mut out,
                        style,
                    };
                    let language = session::ask_language(&mut term, &languages)?;
                    progress.language = Some(language);
                } else if let Some(only) = languages.first() {
                    progress.language = Some(only.clone());
                }
                progress.save(&progress_path)?;
            } else if cli.language.is_some() {
                // Chosen with the flag for a sitting: remember it. A run in a
                // language is a commitment in a way that `--language` on
                // `status` is not.
                progress.save(&progress_path)?;
            }

            let active = progress.language.clone();
            let needs_level = match &active {
                Some(language) => progress.level_of(language).is_none(),
                None => progress.level.is_none(),
            };
            if needs_level && cli.snippet.is_none() && resumed.is_none() {
                let level = {
                    let mut term = session::Terminal {
                        input: &mut input,
                        out: &mut out,
                        style,
                    };
                    session::ask_level(
                        &mut term,
                        active
                            .as_deref()
                            .or(corpus.languages().first().map(String::as_str))
                            .unwrap_or("rust"),
                    )?
                };
                match &active {
                    Some(language) => progress.set_level(language, level),
                    None => progress.level = Some(level),
                }
                progress.save(&progress_path)?;
            }

            let chosen = match resumed {
                Some(snippet) => snippet,
                None => match &cli.snippet {
                    Some(needle) => corpus.find(needle)?,
                    None => session::pick(&corpus, &progress, now)
                        .context("the corpus is empty — `--corpus <dir>` to load some")?,
                },
            };

            let backend = choose_backend(&cli, &config).ok_or_else(no_reviewer)?;
            let agent_cwd = data_dir.join("agent");
            std::fs::create_dir_all(&agent_cwd).ok();
            let reviewer = build_reviewer(&cli, backend, agent_cwd, &config)?;

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
                        &mut draft,
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
                // Graded: the draft has served its purpose.
                store::Draft::clear(&data_dir);
            } else if draft.answered() > 0 {
                draft.save(&data_dir)?;
            }
            if let Some(outcome) = &outcome {
                progress.save(&progress_path)?;
                // The grade is worth keeping: mastery keeps the number, this keeps
                // the page it was written on.
                let mut history = readcode::history::History::load(&data_dir);
                history.record(readcode::history::Sitting::new(
                    chosen,
                    outcome.mode,
                    outcome.grade.clone(),
                    outcome.answers.clone(),
                    now,
                ));
                history.save(&data_dir)?;
            }
            if outcome.is_some() {
                let view = session::progress_view(&corpus, &progress, now);
                if let Some(rung) = view
                    .ladder
                    .iter()
                    .find(|rung| Some(rung.level) == view.level)
                {
                    writeln!(
                        out,
                        "{}",
                        style.dim(&format!(
                            "level {} · {} of {} solid",
                            rung.level, rung.solid, rung.total
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

            let backend = choose_backend(&cli, &config).ok_or_else(no_reviewer)?;
            let agent_cwd = data_dir.join("agent");
            std::fs::create_dir_all(&agent_cwd).ok();
            let reviewer = build_reviewer(&cli, backend, agent_cwd, &config)?;

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
                // `draft` knows the language from the extension, so a Go file
                // lands in `snippets/go` beside the Rust ones rather than in the
                // root of the corpus.
                let root = out.unwrap_or_else(|| PathBuf::from("snippets").join(&language));
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

        Command::Lang { language } => {
            let mut out = io::stdout();
            let languages = corpus.languages();
            match language {
                Some(language) => {
                    let language = language.to_lowercase();
                    if !languages.contains(&language) {
                        bail!(
                            "the corpus has no `{language}` — it has: {}",
                            languages.join(", ")
                        );
                    }
                    progress.language = Some(language.clone());
                    progress.save(&progress_path)?;
                    writeln!(out, "{}", style.bold(&format!("reading {language}")))?;
                    match progress.level_of(&language) {
                        Some(level) => writeln!(
                            out,
                            "{}",
                            style.dim(&format!(
                                "level {level} · {}",
                                readcode::snippet::LEVELS[level as usize - 1]
                            ))
                        )?,
                        None => writeln!(
                            out,
                            "{}",
                            style.dim("no level for this language yet — `readcode` will ask")
                        )?,
                    }
                }
                None => {
                    writeln!(out, "{}", style.bold("languages"))?;
                    for language in &languages {
                        let all: Vec<&readcode::snippet::Snippet> = corpus
                            .snippets()
                            .iter()
                            .filter(|snippet| &snippet.language == language)
                            .collect();
                        let seen = all
                            .iter()
                            .filter(|snippet| progress.seen(&snippet.id))
                            .count();
                        let here = progress.active_language() == Some(language.as_str());
                        writeln!(
                            out,
                            "  {} {:<10} {} of {} snippets seen · {}",
                            if here {
                                style.cyan("→")
                            } else {
                                " ".to_string()
                            },
                            language,
                            seen,
                            all.len(),
                            match progress.level_of(language) {
                                Some(level) => format!("level {level}"),
                                None => "no level yet".to_string(),
                            }
                        )?;
                    }
                    writeln!(
                        out,
                        "{}",
                        style.dim("`readcode lang <name>` switches; each keeps its own progress")
                    )?;
                }
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
                    match progress.active_language().map(str::to_string) {
                        Some(language) => progress.set_level(&language, level),
                        None => progress.level = Some(level),
                    }
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
            let view = session::progress_view(&corpus, &progress, now);
            ui::print_progress(&mut out, style, &view)?;
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
                style.dim(
                    "pin one with --model <id> or READCODE_MODEL=<id>; readcode never grades with the agent's default"
                )
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
                    let reviewer =
                        build_reviewer(&cli, Backend::Api, agent_cwd, &Config::default())?;
                    readcode::eval::run_suite(&corpus, &suite, reviewer.as_ref(), trials)?
                }
                EvalTarget::Acp => {
                    let reviewer =
                        build_reviewer(&cli, Backend::Acp, agent_cwd, &Config::default())?;
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
fn choose_backend(cli: &Cli, config: &Config) -> Option<Backend> {
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
    // The settings file is the answer the user already gave to this question;
    // only a variable set in the current shell outranks it.
    match config.reviewer.as_deref() {
        Some("api") => return Some(Backend::Api),
        Some("acp") => return Some(Backend::Acp),
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

/// `--model` beats `READCODE_MODEL` beats the settings file. The env var is read
/// by the reviewers already; this only needs the two ends it cannot see.
fn effective_model(cli: &Cli, config: &Config) -> Option<String> {
    cli.model
        .clone()
        .or_else(|| {
            std::env::var("READCODE_MODEL")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| config.model.clone())
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

fn build_reviewer(
    cli: &Cli,
    backend: Backend,
    agent_cwd: PathBuf,
    config: &Config,
) -> Result<Box<dyn Reviewer>> {
    match backend {
        Backend::Api => {
            let mut reviewer = ApiReviewer::from_env().context(
                "no API key found — set READCODE_API_KEY (or OPENAI_API_KEY / ANTHROPIC_API_KEY), or use --reviewer acp",
            )?;
            reviewer.model = effective_model(cli, config).unwrap_or(reviewer.model);
            Ok(Box::new(reviewer))
        }
        Backend::Acp => {
            let mut reviewer = AcpReviewer::from_env(agent_cwd);
            // A command from the settings file, unless this shell says otherwise.
            if std::env::var("READCODE_AGENT").is_err()
                && let Some(agent) = &config.agent
            {
                reviewer.set_command(agent);
            }
            reviewer.model = effective_model(cli, config).or(reviewer.model);
            // Fail here, before the level question and before a single answer is
            // typed: the reviewer would refuse later anyway, and losing a sitting
            // to it is unforgivable.
            reviewer.require_model()?;
            reviewer.check()?;
            Ok(Box::new(reviewer))
        }
    }
}
