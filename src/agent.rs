//! The loop: call, act, observe, repeat until an answer.
//!
//! Three rules live here.
//!
//! * The ceiling counts **model calls**, not tool executions, so a turn that
//!   asks for four things still costs one turn.
//! * The stopping rule is **"are there calls?"**, not `finish_reason`, which
//!   providers populate inconsistently.
//! * A tool result goes back as an observation whatever happened — including a
//!   time-out or arguments that did not parse. Only a broken provider response
//!   ends the run, and that error comes from the client.
//!
//! Every turn is recorded before it is acted on, so a run that dies mid-tool
//! still leaves the reply that asked for it.

use crate::client::{Client, Error as ClientError};
use crate::event::{Event, LoggedCall};
use crate::log::{LogError, Recorder};
use crate::message::Message;
use crate::reply::{Reply, Usage};
use crate::request::Request;
use crate::tool::{Registry, MAX_OBSERVATION_BYTES};
use crate::workspace::Workspace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_turns: u32,
    pub max_observation_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_turns: 8,
            max_observation_bytes: MAX_OBSERVATION_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub answer: String,
    pub usage: Usage,
    pub turns: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Client(#[from] ClientError),
    #[error("{0}")]
    Log(#[from] LogError),
    #[error("no answer after {turns} turns ({input} in · {output} out)")]
    TurnsExceeded {
        turns: u32,
        input: u32,
        output: u32,
    },
}

/// What a reply means for the loop, separated from how the reply was obtained.
/// Replay judges a recorded reply with the same function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Act {
        calls: Vec<crate::message::ToolCall>,
        text: Option<String>,
    },
    Answer {
        text: String,
    },
}

pub fn decide(reply: Reply) -> Decision {
    match reply {
        Reply::Answer { text, .. } => Decision::Answer { text },
        Reply::Calling { calls, text, .. } => Decision::Act { calls, text },
    }
}

pub async fn run(
    client: &Client,
    registry: &Registry,
    workspace: &Workspace,
    request: Request,
    limits: Limits,
    recorder: &mut dyn Recorder,
) -> Result<Outcome, Error> {
    recorder.record(Event::Start {
        model: request.model().to_string(),
        system: request.system().unwrap_or_default().to_string(),
        task: request.task().unwrap_or_default().to_string(),
        max_turns: limits.max_turns,
    })?;

    let mut request = request;
    let mut spent = Usage::new(0, 0);

    for turn in 1..=limits.max_turns {
        let reply = client.complete(&request).await?;
        let usage = reply.usage();
        spent = spent.plus(usage);

        recorder.record(Event::Reply {
            text: reply.text().map(str::to_string),
            calls: reply.calls().iter().map(LoggedCall::from).collect(),
            input: usage.input(),
            output: usage.output(),
        })?;

        match decide(reply) {
            Decision::Answer { text } => {
                recorder.record(Event::Answer {
                    text: text.clone(),
                    turns: turn,
                })?;
                return Ok(Outcome {
                    answer: text,
                    usage: spent,
                    turns: turn,
                });
            }
            Decision::Act { calls, text } => {
                let mut messages = vec![Message::calling(calls.clone(), text)];
                for call in &calls {
                    let observed = registry.dispatch(call, workspace).await;
                    let shaped = observed.shaped(limits.max_observation_bytes);
                    recorder.record(Event::Tool {
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        ok: shaped.ok,
                        bytes: shaped.text.len(),
                        text: shaped.text.clone(),
                    })?;
                    messages.push(Message::tool_result(
                        call.id.clone(),
                        call.name.clone(),
                        shaped.text,
                    ));
                }
                request = request.continued(messages);
            }
        }
    }

    recorder.record(Event::Halt {
        reason: format!("no answer after {} turns", limits.max_turns),
        turns: limits.max_turns,
    })?;
    Err(Error::TurnsExceeded {
        turns: limits.max_turns,
        input: spent.input(),
        output: spent.output(),
    })
}
