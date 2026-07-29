use std::process::Command;

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

struct TestBrowser(&'static str);
impl Drop for TestBrowser {
    fn drop(&mut self) {
        let _ = run_cli(&["--browser", self.0, "close", "--purge"]);
    }
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

/// The worst shape of bug: the tool reports success and leaves the page in the
/// opposite state. `el.checked` is undefined on a div, so a truthiness read calls a
/// checked ARIA checkbox unchecked, clicks it, and turns it off.
#[test]
fn checking_an_aria_checkbox_that_is_already_on_leaves_it_on() {
    let b = TestBrowser("chk-aria-on");
    if !open(b.0) {
        return;
    }
    assert_eq!(eval(b.0, "document.getElementById('aria_on').getAttribute('aria-checked')"), "true");

    let (v, code) = check(b.0, "#aria_on");
    assert_eq!(code, 0, "check should succeed: {v}");
    assert_eq!(
        eval(b.0, "document.getElementById('aria_on').getAttribute('aria-checked')"),
        "true",
        "check must never turn a checked box off: {v}"
    );
    assert!(
        v["message"].as_str().unwrap_or_default().contains("Already"),
        "an already-checked box should report as already checked: {v}"
    );
}

/// The complement: an ARIA checkbox that is off must actually be turned on.
#[test]
fn checking_an_aria_checkbox_that_is_off_turns_it_on() {
    let b = TestBrowser("chk-aria-off");
    if !open(b.0) {
        return;
    }
    let (v, code) = check(b.0, "#aria_off");
    assert_eq!(code, 0, "check should succeed: {v}");
    assert_eq!(
        eval(b.0, "document.getElementById('aria_off').getAttribute('aria-checked')"),
        "true",
        "check should have turned it on: {v}"
    );
}

/// Every `HTMLInputElement` exposes `.checked`, including a text input, where it means
/// nothing. Reporting "Checked" there is a plain lie.
#[test]
fn checking_a_text_input_is_refused() {
    let b = TestBrowser("chk-text");
    if !open(b.0) {
        return;
    }
    let (v, code) = check(b.0, "#text");
    assert_ne!(code, 0, "checking a text input should fail, got: {v}");
    assert_eq!(v["ok"], false, "{v}");
    let err = v["error"].as_str().unwrap_or_default();
    assert!(
        err.contains("checkbox") || err.contains("checkable") || err.contains("radio"),
        "the error should say what kind of element is required: {v}"
    );
}

/// A radio cannot be unchecked by clicking it. Clicking anyway leaves it checked and
/// reporting "Unchecked" would be false.
#[test]
fn unchecking_a_radio_is_refused() {
    let b = TestBrowser("chk-radio");
    if !open(b.0) {
        return;
    }
    let (out, code) = run_cli(&[
        "--browser", b.0, "--verdict", "off", "--json", "uncheck", "--selector", "#radio",
    ]);
    let v: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
    assert_ne!(code, 0, "unchecking a radio should fail, got: {v}");
    assert_eq!(eval(b.0, "document.getElementById('radio').checked"), "true", "still checked: {v}");
    // The refusal has to come from the guard that knows why, not from the read-back
    // noticing afterwards that the click failed to do anything.
    assert!(
        v["error"].as_str().unwrap_or_default().contains("radio cannot be unchecked"),
        "the reason must name the real constraint, not just report the click did nothing: {v}"
    );
}

/// Native checkboxes must keep working, both directions.
#[test]
fn native_checkboxes_still_work() {
    let b = TestBrowser("chk-native");
    if !open(b.0) {
        return;
    }
    let (v, code) = check(b.0, "#native");
    assert_eq!(code, 0, "{v}");
    assert_eq!(eval(b.0, "document.getElementById('native').checked"), "true", "{v}");

    let (v, code) = check(b.0, "#native_on");
    assert_eq!(code, 0, "{v}");
    assert_eq!(eval(b.0, "document.getElementById('native_on').checked"), "true", "already on stays on: {v}");
    assert!(v["message"].as_str().unwrap_or_default().contains("Already"), "{v}");
}
