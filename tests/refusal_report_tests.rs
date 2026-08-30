//! What an action says when it dispatched nothing, across the hit test's refusal path.
//!
//! Two halves: a stable miss must not be classified as a transient one, and `--on-intercept
//! refuse` must carry `hint`, `intercepted_by` and `next`, not just prose in `error`.

use std::io::Write as _;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

mod common;
use common::TestBrowser;

fn run_cli(args: &[&str]) -> (String, i32) {
    let output = Command::new(common::binary()).args(args).output().expect("Failed to run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

fn run_batch(browser: &str, commands: &Value) -> Value {
    let mut child = Command::new(common::binary())
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
        .write_all(commands.to_string().as_bytes())
        .expect("write batch input");
    let output = child.wait_with_output().expect("batch output");
    serde_json::from_slice(&output.stdout).expect("batch JSON")
}

fn open(browser: &str, fixture: &str) -> bool {
    if !common::browser_ready() {
        return false;
    }
    let url = common::fixture_url(fixture);
    let (_, code) = run_cli(&["--browser", browser, "goto", &url]);
    if code != 0 {
        return common::unavailable(&format!("goto {fixture} failed"));
    }
    let (_, code) = run_cli(&["--browser", browser, "--json", "inspect"]);
    if code != 0 {
        return common::unavailable(&format!("inspect {fixture} failed"));
    }
    true
}

fn eval(browser: &str, expression: &str) -> Value {
    let (stdout, code) = run_cli(&["--browser", browser, "--json", "eval", expression]);
    assert_eq!(code, 0, "eval failed: {stdout}");
    let v: Value = serde_json::from_str(&stdout).expect("JSON eval response");
    v["result"].clone()
}

/// A live pipe session: one command in, one response out, connection state preserved.
struct PipeSession {
    child: std::process::Child,
    responses: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
}

impl PipeSession {
    fn start(browser: &str) -> Self {
        use std::io::BufRead as _;
        let mut child = Command::new(common::binary())
            .args(["--browser", browser, "pipe"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn pipe");
        let stdout = child.stdout.take().expect("pipe stdout");
        Self { child, responses: std::io::BufReader::new(stdout).lines() }
    }

    fn send(&mut self, cmd: &Value) -> Value {
        let stdin = self.child.stdin.as_mut().expect("pipe stdin");
        writeln!(stdin, "{cmd}").expect("write command");
        stdin.flush().expect("flush command");
        let line = self
            .responses
            .next()
            .expect("a response per command")
            .expect("readable response");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad pipe line {line}: {e}"))
    }
}

impl Drop for PipeSession {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.wait();
    }
}

// --- 1. A miss that will not change on its own ---

/// A `position: fixed` wall above the viewport on a document that cannot scroll: every
/// reading agrees with the last, and every one is off screen.
#[test]
fn a_pinned_control_above_the_viewport_is_a_stable_miss_not_a_transient_one() {
    let b = TestBrowser::new("refusal-pinned");
    if !open(b.name(), "fixed_wall_above_viewport.html") {
        return;
    }
    let uid = uid_for(b.name(), &["button", "Reject all cookies"]);
    let response = click_json(b.name(), &[uid.as_str()]);

    assert_eq!(
        response["delivery"], "off_target",
        "the readings converged, so the point is not \"still moving\": {response}"
    );
    assert_eq!(response["verdict"], "unknown", "{response}");
    assert_eq!(response["verdict_reason"], "aim_point_off_target", "{response}");
    assert_eq!(
        response["next"], "inspect",
        "a stable miss may never answer `retry` — the retry measures the same coordinate: {response}"
    );
    assert!(
        response["message"].as_str().unwrap_or_default().starts_with("Did not click"),
        "nothing was dispatched, so the message may not say \"Clicked\": {response}"
    );
    assert_eq!(
        eval(b.name(), "window.receiver"),
        Value::Null,
        "refusing to aim means refusing to dispatch"
    );

    // The property the classification rests on: the scroll succeeds and moves nothing.
    let (_, code) = run_cli(&["--browser", b.name(), "scroll", uid.as_str()]);
    assert_eq!(code, 0, "the scroll itself succeeds — and moves nothing");
    let again = click_json(b.name(), &[uid.as_str()]);
    assert_eq!(
        again["verdict_reason"], "aim_point_off_target",
        "the second attempt measures the same point as the first: {again}"
    );
    assert_eq!(again["aim"], response["aim"], "to the pixel: {again} vs {response}");
    assert!(
        again["aim"][1].as_f64().unwrap_or(0.0) < 0.0,
        "the aim point is above the top edge of the viewport: {again}"
    );
}

/// The same stable miss in the other axis: past the LEFT edge, y inside the viewport.
#[test]
fn a_control_pinned_past_the_left_edge_is_the_same_stable_miss() {
    let b = TestBrowser::new("refusal-drawer");
    if !open(b.name(), "fixed_wall_above_viewport.html") {
        return;
    }
    let uid = uid_for(b.name(), &["button", "Reject from the drawer"]);
    let response = click_json(b.name(), &[uid.as_str()]);

    assert_eq!(response["delivery"], "off_target", "{response}");
    assert_eq!(response["verdict_reason"], "aim_point_off_target", "{response}");
    assert_eq!(response["next"], "inspect", "{response}");
    let aim = &response["aim"];
    assert!(
        aim[0].as_f64().unwrap_or(0.0) < 0.0,
        "this one is off the left edge, not the top: {response}"
    );
    assert!(
        aim[1].as_f64().unwrap_or(-1.0) >= 0.0,
        "and its y is inside the viewport, which is what makes it a different reading: {response}"
    );
    assert_eq!(eval(b.name(), "window.receiver"), Value::Null, "nothing was dispatched");
}

/// The transient shape still answers `retry`, the one rung that licenses a repeat.
#[test]
fn a_point_still_moving_keeps_its_retry() {
    let b = TestBrowser::new("refusal-smooth");
    if !open(b.name(), "smooth_scroll_click.html") {
        return;
    }
    let response = click_json(b.name(), &["--selector", "#target"]);
    if response["delivery"] == "not_settled" {
        assert_eq!(response["verdict_reason"], "scroll_not_settled", "{response}");
        assert_eq!(response["next"], "retry", "{response}");
    } else {
        // The scroll finished before the probe: no settle to assert about.
        assert_ne!(response["verdict_reason"], "scroll_not_settled", "{response}");
    }
}

// --- 2. A refusal that names what it refused ---

/// Every field the caller has to branch on, in the mode that dispatches nothing.
fn assert_refusal_payload(response: &Value, mode: &str) {
    assert_eq!(response["ok"], Value::Bool(false), "{mode}: {response}");
    assert_eq!(response["delivery"], "intercepted", "{mode}: {response}");
    assert_eq!(
        response["dispatched"], Value::Bool(false),
        "{mode}: a refusal has to say that nothing was sent: {response}"
    );
    assert_eq!(response["intercepted_by"]["id"], "scrim", "{mode}: {response}");
    assert_eq!(response["intercepted_by"]["tag"], "DIV", "{mode}: {response}");
    assert!(
        response["uid"].is_string(),
        "{mode}: a refusal names the node it aimed at, like every other targeted action: {response}"
    );
    assert_eq!(response["verdict"], "intercepted", "{mode}: {response}");
    assert_eq!(response["verdict_reason"], "hit_test_receiver", "{mode}: {response}");
    assert_eq!(
        response["next"], "dismiss",
        "{mode}: the receiver is what stands between the caller and the action: {response}"
    );
    let hint = response["hint"].as_str().unwrap_or_default();
    assert!(
        hint.contains("div#scrim"),
        "{mode}: every error carries a hint, and this one can name the element: {hint}"
    );
    assert!(
        hint.contains("chrome-agent"),
        "{mode}: the hint owes one imperative command: {hint}"
    );
    assert!(
        response["error"].as_str().unwrap_or_default().contains("div#scrim"),
        "{mode}: {response}"
    );
}

#[test]
fn a_refused_interception_carries_the_payload_a_dispatch_would_have() {
    let b = TestBrowser::new("refusal-cli");
    if !open(b.name(), "click_overlay.html") {
        return;
    }
    let _ = eval(b.name(), "window.receiver = null; 1");
    let (stdout, code) = run_cli(&[
        "--browser", b.name(), "--json", "--on-intercept", "refuse",
        "click", "--selector", "#target",
    ]);
    assert_ne!(code, 0, "a refusal is a failure the caller has to handle: {stdout}");
    let response: Value = serde_json::from_str(&stdout).expect("JSON error response");
    assert_refusal_payload(&response, "cli");
    assert_eq!(
        eval(b.name(), "window.receiver"),
        Value::Null,
        "refuse means nothing was dispatched — not even to the overlay"
    );
}

#[test]
fn pipe_and_batch_refuse_with_the_same_fields_as_the_cli() {
    let b = TestBrowser::new("refusal-modes");
    if !common::browser_ready() {
        return;
    }
    let url = common::fixture_url("click_overlay.html");

    let mut session = PipeSession::start(b.name());
    session.send(&json!({"cmd": "goto", "url": url}));
    session.send(&json!({"cmd": "inspect"}));
    let refused = session.send(&json!({
        "cmd": "click", "selector": "#target", "on_intercept": "refuse"
    }));
    assert_refusal_payload(&refused, "pipe");
    let receiver = session.send(&json!({"cmd": "eval", "expression": "window.receiver"}));
    assert_eq!(receiver["result"], Value::Null, "pipe: {receiver}");
    drop(session);

    let batch = run_batch(
        b.name(),
        &json!([
            {"cmd": "goto", "url": url},
            {"cmd": "inspect"},
            {"cmd": "click", "selector": "#target", "on_intercept": "refuse"}
        ]),
    );
    let results = batch["results"].as_array().expect("batch results");
    assert_refusal_payload(&results[2], "batch");
}

/// The uid the current snapshot gives the node whose line contains every needle.
fn uid_for(browser: &str, needles: &[&str]) -> String {
    let (stdout, code) = run_cli(&["--browser", browser, "--json", "inspect"]);
    assert_eq!(code, 0, "{stdout}");
    let snapshot: Value = serde_json::from_str(&stdout).expect("JSON inspect");
    let text = snapshot["snapshot"].as_str().unwrap_or_default().to_string();
    text.lines()
        .find(|line| needles.iter().all(|n| line.contains(n)))
        .and_then(|line| line.trim_start().strip_prefix("uid="))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no uid matching {needles:?} in:\n{text}"))
        .to_string()
}

fn click_json(browser: &str, args: &[&str]) -> Value {
    let mut argv = vec!["--browser", browser, "--json", "click"];
    argv.extend_from_slice(args);
    let (stdout, code) = run_cli(&argv);
    assert_eq!(code, 0, "click failed: {stdout}");
    serde_json::from_str(&stdout).expect("JSON click response")
}
