//! S1 end to end: argv and environment in, a report or a failure out.
//!
//! These run the real binary. The provider is the stub in `common`, so the whole
//! path is exercised — argument joining, config from the environment, the POST,
//! the report, the exit code — with no key and no network.

mod common;

use std::process::Command;

use common::{response, serve_many, serve_once, TempWorkspace};

const BIN: &str = env!("CARGO_BIN_EXE_harness");

const BODY: &str = r#"{"choices":[{"message":{"content":"ok"}}],"usage":{"prompt_tokens":27,"completion_tokens":1}}"#;

#[tokio::test(flavor = "multi_thread")]
async fn one_task_in_one_answer_and_its_cost_out() {
    let (addr, seen) = serve_once(response("200 OK", BODY)).await;

    let output = tokio::process::Command::new(BIN)
        .args(["reply", "with", "the", "word", "ok"])
        .env("LLM_BASE_URL", format!("http://{addr}/v1"))
        .env("LLM_API_KEY", "k")
        .env("LLM_MODEL", "m")
        .output()
        .await
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "exit was non-zero. stderr: {stderr}"
    );
    assert!(stdout.contains("ok"), "stdout: {stdout}");
    assert!(stdout.contains("27 in"), "stdout: {stdout}");
    assert!(stdout.contains("1 out"), "stdout: {stdout}");

    let seen = seen.await.unwrap();
    assert!(
        seen.contains("reply with the word ok"),
        "the unquoted arguments should reach the model as one task: {seen}"
    );
}

#[test]
fn no_task_prints_usage_and_fails() {
    let output = Command::new(BIN)
        .current_dir(std::env::temp_dir())
        .env_remove("LLM_BASE_URL")
        .env_remove("LLM_API_KEY")
        .env_remove("LLM_MODEL")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("usage:"), "stderr: {stderr}");
}

#[test]
fn a_missing_setting_is_named() {
    let output = Command::new(BIN)
        .arg("hi")
        .current_dir(std::env::temp_dir())
        .env_remove("LLM_BASE_URL")
        .env_remove("LLM_API_KEY")
        .env_remove("LLM_MODEL")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LLM_BASE_URL"), "stderr: {stderr}");
}

#[test]
fn help_is_answered_without_a_call() {
    let output = Command::new(BIN)
        .arg("--help")
        .current_dir(std::env::temp_dir())
        .env_remove("LLM_BASE_URL")
        .env_remove("LLM_API_KEY")
        .env_remove("LLM_MODEL")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("usage:"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_is_recorded_and_replays_with_no_credentials_at_all() {
    let (addr, _seen) = serve_many(vec![response("200 OK", BODY)]).await;
    let dir = TempWorkspace::new("cli-record");
    let runs = dir.path.join("runs");

    let output = tokio::process::Command::new(BIN)
        .args([
            "--workspace",
            dir.path.to_str().unwrap(),
            "--log",
            runs.to_str().unwrap(),
            "say",
            "ok",
        ])
        .env("LLM_BASE_URL", format!("http://{addr}/v1"))
        .env("LLM_API_KEY", "k")
        .env("LLM_MODEL", "m")
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let record = stdout
        .lines()
        .find_map(|line| line.strip_prefix("record "))
        .expect("the run prints where it recorded itself")
        .to_string();

    // Replay with no provider settings in the environment at all.
    let replayed = Command::new(BIN)
        .args(["replay", &record])
        .current_dir(std::env::temp_dir())
        .env_remove("LLM_BASE_URL")
        .env_remove("LLM_API_KEY")
        .env_remove("LLM_MODEL")
        .output()
        .unwrap();

    assert!(
        replayed.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&replayed.stderr)
    );
    let replayed_stdout = String::from_utf8_lossy(&replayed.stdout);
    assert!(
        replayed_stdout.contains("replayed 1 turn(s)"),
        "stdout: {replayed_stdout}"
    );
    assert!(
        replayed_stdout.contains("27 in"),
        "stdout: {replayed_stdout}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_record_defaults_outside_the_workspace() {
    let (addr, _seen) = serve_many(vec![response("200 OK", BODY)]).await;
    let home = TempWorkspace::new("home");
    let work = TempWorkspace::new("work");

    let output = tokio::process::Command::new(BIN)
        .args(["--workspace", work.path.to_str().unwrap(), "say", "ok"])
        .env("HOME", home.path.to_str().unwrap())
        .env("LLM_BASE_URL", format!("http://{addr}/v1"))
        .env("LLM_API_KEY", "k")
        .env("LLM_MODEL", "m")
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let record = stdout
        .lines()
        .find_map(|line| line.strip_prefix("record "))
        .expect("the run prints its record path");

    assert!(
        record.starts_with(home.path.to_str().unwrap()),
        "the record went to {record}, which is not under HOME"
    );
    assert!(
        !record.starts_with(work.path.to_str().unwrap()),
        "the record must not be inside the workspace: {record}"
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("warning"),
        "a default placement should not warn"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_record_inside_the_workspace_is_warned_about() {
    let (addr, _seen) = serve_many(vec![response("200 OK", BODY)]).await;
    let work = TempWorkspace::new("work-warn");

    let output = tokio::process::Command::new(BIN)
        .args([
            "--workspace",
            work.path.to_str().unwrap(),
            "--log",
            work.path.join("runs").to_str().unwrap(),
            "say",
            "ok",
        ])
        .env("LLM_BASE_URL", format!("http://{addr}/v1"))
        .env("LLM_API_KEY", "k")
        .env("LLM_MODEL", "m")
        .output()
        .await
        .unwrap();

    assert!(output.status.success(), "the run still works");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("the record is inside the workspace"),
        "stderr: {stderr}"
    );
}
