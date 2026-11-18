//! Re-deriving a run from its record.
//!
//! Replay calls nothing and touches nothing. It reads the model's replies out of
//! the record, runs each through the same `decide` the live loop uses, and then
//! checks that the record contains exactly the effects that decision implies —
//! the right tools, in the right order, ending in the same answer, the same
//! halt, or the same pause.
//!
//! The point is not to re-print the transcript. It is to catch the case where
//! the loop would now decide something the record says it did not: a change in
//! stopping rules, or in how a reply is read, shows up as a mismatch instead of
//! as a subtly different run.

use crate::agent::{decide, Decision};
use crate::event::{Event, Halt};
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
    #[error("an approval appeared with no reply asking for that call")]
    OrphanApproval,
    #[error("after an approval request the record held {found}")]
    ExpectedDecision { found: String },
    #[error("call {call} was denied but the record shows it succeeded")]
    DenialSucceeded { call: String },
    #[error("an answer appeared with no reply producing it")]
    OrphanAnswer,
    #[error("the record continues after the run had finished")]
    AfterFinish,
    #[error("the record ends without an answer, a halt or a pause")]
    Incomplete,
    #[error("the record halts for {rule}, which this run's limits do not allow")]
    HaltRuleDiffers { rule: String },
}

/// The limits the run started with. Replay reads the stopping rules from the
/// record and holds the recorded halt against them.
#[derive(Default)]
struct Started {
    max_turns: u32,
    stuck_after: u32,
    max_usd: Option<f64>,
}

/// A recorded halt has to be one the run's own limits could have produced —
/// otherwise a loop that stopped halting correctly would replay clean.
fn check_halt(halt: &Halt, turn: u32, limits: &Started) -> Result<(), ReplayError> {
    let recorded = match halt {
        Halt::Ceiling { turns }
        | Halt::Budget { turns, .. }
        | Halt::Stuck { turns, .. }
        | Halt::Provider { turns, .. } => *turns,
    };
    if recorded != turn {
        return Err(ReplayError::TurnCountDiffers {
            decided: turn,
            recorded,
        });
    }
    let violated = match halt {
        // The ceiling only stops a run that reached it.
        Halt::Ceiling { .. } if turn != limits.max_turns => "the turn ceiling",
        // A stuck stop before it could trigger, or with the check disabled.
        Halt::Stuck { .. } if limits.stuck_after == 0 || turn < limits.stuck_after => {
            "the stuck check"
        }
        // A budget stop on a run that was never given a budget.
        Halt::Budget { .. } if limits.max_usd.is_none() => "a money budget",
        _ => return Ok(()),
    };
    Err(ReplayError::HaltRuleDiffers {
        rule: violated.to_string(),
    })
}

fn name(event: &Event) -> String {
    match event {
        Event::Start { .. } => "a start event",
        Event::Reply { .. } => "a reply",
        Event::Tool { .. } => "a tool result",
        Event::ApprovalRequested { .. } => "an approval request",
        Event::ApprovalDecided { .. } => "an approval decision",
        Event::Answer { .. } => "an answer",
        Event::Halt(_) => "a halt",
    }
    .to_string()
}

pub fn replay(events: &[Event]) -> Result<Replayed, ReplayError> {
    let mut turn = 0u32;
    let mut input = 0u32;
    let mut output = 0u32;
    let mut tools = 0usize;
    let mut started = false;
    let mut limits = Started::default();
    let mut finished = false;
    let mut waiting = false;

    let mut index = 0;
    while index < events.len() {
        let event = &events[index];

        if finished {
            return Err(ReplayError::AfterFinish);
        }

        match event {
            Event::Start {
                max_turns,
                stuck_after,
                max_usd,
                ..
            } => {
                if started {
                    return Err(ReplayError::RepeatedStart);
                }
                limits = Started {
                    max_turns: *max_turns,
                    stuck_after: *stuck_after,
                    max_usd: *max_usd,
                };
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

                // What follows a reply can end the run before it acts: a halt
                // (the budget and the provider both stop there) or an approval
                // request (the policy stopped it there). Both are endings, not
                // torn records.
                let mut asked: Option<String> = None;
                let mut approved: Option<bool> = None;
                match events.get(index) {
                    Some(Event::Halt(halt)) => {
                        check_halt(halt, turn, &limits)?;
                        index += 1;
                        finished = true;
                        continue;
                    }
                    Some(Event::ApprovalRequested { call_id, .. }) => {
                        if !calls.iter().any(|call| call.id == *call_id) {
                            return Err(ReplayError::OrphanApproval);
                        }
                        asked = Some(call_id.clone());
                        index += 1;
                        match events.get(index) {
                            // The record ends at the pause: the run is waiting,
                            // which is an ending of its own.
                            None => {
                                waiting = true;
                                continue;
                            }
                            Some(Event::ApprovalDecided {
                                call_id: decided,
                                approved: verdict,
                            }) if decided == call_id => {
                                approved = Some(*verdict);
                                index += 1;
                            }
                            Some(found) => {
                                return Err(ReplayError::ExpectedDecision { found: name(found) })
                            }
                        }
                    }
                    _ => {}
                }

                match decide(reply) {
                    Decision::Answer { text: decided } => {
                        let Some(Event::Answer {
                            text: recorded,
                            turns: recorded_turns,
                        }) = events.get(index)
                        else {
                            return Err(match events.get(index) {
                                Some(found) => ReplayError::ExpectedTool { found: name(found) },
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
                            let Event::Tool { call_id, ok, .. } = found else {
                                return Err(ReplayError::ExpectedTool { found: name(found) });
                            };
                            if *call_id != expected.id {
                                return Err(ReplayError::ToolOrder {
                                    expected: expected.id,
                                    found: call_id.clone(),
                                });
                            }
                            // A denied call must not have done anything.
                            let denied = asked.as_deref() == Some(expected.id.as_str())
                                && approved == Some(false);
                            if denied && *ok {
                                return Err(ReplayError::DenialSucceeded { call: expected.id });
                            }
                            tools += 1;
                            index += 1;
                        }
                    }
                }
            }
            Event::Tool { .. } => return Err(ReplayError::OrphanTool),
            Event::ApprovalRequested { .. } | Event::ApprovalDecided { .. } => {
                return Err(ReplayError::OrphanApproval)
            }
            Event::Answer { .. } => return Err(ReplayError::OrphanAnswer),
            Event::Halt(halt) => {
                check_halt(halt, turn, &limits)?;
                index += 1;
                finished = true;
            }
        }
    }

    if !started {
        return Err(ReplayError::NotStarted);
    }
    if !finished && !waiting {
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
