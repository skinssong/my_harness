//! The CLI: one task in, an answer or an honest failure out.
//!
//! The model gets a workspace and four tools. It may take several turns; the
//! ceiling counts model calls, so asking for four files is still one turn.
//! Nothing is placed in the workspace by this program — every change is one the
//! model asked for and a tool performed.
//!
//! Every run is recorded, and `replay` reads a record back and checks the loop
//! would still make the same decisions.

use std::path::PathBuf;
use std::process::ExitCode;

use harness::agent::{self, Limits};
use harness::log::Log;
use harness::replay;
use harness::tools;
use harness::{Client, Config, Request, Workspace};

const SYSTEM: &str = "You work inside a small directory. Use the tools to look around \
and to change things. Answer in as few words as possible.";

const USAGE: &str = "usage: harness [--max-turns N] [--workspace DIR] [--log DIR]
                  [--max-usd X] [--stuck-after N] <task…>
       harness replay <record.jsonl>

Asks one model one task. The model may list, read, write and run commands in the
workspace, for up to N model calls (default 8). Every run is recorded under DIR
(default ~/.harness/runs, deliberately outside the workspace); replay re-derives a recorded run without calling
anything.

  --log DIR        where runs are recorded (default ~/.harness/runs; a
                   directory inside the workspace lets the model rewrite its
                   own record, which is warned about)
  --max-usd X      stop once the run has cost more than X dollars; needs
                   LLM_PRICE_IN and LLM_PRICE_OUT (dollars per million tokens)
  --stuck-after N  stop when the same call produces the same result N times
                   in a row (default 3; 0 disables)";

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
        Err(message) => Err(message),
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

enum Command {
    Run(Options),
    Replay(PathBuf),
}

struct Options {
    task: String,
    workspace: PathBuf,
    log_dir: PathBuf,
    max_turns: u32,
    max_usd: Option<f64>,
    stuck_after: u32,
}

fn parse(args: &[String]) -> Result<Command, String> {
    if args.first().map(String::as_str) == Some("replay") {
        let path = args
            .get(1)
            .ok_or_else(|| format!("replay needs a record path\n\n{USAGE}"))?;
        return Ok(Command::Replay(PathBuf::from(path)));
    }

    let mut workspace = PathBuf::from(".");
    let mut log_dir = default_log_dir();
    let mut max_turns = Limits::default().max_turns;
    let mut max_usd = None;
    let mut stuck_after = Limits::default().stuck_after;
    let mut task: Vec<String> = Vec::new();

    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--max-turns" => {
                let value = rest
                    .next()
                    .ok_or_else(|| format!("--max-turns needs a number\n\n{USAGE}"))?;
                max_turns = value
                    .parse()
                    .map_err(|_| format!("--max-turns must be a whole number, got {value}"))?;
            }
            "--workspace" => {
                let value = rest
                    .next()
                    .ok_or_else(|| format!("--workspace needs a directory\n\n{USAGE}"))?;
                workspace = PathBuf::from(value);
            }
            "--max-usd" => {
                let value = rest
                    .next()
                    .ok_or_else(|| format!("--max-usd needs an amount\n\n{USAGE}"))?;
                max_usd = Some(
                    value
                        .parse()
                        .map_err(|_| format!("--max-usd must be a number, got {value}"))?,
                );
            }
            "--stuck-after" => {
                let value = rest
                    .next()
                    .ok_or_else(|| format!("--stuck-after needs a number\n\n{USAGE}"))?;
                stuck_after = value
                    .parse()
                    .map_err(|_| format!("--stuck-after must be a whole number, got {value}"))?;
            }
            "--log" => {
                let value = rest
                    .next()
                    .ok_or_else(|| format!("--log needs a directory\n\n{USAGE}"))?;
                log_dir = PathBuf::from(value);
            }
            word => task.push(word.to_string()),
        }
    }

    if task.iter().all(|word| word.trim().is_empty()) {
        return Err(USAGE.to_string());
    }
    Ok(Command::Run(Options {
        task: task.join(" "),
        workspace,
        log_dir,
        max_turns,
        max_usd,
        stuck_after,
    }))
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
fn warn_if_record_is_reachable(record: &std::path::Path, workspace: &Workspace) {
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

async fn run(options: &Options) -> Result<(), String> {
    let config = Config::from_env().map_err(|err| err.to_string())?;
    let request =
        Request::new(config.model(), SYSTEM, &options.task).map_err(|err| err.to_string())?;
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

    let mut log = Log::create(&options.log_dir).map_err(|err| err.to_string())?;
    let record = log.path().to_path_buf();
    warn_if_record_is_reachable(&record, &workspace);

    let limits = Limits {
        max_turns: options.max_turns,
        max_usd: options.max_usd,
        stuck_after: options.stuck_after,
        prices,
        ..Limits::default()
    };
    let outcome = agent::run(&client, &registry, &workspace, request, limits, &mut log)
        .await
        .map_err(|err| format!("{err}\nrecord: {}", record.display()))?;

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
    Ok(())
}

fn replay_record(path: &PathBuf) -> Result<(), String> {
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
    Ok(())
}
