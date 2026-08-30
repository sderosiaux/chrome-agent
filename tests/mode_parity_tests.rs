//! The three dispatch modes agree on which commands carry a change report.
//!
//! `mutates_page` (`pipe_report.rs`) is the single allowlist: a mutating command owes the
//! caller `changed`/`delta`/`verdict`, everything else answers plainly. Pipe and batch
//! already obeyed it; the CLI attached the full observation machinery to *every* command
//! routed through `output_action` — so `wait`, `frame` and `forward` returned a
//! structurally different JSON shape depending on which mode ran them. An agent script
//! ported between modes silently gained or lost the `verdict` field it keyed on.

use std::io::Write as _;
use std::process::{Command, Stdio};

use serde_json::Value;

mod common;
use common::TestBrowser;

fn binary() -> String {
    let mut path = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
    path.push("chrome-agent");
    path.to_string_lossy().into_owned()
}

fn run_cli(args: &[&str]) -> (String, i32) {
    let output = Command::new(binary()).args(args).output().expect("Failed to run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

fn run_batch(browser: &str, commands_json: &str) -> Value {
    let mut child = Command::new(binary())
        .args(["--browser", browser, "--json", "batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn batch");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(commands_json.as_bytes())
        .expect("write batch input");
    let output = child.wait_with_output().expect("batch output");
    serde_json::from_slice(&output.stdout).expect("batch JSON")
}

/// Establish a page and a baseline snapshot, so that a drifting CLI would have
/// everything it needs to attach a change report to the next command.
fn setup(browser: &str) -> bool {
    if !common::browser_ready() {
        return false;
    }
    let url = common::fixture_url("extract_cards.html");
    let (_, code) = run_cli(&["--browser", browser, "goto", &url]);
    if code != 0 {
        return common::unavailable("goto extract_cards.html failed");
    }
    let (_, code) = run_cli(&["--browser", browser, "inspect"]);
    assert_eq!(code, 0, "inspect should establish the baseline");
    true
}

fn assert_no_observation(v: &Value, context: &str) {
    assert_eq!(v["ok"], true, "{context}: {v}");
    for key in ["verdict", "verdict_reason", "changed", "delta"] {
        assert!(
            v.get(key).is_none(),
            "{context}: non-mutating command must not carry `{key}` (pipe/batch never attach it): {v}"
        );
    }
}

#[test]
fn wait_answers_with_the_same_shape_in_cli_and_batch() {
    let b = TestBrowser::new("parity-wait");
    if !setup(b.name()) {
        return;
    }
    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "wait", "selector", "body"]);
    assert_eq!(code, 0, "wait should succeed: {stdout}");
    let cli: Value = serde_json::from_str(&stdout).expect("JSON response");
    assert_no_observation(&cli, "CLI wait");

    let batch = run_batch(b.name(), r#"[{"cmd":"wait","what":"selector","pattern":"body"}]"#);
    let results = batch["results"].as_array().expect("batch results");
    assert_no_observation(&results[0], "batch wait");
}

#[test]
fn frame_answers_with_the_same_shape_in_cli_and_batch() {
    let b = TestBrowser::new("parity-frame");
    if !setup(b.name()) {
        return;
    }
    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "frame", "main"]);
    assert_eq!(code, 0, "frame main should succeed: {stdout}");
    let cli: Value = serde_json::from_str(&stdout).expect("JSON response");
    assert_no_observation(&cli, "CLI frame");

    let batch = run_batch(b.name(), r#"[{"cmd":"frame","target":"main"}]"#);
    let results = batch["results"].as_array().expect("batch results");
    assert_no_observation(&results[0], "batch frame");
}

#[test]
fn forward_answers_with_the_same_shape_in_cli_and_batch() {
    let b = TestBrowser::new("parity-forward");
    if !setup(b.name()) {
        return;
    }
    // Build history: page A -> page B -> back, so forward has somewhere to go.
    let url_b = common::fixture_url("form_value_microtask_revert.html");
    let (_, code) = run_cli(&["--browser", b.name(), "goto", &url_b]);
    assert_eq!(code, 0, "goto page B should succeed");
    let (_, code) = run_cli(&["--browser", b.name(), "back"]);
    assert_eq!(code, 0, "back should succeed");

    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "forward"]);
    assert_eq!(code, 0, "forward should succeed: {stdout}");
    let cli: Value = serde_json::from_str(&stdout).expect("JSON response");
    assert_no_observation(&cli, "CLI forward");

    // uid_map hygiene, same as `back` and `goto`: the document was replaced, so a
    // uid from the old page must answer "not found", not resolve into the new one.
    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "click", "n999999"]);
    assert!(code != 0 || stdout.contains("not found"), "stale uid must not survive forward: {stdout}");
}
