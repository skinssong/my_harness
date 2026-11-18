//! One message on the wire.
//!
//! A single `role` field hides four different shapes. Modelling them as an
//! enum keeps the assistant's call list present exactly when the variant says
//! so, and the constructors below only produce messages the provider accepts.
//! Note what that does and does not buy: an enum's variant fields are public,
//! so `Assistant { content: None, tool_calls: vec![] }` is still writable by
//! hand. The guarantee is that nothing here builds it, not that it is
//! unrepresentable — enforcing it would cost a hand-written `Serialize`.
//!
//! Only serialisation lives here: every message this crate sends is one it
//! built itself. Incoming messages are read by `reply.rs`, which keeps its wire
//! structs private.

use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

/// A call the model asked for. `arguments` stays a string: the provider sends
/// JSON *inside* JSON, and it is the owning tool that knows how to read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl Serialize for ToolCall {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut call = serializer.serialize_struct("ToolCall", 3)?;
        call.serialize_field("id", &self.id)?;
        call.serialize_field("type", "function")?;
        call.serialize_field(
            "function",
            &Function {
                name: &self.name,
                arguments: &self.arguments,
            },
        )?;
        call.end()
    }
}

#[derive(Serialize)]
struct Function<'a> {
    name: &'a str,
    arguments: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        content: Option<String>,
        /// Omitted when empty, so a plain answer stays a plain answer.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        content: String,
        tool_call_id: String,
        name: String,
    },
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self::System {
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self::Assistant {
            content: Some(text.into()),
            tool_calls: Vec::new(),
        }
    }

    /// The assistant asking for one or more actions. `text` is whatever it said
    /// alongside them, which is often nothing.
    pub fn calling(calls: Vec<ToolCall>, text: Option<String>) -> Self {
        Self::Assistant {
            content: text,
            tool_calls: calls,
        }
    }

    /// The result of a call, owed back under the id that asked for it.
    pub fn tool_result(
        call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self::Tool {
            content: content.into(),
            tool_call_id: call_id.into(),
            name: name.into(),
        }
    }

    /// The calls this message carries, if it is an assistant asking for work.
    pub fn calls(&self) -> &[ToolCall] {
        match self {
            Self::Assistant { tool_calls, .. } => tool_calls,
            _ => &[],
        }
    }
}
