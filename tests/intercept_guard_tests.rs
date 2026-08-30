//! `--on-intercept guard`: the middle ground between `dispatch` and `refuse`, decided by
//! `Hit::looks_inert` in `hit_test.rs`. One fixture per family, plus an iframe receiver.

use std::process::Command;

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
    true
}

fn eval(browser: &str, expression: &str) -> Value {
    let (stdout, code) = run_cli(&["--browser", browser, "--json", "eval", expression]);
    assert_eq!(code, 0, "eval failed: {stdout}");
    let v: Value = serde_json::from_str(&stdout).expect("JSON eval response");
    v["result"].clone()
}

// --- Inert family: guard dispatches, matching `dispatch` --------------------

/// `#scrim` is a plain `<div>`: no interactive tag, no role, no `tabindex`, no `cursor`.
#[test]
fn guard_dispatches_through_an_inert_overlay() {
    let b = TestBrowser::new("guard-inert");
    if !open(b.name(), "click_overlay.html") {
        return;
    }
    let _ = eval(b.name(), "window.receiver = null; 1");
    let (stdout, code) = run_cli(&[
        "--browser",
        b.name(),
        "--json",
        "--on-intercept",
        "guard",
        "click",
        "--selector",
        "#target",
    ]);
    assert_eq!(code, 0, "guard must not refuse an inert receiver: {stdout}");
    let response: Value = serde_json::from_str(&stdout).expect("JSON click response");
    assert_eq!(response["ok"], Value::Bool(true));
    assert_eq!(
        response["delivery"], "intercepted",
        "the scrim still occupies the point"
    );
    assert_eq!(
        response["intercepted_by"]["actionable"],
        Value::Bool(false),
        "the receiver guard read the decision from: {response}"
    );
    assert_eq!(
        eval(b.name(), "window.receiver"),
        Value::String("scrim".into()),
        "dispatch semantics: the scrim received the event, same as plain dispatch would report"
    );
}

// --- Actionable family: guard refuses, matching `refuse` --------------------

/// The fixture's consent overlay is a real `<button>`, so `guard` refuses and names it.
#[test]
fn guard_refuses_an_actionable_overlay() {
    let b = TestBrowser::new("guard-actionable");
    if !open(b.name(), "intercept_actionable_overlay.html") {
        return;
    }
    let _ = eval(b.name(), "window.receiver = null; 1");
    let (stdout, code) = run_cli(&[
        "--browser",
        b.name(),
        "--json",
        "--on-intercept",
        "guard",
        "click",
        "--selector",
        "#target",
    ]);
    assert_ne!(
        code, 0,
        "a refusal is a failure the caller has to handle: {stdout}"
    );
    let response: Value = serde_json::from_str(&stdout).expect("JSON error response");
    assert_eq!(response["ok"], Value::Bool(false));
    assert_eq!(response["dispatched"], Value::Bool(false));
    let error = response["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("button#accept"),
        "the refusal names the receiver: {error}"
    );
    assert!(
        error.contains("--on-intercept guard judged it a control"),
        "the reason is guard's own, not a hardcoded 'refuse was set': {error}"
    );
    assert_eq!(response["intercepted_by"]["actionable"], Value::Bool(true));
    let hint = response["hint"].as_str().unwrap_or_default();
    assert!(hint.contains("judged it a control"), "{hint}");
    assert_eq!(
        eval(b.name(), "window.receiver"),
        Value::Null,
        "guard refused before dispatching — the consent button never saw the event either"
    );
}

/// The same fixture under plain `dispatch`, for contrast.
#[test]
fn dispatch_accepts_the_consent_wall_on_the_callers_behalf() {
    let b = TestBrowser::new("guard-contrast-dispatch");
    if !open(b.name(), "intercept_actionable_overlay.html") {
        return;
    }
    let _ = eval(b.name(), "window.receiver = null; 1");
    let (stdout, code) = run_cli(&[
        "--browser",
        b.name(),
        "--json",
        "click",
        "--selector",
        "#target",
    ]);
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(
        eval(b.name(), "window.receiver"),
        Value::String("accept".into()),
        "default dispatch sends the click to whatever is on top, consent button included"
    );
}

// --- Iframe: opaque content refuses under guard even when inert -------------

/// The fixture's iframe holds one inert paragraph and `guard` refuses it anyway: reading its
/// content would need a second execution context this probe does not open.
#[test]
fn guard_refuses_an_iframe_receiver_even_when_its_content_is_inert() {
    let b = TestBrowser::new("guard-iframe");
    if !open(b.name(), "intercept_iframe_overlay.html") {
        return;
    }
    let _ = eval(b.name(), "window.receiver = null; 1");
    let (stdout, code) = run_cli(&[
        "--browser",
        b.name(),
        "--json",
        "--on-intercept",
        "guard",
        "click",
        "--selector",
        "#target",
    ]);
    assert_ne!(
        code, 0,
        "an iframe receiver refuses under guard regardless of actionable: {stdout}"
    );
    let response: Value = serde_json::from_str(&stdout).expect("JSON error response");
    assert_eq!(response["ok"], Value::Bool(false));
    assert_eq!(response["intercepted_by"]["iframe"], Value::Bool(true));
    assert_eq!(
        eval(b.name(), "window.receiver"),
        Value::Null,
        "nothing was dispatched — not even into the iframe"
    );
}

/// On the same iframe, `refuse` still refuses and `dispatch` still sends the click.
#[test]
fn dispatch_and_refuse_are_unchanged_by_guards_arrival() {
    let b = TestBrowser::new("guard-iframe-contrast");
    if !open(b.name(), "intercept_iframe_overlay.html") {
        return;
    }
    let (_, code) = run_cli(&[
        "--browser",
        b.name(),
        "--json",
        "--on-intercept",
        "refuse",
        "click",
        "--selector",
        "#target",
    ]);
    assert_ne!(code, 0, "refuse still refuses an iframe receiver");

    let (stdout, code) = run_cli(&[
        "--browser",
        b.name(),
        "--json",
        "click",
        "--selector",
        "#target",
    ]);
    assert_eq!(
        code, 0,
        "dispatch (default) still sends the click: {stdout}"
    );
    let response: Value = serde_json::from_str(&stdout).expect("JSON response");
    assert_eq!(response["ok"], Value::Bool(true));
    assert!(
        response["dispatched"].is_null(),
        "a successful dispatch says nothing here"
    );
}
