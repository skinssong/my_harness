//! S1 behavioural tests.
//!
//! Each test calls the code the way its user does and checks what that user
//! would see: a config read from three named variables, a request rendered as a
//! body, a provider response read back, an answer reported.

use harness::config::{Config, ConfigError, Prices};
use harness::reply::{Reply, ReplyError};
use harness::message::Message;
use harness::request::{Request, RequestError};

fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let owned: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |key| {
        owned
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }
}

fn full(base: &str) -> Vec<(&'static str, String)> {
    vec![
        ("LLM_BASE_URL", base.to_string()),
        ("LLM_API_KEY", "k".to_string()),
        ("LLM_MODEL", "m".to_string()),
    ]
}

fn config_from(base: &str) -> Config {
    let owned = full(base);
    let pairs: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
    Config::from_lookup(lookup(&pairs)).unwrap()
}

// -- config: three variables, all required, no inference -------------------

#[test]
fn a_missing_key_is_named() {
    let result = Config::from_lookup(lookup(&[
        ("LLM_BASE_URL", "https://example.test/v1"),
        ("LLM_MODEL", "m"),
    ]));
    assert_eq!(result.unwrap_err(), ConfigError::Missing("LLM_API_KEY"));
}

#[test]
fn a_missing_base_url_is_named() {
    let result = Config::from_lookup(lookup(&[("LLM_API_KEY", "k"), ("LLM_MODEL", "m")]));
    assert_eq!(result.unwrap_err(), ConfigError::Missing("LLM_BASE_URL"));
}

#[test]
fn a_missing_model_is_named() {
    let result = Config::from_lookup(lookup(&[
        ("LLM_BASE_URL", "https://example.test/v1"),
        ("LLM_API_KEY", "k"),
    ]));
    assert_eq!(result.unwrap_err(), ConfigError::Missing("LLM_MODEL"));
}

#[test]
fn a_blank_variable_counts_as_missing() {
    let result = Config::from_lookup(lookup(&[
        ("LLM_BASE_URL", "  "),
        ("LLM_API_KEY", "k"),
        ("LLM_MODEL", "m"),
    ]));
    assert_eq!(result.unwrap_err(), ConfigError::Missing("LLM_BASE_URL"));
}

#[test]
fn three_variables_give_back_exactly_three_values() {
    let config = config_from("https://example.test/v1");
    assert_eq!(config.base_url(), "https://example.test/v1");
    assert_eq!(config.api_key(), "k");
    assert_eq!(config.model(), "m");
}

#[test]
fn a_non_http_scheme_is_refused() {
    let result = Config::from_lookup(lookup(&[
        ("LLM_BASE_URL", "ftp://example.test/v1"),
        ("LLM_API_KEY", "k"),
        ("LLM_MODEL", "m"),
    ]));
    assert_eq!(
        result.unwrap_err(),
        ConfigError::BadBaseUrl("ftp://example.test/v1".to_string())
    );
}

#[test]
fn a_config_never_prints_its_key() {
    let config = Config::from_lookup(lookup(&[
        ("LLM_BASE_URL", "https://example.test/v1"),
        ("LLM_API_KEY", "sk-super-secret"),
        ("LLM_MODEL", "m"),
    ]))
    .unwrap();

    let printed = format!("{config:?}");
    assert!(!printed.contains("sk-super-secret"), "printed: {printed}");
    assert!(printed.contains("<redacted>"), "printed: {printed}");
    assert!(printed.contains("example.test"), "printed: {printed}");
}

#[test]
fn a_base_url_that_is_not_a_url_is_refused() {
    let result = Config::from_lookup(lookup(&[
        ("LLM_BASE_URL", "not a url"),
        ("LLM_API_KEY", "k"),
        ("LLM_MODEL", "m"),
    ]));
    assert_eq!(
        result.unwrap_err(),
        ConfigError::BadBaseUrl("not a url".to_string())
    );
}

#[test]
fn the_endpoint_keeps_the_base_path() {
    assert_eq!(
        config_from("https://api.deepseek.com/v1").endpoint(),
        "https://api.deepseek.com/v1/chat/completions"
    );
}

#[test]
fn a_trailing_slash_does_not_double_up() {
    assert_eq!(
        config_from("https://api.deepseek.com/v1/").endpoint(),
        "https://api.deepseek.com/v1/chat/completions"
    );
}

// -- request: model and messages; temperature is a constant ----------------

#[test]
fn a_request_carries_the_system_prompt_then_the_task() {
    let request = Request::new("m", "be brief", "reply with the word ok").unwrap();

    assert_eq!(request.model(), "m");
    assert_eq!(
        request.messages(),
        [
            Message::system("be brief"),
            Message::user("reply with the word ok"),
        ]
    );
}

#[test]
fn an_empty_task_is_refused_rather_than_sent() {
    let result = Request::new("m", "system", "   ");
    assert_eq!(result.unwrap_err(), RequestError::EmptyTask);
}

#[test]
fn the_task_is_trimmed() {
    let request = Request::new("m", "s", "  hello  ").unwrap();
    assert_eq!(request.messages()[1], Message::user("hello"));
}

// -- reply: text plus token counts, and nothing else ----------------------

const GOOD: &str = r#"{
    "id": "chat-1",
    "object": "chat.completion",
    "model": "deepseek-chat",
    "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
    "usage": {"prompt_tokens": 27, "completion_tokens": 1, "total_tokens": 28}
}"#;

#[test]
fn a_reply_carries_the_text_and_the_two_counts() {
    let reply = Reply::from_json(GOOD).unwrap();
    assert_eq!(reply.text(), Some("ok"));
    assert_eq!(reply.usage().input(), 27);
    assert_eq!(reply.usage().output(), 1);
}

#[test]
fn the_total_is_derived_not_trusted_from_the_wire() {
    let lying = r#"{"choices":[{"message":{"content":"ok"}}],
                    "usage":{"prompt_tokens":27,"completion_tokens":1,"total_tokens":9999}}"#;
    assert_eq!(Reply::from_json(lying).unwrap().usage().total(), 28);
}

#[test]
fn a_reply_without_usage_is_refused_because_cost_is_the_point() {
    let no_usage = r#"{"choices":[{"message":{"content":"ok"}}]}"#;
    assert_eq!(Reply::from_json(no_usage).unwrap_err(), ReplyError::NoUsage);
}

#[test]
fn a_reply_with_no_choices_is_refused() {
    let empty = r#"{"choices":[],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#;
    assert_eq!(Reply::from_json(empty).unwrap_err(), ReplyError::NoChoices);
}

#[test]
fn a_reply_with_neither_text_nor_calls_is_refused() {
    let empty = r#"{"choices":[{"message":{}}],
                    "usage":{"prompt_tokens":1,"completion_tokens":1}}"#;
    assert_eq!(Reply::from_json(empty).unwrap_err(), ReplyError::Empty);
}

#[test]
fn a_reply_that_is_not_json_is_refused() {
    assert!(matches!(
        Reply::from_json("<html>502 Bad Gateway</html>"),
        Err(ReplyError::Malformed(_))
    ));
}

#[test]
fn the_report_shows_the_answer_and_what_it_cost() {
    let report = Reply::from_json(GOOD).unwrap().report();
    assert!(report.starts_with("ok"), "report was: {report}");
    assert!(report.contains("27 in"), "report was: {report}");
    assert!(report.contains("1 out"), "report was: {report}");
    assert!(report.contains("28 total"), "report was: {report}");
}

// -- prices: optional, but never half a price -----------------------------

#[test]
fn prices_are_absent_when_not_configured() {
    assert_eq!(config_from("https://example.test/v1").prices(), None);
}

#[test]
fn half_a_price_is_refused() {
    let result = Config::from_lookup(lookup(&[
        ("LLM_BASE_URL", "https://example.test/v1"),
        ("LLM_API_KEY", "k"),
        ("LLM_MODEL", "m"),
        ("LLM_PRICE_IN", "1.5"),
    ]));
    assert_eq!(
        result.unwrap_err(),
        ConfigError::IncompletePrices("LLM_PRICE_IN", "LLM_PRICE_OUT")
    );
}

#[test]
fn both_halves_give_a_price_per_million_tokens() {
    let config = Config::from_lookup(lookup(&[
        ("LLM_BASE_URL", "https://example.test/v1"),
        ("LLM_API_KEY", "k"),
        ("LLM_MODEL", "m"),
        ("LLM_PRICE_IN", "0.27"),
        ("LLM_PRICE_OUT", "1.10"),
    ]))
    .unwrap();

    let prices: Prices = config.prices().expect("prices parsed");
    assert_eq!(prices.input_per_mtok, 0.27);
    assert_eq!(prices.output_per_mtok, 1.10);
    // a million in, a million out
    assert!((prices.cost(1_000_000, 1_000_000) - 1.37).abs() < 1e-9);
    // 2,605 in, 142 out
    let small = prices.cost(2_605, 142);
    assert!((small - (2_605.0 * 0.27 + 142.0 * 1.10) / 1e6).abs() < 1e-12);
}

#[test]
fn a_price_that_is_not_a_number_is_refused() {
    let result = Config::from_lookup(lookup(&[
        ("LLM_BASE_URL", "https://example.test/v1"),
        ("LLM_API_KEY", "k"),
        ("LLM_MODEL", "m"),
        ("LLM_PRICE_IN", "cheap"),
        ("LLM_PRICE_OUT", "1.10"),
    ]));
    assert_eq!(
        result.unwrap_err(),
        ConfigError::BadPrice("LLM_PRICE_IN", "cheap".to_string())
    );
}
