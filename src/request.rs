//! The request: a model and the messages so far.
//!
//! This type knows nothing about the provider. The envelope it will be posted
//! in — field names, the temperature constant — belongs to the adapter that
//! speaks to that provider, in the same way `reply.rs` keeps its wire structs
//! private.

use thiserror::Error;

use crate::message::Message;

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
            messages: vec![Message::system(system), Message::user(task)],
        })
    }

    /// The same conversation with more said in it: used by the loop to carry a
    /// tool result back.
    pub fn continued(&self, extra: Vec<Message>) -> Self {
        let mut messages = self.messages.clone();
        messages.extend(extra);
        Self {
            model: self.model.clone(),
            messages,
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn system(&self) -> Option<&str> {
        match self.messages.first() {
            Some(Message::System { content }) => Some(content),
            _ => None,
        }
    }

    pub fn task(&self) -> Option<&str> {
        match self.messages.get(1) {
            Some(Message::User { content }) => Some(content),
            _ => None,
        }
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }
}
