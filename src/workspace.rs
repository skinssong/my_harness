//! The directory the tools are allowed to touch.
//!
//! Containment here is lexical: a path is resolved against the root and any
//! attempt to walk out with `..` or an absolute path is refused before the
//! filesystem is consulted. That keeps `read_file` and `write_file` inside the
//! workspace without depending on what happens to exist.
//!
//! It is *not* a sandbox. `bash` runs a shell, and a shell can leave the root;
//! bounding that is S5's job, not this type's.

use std::ffi::OsStr;
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

/// Drop `.` and fold `..` into the path textually. Safe only after the
/// component check above has proved the path never climbs past its start.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
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

        self.confine(&normalise(&self.root.join(trimmed)), trimmed)
    }

    /// The lexical check above cannot see a symlink. `up/../etc/hosts` has no
    /// net `..`, but if `up` is a link to the root of the disk the kernel
    /// resolves it to `/etc/hosts` — so the real path is checked too, and the
    /// part that does not exist yet is appended to the real path of its
    /// deepest existing ancestor.
    fn confine(&self, joined: &Path, shown: &str) -> Result<PathBuf, WorkspaceError> {
        let escape = || WorkspaceError::Escapes(shown.to_string());
        let mut probe = joined;
        let mut tail: Vec<&OsStr> = Vec::new();

        let real = loop {
            if let Ok(real) = probe.canonicalize() {
                break real;
            }
            let name = probe.file_name().ok_or_else(escape)?;
            tail.push(name);
            probe = probe.parent().ok_or_else(escape)?;
        };

        if !real.starts_with(&self.root) {
            return Err(escape());
        }
        let mut resolved = real;
        resolved.extend(tail.iter().rev().copied());
        Ok(resolved)
    }

    /// Path as the model should see it: relative to the root.
    pub fn display(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }
}
