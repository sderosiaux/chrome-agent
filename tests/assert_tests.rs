//! `assert` end to end: the exit contract, and the readers it shares with the actions.
//!
//! The exit code is the feature. Every test here checks the code as well as the JSON,
//! because a caller that reads only stdout cannot tell a page that is wrong from a tool
//! that is broken — which is precisely what the third code exists to fix.

use std::process::Command;

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

/// Open a fixture. Returns false (and skips) when Chrome isn't available.
fn open(browser: &str, fixture: &str) -> bool {
    if !common::browser_ready() {
        return false;
    }
    let (_, code) = run_cli(&["--browser", browser, "goto", &common::fixture_url(fixture)]);
    if code != 0 {
        return common::unavailable(&format!("goto {fixture} failed"));
    }
    true
}

/// An assert invocation in JSON mode: returns the parsed response and the exit code.
fn assert_cmd(browser: &str, args: &[&str]) -> (Value, i32) {
    let mut argv = vec!["--browser", browser, "--verdict", "off", "--json", "assert"];
    argv.extend_from_slice(args);
    let (out, code) = run_cli(&argv);
    (serde_json::from_str(&out).unwrap_or(Value::Null), code)
}

fn cli(browser: &str, args: &[&str]) -> (Value, i32) {
    let mut argv = vec!["--browser", browser, "--verdict", "off", "--json"];
    argv.extend_from_slice(args);
    let (out, code) = run_cli(&argv);
    (serde_json::from_str(&out).unwrap_or(Value::Null), code)
}

/// The whole point: 2 is "the page is not in that state", 1 is "I could not check".
///
/// The fixture reverts the value in a promise callback, so `fill` reports
/// `verbatim:false` and the field is empty by the time anything looks — the same
/// microtask revert the fill report exists to expose, now assertable.
#[test]
fn a_value_the_page_did_not_keep_exits_2_and_names_what_it_kept() {
    let b = TestBrowser::new("assert-exit-2");
    if !open(b.name(), "form_value_microtask_revert.html") {
        return;
    }
    let (fill, code) = cli(b.name(), &["fill", "--selector", "#micro", "hello@example.com"]);
    assert_eq!(code, 0, "the fill itself succeeds: {fill}");

    let (v, code) = assert_cmd(b.name(), &["value", "--selector", "#micro", "--equals", "hello@example.com"]);
    assert_eq!(code, 2, "a claim the page contradicts is exit 2, not 1: {v}");
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["assertion"]["held"], false, "{v}");
    assert_eq!(v["assertion"]["kind"], "value");
    assert_eq!(v["assertion"]["expected"], "hello@example.com");
    assert_eq!(v["assertion"]["actual"], "", "the page kept nothing, and the report says so: {v}");
    assert!(v["hint"].is_string(), "a failed assertion says what to do next: {v}");
    // The node it read is named, whichever way the caller aimed — same contract as an action.
    assert!(v["assertion"]["uid"].as_str().is_some_and(|u| u.starts_with('n')), "{v}");

    // The complement, on the same page and the same field: what the page DOES hold holds.
    let (v, code) = assert_cmd(b.name(), &["value", "--selector", "#micro", "--equals", ""]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["assertion"]["held"], true, "{v}");
}

/// A selector that matches nothing, an unparseable selector and an unparseable regex are all
/// the same kind of answer: the claim was never checked. Exit 1, never 2 — a caller retrying
/// on 1 and reporting on 2 must not have those swapped.
#[test]
fn an_unanswerable_claim_exits_1_not_2() {
    let b = TestBrowser::new("assert-exit-1");
    if !open(b.name(), "form_value_microtask_revert.html") {
        return;
    }
    for (args, expect) in [
        (vec!["value", "--selector", "#nope", "--equals", "x"], "No element matches selector"),
        (vec!["value", "--selector", "#(", "--equals", "x"], "not a valid selector"),
        (vec!["text", "--matches", "(unclosed"], "invalid regular expression"),
        (vec!["value", "--uid", "n99999", "--equals", "x"], "not found"),
    ] {
        let (v, code) = assert_cmd(b.name(), &args);
        assert_eq!(code, 1, "{args:?} could not be checked, so it is 1: {v}");
        assert_eq!(v["ok"], false, "{args:?}: {v}");
        assert!(
            v["error"].as_str().unwrap_or_default().contains(expect),
            "{args:?} should say why it could not be checked: {v}"
        );
        assert!(v.get("assertion").is_none(), "nothing was compared, so there is no assertion to report: {v}");
    }
}

/// Assertion and action must agree about what "checked" means, by sharing the reader.
///
/// `el.checked` is undefined on a `<div role=checkbox>`, so a naive assertion would call a
/// checked ARIA box unchecked — and an agent trusting it would click the box OFF. Both
/// targeting modes are checked because both must resolve to the same answer.
#[test]
fn what_check_did_is_what_assert_reads_by_uid_and_by_selector() {
    let b = TestBrowser::new("assert-agreement");
    if !open(b.name(), "checkable_kinds.html") {
        return;
    }
    // A uid resolves through the stored snapshot, like every other uid-targeted command, so
    // inspect first — `goto` deliberately clears the map.
    let (_, code) = run_cli(&["--browser", b.name(), "inspect"]);
    assert_eq!(code, 0, "inspect populates the uid map");

    // Native checkbox: turn it on with the tool, then assert it both ways.
    let (checked, code) = cli(b.name(), &["check", "--selector", "#native"]);
    assert_eq!(code, 0, "{checked}");
    let uid = checked["uid"].as_str().expect("check names the node it hit").to_string();

    for args in [
        vec!["state", "--selector", "#native", "--checked"],
        vec!["state", "--uid", uid.as_str(), "--checked"],
    ] {
        let (v, code) = assert_cmd(b.name(), &args);
        assert_eq!(code, 0, "{args:?} must agree with the check that just ran: {v}");
        assert_eq!(v["assertion"]["actual"], "true", "{v}");
        assert_eq!(v["assertion"]["reading"], "native", "{v}");
    }

    // The ARIA checkbox that starts checked: the shared classification reads the attribute.
    let (v, code) = assert_cmd(b.name(), &["state", "--selector", "#aria_on", "--checked"]);
    assert_eq!(code, 0, "an aria-checked=true div is checked: {v}");
    assert_eq!(v["assertion"]["reading"], "aria", "{v}");

    // And the one that starts off: --unchecked holds, --checked is a plain exit 2.
    let (v, code) = assert_cmd(b.name(), &["state", "--selector", "#aria_off", "--unchecked"]);
    assert_eq!(code, 0, "{v}");
    let (v, code) = assert_cmd(b.name(), &["state", "--selector", "#aria_off", "--checked"]);
    assert_eq!(code, 2, "{v}");
    assert_eq!(v["assertion"]["actual"], "false", "{v}");
}

/// Asking whether a text input is checked is as unanswerable as asking to check it, and it
/// is refused by the same guard — exit 1, with the message that names what is required.
#[test]
fn a_state_the_element_cannot_hold_is_refused_not_answered() {
    let b = TestBrowser::new("assert-unanswerable");
    if !open(b.name(), "checkable_kinds.html") {
        return;
    }
    let (v, code) = assert_cmd(b.name(), &["state", "--selector", "#text", "--checked"]);
    assert_eq!(code, 1, "an unanswerable state is not a failed assertion: {v}");
    let err = v["error"].as_str().unwrap_or_default();
    assert!(err.contains("has no checked state"), "{v}");
    assert!(err.contains("checkbox"), "the message names what would be checkable: {v}");
}

/// `select` and `assert state --selected` read the selection through one JS reader.
#[test]
fn what_select_chose_is_what_assert_reads() {
    let b = TestBrowser::new("assert-selected");
    if !open(b.name(), "assert_page.html") {
        return;
    }
    let (sel, code) = cli(b.name(), &["select", "--selector", "#state", "California"]);
    assert_eq!(code, 0, "{sel}");

    // Both spellings select accepts are both spellings assert accepts.
    for expected in ["California", "CA"] {
        let (v, code) = assert_cmd(b.name(), &["state", "--selector", "#state", "--selected", expected]);
        assert_eq!(code, 0, "selected by {expected}: {v}");
        assert_eq!(v["assertion"]["actual"], "California", "{v}");
        assert_eq!(v["assertion"]["selected_value"], "CA", "{v}");
    }
    let (v, code) = assert_cmd(b.name(), &["state", "--selector", "#state", "--selected", "New York"]);
    assert_eq!(code, 2, "{v}");
    assert_eq!(v["assertion"]["actual"], "California", "the report names what IS selected: {v}");
}

/// `:disabled` plus `aria-disabled`, because `el.disabled` is wrong in both directions: it
/// is false for an input inside a disabled `<fieldset>` and undefined on a div.
#[test]
fn disabled_is_read_the_way_fill_refuses_and_includes_aria() {
    let b = TestBrowser::new("assert-disabled");
    if !open(b.name(), "assert_page.html") {
        return;
    }
    for (selector, want, expect_code, actual) in [
        ("#live", "--enabled", 0, "enabled"),
        ("#dead", "--disabled", 0, "disabled"),
        // The property says false here; the pseudo-class knows about the ancestor.
        ("#in_disabled_fieldset", "--disabled", 0, "disabled"),
        // Inert to everything that reads the page, so never "enabled".
        ("#aria_dead", "--disabled", 0, "aria-disabled"),
        ("#aria_dead", "--enabled", 2, "aria-disabled"),
    ] {
        let (v, code) = assert_cmd(b.name(), &["state", "--selector", selector, want]);
        assert_eq!(code, expect_code, "{selector} {want}: {v}");
        assert_eq!(v["assertion"]["actual"], actual, "{selector} {want}: {v}");
    }
}

/// `--visible` separates the three ways a page hides something, and says in the response
/// what it does not mean.
#[test]
fn visible_names_which_flavour_of_hidden_it_found() {
    let b = TestBrowser::new("assert-visible");
    if !open(b.name(), "assert_page.html") {
        return;
    }
    let (v, code) = assert_cmd(b.name(), &["state", "--selector", "#shown", "--visible"]);
    assert_eq!(code, 0, "{v}");
    assert!(
        v["assertion"]["means"].as_str().unwrap_or_default().contains("not 'in the viewport'"),
        "the response must refuse to be read as 'clickable': {v}"
    );
    for (selector, actual) in [
        ("#gone", "no-box"),
        ("#invisible", "visibility:hidden"),
        ("#transparent", "opacity:0"),
    ] {
        let (v, code) = assert_cmd(b.name(), &["state", "--selector", selector, "--visible"]);
        assert_eq!(code, 2, "{selector}: {v}");
        assert_eq!(v["assertion"]["actual"], actual, "{selector}: {v}");
    }
}

/// A count is a claim: an exact count, a floor, bare presence, and `--count 0` for absence.
#[test]
fn exists_counts_and_absence() {
    let b = TestBrowser::new("assert-exists");
    if !open(b.name(), "assert_page.html") {
        return;
    }
    for (args, expect) in [
        (vec!["exists", "--selector", ".row", "--count", "3"], 0),
        (vec!["exists", "--selector", ".row", "--count", "2"], 2),
        (vec!["exists", "--selector", ".row", "--min", "3"], 0),
        (vec!["exists", "--selector", ".row", "--min", "4"], 2),
        (vec!["exists", "--selector", ".row"], 0),
        // Absence, asserted deliberately — and the same selector under bare presence fails.
        (vec!["exists", "--selector", ".ghost", "--count", "0"], 0),
        (vec!["exists", "--selector", ".ghost"], 2),
    ] {
        let (v, code) = assert_cmd(b.name(), &args);
        assert_eq!(code, expect, "{args:?}: {v}");
    }
    // The count itself travels, so a failure says how many there were.
    let (v, _) = assert_cmd(b.name(), &["exists", "--selector", ".row", "--count", "2"]);
    assert_eq!(v["assertion"]["actual"], 3, "{v}");
    assert_eq!(v["assertion"]["expected"], 2, "{v}");
}

/// Text and URL: the two reads that need no element.
#[test]
fn text_and_url_after_a_navigation() {
    let b = TestBrowser::new("assert-text-url");
    if !open(b.name(), "assert_page.html") {
        return;
    }
    let (v, code) = assert_cmd(b.name(), &["text", "--contains", "Order 4815"]);
    assert_eq!(code, 0, "whole-page text by default: {v}");
    let (v, code) = assert_cmd(b.name(), &["text", "--selector", "#status", "--matches", r"Shipped on \d{4}-\d{2}-\d{2}"]);
    assert_eq!(code, 0, "scoped to an element, matched by pattern: {v}");
    let (v, code) = assert_cmd(b.name(), &["text", "--selector", "#status", "--contains", "Delivered"]);
    assert_eq!(code, 2, "{v}");

    let (v, code) = assert_cmd(b.name(), &["url", "--matches", r"assert_page\.html$"]);
    assert_eq!(code, 0, "{v}");
    assert!(v["assertion"]["actual"].as_str().unwrap_or_default().ends_with("assert_page.html"), "{v}");
    let (v, code) = assert_cmd(b.name(), &["url", "--equals", "https://example.com/"]);
    assert_eq!(code, 2, "{v}");
}

/// A password is compared but never printed. The response reaches stdout, the agent
/// transcript and any recording, and `fill` redacts on exactly this test — an assertion that
/// echoed the value would be a way around it.
#[test]
fn a_secret_field_is_compared_without_echoing_the_secret() {
    let b = TestBrowser::new("assert-secret");
    if !open(b.name(), "assert_page.html") {
        return;
    }
    let (v, code) = assert_cmd(b.name(), &["value", "--selector", "#secret", "--equals", "hunter2"]);
    assert_eq!(code, 0, "the comparison still happens: {v}");
    assert_eq!(v["assertion"]["redacted"], true, "{v}");
    let printed = serde_json::to_string(&v).unwrap();
    assert!(!printed.contains("hunter2"), "the secret must not appear anywhere in the response: {printed}");
    // Lengths still travel: they are what separates "the mask reformatted it" from "empty".
    assert_eq!(v["assertion"]["actual_length"], 7, "{v}");

    let (v, code) = assert_cmd(b.name(), &["value", "--selector", "#secret", "--equals", "wrong"]);
    assert_eq!(code, 2, "{v}");
    let printed = serde_json::to_string(&v).unwrap();
    assert!(!printed.contains("hunter2"), "not even on the failure path: {printed}");
}

/// An element with no `value` property is a refusal that names the alternative, not a
/// silent `null` compared against the expectation.
#[test]
fn a_value_assertion_on_something_that_holds_no_value_is_refused() {
    let b = TestBrowser::new("assert-novalue");
    if !open(b.name(), "assert_page.html") {
        return;
    }
    let (v, code) = assert_cmd(b.name(), &["value", "--selector", "#editable", "--equals", "typed here"]);
    assert_eq!(code, 1, "{v}");
    let err = v["error"].as_str().unwrap_or_default();
    assert!(err.contains("no value property"), "{v}");
    assert!(err.contains("assert text"), "the refusal names what to use instead: {v}");
    // And the text of that same element does hold.
    let (v, code) = assert_cmd(b.name(), &["text", "--selector", "#editable", "--contains", "typed here"]);
    assert_eq!(code, 0, "{v}");
}

/// In batch and pipe there is no exit code, so `held` rides on `ok` — and `stop_on_error`
/// stops at the first one that is false, saying where.
#[test]
fn batch_stops_at_the_first_failed_assertion_only_when_asked() {
    let b = TestBrowser::new("assert-batch");
    if !open(b.name(), "assert_page.html") {
        return;
    }
    let commands = r#"[{"cmd":"assert","what":"exists","selector":".ghost"},{"cmd":"assert","what":"exists","selector":".row","min":3}]"#;

    // Default: every command runs, exactly as before this flag existed.
    let out = Command::new(binary())
        .args(["--browser", b.name(), "--verdict", "off", "batch"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            child.stdin.as_mut().unwrap().write_all(commands.as_bytes())?;
            child.wait_with_output()
        })
        .expect("run batch");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    assert_eq!(v["ok"], false, "one failed assertion makes the batch not ok: {v}");
    assert_eq!(v["results"].as_array().map(Vec::len), Some(2), "both commands ran: {v}");
    assert!(v.get("stopped_at").is_none(), "nothing was skipped: {v}");

    // Opt in, and the second command never runs.
    let out = Command::new(binary())
        .args(["--browser", b.name(), "--verdict", "off", "batch", "--stop-on-error"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            child.stdin.as_mut().unwrap().write_all(commands.as_bytes())?;
            child.wait_with_output()
        })
        .expect("run batch");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    assert_eq!(v["results"].as_array().map(Vec::len), Some(1), "it stopped: {v}");
    assert_eq!(v["stopped_at"], 0, "{v}");
    assert_eq!(v["skipped"], 1, "{v}");
    assert_eq!(v["results"][0]["assertion"]["held"], false, "{v}");
}

/// A read carries no verdict and no change report: nothing moved, and claiming a verdict
/// would put an action's vocabulary on an observation.
#[test]
fn an_assertion_reports_no_verdict_and_no_change() {
    let b = TestBrowser::new("assert-no-verdict");
    if !open(b.name(), "assert_page.html") {
        return;
    }
    // Note: no `--verdict off` here — the default is on, and a read must still stay silent.
    let (out, code) = run_cli(&["--browser", b.name(), "--json", "assert", "exists", "--selector", ".row"]);
    let v: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
    assert_eq!(code, 0, "{v}");
    for absent in ["verdict", "verdict_reason", "changed", "delta"] {
        assert!(v.get(absent).is_none(), "an assertion is a read, so it carries no {absent}: {v}");
    }
}

/// Text mode keeps stdout clean on a failure: the line goes to stderr like every other
/// refusal this binary prints, and the exit code is the answer.
#[test]
fn text_mode_puts_a_failed_assertion_on_stderr() {
    let b = TestBrowser::new("assert-stderr");
    if !open(b.name(), "assert_page.html") {
        return;
    }
    let out = Command::new(binary())
        .args(["--browser", b.name(), "assert", "url", "--equals", "https://example.com/"])
        .output()
        .expect("run assert");
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty(), "stdout must stay clean: {:?}", String::from_utf8_lossy(&out.stdout));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("did NOT hold"), "{stderr}");
    assert!(stderr.contains("hint:"), "{stderr}");
}
