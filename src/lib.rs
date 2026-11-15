//! S1: one prompt in, one answer out, cost known.
//!
//! Data travels one way and crosses exactly one boundary, the HTTP call.
//! Everything on either side of it is a pure function of values.

pub mod client;
pub mod config;
pub mod message;
pub mod reply;
pub mod request;

pub use client::Client;
pub use config::{Config, ConfigError};
pub use message::{Message, ToolCall};
pub use reply::{Reply, ReplyError, Usage};
pub use request::{Request, RequestError};
