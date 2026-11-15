//! Write a text file.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use super::outside;
use crate::tool::{Observation, Tool, ToolError};
use crate::workspace::Workspace;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Args {
    /// Path relative to the workspace root.
    pub path: String,
    /// Full contents to write. This replaces the file.
    pub content: String,
}

pub struct WriteFile;

#[async_trait]
impl Tool for WriteFile {
    type Args = Args;

    const NAME: &'static str = "write_file";
    const DESCRIPTION: &'static str =
        "Write a text file in the workspace, replacing any existing contents. Missing parent directories are created.";

    async fn run(&self, args: Args, workspace: &Workspace) -> Result<Observation, ToolError> {
        let path = workspace.resolve(&args.path).map_err(outside)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|err| ToolError::Failed(format!("cannot create {}: {err}", args.path)))?;
        }
        tokio::fs::write(&path, args.content.as_bytes())
            .await
            .map_err(|err| ToolError::Failed(format!("cannot write {}: {err}", args.path)))?;
        Ok(Observation::ok(format!(
            "wrote {} bytes to {}",
            args.content.len(),
            args.path
        )))
    }
}
