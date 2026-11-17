//! The record on disk: one JSON object per line, appended as the run happens.
//!
//! Appending and flushing per event is deliberate. A run that dies at turn nine
//! should leave nine turns behind, not an empty file — that is the whole point
//! of recording.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::event::Event;

#[derive(Debug, thiserror::Error)]
pub enum LogError {
    #[error("cannot write the record: {0}")]
    Write(String),
    #[error("cannot read {path}: {reason}")]
    Open { path: String, reason: String },
    #[error("{path}, line {line}: {reason}")]
    Line {
        path: String,
        line: usize,
        reason: String,
    },
}

/// Somewhere to put events. A run takes one of these so the loop never has to
/// know whether it is being recorded.
pub trait Recorder {
    fn record(&mut self, event: Event) -> Result<(), LogError>;
}

/// For tests and for runs that are not being kept.
pub struct Discard;

impl Recorder for Discard {
    fn record(&mut self, _event: Event) -> Result<(), LogError> {
        Ok(())
    }
}

/// Two records in the same second from one process must not collide.
static NEXT_RECORD: AtomicU64 = AtomicU64::new(0);

pub struct Log {
    path: PathBuf,
    file: File,
}

impl Log {
    pub fn create(dir: impl AsRef<Path>) -> Result<Self, LogError> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir).map_err(|err| LogError::Write(err.to_string()))?;

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_secs())
            .unwrap_or(0);
        let seq = NEXT_RECORD.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(
            "run-{stamp}-{}-{seq}.jsonl",
            std::process::id()
        ));
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)
            .map_err(|err| LogError::Write(err.to_string()))?;
        Ok(Self { path, file })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&mut self, event: &Event) -> Result<(), LogError> {
        let line = serde_json::to_string(event).map_err(|err| LogError::Write(err.to_string()))?;
        writeln!(self.file, "{line}").map_err(|err| LogError::Write(err.to_string()))?;
        self.file
            .flush()
            .map_err(|err| LogError::Write(err.to_string()))
    }

    pub fn read(path: impl AsRef<Path>) -> Result<Vec<Event>, LogError> {
        let path = path.as_ref();
        let shown = path.display().to_string();
        let text = std::fs::read_to_string(path).map_err(|err| LogError::Open {
            path: shown.clone(),
            reason: err.to_string(),
        })?;

        // A record is written one line at a time and flushed per event, so a
        // process killed mid-write leaves a final fragment with no newline.
        // That is the crash this file exists for: drop the fragment and replay
        // everything that did land, rather than refusing the whole record.
        let clean = text.ends_with('\n');
        let body = text.strip_suffix('\n').unwrap_or(&text);
        let total = body.split('\n').count();

        let mut events = Vec::new();
        for (index, line) in body.split('\n').enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str(line) {
                Ok(event) => events.push(event),
                Err(_) if index + 1 == total && !clean => break,
                Err(err) => {
                    return Err(LogError::Line {
                        path: shown,
                        line: index + 1,
                        reason: err.to_string(),
                    })
                }
            }
        }
        Ok(events)
    }
}

impl Recorder for Log {
    fn record(&mut self, event: Event) -> Result<(), LogError> {
        self.append(&event)
    }
}
