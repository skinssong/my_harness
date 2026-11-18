//! Run a shell command.
//!
//! This is the tool that does the work and the tool that can do damage. It runs
//! from the workspace root with a timeout, but a shell can `cd` out of any
//! directory it likes — there is no sandbox here. Bounding what it may run is
//! S5's job.
//!
//! Two bounds are the harness's own. Output is captured into a fixed window
//! rather than buffered whole, so a command that prints without end costs a
//! bounded amount of memory. And the wait is on the shell, not on its pipes:
//! a background child inherits them and can hold them open long after `sh` has
//! exited, which would otherwise be reported as a timeout on a command that
//! succeeded.

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::task::JoinHandle;

use crate::tool::{Observation, Tool, ToolError};
use crate::workspace::Workspace;

const DEFAULT_TIMEOUT_SECS: u64 = 10;
const MAX_TIMEOUT_SECS: u64 = 120;

/// What one stream may keep. The observation is shaped to 8 KiB afterwards; this
/// bound is about the harness's memory, not the size of a request.
const CAPTURE: usize = 32 * 1024;

/// How long to wait for a pipe to close once the shell has exited. A
/// background child may never close it, so this is a grace period, not a wait.
const DRAIN_GRACE: Duration = Duration::from_millis(50);

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Args {
    /// Command line, passed to `sh -c`.
    pub command: String,
    /// Seconds before the command is stopped. Defaults to 10.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

pub struct Bash;

/// The first and last `CAPTURE` bytes of a stream, with the count of what fell
/// between them. Keeping both ends is what makes a truncated failure readable:
/// the head says what was attempted, the tail says how it ended.
#[derive(Default)]
struct Capture {
    head: Vec<u8>,
    tail: Vec<u8>,
    total: usize,
}

impl Capture {
    fn push(&mut self, chunk: &[u8]) {
        self.total += chunk.len();
        let room = CAPTURE.saturating_sub(self.head.len());
        let keep = room.min(chunk.len());
        self.head.extend_from_slice(&chunk[..keep]);
        self.tail.extend_from_slice(&chunk[keep..]);
        // Trim only once the tail is twice its budget, so trimming is
        // amortised rather than once per read.
        if self.tail.len() >= CAPTURE * 2 {
            let excess = self.tail.len() - CAPTURE;
            self.tail.drain(..excess);
        }
    }

    fn into_text(self) -> String {
        let held = self.head.len() + self.tail.len();
        let mut text = String::from_utf8_lossy(&self.head).into_owned();
        if self.total > held {
            text.push_str(&format!("\n[{} bytes elided]\n", self.total - held));
        }
        text.push_str(&String::from_utf8_lossy(&self.tail));
        text
    }
}

async fn pump<R: AsyncRead + Unpin>(mut source: R, sink: Arc<Mutex<Capture>>) {
    let mut chunk = [0u8; 8 * 1024];
    loop {
        match source.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if let Ok(mut capture) = sink.lock() {
                    capture.push(&chunk[..read]);
                }
            }
        }
    }
}

/// Take what a pump collected. If the pump is still holding the lock, this
/// waits for it — the callers run only after the shell has exited.
fn take(cell: &Arc<Mutex<Capture>>) -> String {
    let mut guard = cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    std::mem::take(&mut *guard).into_text()
}

/// Let a reader finish if the pipe closes promptly; stop it if it does not.
async fn settle(handle: Option<JoinHandle<()>>) {
    let Some(mut handle) = handle else { return };
    if tokio::time::timeout(DRAIN_GRACE, &mut handle)
        .await
        .is_err()
    {
        handle.abort();
    }
}

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

        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&args.command)
            .current_dir(workspace.root())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| ToolError::Failed(format!("cannot start a shell: {err}")))?;

        let out = Arc::new(Mutex::new(Capture::default()));
        let err = Arc::new(Mutex::new(Capture::default()));
        let stdout = child
            .stdout
            .take()
            .map(|stream| tokio::spawn(pump(stream, out.clone())));
        let stderr = child
            .stderr
            .take()
            .map(|stream| tokio::spawn(pump(stream, err.clone())));

        // Wait for the shell, not for EOF on pipes its children may hold.
        let status = match tokio::time::timeout(Duration::from_secs(secs), child.wait()).await {
            Ok(Ok(status)) => Some(status),
            Ok(Err(err)) => {
                settle(stdout).await;
                settle(stderr).await;
                return Err(ToolError::Failed(format!("the shell failed: {err}")));
            }
            Err(_) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                None
            }
        };

        settle(stdout).await;
        settle(stderr).await;

        let Some(status) = status else {
            return Ok(Observation::failed(format!(
                "timed out after {secs}s and was stopped"
            )));
        };

        let mut body = format!("exit {}\n", status.code().unwrap_or(-1));
        body.push_str(&take(&out));
        let errors = take(&err);
        if !errors.trim().is_empty() {
            body.push_str("\n[stderr]\n");
            body.push_str(&errors);
        }
        Ok(Observation {
            ok: status.success(),
            text: body,
        })
    }
}
