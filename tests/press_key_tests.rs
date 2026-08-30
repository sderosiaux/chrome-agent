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

fn open_and_focus(browser: &str) -> bool {
    if !common::browser_ready() {
        return false;
    }
    let (_, code) = run_cli(&["--browser", browser, "goto", &common::fixture_url("press_keys.html")]);
    if code != 0 {
        return common::unavailable("goto press_keys.html failed");
    }
    let (_, code) = run_cli(&["--browser", browser, "eval", "document.getElementById('i').focus(); 1"]);
    code == 0
}

/// Without `text` on the CDP event the page sees a keydown and inserts nothing.
#[test]
fn pressing_a_printable_character_types_it() {
    let b = TestBrowser::new("press-char");
    if !open_and_focus(b.name()) {
        return;
    }
    for key in ["h", "i"] {
        let (out, code) = run_cli(&["--browser", b.name(), "--verdict", "off", "--json", "press", key]);
        assert_eq!(code, 0, "press {key} should succeed: {out}");
    }
    let (value, _) = run_cli(&["--browser", b.name(), "eval", "document.getElementById('i').value"]);
    assert_eq!(value.trim().trim_matches('"'), "hi", "the characters should have been typed");
}

/// An unmapped key name must be refused, not dispatched with virtual key code 0.
#[test]
fn an_unknown_key_name_is_refused_rather_than_sent_as_nothing() {
    let b = TestBrowser::new("press-unknown");
    if !open_and_focus(b.name()) {
        return;
    }
    let (out, code) = run_cli(&["--browser", b.name(), "--verdict", "off", "--json", "press", "Zorglub"]);
    let v: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
    assert_ne!(code, 0, "an unknown key should fail: {v}");
    assert!(
        v["error"].as_str().unwrap_or_default().contains("Unknown key"),
        "and say which one: {v}"
    );
}

#[test]
fn navigation_keys_reach_the_page() {
    let b = TestBrowser::new("press-nav");
    if !open_and_focus(b.name()) {
        return;
    }
    for key in ["Home", "End", "PageDown", "F5"] {
        let (out, code) = run_cli(&["--browser", b.name(), "--verdict", "off", "--json", "press", key]);
        assert_eq!(code, 0, "press {key} should succeed: {out}");
    }
    let (log, _) = run_cli(&["--browser", b.name(), "eval", "document.getElementById('log').textContent"]);
    for key in ["Home", "End", "PageDown", "F5"] {
        assert!(log.contains(key), "the page should have seen {key}: {log}");
    }
}

/// A full stop is ASCII 46, which is also `VK_DELETE`: deriving the virtual key code from the
/// character's byte turns `press .` into a delete.
#[test]
fn punctuation_types_instead_of_deleting() {
    let b = TestBrowser::new("press-punct");
    if !open_and_focus(b.name()) {
        return;
    }
    let (_, code) = run_cli(&[
        "--browser", b.name(), "eval",
        "const i=document.getElementById('i'); i.value='XYZ'; i.focus(); i.setSelectionRange(0,0); 1",
    ]);
    assert_eq!(code, 0);
    let (out, code) = run_cli(&["--browser", b.name(), "--verdict", "off", "--json", "press", "."]);
    assert_eq!(code, 0, "{out}");
    let (value, _) = run_cli(&["--browser", b.name(), "eval", "document.getElementById('i').value"]);
    assert_eq!(
        value.trim().trim_matches('"'),
        ".XYZ",
        "the character must be inserted, and nothing deleted"
    );
}

/// Text insertion goes to whatever holds focus, so with focus on BODY it goes nowhere.
#[test]
fn typing_with_nothing_focused_is_refused() {
    let b = TestBrowser::new("type-nofocus");
    if !common::browser_ready() {
        return;
    }
    let (_, code) = run_cli(&["--browser", b.name(), "goto", &common::fixture_url("press_keys.html")]);
    if code != 0 {
        return;
    }
    let (out, code) = run_cli(&["--browser", b.name(), "--verdict", "off", "--json", "type", "hello"]);
    let v: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
    assert_ne!(code, 0, "typing into nothing should fail: {v}");
    assert!(
        v["error"].as_str().unwrap_or_default().contains("focus"),
        "and say why: {v}"
    );
}
