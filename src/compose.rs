//! Composition: one subagent per file, run concurrently.
//!
//! The case this exists for is a directory whose files are pieces of one
//! document, in an order only a reader can work out. Each child reads one file
//! and returns a *claim* about it — what it covers, what it must follow — and
//! the parent, which never sees the file's text, does the ordering.
//!
//! Three properties matter more than the code:
//!
//! * **The join is by index, not by completion.** Children finish in whatever
//!   order the network gives them; the claims are placed where their file was.
//!   Two runs of the same directory therefore produce the same request.
//! * **A child cannot act.** Its policy is read-only and starts from an empty
//!   table, so a tool added later is refused inside a child by default.
//! * **What comes back is capped.** The whole point of the boundary is that the
//!   parent's context does not grow with the material; a child that could
//!   return a transcript would undo it.

use std::path::{Path, PathBuf};

use tokio::task::{JoinError, JoinSet};

use crate::agent::{self, Ending, Limits};
use crate::client::Client;
use crate::log::Log;
use crate::policy::Policy;
use crate::request::Request;
use crate::tools::Registry;
use crate::workspace::{Workspace, WorkspaceError};

/// How many children may be in flight at once. Unbounded fan-out over a
/// directory is unbounded burst spend, and the provider rate-limits before it
/// says anything useful.
pub const DEFAULT_WIDTH: usize = 4;

/// How many model calls a child may make. One to read, one to report.
const CHILD_TURNS: u32 = 3;

/// What a child may return. The parent's context is the budget this protects.
pub const CLAIM_CAP: usize = 2 * 1024;

const CHILD_SYSTEM: &str = "You read exactly one file and report on it in a single short paragraph. \
Do not copy the text back; describe it. Do not explore — the file is named in the task.";

/// One child's report about one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub file: String,
    pub text: String,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Workspace(#[from] WorkspaceError),
    #[error("cannot read {dir}: {reason}")]
    Directory { dir: String, reason: String },
    #[error("{0}")]
    Agent(#[from] agent::Error),
    #[error("a child task panicked")]
    Panicked,
    #[error("a child was given an empty task")]
    EmptyTask,
}

/// Read every file in `dir`, one child each, and return their claims **in file
/// order** however they happened to finish.
pub async fn fan_out(
    client: &Client,
    workspace: &Workspace,
    limits: &Limits,
    log_dir: &Path,
    dir: &str,
    width: usize,
) -> Result<Vec<Claim>, Error> {
    let files = list(workspace, dir).await?;
    let width = width.max(1);

    let mut claims: Vec<Option<Claim>> = files.iter().map(|_| None).collect();
    let mut set = JoinSet::new();
    // The menu and the reachable set have to agree: a child told it may call
    // `bash` and then refused is worse than one never offered it.
    let reader = crate::tools::reader();
    let child = Child {
        client: client.clone().with_tools(reader.specs()),
        registry: reader,
        workspace: workspace.clone(),
        limits: child_limits(limits),
        log_dir: log_dir.to_path_buf(),
    };

    for (index, file) in files.into_iter().enumerate() {
        while set.len() >= width {
            settle(set.join_next().await, &mut claims)?;
        }
        let task = child_task(dir, &file);
        let child = child.clone();
        set.spawn(async move { run_child(child, index, file, task).await });
    }

    while let Some(done) = set.join_next().await {
        settle(Some(done), &mut claims)?;
    }

    Ok(claims.into_iter().flatten().collect())
}

/// What the parent is told, in the order the files were found.
pub fn render(claims: &[Claim]) -> String {
    claims
        .iter()
        .map(|claim| format!("{}:\n  {}", claim.file, claim.text))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The whole of the parent's turn budget, minus what the parent needs: a child
/// reads once and reports once, and may only read.
fn child_limits(parent: &Limits) -> Limits {
    Limits {
        max_turns: CHILD_TURNS,
        policy: Policy::read_only(),
        ..parent.clone()
    }
}

fn child_task(dir: &str, file: &str) -> String {
    format!(
        "Read {dir}/{file} and report on it in one paragraph: which part of the whole it covers, \
         what it must follow (or say that it opens), and its first sentence quoted verbatim."
    )
}

async fn run_child(
    child: Child,
    index: usize,
    file: String,
    task: String,
) -> Result<(usize, Claim), Error> {
    let mut log = Log::create_tagged(&child.log_dir, "sub").map_err(agent::Error::Log)?;
    let request =
        Request::new(child.client.model(), CHILD_SYSTEM, &task).map_err(|_| Error::EmptyTask)?;
    let ending = agent::run(
        &child.client,
        &child.registry,
        &child.workspace,
        request,
        &child.limits,
        &mut log,
    )
    .await?;
    Ok((
        index,
        Claim {
            file,
            text: claim_text(ending),
        },
    ))
}

/// Everything a child needs, cloned once per task.
#[derive(Clone)]
struct Child {
    client: Client,
    registry: Registry,
    workspace: Workspace,
    limits: Limits,
    log_dir: PathBuf,
}

/// A child that halted still reports; the parent is better off with a claim
/// that says "this one could not be read" than with a fan-out that failed.
fn claim_text(ending: Ending) -> String {
    match ending {
        Ending::Answered(outcome) => capped(outcome.answer),
        Ending::Halted(halt) => format!("(this chunk could not be read: {halt})"),
        Ending::Suspended(_) => "(this chunk needed approval and was left unread)".to_string(),
    }
}

fn capped(text: String) -> String {
    if text.len() <= CLAIM_CAP {
        return text;
    }
    let mut end = CLAIM_CAP;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [truncated]", &text[..end])
}

/// A finished child task: the join, the child's own result, then its index.
type Finished = Result<Result<(usize, Claim), Error>, JoinError>;

/// Place a finished child where its file was, not where it arrived.
fn settle(done: Option<Finished>, claims: &mut [Option<Claim>]) -> Result<(), Error> {
    match done {
        None => Ok(()),
        Some(Ok(Ok((index, claim)))) => {
            claims[index] = Some(claim);
            Ok(())
        }
        Some(Ok(Err(err))) => Err(err),
        Some(Err(_)) => Err(Error::Panicked),
    }
}

/// The directory's files, sorted by name. Not a tool call: this runs before the
/// parent's loop starts, and no model chooses it.
async fn list(workspace: &Workspace, dir: &str) -> Result<Vec<String>, Error> {
    let root = workspace.resolve(dir)?;
    let mut reader = tokio::fs::read_dir(&root).await.map_err(|err| Error::Directory {
        dir: dir.to_string(),
        reason: err.to_string(),
    })?;

    let mut files = Vec::new();
    while let Some(entry) = reader.next_entry().await.map_err(|err| Error::Directory {
        dir: dir.to_string(),
        reason: err.to_string(),
    })? {
        let is_file = entry.file_type().await.map(|kind| kind.is_file()).unwrap_or(false);
        if is_file {
            files.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    files.sort();
    Ok(files)
}
