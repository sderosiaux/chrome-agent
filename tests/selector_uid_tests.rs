//! A selector-targeted action echoes the uid it resolved, so the change report's uids and the
//! node the action aimed at are comparable.

use std::io::Write as _;
use std::process::{Command, Stdio};

use serde_json::Value;

mod common;
use common::TestBrowser;

fn run_cli(args: &[&str]) -> (String, i32) {
    let output = Command::new(common::binary())
        .args(args)
        .output()
        .expect("Failed to run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
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

/// The echoed uid must be the uid the snapshot gives that element.
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

#[test]
fn the_reported_uid_can_be_matched_against_the_delta() {
    let b = TestBrowser::new("selector-uid-delta");
    if !open(b.name(), "form_value_plain_input.html") {
        return;
    }
    let v = act(b.name(), &["fill", "--selector", "#plain", "hello"]);
    let uid = v["uid"].as_str().unwrap_or_default().to_string();
    assert!(
        !uid.is_empty(),
        "a fill by selector names its node too: {v}"
    );
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
    // Unique per process: a fixed name would let a concurrent run drive the same browser.
    let guard = TestBrowser::new("selector-uid-pipe");
    let browser = guard.name().to_string();
    let mut child = Command::new(common::binary())
        .args(["--browser", &browser, "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pipe");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("pipe output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last: Value = stdout
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("JSON"))
        .expect("a click response");

    assert!(
        last["uid"].as_str().is_some_and(|u| u.starts_with('n')),
        "pipe must name the resolved node: {last}"
    );
}

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

/// Pipe must echo the uid for `select` and `dblclick` too, not only for click and fill.
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
    // Unique per process: a fixed name would let a concurrent run drive the same browser.
    let guard = TestBrowser::new("selector-uid-pipe-all");
    let browser = guard.name().to_string();
    let mut child = Command::new(common::binary())
        .args(["--browser", &browser, "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pipe");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("pipe output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let responses: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("JSON"))
        .collect();

    assert_eq!(
        responses.len(),
        3,
        "expected one response per command: {stdout}"
    );
    for (name, response) in [("select", &responses[1]), ("dblclick", &responses[2])] {
        assert_eq!(response["ok"], true, "{name} should succeed: {response}");
        assert!(
            response["uid"].as_str().is_some_and(|u| u.starts_with('n')),
            "pipe {name} must name the resolved node: {response}"
        );
    }
}

#[test]
fn selector_identity_and_fill_use_the_same_resolved_node() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("selector-one-handle-fill");
    let url = common::fixture_url("form_value_plain_input.html");
    let setup = r#"(() => {
        document.body.innerHTML = '<input id="first" class="flip" name="first"><input id="second" class="flip" name="second">';
        const original = document.querySelector.bind(document);
        let calls = 0;
        document.querySelector = selector => selector === '.flip'
            ? (++calls % 2 ? document.getElementById('first') : document.getElementById('second'))
            : original(selector);
        return true;
    })()"#;
    let script = format!(
        "{}\n{}\n{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": url}),
        serde_json::json!({"cmd": "eval", "expression": setup}),
        serde_json::json!({"cmd": "fill", "selector": ".flip", "value": "WRITTEN"}),
        serde_json::json!({"cmd": "eval", "expression": "({first: document.getElementById('first').value, second: document.getElementById('second').value})"}),
    );
    let mut child = Command::new(common::binary())
        .args(["--browser", guard.name(), "--verdict", "off", "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pipe");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("pipe output");
    let responses: Vec<Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("JSON"))
        .collect();
    assert_eq!(
        responses.len(),
        4,
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(responses[2]["ok"], true, "{}", responses[2]);
    assert_eq!(responses[2]["name"], "first", "{}", responses[2]);
    assert_eq!(
        responses[3]["result"]["first"], "WRITTEN",
        "{}",
        responses[3]
    );
    assert_eq!(responses[3]["result"]["second"], "", "{}", responses[3]);
}
