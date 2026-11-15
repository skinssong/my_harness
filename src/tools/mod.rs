//! The four built-in tools, and the registry that holds them.
//!
//! Each tool is a type with an `Args` struct. That struct is both the schema
//! sent to the model and the parser applied to what comes back, so the two
//! cannot disagree.
//!
//! `bash` deserves a warning: it runs a real shell, and a shell can leave the
//! workspace. Containment for the file tools is lexical and lives in
//! `Workspace`; containing a shell needs permissions, which is S5.

pub mod bash;
pub mod list_dir;
pub mod read_file;
pub mod write_file;

use std::sync::Arc;

use crate::tool::{Registry, ToolError};
use crate::workspace::WorkspaceError;

pub fn builtin() -> Registry {
    Registry::new(vec![
        Arc::new(read_file::ReadFile),
        Arc::new(list_dir::ListDir),
        Arc::new(write_file::WriteFile),
        Arc::new(bash::Bash),
    ])
}

fn outside(error: WorkspaceError) -> ToolError {
    ToolError::Failed(error.to_string())
}
