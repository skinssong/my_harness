//! The directory the tools are allowed to touch.
//!
//! Containment here is lexical: a path is resolved against the root and any
//! attempt to walk out with `..` or an absolute path is refused before the
//! filesystem is consulted. That keeps `read_file` and `write_file` inside the
//! workspace without depending on what happens to exist.
//!
//! It is *not* a sandbox. `bash` runs a shell, and a shell can leave the root;
//! bounding that is S5's job, not this type's.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WorkspaceError {
    #[error("the workspace root is unusable: {0}")]
    Root(String),
    #[error("{0} is outside the workspace")]
    Escapes(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WorkspaceError> {
        let path = path.as_ref();
        std::fs::create_dir_all(path).map_err(|err| WorkspaceError::Root(err.to_string()))?;
        let root = path
            .canonicalize()
            .map_err(|err| WorkspaceError::Root(err.to_string()))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Turn a path the model supplied into one inside the workspace.
    pub fn resolve(&self, relative: &str) -> Result<PathBuf, WorkspaceError> {
        let trimmed = relative.trim();
        if trimmed.is_empty() {
            return Ok(self.root.clone());
        }

        let mut depth: i32 = 0;
        for component in Path::new(trimmed).components() {
            match component {
                Component::Normal(_) => depth += 1,
                Component::CurDir => {}
                Component::ParentDir => {
                    depth -= 1;
                    if depth < 0 {
                        return Err(WorkspaceError::Escapes(trimmed.to_string()));
                    }
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(WorkspaceError::Escapes(trimmed.to_string()))
                }
            }
        }

        Ok(self.root.join(trimmed))
    }

    /// Path as the model should see it: relative to the root.
    pub fn display(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }
}
