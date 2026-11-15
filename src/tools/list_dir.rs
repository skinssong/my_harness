//! List a directory.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use super::outside;
use crate::tool::{Observation, Tool, ToolError};
use crate::workspace::Workspace;

const MAX_ENTRIES: usize = 500;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Args {
    /// Directory relative to the workspace root. Defaults to the root itself.
    #[serde(default)]
    pub path: Option<String>,
}

pub struct ListDir;

#[async_trait]
impl Tool for ListDir {
    type Args = Args;

    const NAME: &'static str = "list_dir";
    const DESCRIPTION: &'static str =
        "List the entries of a workspace directory. Directories are shown with a trailing slash.";

    async fn run(&self, args: Args, workspace: &Workspace) -> Result<Observation, ToolError> {
        let shown_as = args.path.clone().unwrap_or_else(|| ".".to_string());
        let path = workspace.resolve(&shown_as).map_err(outside)?;

        let mut entries = Vec::new();
        let mut reader = tokio::fs::read_dir(&path)
            .await
            .map_err(|err| ToolError::Failed(format!("cannot list {shown_as}: {err}")))?;
        while let Some(entry) = reader
            .next_entry()
            .await
            .map_err(|err| ToolError::Failed(format!("cannot list {shown_as}: {err}")))?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry
                .file_type()
                .await
                .map(|kind| kind.is_dir())
                .unwrap_or(false);
            entries.push(if is_dir { format!("{name}/") } else { name });
        }

        entries.sort();
        if entries.is_empty() {
            return Ok(Observation::ok(format!("({shown_as} is empty)")));
        }

        let total = entries.len();
        entries.truncate(MAX_ENTRIES);
        let mut body = entries.join("\n");
        if total > MAX_ENTRIES {
            body.push_str(&format!("\n[{} more entries]", total - MAX_ENTRIES));
        }
        Ok(Observation::ok(body))
    }
}
