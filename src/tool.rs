//! The tool contract, and the one place a call is executed.
//!
//! A tool is a name, a description, an argument type and some work. The
//! argument type is the contract in both directions: it renders the JSON Schema
//! the model is shown, and it is what the model's arguments are parsed into. One
//! type, so the schema cannot advertise a field the parser does not accept.
//!
//! `Registry::dispatch` never returns an error. A tool that fails, or arguments
//! that do not parse, become an `Observation` the model can read and react to —
//! a malformed call is a behaviour to recover from, not a reason to end a run.
//! Only a broken provider response is fatal, and that lives in the client.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::message::ToolCall;
use crate::workspace::Workspace;

/// Enough of a result to be useful, never enough to swallow the next request.
pub const MAX_OBSERVATION_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub ok: bool,
    pub text: String,
}

impl Observation {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            ok: true,
            text: text.into(),
        }
    }

    pub fn failed(reason: impl Into<String>) -> Self {
        Self {
            ok: false,
            text: format!("tool_error: {}", reason.into()),
        }
    }

    /// Head and tail kept, middle elided. A command that prints a megabyte must
    /// not bury the one line that matters.
    pub fn shaped(self, max_bytes: usize) -> Self {
        if self.text.len() <= max_bytes {
            return self;
        }
        let keep = max_bytes / 2;
        let bytes = self.text.as_bytes();
        let head = String::from_utf8_lossy(&bytes[..keep]);
        let tail = String::from_utf8_lossy(&bytes[bytes.len() - keep..]);
        let elided = bytes.len() - keep * 2;
        Self {
            ok: self.ok,
            text: format!("{head}\n[{elided} bytes elided]\n{tail}"),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ToolError {
    /// The arguments were the wrong shape for this tool.
    #[error("{0}")]
    Invalid(String),
    /// The tool ran and the world said no.
    #[error("{0}")]
    Failed(String),
}

#[async_trait]
pub trait Tool: Send + Sync {
    type Args: DeserializeOwned + JsonSchema + Send;

    const NAME: &'static str;
    const DESCRIPTION: &'static str;

    async fn run(&self, args: Self::Args, workspace: &Workspace)
        -> Result<Observation, ToolError>;
}

/// A tool with its argument type erased, so unlike tools can share a registry.
#[async_trait]
pub trait ErasedTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// The `parameters` object sent to the model.
    fn parameters(&self) -> Value;
    async fn call(&self, arguments: Value, workspace: &Workspace)
        -> Result<Observation, ToolError>;
}

#[async_trait]
impl<T: Tool> ErasedTool for T {
    fn name(&self) -> &'static str {
        T::NAME
    }

    fn description(&self) -> &'static str {
        T::DESCRIPTION
    }

    fn parameters(&self) -> Value {
        let mut schema = schemars::schema_for!(T::Args);
        // Providers accept a bare object schema; `$schema` is noise and some
        // gateways reject unknown keys.
        let mut value = serde_json::to_value(&mut schema).unwrap_or(Value::Null);
        if let Value::Object(map) = &mut value {
            map.remove("$schema");
        }
        value
    }

    async fn call(
        &self,
        arguments: Value,
        workspace: &Workspace,
    ) -> Result<Observation, ToolError> {
        let args: T::Args = serde_json::from_value(arguments).map_err(|err| {
            ToolError::Invalid(format!(
                "arguments do not match {}: {err}",
                T::NAME
            ))
        })?;
        self.run(args, workspace).await
    }
}

#[derive(Default)]
pub struct Registry {
    tools: Vec<Arc<dyn ErasedTool>>,
}

impl Registry {
    pub fn new(tools: Vec<Arc<dyn ErasedTool>>) -> Self {
        Self { tools }
    }

    pub fn with(mut self, tool: Arc<dyn ErasedTool>) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.tools.iter().map(|tool| tool.name()).collect()
    }

    /// The `tools` array sent with every request. Built from the registry, so it
    /// cannot drift from what dispatch will accept.
    pub fn specs(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name(),
                        "description": tool.description(),
                        "parameters": tool.parameters(),
                    }
                })
            })
            .collect()
    }

    /// Whether a tool of this name is registered. The gate asks before it asks
    /// for permission, so an unknown name gets the registry's own answer.
    pub fn knows(&self, name: &str) -> bool {
        self.tools.iter().any(|tool| tool.name() == name)
    }

    /// Run one call. Returns an observation whatever happens.
    pub async fn dispatch(&self, call: &ToolCall, workspace: &Workspace) -> Observation {
        if call.id.trim().is_empty() {
            return Observation::failed("the call carried no id, so its result cannot be returned");
        }

        let Some(tool) = self.tools.iter().find(|tool| tool.name() == call.name) else {
            let known = self.names().join(", ");
            return Observation::failed(format!(
                "no tool named {}; available: {known}",
                call.name
            ));
        };

        let arguments: Value = match serde_json::from_str(&call.arguments) {
            Ok(value) => value,
            Err(err) => {
                return Observation::failed(format!(
                    "arguments for {} are not valid JSON: {err}",
                    call.name
                ))
            }
        };

        match tool.call(arguments, workspace).await {
            Ok(observation) => observation,
            Err(err) => Observation::failed(err.to_string()),
        }
    }
}
