//! Run a shell command.
//!
//! This is the tool that does the work and the tool that can do damage. It runs
//! from the workspace root with a timeout, but a shell can `cd` out of any
//! directory it likes — there is no sandbox here. Bounding what it may run is
//! S5's job.

use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::tool::{Observation, Tool, ToolError};
use crate::workspace::Workspace;

const DEFAULT_TIMEOUT_SECS: u64 = 10;
const MAX_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Args {
    /// Command line, passed to `sh -c`.
    pub command: String,
    /// Seconds before the command is stopped. Defaults to 10.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

pub struct Bash;

#[async_trait]
impl Tool for Bash {
    type Args = Args;

    const NAME: &'static str = "bash";
    const DESCRIPTION: &'static str =
        "Run a shell command with `sh -c` from the workspace root and return its exit code and output. Not sandboxed.";

    async fn run(&self, args: Args, workspace: &Workspace) -> Result<Observation, ToolError> {
        let secs = args
            .timeout_secs
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, MAX_TIMEOUT_SECS);

        let child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&args.command)
            .current_dir(workspace.root())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| ToolError::Failed(format!("cannot start a shell: {err}")))?;

        let output = match tokio::time::timeout(
            Duration::from_secs(secs),
            child.wait_with_output(),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => return Err(ToolError::Failed(format!("command did not run: {err}"))),
            Err(_) => {
                return Err(ToolError::Failed(format!(
                    "command timed out after {secs}s and was stopped"
                )))
            }
        };

        let mut body = format!("exit {}\n", output.status.code().unwrap_or(-1));
        body.push_str(&String::from_utf8_lossy(&output.stdout));
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            body.push_str("\n[stderr]\n");
            body.push_str(&stderr);
        }

        Ok(Observation {
            ok: output.status.success(),
            text: body,
        })
    }
}
