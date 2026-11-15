//! The CLI: one task in, an answer or an honest failure out.
//!
//! The model gets a workspace and four tools. It may take several turns; the
//! ceiling counts model calls, so asking for four files is still one turn.
//! Nothing is placed in the workspace by this program — every change is one the
//! model asked for and a tool performed.

use std::path::PathBuf;
use std::process::ExitCode;

use harness::agent::{self, Limits};
use harness::tools;
use harness::{Client, Config, Request, Workspace};

const SYSTEM: &str = "You work inside a small directory. Use the tools to look around \
and to change things. Answer in as few words as possible.";

const USAGE: &str = "usage: harness [--max-turns N] [--workspace DIR] <task…>

Asks one model one task. The model may list, read, write and run commands in the
workspace, for up to N model calls (default 8).";

#[tokio::main]
async fn main() -> ExitCode {
    dotenvy::dotenv().ok();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("-h" | "--help")) {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    match run(&args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

struct Options {
    task: String,
    workspace: PathBuf,
    max_turns: u32,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut workspace = PathBuf::from(".");
    let mut max_turns = Limits::default().max_turns;
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
            word => task.push(word.to_string()),
        }
    }

    if task.iter().all(|word| word.trim().is_empty()) {
        return Err(USAGE.to_string());
    }
    Ok(Options {
        task: task.join(" "),
        workspace,
        max_turns,
    })
}

async fn run(args: &[String]) -> Result<(), String> {
    let options = parse(args)?;
    let config = Config::from_env().map_err(|err| err.to_string())?;
    let request = Request::new(config.model(), SYSTEM, &options.task).map_err(|err| err.to_string())?;
    let workspace = Workspace::open(&options.workspace).map_err(|err| err.to_string())?;
    let registry = tools::builtin();
    let client = Client::new(config)
        .map_err(|err| err.to_string())?
        .with_tools(registry.specs());

    let limits = Limits {
        max_turns: options.max_turns,
        ..Limits::default()
    };
    let outcome = agent::run(&client, &registry, &workspace, request, limits)
        .await
        .map_err(|err| err.to_string())?;

    println!("{}", outcome.answer);
    println!(
        "\ntokens {} in · {} out · {} total · {} turn(s)",
        outcome.usage.input(),
        outcome.usage.output(),
        outcome.usage.total(),
        outcome.turns
    );
    Ok(())
}
