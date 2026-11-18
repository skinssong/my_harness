//! The single boundary: one POST, and everything that can go wrong doing it.
//!
//! Two rules hold here. Everything upstream of the call is trusted, because it
//! was validated where it entered — the key is non-empty and the URL is http(s)
//! by construction. And what comes back is checked once, here: a body that is
//! not a reply is trimmed before it reaches an operator's terminal, so a proxy's
//! error page cannot flood the screen.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use crate::config::Config;
use crate::message::Message;
use crate::reply::{Reply, ReplyError};
use crate::request::Request;

/// A call that never returns is worse than one that fails. One place, one value.
const TIMEOUT: Duration = Duration::from_secs(60);

/// Enough of a failing body to diagnose it, never enough to flood a terminal.
const MAX_ERROR_BODY: usize = 400;

/// Some gateways refuse a request with no `User-Agent` before they even look at
/// the key, and report it as an auth failure. Saying who we are costs nothing.
const USER_AGENT: &str = concat!("harness/", env!("CARGO_PKG_VERSION"));

/// The same gateways also want to know which session a call belongs to, so they
/// can route it. Sent unconditionally: an unused header is ignored, and one run
/// is one session. Without it they answer 400 before doing anything.
const SESSION_HEADER: &str = "x-opencode-session";

/// Determinism stated on the wire rather than inherited from a provider default.
const TEMPERATURE: f32 = 0.0;

/// The provider's envelope. Private because it is this adapter's business, not
/// the domain's: `Request` says what is asked, this says how to say it.
#[derive(Serialize)]
struct Wire<'a> {
    model: &'a str,
    messages: &'a [Message],
    temperature: f32,
    #[serde(skip_serializing_if = "none_when_empty")]
    tools: &'a [Value],
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
}

impl<'a> Wire<'a> {
    fn new(request: &'a Request, tools: &'a [Value]) -> Self {
        Self {
            model: request.model(),
            messages: request.messages(),
            temperature: TEMPERATURE,
            tools,
            tool_choice: if tools.is_empty() { None } else { Some("auto") },
        }
    }
}

fn none_when_empty<T>(value: &&[T]) -> bool {
    value.is_empty()
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("request failed: {0}")]
    Transport(String),
    #[error("provider returned {code}: {body}")]
    Status { code: u16, body: String },
    #[error("{0}")]
    Reply(#[from] ReplyError),
}

#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    config: Config,
    session: String,
    /// The `tools` array sent with every request. Empty means a plain chat.
    tools: Vec<Value>,
}

impl Client {
    pub fn new(config: Config) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|err| Error::Transport(err.to_string()))?;
        Ok(Self {
            http,
            config,
            session: session_id(),
            tools: Vec::new(),
        })
    }

    /// Offer the model a set of tools. Without this the request is a plain chat.
    pub fn with_tools(mut self, tools: Vec<Value>) -> Self {
        self.tools = tools;
        self
    }

    /// The model this client was built for. The loop records it, so that a
    /// record describes its run without the environment.
    pub fn model(&self) -> &str {
        self.config.model()
    }

    pub async fn complete(&self, request: &Request) -> Result<Reply, Error> {
        let response = self
            .http
            .post(self.config.endpoint())
            .bearer_auth(self.config.api_key())
            .header(SESSION_HEADER, self.session.as_str())
            .json(&Wire::new(request, &self.tools))
            .send()
            .await
            .map_err(|err| Error::Transport(err.to_string()))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| Error::Transport(err.to_string()))?;

        if !status.is_success() {
            return Err(Error::Status {
                code: status.as_u16(),
                body: bounded(body),
            });
        }

        Reply::from_json(&body).map_err(Error::Reply)
    }
}

/// One run is one session. Process id and a timestamp tell two runs apart and
/// need no dependency.
fn session_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0);
    format!("harness-{}-{nanos}", std::process::id())
}

fn bounded(body: String) -> String {
    if body.chars().count() <= MAX_ERROR_BODY {
        return body;
    }
    let mut kept: String = body.chars().take(MAX_ERROR_BODY).collect();
    kept.push('…');
    kept
}
