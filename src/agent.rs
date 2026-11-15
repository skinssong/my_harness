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

use crate::client::{Client, Error as ClientError};
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
    #[error("no answer after {turns} turns ({input} in · {output} out)")]
    TurnsExceeded {
        turns: u32,
        input: u32,
        output: u32,
    },
}

pub async fn run(
    client: &Client,
    registry: &Registry,
    workspace: &Workspace,
    request: Request,
    limits: Limits,
) -> Result<Outcome, Error> {
    let mut request = request;
    let mut spent = Usage::new(0, 0);

    for turn in 1..=limits.max_turns {
        let reply = client.complete(&request).await?;
        spent = spent.plus(reply.usage());

        match reply {
            Reply::Answer { text, .. } => {
                return Ok(Outcome {
                    answer: text,
                    usage: spent,
                    turns: turn,
                })
            }
            Reply::Calling { calls, text, .. } => {
                let mut messages = vec![Message::calling(calls.clone(), text)];
                for call in &calls {
                    let observed = registry.dispatch(call, workspace).await;
                    let shaped = observed.shaped(limits.max_observation_bytes);
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

    Err(Error::TurnsExceeded {
        turns: limits.max_turns,
        input: spent.input(),
        output: spent.output(),
    })
}
