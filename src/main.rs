//! S1's CLI: one task in, one answer and its cost out.
//!
//! Thin by design. Everything worth testing already lives in the library; this
//! reads the environment and argv, makes one call, prints the report, and turns
//! a failure into a non-zero exit. The task is every argument joined, so a task
//! does not have to be quoted.

use std::process::ExitCode;

use harness::{Client, Config, Reply, Request};

const SYSTEM: &str = "Answer in as few words as possible.";

const USAGE: &str = "usage: harness <task…>

Asks one model one question and reports what it cost.";

#[tokio::main]
async fn main() -> ExitCode {
    dotenvy::dotenv().ok();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("-h" | "--help")) {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    match run(&args).await {
        Ok(Reply::Answer { .. }) => ExitCode::SUCCESS,
        Ok(reply) => {
            eprintln!("{}", reply.report());
            ExitCode::FAILURE
        }
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: &[String]) -> Result<Reply, String> {
    let task = task(args)?;
    let config = Config::from_env().map_err(|err| err.to_string())?;
    let request = Request::new(config.model(), SYSTEM, &task).map_err(|err| err.to_string())?;
    let client = Client::new(config).map_err(|err| err.to_string())?;
    let reply = client
        .complete(&request)
        .await
        .map_err(|err| err.to_string())?;
    if reply.is_answer() {
        println!("{}", reply.report());
    }
    Ok(reply)
}

fn task(args: &[String]) -> Result<String, String> {
    if args.iter().all(|arg| arg.trim().is_empty()) {
        return Err(USAGE.to_string());
    }
    Ok(args.join(" "))
}
