//! An agent harness built from the loop outwards.
//!
//! S1 was one call and its cost. S2 adds tools: the model can ask for work, see
//! the result, and continue. Every phase after this composes with the same
//! pieces: a request, a client, a registry of tools, and the loop that joins
//! them.

pub mod agent;
pub mod client;
pub mod config;
pub mod context;
pub mod event;
pub mod log;
pub mod message;
pub mod policy;
pub mod reply;
pub mod replay;
pub mod request;
pub mod tool;
pub mod tools;
pub mod workspace;

pub use agent::{Approval, Ending, Limits, Outcome, Suspension};
pub use client::Client;
pub use config::{Config, ConfigError};
pub use context::{project, ContextPolicy};
pub use event::{Event, Halt};
pub use log::{Discard, Log, Recorder};
pub use message::{Message, ToolCall};
pub use policy::{Permission, Policy};
pub use reply::{Reply, ReplyError, Usage};
pub use replay::{replay, ReplayError, Replayed};
pub use request::{Request, RequestError};
pub use tool::{Observation, Registry, Tool, ToolError};
pub use workspace::Workspace;
