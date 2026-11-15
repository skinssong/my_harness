//! S2's shapes: the four messages a role can hide, and a reply that asks for
//! work instead of answering.

use harness::message::{Message, ToolCall};
use harness::reply::{Reply, ReplyError, Usage};
use harness::request::Request;

fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

fn json(message: Message) -> serde_json::Value {
    serde_json::to_value(message).unwrap()
}

// -- messages -------------------------------------------------------------

#[test]
fn a_plain_message_carries_its_role_and_content() {
    assert_eq!(
        json(Message::system("be brief")),
        serde_json::json!({"role": "system", "content": "be brief"})
    );
    assert_eq!(
        json(Message::user("read greeting.txt")),
        serde_json::json!({"role": "user", "content": "read greeting.txt"})
    );
}

#[test]
fn an_assistant_answer_has_no_tool_calls_key_at_all() {
    let value = json(Message::assistant("it says hello"));
    assert_eq!(value["role"], "assistant");
    assert_eq!(value["content"], "it says hello");
    assert!(
        value.get("tool_calls").is_none(),
        "an empty call list must not be sent: {value}"
    );
}

#[test]
fn an_assistant_requesting_work_carries_null_content_and_the_calls() {
    let value = json(Message::calling(
        vec![call("call_1", "read_file", "{\"path\":\"greeting.txt\"}")],
        None,
    ));

    assert_eq!(value["role"], "assistant");
    assert!(value["content"].is_null(), "content must be null: {value}");
    assert_eq!(value["tool_calls"][0]["id"], "call_1");
    assert_eq!(value["tool_calls"][0]["type"], "function");
    assert_eq!(value["tool_calls"][0]["function"]["name"], "read_file");
    assert_eq!(
        value["tool_calls"][0]["function"]["arguments"],
        "{\"path\":\"greeting.txt\"}",
        "arguments travel as a JSON string, not as an object"
    );
}

#[test]
fn a_result_is_owed_back_under_the_id_that_asked() {
    assert_eq!(
        json(Message::tool_result("call_1", "read_file", "hello from disk\n")),
        serde_json::json!({
            "role": "tool",
            "content": "hello from disk\n",
            "tool_call_id": "call_1",
            "name": "read_file"
        })
    );
}

#[test]
fn only_an_assistant_message_reports_calls() {
    assert_eq!(Message::calling(vec![call("a", "read_file", "{}")], None).calls().len(), 1);
    assert!(Message::user("hi").calls().is_empty());
    assert!(Message::assistant("hi").calls().is_empty());
}

// -- reply: answering or asking -------------------------------------------

const CALLING: &str = r#"{
  "choices": [{"index": 0, "finish_reason": "tool_calls", "message": {
    "role": "assistant",
    "content": null,
    "tool_calls": [{"id": "call_1", "type": "function",
      "function": {"name": "read_file", "arguments": "{\"path\":\"greeting.txt\"}"}}]
  }}],
  "usage": {"prompt_tokens": 212, "completion_tokens": 31, "total_tokens": 243}
}"#;

#[test]
fn a_tool_calling_reply_is_not_refused_for_having_no_text() {
    let reply = Reply::from_json(CALLING).unwrap();
    assert!(!reply.is_answer());
    assert_eq!(reply.text(), None);
    assert_eq!(reply.calls().len(), 1);
    assert_eq!(reply.calls()[0].name, "read_file");
    assert_eq!(reply.calls()[0].arguments, "{\"path\":\"greeting.txt\"}");
    assert_eq!(reply.usage().total(), 243);
}

#[test]
fn several_calls_in_one_turn_are_all_carried() {
    let two = r#"{"choices":[{"message":{"content":null,"tool_calls":[
        {"id":"a","function":{"name":"read_file","arguments":"{\"path\":\"a\"}"}},
        {"id":"b","function":{"name":"read_file","arguments":"{\"path\":\"b\"}"}}]}}],
        "usage":{"prompt_tokens":1,"completion_tokens":1}}"#;
    let reply = Reply::from_json(two).unwrap();
    let ids: Vec<&str> = reply.calls().iter().map(|call| call.id.as_str()).collect();
    assert_eq!(ids, ["a", "b"]);
}

#[test]
fn text_beside_calls_is_kept_but_does_not_make_it_an_answer() {
    let with_text = CALLING.replace("\"content\": null", "\"content\": \"let me look\"");
    let reply = Reply::from_json(&with_text).unwrap();
    assert_eq!(reply.text(), Some("let me look"));
    assert!(!reply.is_answer());
}

#[test]
fn a_calling_reply_still_owes_its_cost() {
    let no_usage = r#"{"choices":[{"message":{"content":null,
        "tool_calls":[{"id":"a","function":{"name":"read_file","arguments":"{}"}}]}}]}"#;
    assert_eq!(Reply::from_json(no_usage).unwrap_err(), ReplyError::NoUsage);
}

#[test]
fn the_report_for_a_calling_reply_names_what_it_asked_for() {
    let report = Reply::from_json(CALLING).unwrap().report();
    assert!(report.contains("1 call"), "report was: {report}");
    assert!(report.contains("read_file"), "report was: {report}");
}

// -- carrying a conversation ----------------------------------------------

#[test]
fn continuing_a_request_appends_rather_than_replaces() {
    let first = Request::new("m", "system", "read it").unwrap();
    let second = first.continued(vec![
        Message::calling(vec![call("call_1", "read_file", "{\"path\":\"a\"}")], None),
        Message::tool_result("call_1", "read_file", "hello"),
    ]);

    assert_eq!(first.messages().len(), 2, "the first request is untouched");
    assert_eq!(second.messages().len(), 4);
    assert_eq!(second.messages()[0], Message::system("system"));
    assert_eq!(second.messages()[3], Message::tool_result("call_1", "read_file", "hello"));
}

#[test]
fn usage_adds_up_across_calls() {
    let a = Usage::new(44, 24);
    let b = Usage::new(268, 12);
    assert_eq!(a.plus(b).total(), 348);
}
