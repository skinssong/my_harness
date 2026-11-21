//! Tools: the contract they satisfy, and the four that are built in.
//!
//! A tool is a name, a description, an argument type and some work. The
//! argument type is the contract in both directions: it renders the JSON Schema
//! the model is shown, and it is what the model's arguments are parsed into. One
//! type, so the schema cannot advertise a field the parser does not accept. That
//! contract, its registry and observation shaping live in [`contract`]; the
//! implementations live beside it.
//!
//! `bash` deserves a warning: it runs a real shell, and a shell can leave the
//! workspace. Containment for the file tools is lexical and lives in
//! `Workspace`; containing a shell needs permissions, which is S5.

pub mod bash;
mod contract;
pub mod list_dir;
pub mod read_file;
pub mod write_file;

pub use contract::{Observation, Registry, Tool, ToolError, MAX_OBSERVATION_BYTES};

use std::sync::Arc;

use crate::workspace::WorkspaceError;

pub fn builtin() -> Registry {
    Registry::new(vec![
        Arc::new(read_file::ReadFile),
        Arc::new(list_dir::ListDir),
        Arc::new(write_file::WriteFile),
        Arc::new(bash::Bash),
    ])
}

/// What a subagent is given: one way to read the file it was pointed at.
///
/// A narrower registry rather than a narrower policy, because the tool list is
/// also the menu the model reads. A child offered `list_dir` will list the
/// directory instead of reading the file it was named — observed, not
/// hypothesised.
pub fn reader() -> Registry {
    Registry::new(vec![Arc::new(read_file::ReadFile)])
}

fn outside(error: WorkspaceError) -> ToolError {
    ToolError::Failed(error.to_string())
}
