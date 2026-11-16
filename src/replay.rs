//! Re-deriving a run from its record.
//!
//! Replay calls nothing and touches nothing. It reads the model's replies out of
//! the record, runs each through the same `decide` the live loop uses, and then
//! checks that the record contains exactly the effects that decision implies —
//! the right tools, in the right order, ending in the same answer or the same
//! halt.
//!
//! The point is not to re-print the transcript. It is to catch the case where
//! the loop would now decide something the record says it did not: a change in
//! stopping rules, or in how a reply is read, shows up as a mismatch instead of
//! as a subtly different run.

use crate::agent::{decide, Decision};
use crate::event::Event;
use crate::message::ToolCall;
use crate::reply::{Reply, Usage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replayed {
    pub turns: u32,
    pub usage: Usage,
    pub tools: usize,
    pub answer: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReplayError {
    #[error("the record does not begin with a start event")]
    NotStarted,
    #[error("the record has more than one start event")]
    RepeatedStart,
    #[error("a reply carried neither text nor a call id")]
    EmptyReply,
    #[error("the loop decided {decided:?} but the record holds {recorded:?}")]
    AnswerDiffers { decided: String, recorded: String },
    #[error("the loop counted {decided} turns but the record holds {recorded}")]
    TurnCountDiffers { decided: u32, recorded: u32 },
    #[error("after a reply the loop expected a tool result, found {found}")]
    ExpectedTool { found: String },
    #[error("tool results out of order: expected {expected}, found {found}")]
    ToolOrder { expected: String, found: String },
    #[error("a tool result appeared with no reply asking for it")]
    OrphanTool,
    #[error("an answer appeared with no reply producing it")]
    OrphanAnswer,
    #[error("the record continues after the run had finished")]
    AfterFinish,
    #[error("the record ends without an answer or a halt")]
    Incomplete,
}

fn name(event: &Event) -> String {
    match event {
        Event::Start { .. } => "a start event",
        Event::Reply { .. } => "a reply",
        Event::Tool { .. } => "a tool result",
        Event::Answer { .. } => "an answer",
        Event::Halt { .. } => "a halt",
    }
    .to_string()
}

pub fn replay(events: &[Event]) -> Result<Replayed, ReplayError> {
    let mut turn = 0u32;
    let mut input = 0u32;
    let mut output = 0u32;
    let mut tools = 0usize;
    let mut started = false;
    let mut finished = false;

    let mut index = 0;
    while index < events.len() {
        let event = &events[index];

        if finished {
            return Err(ReplayError::AfterFinish);
        }

        match event {
            Event::Start { .. } => {
                if started {
                    return Err(ReplayError::RepeatedStart);
                }
                started = true;
                index += 1;
            }
            _ if !started => return Err(ReplayError::NotStarted),
            Event::Reply {
                text,
                calls,
                input: turn_input,
                output: turn_output,
            } => {
                turn += 1;
                input += turn_input;
                output += turn_output;
                let usage = Usage::new(*turn_input, *turn_output);

                let reply = if calls.is_empty() {
                    Reply::Answer {
                        text: text.clone().ok_or(ReplayError::EmptyReply)?,
                        usage,
                    }
                } else {
                    Reply::Calling {
                        calls: calls.iter().map(ToolCall::from).collect(),
                        text: text.clone(),
                        usage,
                    }
                };

                index += 1;
                match decide(reply) {
                    Decision::Answer { text: decided } => {
                        let Some(Event::Answer {
                            text: recorded,
                            turns: recorded_turns,
                        }) = events.get(index)
                        else {
                            return Err(match events.get(index) {
                                Some(found) => ReplayError::ExpectedTool {
                                    found: name(found),
                                },
                                None => ReplayError::Incomplete,
                            });
                        };
                        if *recorded != decided {
                            return Err(ReplayError::AnswerDiffers {
                                decided,
                                recorded: recorded.clone(),
                            });
                        }
                        if *recorded_turns != turn {
                            return Err(ReplayError::TurnCountDiffers {
                                decided: turn,
                                recorded: *recorded_turns,
                            });
                        }
                        index += 1;
                        finished = true;
                    }
                    Decision::Act { calls, .. } => {
                        for expected in calls {
                            let Some(found) = events.get(index) else {
                                return Err(ReplayError::ExpectedTool {
                                    found: "the end of the record".to_string(),
                                });
                            };
                            let Event::Tool { call_id, .. } = found else {
                                return Err(ReplayError::ExpectedTool {
                                    found: name(found),
                                });
                            };
                            if *call_id != expected.id {
                                return Err(ReplayError::ToolOrder {
                                    expected: expected.id,
                                    found: call_id.clone(),
                                });
                            }
                            tools += 1;
                            index += 1;
                        }
                    }
                }
            }
            Event::Tool { .. } => return Err(ReplayError::OrphanTool),
            Event::Answer { .. } => return Err(ReplayError::OrphanAnswer),
            Event::Halt { turns: halted, .. } => {
                if *halted != turn {
                    return Err(ReplayError::TurnCountDiffers {
                        decided: turn,
                        recorded: *halted,
                    });
                }
                index += 1;
                finished = true;
            }
        }
    }

    if !started {
        return Err(ReplayError::NotStarted);
    }
    if !finished {
        return Err(ReplayError::Incomplete);
    }

    let answer = events.iter().find_map(|event| match event {
        Event::Answer { text, .. } => Some(text.clone()),
        _ => None,
    });

    Ok(Replayed {
        turns: turn,
        usage: Usage::new(input, output),
        tools,
        answer,
    })
}
