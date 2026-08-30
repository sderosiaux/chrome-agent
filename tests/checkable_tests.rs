use std::process::Command;

use serde_json::Value;

mod common;
use common::TestBrowser;

fn run_cli(args: &[&str]) -> (String, i32) {
    let output = Command::new(common::binary()).args(args).output().expect("Failed to run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

/// Open the fixture. Returns false (and skips) when Chrome isn't available.
fn open(browser: &str) -> bool {
    if !common::browser_ready() {
        return false;
    }
    let (_, code) = run_cli(&["--browser", browser, "goto", &common::fixture_url("checkable_kinds.html")]);
    if code != 0 {
        return common::unavailable("goto checkable_kinds.html failed");
    }
    true
}

fn eval(browser: &str, expr: &str) -> String {
    let (out, _) = run_cli(&["--browser", browser, "eval", expr]);
    out.trim().trim_matches('"').to_string()
}

fn check(browser: &str, selector: &str) -> (Value, i32) {
    let (out, code) = run_cli(&[
        "--browser", browser, "--verdict", "off", "--json", "check", "--selector", selector,
    ]);
    (serde_json::from_str(&out).unwrap_or(Value::Null), code)
}

/// `el.checked` is undefined on a div, so a truthiness read would click a checked ARIA
/// checkbox off while reporting success.
#[test]
fn checking_an_aria_checkbox_that_is_already_on_leaves_it_on() {
    let b = TestBrowser::new("chk-aria-on");
    if !open(b.name()) {
        return;
    }
    assert_eq!(eval(b.name(), "document.getElementById('aria_on').getAttribute('aria-checked')"), "true");

    let (v, code) = check(b.name(), "#aria_on");
    assert_eq!(code, 0, "check should succeed: {v}");
    assert_eq!(
        eval(b.name(), "document.getElementById('aria_on').getAttribute('aria-checked')"),
        "true",
        "check must never turn a checked box off: {v}"
    );
    assert!(
        v["message"].as_str().unwrap_or_default().contains("Already"),
        "an already-checked box should report as already checked: {v}"
    );
}

#[test]
fn checking_an_aria_checkbox_that_is_off_turns_it_on() {
    let b = TestBrowser::new("chk-aria-off");
    if !open(b.name()) {
        return;
    }
    let (v, code) = check(b.name(), "#aria_off");
    assert_eq!(code, 0, "check should succeed: {v}");
    assert_eq!(
        eval(b.name(), "document.getElementById('aria_off').getAttribute('aria-checked')"),
        "true",
        "check should have turned it on: {v}"
    );
}

/// Every `HTMLInputElement` exposes `.checked`, including a text input, where it means
/// nothing — so the guard refuses rather than reporting "Checked".
#[test]
fn checking_a_text_input_is_refused() {
    let b = TestBrowser::new("chk-text");
    if !open(b.name()) {
        return;
    }
    let (v, code) = check(b.name(), "#text");
    assert_ne!(code, 0, "checking a text input should fail, got: {v}");
    assert_eq!(v["ok"], false, "{v}");
    let err = v["error"].as_str().unwrap_or_default();
    assert!(
        err.contains("checkbox") || err.contains("checkable") || err.contains("radio"),
        "the error should say what kind of element is required: {v}"
    );
}

/// A radio cannot be unchecked by clicking it, so the guard refuses before dispatching.
#[test]
fn unchecking_a_radio_is_refused() {
    let b = TestBrowser::new("chk-radio");
    if !open(b.name()) {
        return;
    }
    let (out, code) = run_cli(&[
        "--browser", b.name(), "--verdict", "off", "--json", "uncheck", "--selector", "#radio",
    ]);
    let v: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
    assert_ne!(code, 0, "unchecking a radio should fail, got: {v}");
    assert_eq!(eval(b.name(), "document.getElementById('radio').checked"), "true", "still checked: {v}");
    // The refusal comes from the guard, not from the read-back noticing afterwards.
    assert!(
        v["error"].as_str().unwrap_or_default().contains("radio cannot be unchecked"),
        "the reason must name the real constraint, not just report the click did nothing: {v}"
    );
}

#[test]
fn native_checkboxes_still_work() {
    let b = TestBrowser::new("chk-native");
    if !open(b.name()) {
        return;
    }
    let (v, code) = check(b.name(), "#native");
    assert_eq!(code, 0, "{v}");
    assert_eq!(eval(b.name(), "document.getElementById('native').checked"), "true", "{v}");

    let (v, code) = check(b.name(), "#native_on");
    assert_eq!(code, 0, "{v}");
    assert_eq!(eval(b.name(), "document.getElementById('native_on').checked"), "true", "already on stays on: {v}");
    assert!(v["message"].as_str().unwrap_or_default().contains("Already"), "{v}");
}
