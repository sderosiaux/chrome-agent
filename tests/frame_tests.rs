//! `frame` scopes subsequent `eval` and `inspect` to the bound iframe (issue #8).
//!
//! Driven through one `chrome-agent pipe` process: the binding lives on the connection, so a
//! sequence of CLI invocations would not exercise it.

use std::io::Write as _;
use std::process::{Command, Stdio};

use serde_json::Value;

mod common;
use common::TestBrowser;

/// Run `chrome-agent pipe` over the given JSON commands, one parsed `Value` per output line.
fn run_pipe(browser: &str, commands: &[Value]) -> Vec<Value> {
    let mut child = Command::new(common::binary())
        .args(["--browser", browser, "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn chrome-agent pipe");

    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for cmd in commands {
            writeln!(stdin, "{}", serde_json::to_string(cmd).unwrap()).unwrap();
        }
    }

    let output = child.wait_with_output().expect("wait pipe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).unwrap_or_else(|_| Value::String(l.to_string())))
        .collect()
}

#[test]
fn frame_switch_scopes_eval_location_to_iframe() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-frame-eval-loc");
    let browser = guard.name();
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "frame", "target": "iframe"}),
            serde_json::json!({"cmd": "eval", "expression": "location.href"}),
        ],
    );

    assert_eq!(responses.len(), 3, "expected 3 responses: {responses:?}");
    assert_eq!(responses[1]["ok"], Value::Bool(true), "frame switch: {:?}", responses[1]);

    let href = responses[2]["result"].as_str().unwrap_or_default();
    assert!(
        href.contains("frame_child.html"),
        "after frame switch, eval location.href must be the iframe URL, got: {href:?}"
    );
    assert!(
        !href.contains("frame_parent.html"),
        "eval location.href must NOT be the parent URL, got: {href:?}"
    );
}

#[test]
fn frame_switch_scopes_eval_dom_to_iframe() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-frame-eval-dom");
    let browser = guard.name();
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "frame", "target": "iframe"}),
            serde_json::json!({"cmd": "eval", "expression": "document.querySelector('#child-marker').textContent"}),
        ],
    );

    assert_eq!(responses.len(), 3, "responses: {responses:?}");
    let text = responses[2]["result"].as_str().unwrap_or_default();
    assert!(
        text.contains("child-only-marker-xyz"),
        "eval in iframe must see the iframe DOM, got: {:?}",
        responses[2]
    );
}

#[test]
fn frame_switch_scopes_inspect_to_iframe() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-frame-inspect");
    let browser = guard.name();
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "frame", "target": "iframe"}),
            serde_json::json!({"cmd": "inspect"}),
        ],
    );

    assert_eq!(responses.len(), 3, "responses: {responses:?}");
    let snap = responses[2]["snapshot"].as_str().unwrap_or_default();
    assert!(
        snap.contains("CHILD FRAME CONTENT"),
        "inspect after frame switch must show iframe content, got: {snap:?}"
    );
    assert!(
        !snap.contains("PARENT PAGE CONTENT"),
        "inspect after frame switch must NOT show parent content, got: {snap:?}"
    );
}

#[test]
fn frame_main_switches_back_to_top_document() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-frame-main-back");
    let browser = guard.name();
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "frame", "target": "iframe"}),
            serde_json::json!({"cmd": "frame", "target": "main"}),
            serde_json::json!({"cmd": "eval", "expression": "location.href"}),
        ],
    );

    assert_eq!(responses.len(), 4, "responses: {responses:?}");
    let href = responses[3]["result"].as_str().unwrap_or_default();
    assert!(
        href.contains("frame_parent.html"),
        "after 'frame main', eval location.href must be the parent URL again, got: {href:?}"
    );
}

#[test]
fn navigation_resets_frame_binding() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-frame-nav-reset");
    let browser = guard.name();
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "frame", "target": "iframe"}),
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("frame_child.html")}),
            serde_json::json!({"cmd": "eval", "expression": "location.href"}),
        ],
    );

    assert_eq!(responses.len(), 4, "responses: {responses:?}");
    // The iframe's isolated world died with the navigation, so eval must run against the new
    // top document rather than error on a dead context.
    assert_eq!(
        responses[3]["ok"],
        Value::Bool(true),
        "eval after navigation must succeed (frame binding reset), got: {:?}",
        responses[3]
    );
    let href = responses[3]["result"].as_str().unwrap_or_default();
    assert!(
        href.contains("frame_child.html"),
        "eval after navigation targets the newly loaded top document, got: {href:?}"
    );
}

#[test]
fn frame_on_non_iframe_element_errors() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-frame-non-iframe");
    let browser = guard.name();
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "frame", "target": "h1"}),
        ],
    );

    assert_eq!(responses.len(), 2, "responses: {responses:?}");
    assert_eq!(responses[1]["ok"], Value::Bool(false), "{:?}", responses[1]);
    let err = responses[1]["error"].as_str().unwrap_or_default();
    assert!(
        err.contains("not an <iframe>") || err.to_lowercase().contains("iframe"),
        "expected 'not an <iframe>' error, got: {err:?}"
    );
}

#[test]
fn frame_on_missing_selector_errors() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-frame-missing");
    let browser = guard.name();
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "frame", "target": ".does-not-exist"}),
        ],
    );

    assert_eq!(responses.len(), 2, "responses: {responses:?}");
    assert_eq!(responses[1]["ok"], Value::Bool(false), "{:?}", responses[1]);
    let err = responses[1]["error"].as_str().unwrap_or_default();
    assert!(
        err.contains("No element matches"),
        "expected 'No element matches' error, got: {err:?}"
    );
}

#[test]
fn frame_missing_target_field_errors() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-frame-no-target");
    let browser = guard.name();
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "frame"}),
        ],
    );

    assert_eq!(responses.len(), 2, "responses: {responses:?}");
    assert_eq!(responses[1]["ok"], Value::Bool(false), "{:?}", responses[1]);
    let err = responses[1]["error"].as_str().unwrap_or_default();
    assert!(err.contains("target"), "expected missing-target error, got: {err:?}");
}

#[test]
fn without_frame_switch_eval_targets_top_document() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-frame-control-eval");
    let browser = guard.name();
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "eval", "expression": "location.href"}),
        ],
    );

    assert_eq!(responses.len(), 2, "responses: {responses:?}");
    let href = responses[1]["result"].as_str().unwrap_or_default();
    assert!(
        href.contains("frame_parent.html"),
        "without frame switch, eval must target the top document, got: {href:?}"
    );
}

#[test]
fn without_frame_switch_inspect_shows_parent() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-frame-control-inspect");
    let browser = guard.name();
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "inspect"}),
        ],
    );

    assert_eq!(responses.len(), 2, "responses: {responses:?}");
    let snap = responses[1]["snapshot"].as_str().unwrap_or_default();
    assert!(
        snap.contains("PARENT PAGE CONTENT"),
        "without frame switch, inspect must show the parent page, got: {snap:?}"
    );
}
