//! A malformed CSS selector is reported as one.
//!
//! `document.querySelector("[")` throws a `SyntaxError`, and CDP still answers with an
//! `objectId` — for the thrown `DOMException`. `hit_test::resolve_selector` read that `objectId`
//! without checking `exceptionDetails`, so the handle was bound to the exception: the probe found
//! no box on it, the JS-click fallback ran, and the caller was told
//! `JS click threw: this.click is not a function` — a type error standing in for a bad selector.

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

/// Every verb that resolves a single handle must name the selector and the syntax error, and
/// never a property of the exception object it would otherwise have been handed.
#[test]
fn a_malformed_selector_is_named_as_a_selector_and_not_as_a_js_type_error() {
    let b = TestBrowser::new("selector-syntax");
    if !open(b.name(), "click_overlay.html") {
        return;
    }

    for verb in ["click", "dblclick"] {
        let (stdout, code) = run_cli(&["--browser", b.name(), "--json", verb, "--selector", "["]);
        assert_eq!(
            code, 1,
            "a selector that cannot parse is an error: {stdout}"
        );
        let response: Value = serde_json::from_str(&stdout).expect("JSON error response");
        assert_eq!(response["ok"], Value::Bool(false), "{response}");
        let error = response["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("Selector '['"),
            "{verb} must quote back what could not be parsed: {error}"
        );
        assert!(
            error.contains("not a valid selector") || error.contains("SyntaxError"),
            "{verb} must carry the browser's own complaint: {error}"
        );
        assert!(
            !error.contains("this.click is not a function"),
            "{verb} must not report a property of the DOMException it was handed: {error}"
        );
    }
}

/// The verbs that resolve the selector IN THE PAGE rather than through a handle
/// (`fill --selector`, `type --selector` via `focus_selector`, `frame`) used to let the browser's
/// own `SyntaxError` through untranslated: `{"error":"SyntaxError: Failed to execute
/// 'querySelector'…"}` names the DOM method, not the argument the caller got wrong.
#[test]
fn the_in_page_selector_paths_name_the_selector_too() {
    let b = TestBrowser::new("selector-syntax-in-page");
    if !open(b.name(), "click_overlay.html") {
        return;
    }

    for args in [
        vec!["fill", "x", "--selector", "["],
        vec!["type", "x", "--selector", "["],
        vec!["frame", "["],
    ] {
        let verb = args[0];
        let mut argv = vec!["--browser", b.name(), "--json"];
        argv.extend_from_slice(&args);
        let (stdout, code) = run_cli(&argv);
        assert_eq!(
            code, 1,
            "a selector that cannot parse is an error: {stdout}"
        );
        let response: Value = serde_json::from_str(&stdout).expect("JSON error response");
        let error = response["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("Selector '['"),
            "{verb} must quote back what could not be parsed: {error}"
        );
        assert!(
            error.contains("not a valid selector") || error.contains("SyntaxError"),
            "{verb} must carry the browser's own complaint: {error}"
        );
    }
}

/// The contrast: a selector that parses and matches nothing keeps its own, different message —
/// on every verb, whether it resolves through a handle or in the page.
#[test]
fn a_well_formed_selector_that_matches_nothing_still_says_so() {
    let b = TestBrowser::new("selector-syntax-absent");
    if !open(b.name(), "click_overlay.html") {
        return;
    }
    for args in [
        vec!["click", "--selector", "#nothing-here"],
        vec!["fill", "x", "--selector", "#nothing-here"],
        vec!["type", "x", "--selector", "#nothing-here"],
        vec!["frame", "#nothing-here"],
    ] {
        let verb = args[0];
        let mut argv = vec!["--browser", b.name(), "--json"];
        argv.extend_from_slice(&args);
        let (stdout, code) = run_cli(&argv);
        assert_eq!(code, 1, "{stdout}");
        let response: Value = serde_json::from_str(&stdout).expect("JSON error response");
        let error = response["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("No element matches selector: #nothing-here"),
            "an absent element is not a malformed selector ({verb}): {error}"
        );
    }
}
