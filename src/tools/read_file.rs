//! Read a text file.
//!
//! The file is streamed rather than loaded: lines before `offset` are skipped
//! without being kept, and reading stops once the window is full. The one bound
//! this does not give is the length of a single line — a file of one enormous
//! line is still read a line at a time.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};

use super::outside;
use crate::tool::{Observation, Tool, ToolError};
use crate::workspace::Workspace;

const DEFAULT_LINES: u32 = 2_000;

/// How much of the window is kept before reading stops. The observation is
/// shaped to 8 KiB afterwards, so this only has to bound the harness's memory.
const STORE_CAP: usize = 64 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Args {
    /// Path relative to the workspace root.
    pub path: String,
    /// First line to return, 1-based.
    #[serde(default)]
    pub offset: Option<u32>,
    /// Maximum number of lines to return.
    #[serde(default)]
    pub limit: Option<u32>,
}

pub struct ReadFile;

#[async_trait]
impl Tool for ReadFile {
    type Args = Args;

    const NAME: &'static str = "read_file";
    const DESCRIPTION: &'static str =
        "Read a UTF-8 text file from the workspace. Use offset and limit for large files.";

    async fn run(&self, args: Args, workspace: &Workspace) -> Result<Observation, ToolError> {
        let path = workspace.resolve(&args.path).map_err(outside)?;
        let file = tokio::fs::File::open(&path)
            .await
            .map_err(|err| ToolError::Failed(format!("cannot read {}: {err}", args.path)))?;

        let start = args.offset.unwrap_or(1).max(1) as usize - 1;
        let limit = args.limit.unwrap_or(DEFAULT_LINES).max(1) as usize;

        let mut reader = BufReader::new(file);
        let mut kept: Vec<String> = Vec::new();
        let mut line = String::new();
        let mut seen = 0usize;
        let mut bytes = 0usize;
        let mut at_end = false;

        loop {
            if kept.len() == limit || bytes >= STORE_CAP {
                break;
            }
            line.clear();
            let read = reader
                .read_line(&mut line)
                .await
                .map_err(|err| ToolError::Failed(format!("cannot read {}: {err}", args.path)))?;
            if read == 0 {
                at_end = true;
                break;
            }
            seen += 1;
            if seen <= start {
                continue;
            }
            // `read_line` keeps the terminator; joining on "\n" would double
            // it. The last line of a file loses its newline this way, which is
            // the same shape the tool returned before it streamed.
            while line.ends_with('\n') || line.ends_with('\r') {
                line.pop();
            }
            bytes += line.len();
            kept.push(std::mem::take(&mut line));
        }

        // The window is full but the file is not: count the rest so the trailer
        // can say how much there is. This reads the remainder and keeps none of
        // it, which is the whole point of not loading the file.
        if !at_end {
            loop {
                line.clear();
                let read = reader.read_line(&mut line).await.map_err(|err| {
                    ToolError::Failed(format!("cannot read {}: {err}", args.path))
                })?;
                if read == 0 {
                    break;
                }
                seen += 1;
            }
        }

        if kept.is_empty() {
            let note = match at_end {
                true => format!("{} has {seen} lines", args.path),
                false => "the offset is past the end".to_string(),
            };
            return Ok(Observation::ok(format!("(no lines: {note})")));
        }

        let mut body = kept.join("\n");
        if !at_end {
            body.push_str(&format!(
                "\n[lines {}-{} of {seen}; use offset to read more]",
                start + 1,
                start + kept.len()
            ));
        }
        Ok(Observation::ok(body))
    }
}
