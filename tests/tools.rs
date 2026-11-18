//! The tools, and the one place a call is executed.
//!
//! Everything here goes through `dispatch`, because that is the seam every
//! later concern — permissions, recording, subagents, remote tools — attaches
//! to. If a behaviour is not reachable through dispatch, it is not reachable.

mod common;

use common::TempWorkspace;

use harness::message::ToolCall;
use harness::tool::{Observation, Registry};
use harness::tools;
use harness::workspace::{Workspace, WorkspaceError};

fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

async fn dispatch(registry: &Registry, workspace: &Workspace, call: &ToolCall) -> Observation {
    registry.dispatch(call, workspace).await
}

// -- what the model is shown ----------------------------------------------

#[test]
fn every_tool_describes_itself_in_the_form_the_model_needs() {
    let specs = tools::builtin().specs();
    let names: Vec<&str> = specs
        .iter()
        .map(|spec| spec["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["read_file", "list_dir", "write_file", "bash"]);

    for spec in &specs {
        assert_eq!(spec["type"], "function");
        assert!(spec["function"]["description"].as_str().unwrap().len() > 20);
        assert_eq!(spec["function"]["parameters"]["type"], "object");
    }
}

#[test]
fn the_schema_is_a_plain_object_without_references() {
    // Some OpenAI-compatible gateways reject `$ref`/`$defs`, and the failure
    // looks like a bad request rather than a bad schema.
    let specs = serde_json::to_string(&tools::builtin().specs()).unwrap();
    assert!(!specs.contains("$ref"), "schema used a reference: {specs}");
    assert!(!specs.contains("$defs"), "schema used definitions: {specs}");

    let read = tools::builtin()
        .specs()
        .into_iter()
        .find(|spec| spec["function"]["name"] == "read_file")
        .unwrap();
    let parameters = &read["function"]["parameters"];
    assert_eq!(parameters["properties"]["path"]["type"], "string");
    assert!(parameters["required"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("path")));
}

// -- the four shapes a dispatch can take ----------------------------------

#[tokio::test]
async fn a_tool_that_does_not_exist_says_which_ones_do() {
    let dir = TempWorkspace::new("unknown");
    let workspace = Workspace::open(&dir.path).unwrap();
    let registry = tools::builtin();

    let observation = dispatch(
        &registry,
        &workspace,
        &call("c1", "delete_everything", "{}"),
    )
    .await;
    assert!(!observation.ok);
    assert!(observation.text.contains("no tool named delete_everything"));
    assert!(observation.text.contains("read_file"));
}

#[tokio::test]
async fn arguments_that_are_not_json_become_an_observation() {
    let dir = TempWorkspace::new("badjson");
    let workspace = Workspace::open(&dir.path).unwrap();

    let observation = dispatch(
        &tools::builtin(),
        &workspace,
        &call("c1", "read_file", "path=x"),
    )
    .await;
    assert!(!observation.ok);
    assert!(observation.text.contains("not valid JSON"));
}

#[tokio::test]
async fn arguments_of_the_wrong_shape_become_an_observation() {
    let dir = TempWorkspace::new("badshape");
    let workspace = Workspace::open(&dir.path).unwrap();

    let observation = dispatch(
        &tools::builtin(),
        &workspace,
        &call("c1", "read_file", "{}"),
    )
    .await;
    assert!(!observation.ok);
    assert!(observation
        .text
        .contains("arguments do not match read_file"));
}

#[tokio::test]
async fn a_call_without_an_id_is_refused_because_its_result_could_not_be_returned() {
    let dir = TempWorkspace::new("noid");
    let workspace = Workspace::open(&dir.path).unwrap();

    let observation = dispatch(&tools::builtin(), &workspace, &call("", "list_dir", "{}")).await;
    assert!(!observation.ok);
    assert!(observation.text.contains("no id"));
}

// -- the tools themselves -------------------------------------------------

#[tokio::test]
async fn a_file_can_be_written_and_read_back() {
    let dir = TempWorkspace::new("roundtrip");
    let workspace = Workspace::open(&dir.path).unwrap();
    let registry = tools::builtin();

    let wrote = dispatch(
        &registry,
        &workspace,
        &call(
            "c1",
            "write_file",
            r#"{"path":"notes/greeting.txt","content":"hello from disk\n"}"#,
        ),
    )
    .await;
    assert!(wrote.ok, "{wrote:?}");
    assert!(wrote.text.contains("wrote 16 bytes"));

    let read = dispatch(
        &registry,
        &workspace,
        &call("c2", "read_file", r#"{"path":"notes/greeting.txt"}"#),
    )
    .await;
    assert!(read.ok, "{read:?}");
    assert_eq!(read.text, "hello from disk");
}

#[tokio::test]
async fn a_long_file_is_windowed_and_says_so() {
    let dir = TempWorkspace::new("window");
    let workspace = Workspace::open(&dir.path).unwrap();
    let registry = tools::builtin();
    let body: String = (1..=10).map(|n| format!("line {n}\n")).collect();
    dispatch(
        &registry,
        &workspace,
        &call(
            "c1",
            "write_file",
            &serde_json::json!({"path": "ten.txt", "content": body}).to_string(),
        ),
    )
    .await;

    let read = dispatch(
        &registry,
        &workspace,
        &call(
            "c2",
            "read_file",
            r#"{"path":"ten.txt","offset":8,"limit":2}"#,
        ),
    )
    .await;
    assert_eq!(
        read.text,
        "line 8\nline 9\n[lines 8-9 of 10; use offset to read more]"
    );
}

#[tokio::test]
async fn a_directory_lists_its_entries_with_directories_marked() {
    let dir = TempWorkspace::new("list");
    let workspace = Workspace::open(&dir.path).unwrap();
    let registry = tools::builtin();
    std::fs::create_dir_all(dir.path.join("sub")).unwrap();
    std::fs::write(dir.path.join("a.txt"), "x").unwrap();

    let listed = dispatch(&registry, &workspace, &call("c1", "list_dir", "{}")).await;
    assert!(listed.ok);
    assert_eq!(listed.text, "a.txt\nsub/");
}

#[tokio::test]
async fn a_command_reports_its_exit_code_and_output() {
    let dir = TempWorkspace::new("bash");
    let workspace = Workspace::open(&dir.path).unwrap();

    let ok = dispatch(
        &tools::builtin(),
        &workspace,
        &call("c1", "bash", r#"{"command":"echo hi"}"#),
    )
    .await;
    assert!(ok.ok);
    assert!(ok.text.starts_with("exit 0"));
    assert!(ok.text.contains("hi"));

    let failed = dispatch(
        &tools::builtin(),
        &workspace,
        &call("c2", "bash", r#"{"command":"exit 3"}"#),
    )
    .await;
    assert!(!failed.ok);
    assert!(failed.text.starts_with("exit 3"));
}

#[tokio::test]
async fn a_command_that_outlives_its_timeout_is_stopped() {
    let dir = TempWorkspace::new("timeout");
    let workspace = Workspace::open(&dir.path).unwrap();

    let observation = dispatch(
        &tools::builtin(),
        &workspace,
        &call("c1", "bash", r#"{"command":"sleep 5","timeout_secs":1}"#),
    )
    .await;
    assert!(!observation.ok);
    assert!(observation.text.contains("timed out"));
}

// -- containment ----------------------------------------------------------

#[tokio::test]
async fn the_file_tools_refuse_to_leave_the_workspace() {
    let dir = TempWorkspace::new("escape");
    let workspace = Workspace::open(&dir.path).unwrap();

    let up = dispatch(
        &tools::builtin(),
        &workspace,
        &call("c1", "read_file", r#"{"path":"../../etc/passwd"}"#),
    )
    .await;
    assert!(!up.ok);
    assert!(up.text.contains("outside the workspace"));

    let absolute = dispatch(
        &tools::builtin(),
        &workspace,
        &call("c2", "read_file", r#"{"path":"/etc/passwd"}"#),
    )
    .await;
    assert!(!absolute.ok);
    assert!(absolute.text.contains("outside the workspace"));
}

#[test]
fn resolving_keeps_paths_inside_the_root() {
    let dir = TempWorkspace::new("resolve");
    let workspace = Workspace::open(&dir.path).unwrap();

    assert!(workspace
        .resolve("a/b.txt")
        .unwrap()
        .starts_with(workspace.root()));
    assert!(workspace
        .resolve("./a/../b.txt")
        .unwrap()
        .starts_with(workspace.root()));
    assert_eq!(
        workspace.resolve("a/../../b").unwrap_err(),
        WorkspaceError::Escapes("a/../../b".to_string())
    );
}

/// Shaping is what the loop sends when a result is too big for the next
/// request. It keeps both ends and says how much went missing.
#[test]
fn shaping_keeps_both_ends_and_says_what_it_dropped() {
    let observation = Observation::ok("x".repeat(1_000)).shaped(100);
    assert!(observation.text.starts_with("xxxxxxxxxx"));
    assert!(observation.text.contains("900 bytes elided"));
    assert!(observation.text.ends_with("xxxxxxxxxx"));
}

#[test]
fn shaping_leaves_a_small_observation_alone() {
    let observation = Observation::ok("short").shaped(100);
    assert_eq!(observation.text, "short");
}

#[test]
fn a_failure_is_an_observation_not_a_panic() {
    let observation = Observation::failed("no such file: x");
    assert!(!observation.ok);
    assert!(observation.text.starts_with("tool_error: "));
}

/// The lexical check cannot see a symlink: with `up` linked to `/`, the path
/// `up/etc/hosts` has no `..` at all yet resolves outside the root.
#[cfg(unix)]
#[tokio::test]
async fn a_symlink_cannot_be_used_to_leave_the_workspace() {
    let dir = TempWorkspace::new("symlink");
    std::os::unix::fs::symlink("/", dir.path.join("up")).unwrap();
    let workspace = Workspace::open(&dir.path).unwrap();

    let through_link = dispatch(
        &tools::builtin(),
        &workspace,
        &call("c1", "read_file", r#"{"path":"up/etc/hosts"}"#),
    )
    .await;
    assert!(!through_link.ok, "{}", through_link.text);
    assert!(
        through_link.text.contains("outside the workspace"),
        "{}",
        through_link.text
    );

    // Folding `..` lexically first means a link cannot be climbed back out of
    // either: this resolves inside the root and simply is not there.
    let climbed = dispatch(
        &tools::builtin(),
        &workspace,
        &call("c2", "read_file", r#"{"path":"up/../etc/hosts"}"#),
    )
    .await;
    assert!(!climbed.ok, "{}", climbed.text);
    assert!(
        !climbed.text.contains("root:"),
        "read a system file: {}",
        climbed.text
    );
}

/// A background child inherits the shell's pipes and can hold them open long
/// after `sh` exits. The tool waits for the shell, so that is not a timeout.
#[tokio::test]
async fn a_child_holding_the_pipe_does_not_look_like_a_timeout() {
    let dir = TempWorkspace::new("background");
    let workspace = Workspace::open(&dir.path).unwrap();

    let observation = dispatch(
        &tools::builtin(),
        &workspace,
        &call(
            "c1",
            "bash",
            r#"{"command":"sleep 5 & echo done","timeout_secs":1}"#,
        ),
    )
    .await;

    assert!(observation.ok, "{}", observation.text);
    assert!(
        observation.text.starts_with("exit 0"),
        "{}",
        observation.text
    );
    assert!(observation.text.contains("done"), "{}", observation.text);
}

/// Output is captured into a fixed window, so a command that prints without
/// end costs a bounded amount of memory instead of the whole stream.
#[tokio::test]
async fn a_command_that_prints_without_end_is_capped() {
    let dir = TempWorkspace::new("flood");
    let workspace = Workspace::open(&dir.path).unwrap();

    let observation = dispatch(
        &tools::builtin(),
        &workspace,
        &call("c1", "bash", r#"{"command":"yes x | head -c 2000000"}"#),
    )
    .await;

    assert!(observation.ok, "{}", observation.text);
    assert!(
        observation.text.contains("bytes elided"),
        "kept {} bytes with no note",
        observation.text.len()
    );
    assert!(
        observation.text.len() < 100_000,
        "kept {} bytes",
        observation.text.len()
    );
}
