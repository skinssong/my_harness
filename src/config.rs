//! Where the three settings come from.
//!
//! Three variables, all required, no inference: nothing here guesses a provider
//! from a key's name. A missing variable is named in the error so the fix is
//! obvious. `BaseUrl` can only be built by validating, so a config that exists
//! always carries a usable URL.

use reqwest::Url;
use thiserror::Error;

pub const BASE_URL_VAR: &str = "LLM_BASE_URL";
pub const API_KEY_VAR: &str = "LLM_API_KEY";
pub const MODEL_VAR: &str = "LLM_MODEL";

const CHAT_COMPLETIONS: &str = "chat/completions";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("{0} is not set")]
    Missing(&'static str),
    #[error("LLM_BASE_URL is not an http(s) URL: {0}")]
    BadBaseUrl(String),
}

/// A base URL that has been parsed once. The user's text is kept verbatim
/// rather than normalised, so what they wrote is what gets called.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BaseUrl(String);

impl BaseUrl {
    fn parse(raw: &str) -> Result<Self, ConfigError> {
        let parsed = Url::parse(raw).map_err(|_| ConfigError::BadBaseUrl(raw.to_string()))?;
        match parsed.scheme() {
            "http" | "https" => Ok(Self(raw.to_string())),
            _ => Err(ConfigError::BadBaseUrl(raw.to_string())),
        }
    }

    /// Appending is done here and nowhere else, because a URL join silently
    /// drops the base path: `/v1` + `chat/completions` becomes `/chat/...`.
    fn endpoint(&self) -> String {
        match self.0.ends_with('/') {
            true => format!("{}{CHAT_COMPLETIONS}", self.0),
            false => format!("{}/{CHAT_COMPLETIONS}", self.0),
        }
    }
}

#[derive(Clone)]
pub struct Config {
    base_url: BaseUrl,
    api_key: String,
    model: String,
}

// Written by hand so a key cannot reach a log through `{:?}`. Everything that
// prints a config — traces, the event log at S3 — goes through this.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("base_url", &self.base_url.0)
            .field("api_key", &"<redacted>")
            .field("model", &self.model)
            .finish()
    }
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let base_url = BaseUrl::parse(&required(&get, BASE_URL_VAR)?)?;
        Ok(Self {
            base_url,
            api_key: required(&get, API_KEY_VAR)?,
            model: required(&get, MODEL_VAR)?,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url.0
    }

    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn endpoint(&self) -> String {
        self.base_url.endpoint()
    }
}

fn required(get: &impl Fn(&str) -> Option<String>, name: &'static str) -> Result<String, ConfigError> {
    match get(name).map(|value| value.trim().to_string()) {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(ConfigError::Missing(name)),
    }
}
