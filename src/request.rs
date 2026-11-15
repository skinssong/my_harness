//! The request: a model and two messages.
//!
//! This type knows nothing about the provider. The envelope it will be posted
//! in — field names, the temperature constant — belongs to the adapter that
//! speaks to that provider, in the same way `reply.rs` keeps its wire structs
//! private.
//!
//! `Role` carries only the two variants S1 can emit. `assistant` and `tool`
//! arrive with history at S2, so today a request cannot name them.

use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RequestError {
    #[error("nothing to ask: the task text is empty")]
    EmptyTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    model: String,
    messages: Vec<Message>,
}

impl Request {
    /// An empty task is refused at the boundary rather than sent, so a request
    /// in hand always carries something to ask.
    pub fn new(model: &str, system: &str, task: &str) -> Result<Self, RequestError> {
        let task = task.trim();
        if task.is_empty() {
            return Err(RequestError::EmptyTask);
        }
        Ok(Self {
            model: model.to_string(),
            messages: vec![
                Message {
                    role: Role::System,
                    content: system.to_string(),
                },
                Message {
                    role: Role::User,
                    content: task.to_string(),
                },
            ],
        })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }
}
