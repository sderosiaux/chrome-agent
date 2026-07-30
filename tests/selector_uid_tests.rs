//! A selector-targeted action names the node it actually hit.
//!
//! `--selector` resolves in the page, the message quotes the selector back, and the change
//! report names uids. Nothing connected the two: an agent could not check that the element
//! the delta describes is the element it aimed at, and a selector matching more than one
//! node gave no clue which one was used. Echoing the resolved uid costs one field and makes
//! the whole response cross-checkable.

use std::io::Write as _;
use std::process::{Command, Stdio};

use serde_json::Value;

mod common;

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

struct TestBrowser(String);
impl TestBrowser {
    /// Unique per process. A fixed name means two concurrent runs of this suite drive the same
    /// browser: one navigates while the other clicks a uid from its own snapshot, and both fail
    /// with "Node with given id does not belong to the document". CLAUDE.md documents the
    /// hazard — `--browser <unique>` per agent — and the suites have to obey it too.
    fn new(label: &str) -> Self {
        Self(format!("{label}-{}", std::process::id()))
    }
    fn name(&self) -> &str {
        &self.0
    }
}
impl Drop for TestBrowser {
    fn drop(&mut self) {
        let _ = run_cli(&["--browser", &self.0, "close", "--purge"]);
    }
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
    let (_, code) = run_cli(&["--browser", browser, "inspect"]);
    assert_eq!(code, 0, "baseline");
    true
}

fn act(browser: &str, args: &[&str]) -> Value {
    let mut full: Vec<&str> = vec!["--browser", browser, "--json"];
    full.extend_from_slice(args);
    let (stdout, code) = run_cli(&full);
    assert_eq!(code, 0, "action should succeed: {stdout}");
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}"))
}

/// The uid in the response has to be the uid the snapshot gives that element, or it is
/// just another unverifiable string.
#[test]
fn a_selector_click_names_the_node_it_resolved() {
    let b = TestBrowser::new("selector-uid-click");
    if !open(b.name(), "verdict_states.html") {
        return;
    }
    let (stdout, _) = run_cli(&["--browser", b.name(), "--json", "inspect"]);
    let snapshot: Value = serde_json::from_str(&stdout).expect("JSON inspect");
    let text = snapshot["snapshot"].as_str().unwrap_or_default();
    let expected = text
        .lines()
        .find(|l| l.contains("button") && l.contains("Add a node"))
        .and_then(|l| l.trim_start().strip_prefix("uid="))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no uid for the button in: {text}"))
        .to_string();

    let v = act(b.name(), &["click", "--selector", "#add"]);
    assert_eq!(
        v["uid"].as_str().unwrap_or_default(),
        expected,
        "the response must name the same node the snapshot does: {v}"
    );
}

/// The point of the field: the delta's uids and the action's target become comparable.
#[test]
fn the_reported_uid_can_be_matched_against_the_delta() {
    let b = TestBrowser::new("selector-uid-delta");
    if !open(b.name(), "form_value_plain_input.html") {
        return;
    }
    let v = act(b.name(), &["fill", "--selector", "#plain", "hello"]);
    let uid = v["uid"].as_str().unwrap_or_default().to_string();
    assert!(!uid.is_empty(), "a fill by selector names its node too: {v}");
    let delta = v["delta"].as_str().unwrap_or_default();
    assert!(
        delta.contains(&uid),
        "the changed line should be the node we targeted: uid={uid} delta={delta}"
    );
}

#[test]
fn check_by_selector_names_its_node() {
    let b = TestBrowser::new("selector-uid-check");
    if !open(b.name(), "checkable_kinds.html") {
        return;
    }
    let v = act(b.name(), &["check", "--selector", "#native"]);
    assert!(
        v["uid"].as_str().is_some_and(|u| u.starts_with('n')),
        "check --selector must name its node: {v}"
    );
}

/// Pipe answers the same way, or the field is useless to the mode that needs it most.
#[test]
fn pipe_echoes_the_resolved_uid_too() {
    if !common::browser_ready() {
        return;
    }
    let url = common::fixture_url("verdict_states.html");
    let script = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": url}),
        serde_json::json!({"cmd": "inspect"}),
        serde_json::json!({"cmd": "click", "selector": "#add"}),
    );
    // Unique per process: a fixed name lets a second concurrent run of this suite drive the
    // same browser and clobber this one's page.
    let browser = format!("selector-uid-pipe-{}", std::process::id());
    let mut child = Command::new(binary())
        .args(["--browser", &browser, "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pipe");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("pipe output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last: Value = stdout
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("JSON"))
        .expect("a click response");
    let _ = run_cli(&["--browser", &browser, "close", "--purge"]);

    assert!(
        last["uid"].as_str().is_some_and(|u| u.starts_with('n')),
        "pipe must name the resolved node: {last}"
    );
}

/// Targeting by uid already names its node — the field must not contradict itself.
#[test]
fn a_uid_targeted_action_reports_the_uid_it_was_given() {
    let b = TestBrowser::new("selector-uid-passthrough");
    if !open(b.name(), "verdict_states.html") {
        return;
    }
    let (stdout, _) = run_cli(&["--browser", b.name(), "--json", "inspect"]);
    let snapshot: Value = serde_json::from_str(&stdout).expect("JSON inspect");
    let text = snapshot["snapshot"].as_str().unwrap_or_default();
    let uid = text
        .lines()
        .find(|l| l.contains("button"))
        .and_then(|l| l.trim_start().strip_prefix("uid="))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no button uid in: {text}"))
        .to_string();

    let v = act(b.name(), &["click", &uid]);
    assert_eq!(v["uid"].as_str().unwrap_or_default(), uid, "{v}");
}

/// Every targeted command, not just the ones that happened to get wired.
///
/// `dblclick`, `select` and `upload` attached the resolved uid on the CLI path and not
/// in pipe/batch — the mode CLAUDE.md's own guarantee matters most in, since that is
/// where an agent correlates a change report with the node it aimed at.
#[test]
fn pipe_names_the_node_for_every_targeted_command() {
    if !common::browser_ready() {
        return;
    }
    let url = common::fixture_url("select_controlled_revert.html");
    let script = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": url}),
        serde_json::json!({"cmd": "select", "selector": "#plain", "value": "b"}),
        serde_json::json!({"cmd": "dblclick", "selector": "#plain"}),
    );
    // Unique per process: a fixed name lets a second concurrent run of this suite drive the
    // same browser and clobber this one's page.
    let browser = format!("selector-uid-pipe-all-{}", std::process::id());
    let mut child = Command::new(binary())
        .args(["--browser", &browser, "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pipe");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("pipe output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let responses: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("JSON"))
        .collect();
    let _ = run_cli(&["--browser", &browser, "close", "--purge"]);

    assert_eq!(responses.len(), 3, "expected one response per command: {stdout}");
    for (name, response) in [("select", &responses[1]), ("dblclick", &responses[2])] {
        assert_eq!(response["ok"], true, "{name} should succeed: {response}");
        assert!(
            response["uid"].as_str().is_some_and(|u| u.starts_with('n')),
            "pipe {name} must name the resolved node: {response}"
        );
    }
}
