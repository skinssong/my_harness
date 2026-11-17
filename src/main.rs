//! The CLI: one task in, an answer or an honest ending out.
//!
//! The model gets a workspace and four tools. It may take several turns; the
//! ceiling counts model calls, so asking for four files is still one turn.
//! Nothing is placed in the workspace by this program — every change is one the
//! model asked for and a tool performed.
//!
//! Every run is recorded, `replay` reads a record back and checks the loop would
//! still make the same decisions, and `resume` answers a pause with a decision.
//!
//! Exit codes say which of the four endings happened, because a script should
//! not have to parse a sentence to tell "needs you" from "broke": 0 answered,
//! 1 stopped by a rule or failed, 3 waiting for a decision.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use harness::agent::{self, Approval, Ending, Limits};
use harness::log::Log;
use harness::replay;
use harness::tools;
use harness::{Client, Config, ContextPolicy, Policy, Request, Workspace};

const SYSTEM: &str = "You work inside a small directory. Use the tools to look around \
and to change things. Answer in as few words as possible.";

const USAGE: &str = "usage: harness [--max-turns N] [--workspace DIR] [--log DIR]
                  [--max-usd X] [--stuck-after N] [--context-window N]
                  [--keep-turns K] <task…>
       harness replay <record.jsonl>
       harness resume <record.jsonl> (--approve | --deny) <call_id>
                      [--workspace DIR] [--max-turns N] [--max-usd X]

Asks one model one task. The model may list, read, write and run commands in the
workspace, for up to N model calls (default 8). Every run is recorded under DIR
(default ~/.harness/runs, deliberately outside the workspace); replay re-derives a recorded run without calling
anything, and resume continues one that stopped for a decision.

  --log DIR        where runs are recorded (default ~/.harness/runs; a
                   directory inside the workspace lets the model rewrite its
                   own record, which is warned about)
  --max-usd X      stop once the run has cost more than X dollars; needs
                   LLM_PRICE_IN and LLM_PRICE_OUT (dollars per million tokens)
  --stuck-after N  stop when the same call produces the same result N more
                   times in a row (default 3; 0 disables)
  --context-window N  what the model accepts, in tokens (default 64000). Past
                   60% of it, old tool results are masked in the request; the
                   record is never masked
  --keep-turns K   turns kept verbatim at the end of the request (default 2)

Which tools may run unasked is LLM_TOOLS_ALLOW / LLM_TOOLS_ASK / LLM_TOOLS_DENY,
comma-separated names applied over the defaults (read and list allowed, write and
bash asked). Anything unlisted is refused.

exit 0 answered · 1 stopped by a rule, or failed · 3 waiting for a decision";

#[tokio::main]
async fn main() -> ExitCode {
    dotenvy::dotenv().ok();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("-h" | "--help")) {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    let outcome = match parse(&args) {
        Ok(Command::Replay(path)) => replay_record(&path),
        Ok(Command::Run(options)) => run(&options).await,
        Ok(Command::Resume(options, record, approval)) => {
            resume(&options, &record, approval).await
        }
        Err(message) => Err(message),
    };

    match outcome {
        Ok(code) => code,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

enum Command {
    Run(Options),
    Replay(PathBuf),
    Resume(Options, PathBuf, Approval),
}

struct Options {
    task: String,
    workspace: PathBuf,
    log_dir: PathBuf,
    max_turns: u32,
    max_usd: Option<f64>,
    stuck_after: u32,
    context_window: usize,
    keep_turns: usize,
}

#[derive(Default)]
struct Flags {
    workspace: Option<PathBuf>,
    log_dir: Option<PathBuf>,
    max_turns: Option<u32>,
    max_usd: Option<f64>,
    stuck_after: Option<u32>,
    context_window: Option<usize>,
    keep_turns: Option<usize>,
    approve: Option<String>,
    deny: Option<String>,
    words: Vec<String>,
}

impl Flags {
    fn options(&self, task: String) -> Options {
        let default = Limits::default();
        Options {
            task,
            workspace: self.workspace.clone().unwrap_or_else(|| PathBuf::from(".")),
            log_dir: self.log_dir.clone().unwrap_or_else(default_log_dir),
            max_turns: self.max_turns.unwrap_or(default.max_turns),
            max_usd: self.max_usd,
            stuck_after: self.stuck_after.unwrap_or(default.stuck_after),
            context_window: self.context_window.unwrap_or(default.context.window),
            keep_turns: self.keep_turns.unwrap_or(default.context.keep_turns),
        }
    }
}

fn parse(args: &[String]) -> Result<Command, String> {
    match args.first().map(String::as_str) {
        Some("replay") => {
            let path = args
                .get(1)
                .ok_or_else(|| format!("replay needs a record path\n\n{USAGE}"))?;
            return Ok(Command::Replay(PathBuf::from(path)));
        }
        Some("resume") => {
            let path = args
                .get(1)
                .ok_or_else(|| format!("resume needs a record path\n\n{USAGE}"))?;
            let flags = flags(&args[2..])?;
            let approval = flags.approval()?;
            if !flags.words.is_empty() {
                return Err(format!("resume takes no task text\n\n{USAGE}"));
            }
            return Ok(Command::Resume(
                flags.options(String::new()),
                PathBuf::from(path),
                approval,
            ));
        }
        _ => {}
    }

    let flags = flags(args)?;
    if flags.approve.is_some() || flags.deny.is_some() {
        return Err(format!(
            "--approve and --deny belong to resume\n\n{USAGE}"
        ));
    }
    if flags.words.iter().all(|word| word.trim().is_empty()) {
        return Err(USAGE.to_string());
    }
    Ok(Command::Run(flags.options(flags.words.join(" "))))
}

fn flags(args: &[String]) -> Result<Flags, String> {
    let mut flags = Flags::default();
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        let take = |name: &str| -> Result<String, String> {
            args.get(index + 1)
                .cloned()
                .ok_or_else(|| format!("{name} needs a value\n\n{USAGE}"))
        };
        match arg {
            "--max-turns" => {
                flags.max_turns = Some(number(&take("--max-turns")?, "--max-turns")?);
                index += 2;
            }
            "--stuck-after" => {
                flags.stuck_after = Some(number(&take("--stuck-after")?, "--stuck-after")?);
                index += 2;
            }
            "--context-window" => {
                let raw = take("--context-window")?;
                flags.context_window = Some(raw.parse().map_err(|_| {
                    format!("--context-window must be a whole number of tokens, got {raw}")
                })?);
                index += 2;
            }
            "--keep-turns" => {
                let raw = take("--keep-turns")?;
                flags.keep_turns = Some(
                    raw.parse()
                        .map_err(|_| format!("--keep-turns must be a whole number, got {raw}"))?,
                );
                index += 2;
            }
            "--max-usd" => {
                let raw = take("--max-usd")?;
                flags.max_usd = Some(
                    raw.parse()
                        .map_err(|_| format!("--max-usd must be a number, got {raw}"))?,
                );
                index += 2;
            }
            "--workspace" => {
                flags.workspace = Some(PathBuf::from(take("--workspace")?));
                index += 2;
            }
            "--log" => {
                flags.log_dir = Some(PathBuf::from(take("--log")?));
                index += 2;
            }
            "--approve" => {
                flags.approve = Some(take("--approve")?);
                index += 2;
            }
            "--deny" => {
                flags.deny = Some(take("--deny")?);
                index += 2;
            }
            word => {
                flags.words.push(word.to_string());
                index += 1;
            }
        }
    }
    Ok(flags)
}

fn number(raw: &str, flag: &str) -> Result<u32, String> {
    raw.parse()
        .map_err(|_| format!("{flag} must be a whole number, got {raw}"))
}

impl Flags {
    fn approval(&self) -> Result<Approval, String> {
        match (&self.approve, &self.deny) {
            (Some(id), None) => Ok(Approval {
                call_id: id.clone(),
                approved: true,
            }),
            (None, Some(id)) => Ok(Approval {
                call_id: id.clone(),
                approved: false,
            }),
            (Some(_), Some(_)) => Err(format!("--approve and --deny are exclusive\n\n{USAGE}")),
            (None, None) => Err(format!(
                "resume needs --approve <call_id> or --deny <call_id>\n\n{USAGE}"
            )),
        }
    }
}

/// The record lives outside the workspace by default, because a record the
/// agent can reach is not an audit trail: `write_file` and `bash` would both
/// reach `.harness/runs` if it sat next to the files being worked on.
fn default_log_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| PathBuf::from(home).join(".harness").join("runs"))
        .unwrap_or_else(|| PathBuf::from(".harness/runs"))
}

/// Warn when the record is somewhere the tools can reach it.
fn warn_if_record_is_reachable(record: &Path, workspace: &Workspace) {
    let Some(dir) = record.parent().and_then(|dir| dir.canonicalize().ok()) else {
        return;
    };
    if dir.starts_with(workspace.root()) {
        eprintln!(
            "warning: the record is inside the workspace ({}); the model can read and rewrite it",
            dir.display()
        );
    }
}

fn policy() -> Policy {
    Policy::from_lookup(|name| std::env::var(name).ok())
}

/// Everything both entry points need to construct: the client, the tools and
/// the limits.
fn setup(options: &Options) -> Result<(Client, Limits, Workspace), String> {
    let config = Config::from_env().map_err(|err| err.to_string())?;
    let workspace = Workspace::open(&options.workspace).map_err(|err| err.to_string())?;
    let registry = tools::builtin();
    let prices = config.prices();
    if options.max_usd.is_some() && prices.is_none() {
        return Err(
            "a money budget needs LLM_PRICE_IN and LLM_PRICE_OUT (dollars per million tokens)"
                .to_string(),
        );
    }
    let client = Client::new(config)
        .map_err(|err| err.to_string())?
        .with_tools(registry.specs());
    let limits = Limits {
        max_turns: options.max_turns,
        max_usd: options.max_usd,
        stuck_after: options.stuck_after,
        prices,
        context: ContextPolicy {
            window: options.context_window,
            keep_turns: options.keep_turns,
            ..ContextPolicy::default()
        },
        policy: policy(),
        ..Limits::default()
    };
    Ok((client, limits, workspace))
}

async fn run(options: &Options) -> Result<ExitCode, String> {
    let (client, limits, workspace) = setup(options)?;
    let request = Request::new(client.model(), SYSTEM, &options.task)
        .map_err(|err| err.to_string())?;
    let registry = tools::builtin();

    let mut log = Log::create(&options.log_dir).map_err(|err| err.to_string())?;
    let record = log.path().to_path_buf();
    warn_if_record_is_reachable(&record, &workspace);

    let ending = agent::run(&client, &registry, &workspace, request, &limits, &mut log)
        .await
        .map_err(|err| format!("{err}\nrecord: {}", record.display()))?;

    report(ending, &limits, &record)
}

async fn resume(
    options: &Options,
    record: &Path,
    approval: Approval,
) -> Result<ExitCode, String> {
    let (client, limits, workspace) = setup(options)?;
    let registry = tools::builtin();

    let events = Log::read(record).map_err(|err| err.to_string())?;
    let mut log = Log::open(record).map_err(|err| err.to_string())?;

    let ending = agent::resume(
        &client,
        &registry,
        &workspace,
        &events,
        &approval,
        &limits,
        &mut log,
    )
    .await
    .map_err(|err| format!("{err}\nrecord: {}", record.display()))?;

    report(ending, &limits, record)
}

/// Print what happened and say which exit code it earns.
fn report(ending: Ending, limits: &Limits, record: &Path) -> Result<ExitCode, String> {
    match ending {
        Ending::Answered(outcome) => {
            println!("{}", outcome.answer);
            let cost = match limits.spent_usd(outcome.usage) {
                Some(usd) => format!(" · ${usd:.6}"),
                None => String::new(),
            };
            println!(
                "\ntokens {} in · {} out · {} total · {} turn(s){}",
                outcome.usage.input(),
                outcome.usage.output(),
                outcome.usage.total(),
                outcome.turns,
                cost
            );
            println!("record {}", record.display());
            Ok(ExitCode::SUCCESS)
        }
        Ending::Halted(halt) => {
            eprintln!("stopped: {halt}");
            eprintln!("record {}", record.display());
            Ok(ExitCode::FAILURE)
        }
        Ending::Suspended(suspension) => {
            println!(
                "approval needed  {}  {}  {}",
                suspension.call.id, suspension.call.name, suspension.call.arguments
            );
            println!("record  {}", record.display());
            println!(
                "decide  harness resume {} --approve {}",
                record.display(),
                suspension.call.id
            );
            println!(
                "        harness resume {} --deny {}",
                record.display(),
                suspension.call.id
            );
            Ok(ExitCode::from(3))
        }
    }
}

fn replay_record(path: &PathBuf) -> Result<ExitCode, String> {
    let events = Log::read(path).map_err(|err| err.to_string())?;
    let replayed = replay::replay(&events).map_err(|err| err.to_string())?;

    match &replayed.answer {
        Some(answer) => println!("{answer}"),
        None => println!("(the recorded run did not answer)"),
    }
    println!(
        "\nreplayed {} turn(s) · {} tool result(s) · tokens {} in · {} out · {} total",
        replayed.turns,
        replayed.tools,
        replayed.usage.input(),
        replayed.usage.output(),
        replayed.usage.total()
    );
    Ok(ExitCode::SUCCESS)
}
