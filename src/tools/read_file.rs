//! Read a text file.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use super::outside;
use crate::tool::{Observation, Tool, ToolError};
use crate::workspace::Workspace;

const DEFAULT_LINES: u32 = 2_000;

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
        let text = tokio::fs::read_to_string(&path)
            .await
            .map_err(|err| ToolError::Failed(format!("cannot read {}: {err}", args.path)))?;

        let all: Vec<&str> = text.lines().collect();
        let start = args.offset.unwrap_or(1).max(1) as usize - 1;
        let limit = args.limit.unwrap_or(DEFAULT_LINES).max(1) as usize;
        let shown: Vec<&str> = all.iter().skip(start).take(limit).copied().collect();

        if shown.is_empty() {
            return Ok(Observation::ok(format!(
                "(no lines: {} has {} lines)",
                args.path,
                all.len()
            )));
        }

        let mut body = shown.join("\n");
        let end = start + shown.len();
        if end < all.len() {
            body.push_str(&format!(
                "\n[lines {}-{} of {}; use offset to read more]",
                start + 1,
                end,
                all.len()
            ));
        }
        Ok(Observation::ok(body))
    }
}
