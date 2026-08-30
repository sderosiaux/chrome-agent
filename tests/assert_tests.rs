//! `assert` end to end: the exit contract, and the readers it shares with the actions.
//!
//! Every test checks the exit code as well as the JSON: 0 held, 2 did not hold, 1 could not
//! be checked.

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

/// A value the page contradicts is exit 2, and the report names what the page kept. The
/// fixture reverts in a promise callback, so the field is empty when read.
#[test]
fn a_value_the_page_did_not_keep_exits_2_and_names_what_it_kept() {
    let b = TestBrowser::new("assert-exit-2");
    if !open(b.name(), "form_value_microtask_revert.html") {
        return;
    }
    let (fill, code) = cli(
        b.name(),
        &["fill", "--selector", "#micro", "hello@example.com"],
    );
    assert_eq!(code, 0, "the fill itself succeeds: {fill}");

    let (v, code) = assert_cmd(
        b.name(),
        &[
            "value",
            "--selector",
            "#micro",
            "--equals",
            "hello@example.com",
        ],
    );
    assert_eq!(
        code, 2,
        "a claim the page contradicts is exit 2, not 1: {v}"
    );
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["assertion"]["held"], false, "{v}");
    assert_eq!(v["assertion"]["kind"], "value");
    assert_eq!(v["assertion"]["expected"], "hello@example.com");
    assert_eq!(
        v["assertion"]["actual"], "",
        "the page kept nothing, and the report says so: {v}"
    );
    assert!(
        v["hint"].is_string(),
        "a failed assertion says what to do next: {v}"
    );
    // The node it read is named, whichever way the caller aimed.
    assert!(
        v["assertion"]["uid"]
            .as_str()
            .is_some_and(|u| u.starts_with('n')),
        "{v}"
    );

    // The complement: what the page does hold, holds.
    let (v, code) = assert_cmd(b.name(), &["value", "--selector", "#micro", "--equals", ""]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["assertion"]["held"], true, "{v}");
}

/// No match, a bad selector, a bad regex and an unknown uid are all exit 1, never 2.
#[test]
fn an_unanswerable_claim_exits_1_not_2() {
    let b = TestBrowser::new("assert-exit-1");
    if !open(b.name(), "form_value_microtask_revert.html") {
        return;
    }
    for (args, expect) in [
        (
            vec!["value", "--selector", "#nope", "--equals", "x"],
            "No element matches selector",
        ),
        (
            vec!["value", "--selector", "#(", "--equals", "x"],
            "not a valid selector",
        ),
        (
            vec!["text", "--matches", "(unclosed"],
            "invalid regular expression",
        ),
        (
            vec!["value", "--uid", "n99999", "--equals", "x"],
            "not found",
        ),
    ] {
        let (v, code) = assert_cmd(b.name(), &args);
        assert_eq!(code, 1, "{args:?} could not be checked, so it is 1: {v}");
        assert_eq!(v["ok"], false, "{args:?}: {v}");
        assert!(
            v["error"].as_str().unwrap_or_default().contains(expect),
            "{args:?} should say why it could not be checked: {v}"
        );
        assert!(
            v.get("assertion").is_none(),
            "nothing was compared, so there is no assertion to report: {v}"
        );
    }
}

/// `check` and `assert state --checked` share one reader, so a `<div role=checkbox>` reads
/// the same either way. Both targeting modes.
#[test]
fn what_check_did_is_what_assert_reads_by_uid_and_by_selector() {
    let b = TestBrowser::new("assert-agreement");
    if !open(b.name(), "checkable_kinds.html") {
        return;
    }
    // A uid resolves through the stored snapshot, and `goto` clears the map, so inspect first.
    let (_, code) = run_cli(&["--browser", b.name(), "inspect"]);
    assert_eq!(code, 0, "inspect populates the uid map");

    // Native checkbox: turn it on, then assert it both ways.
    let (checked, code) = cli(b.name(), &["check", "--selector", "#native"]);
    assert_eq!(code, 0, "{checked}");
    let uid = checked["uid"]
        .as_str()
        .expect("check names the node it hit")
        .to_string();

    for args in [
        vec!["state", "--selector", "#native", "--checked"],
        vec!["state", "--uid", uid.as_str(), "--checked"],
    ] {
        let (v, code) = assert_cmd(b.name(), &args);
        assert_eq!(
            code, 0,
            "{args:?} must agree with the check that just ran: {v}"
        );
        assert_eq!(v["assertion"]["actual"], "true", "{v}");
        assert_eq!(v["assertion"]["reading"], "native", "{v}");
    }

    // The ARIA checkbox that starts checked: the reading is the attribute.
    let (v, code) = assert_cmd(b.name(), &["state", "--selector", "#aria_on", "--checked"]);
    assert_eq!(code, 0, "an aria-checked=true div is checked: {v}");
    assert_eq!(v["assertion"]["reading"], "aria", "{v}");

    // The one that starts off: --unchecked holds, --checked is exit 2.
    let (v, code) = assert_cmd(
        b.name(),
        &["state", "--selector", "#aria_off", "--unchecked"],
    );
    assert_eq!(code, 0, "{v}");
    let (v, code) = assert_cmd(b.name(), &["state", "--selector", "#aria_off", "--checked"]);
    assert_eq!(code, 2, "{v}");
    assert_eq!(v["assertion"]["actual"], "false", "{v}");
}

/// `--checked` on a text input is exit 1 with a message naming what would be checkable.
#[test]
fn a_state_the_element_cannot_hold_is_refused_not_answered() {
    let b = TestBrowser::new("assert-unanswerable");
    if !open(b.name(), "checkable_kinds.html") {
        return;
    }
    let (v, code) = assert_cmd(b.name(), &["state", "--selector", "#text", "--checked"]);
    assert_eq!(
        code, 1,
        "an unanswerable state is not a failed assertion: {v}"
    );
    let err = v["error"].as_str().unwrap_or_default();
    assert!(err.contains("has no checked state"), "{v}");
    assert!(
        err.contains("checkbox"),
        "the message names what would be checkable: {v}"
    );
}

#[test]
fn what_select_chose_is_what_assert_reads() {
    let b = TestBrowser::new("assert-selected");
    if !open(b.name(), "assert_page.html") {
        return;
    }
    let (sel, code) = cli(b.name(), &["select", "--selector", "#state", "California"]);
    assert_eq!(code, 0, "{sel}");

    // Both spellings `select` accepts, `assert` accepts.
    for expected in ["California", "CA"] {
        let (v, code) = assert_cmd(
            b.name(),
            &["state", "--selector", "#state", "--selected", expected],
        );
        assert_eq!(code, 0, "selected by {expected}: {v}");
        assert_eq!(v["assertion"]["actual"], "California", "{v}");
        assert_eq!(v["assertion"]["selected_value"], "CA", "{v}");
    }
    let (v, code) = assert_cmd(
        b.name(),
        &["state", "--selector", "#state", "--selected", "New York"],
    );
    assert_eq!(code, 2, "{v}");
    assert_eq!(
        v["assertion"]["actual"], "California",
        "the report names what IS selected: {v}"
    );
}

/// Disabled is `:disabled` plus `aria-disabled`, not `el.disabled`, which is false inside a
/// disabled `<fieldset>` and undefined on a div.
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
        ("#aria_dead", "--disabled", 0, "aria-disabled"),
        ("#aria_dead", "--enabled", 2, "aria-disabled"),
    ] {
        let (v, code) = assert_cmd(b.name(), &["state", "--selector", selector, want]);
        assert_eq!(code, expect_code, "{selector} {want}: {v}");
        assert_eq!(v["assertion"]["actual"], actual, "{selector} {want}: {v}");
    }
}

/// `--visible` separates the three ways a page hides something, and says what it does not mean.
#[test]
fn visible_names_which_flavour_of_hidden_it_found() {
    let b = TestBrowser::new("assert-visible");
    if !open(b.name(), "assert_page.html") {
        return;
    }
    let (v, code) = assert_cmd(b.name(), &["state", "--selector", "#shown", "--visible"]);
    assert_eq!(code, 0, "{v}");
    assert!(
        v["assertion"]["means"]
            .as_str()
            .unwrap_or_default()
            .contains("not 'in the viewport'"),
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

/// `exists` with an exact count, a floor, bare presence, and `--count 0` for absence.
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
        (vec!["exists", "--selector", ".ghost", "--count", "0"], 0),
        (vec!["exists", "--selector", ".ghost"], 2),
    ] {
        let (v, code) = assert_cmd(b.name(), &args);
        assert_eq!(code, expect, "{args:?}: {v}");
    }
    // A failure reports how many there were.
    let (v, _) = assert_cmd(b.name(), &["exists", "--selector", ".row", "--count", "2"]);
    assert_eq!(v["assertion"]["actual"], 3, "{v}");
    assert_eq!(v["assertion"]["expected"], 2, "{v}");
}

#[test]
fn text_and_url_after_a_navigation() {
    let b = TestBrowser::new("assert-text-url");
    if !open(b.name(), "assert_page.html") {
        return;
    }
    let (v, code) = assert_cmd(b.name(), &["text", "--contains", "Order 4815"]);
    assert_eq!(code, 0, "whole-page text by default: {v}");
    let (v, code) = assert_cmd(
        b.name(),
        &[
            "text",
            "--selector",
            "#status",
            "--matches",
            r"Shipped on \d{4}-\d{2}-\d{2}",
        ],
    );
    assert_eq!(code, 0, "scoped to an element, matched by pattern: {v}");
    let (v, code) = assert_cmd(
        b.name(),
        &["text", "--selector", "#status", "--contains", "Delivered"],
    );
    assert_eq!(code, 2, "{v}");

    let (v, code) = assert_cmd(b.name(), &["url", "--matches", r"assert_page\.html$"]);
    assert_eq!(code, 0, "{v}");
    assert!(
        v["assertion"]["actual"]
            .as_str()
            .unwrap_or_default()
            .ends_with("assert_page.html"),
        "{v}"
    );
    let (v, code) = assert_cmd(b.name(), &["url", "--equals", "https://example.com/"]);
    assert_eq!(code, 2, "{v}");
}

#[test]
fn a_secret_field_is_compared_without_echoing_the_secret() {
    let b = TestBrowser::new("assert-secret");
    if !open(b.name(), "assert_page.html") {
        return;
    }
    let (v, code) = assert_cmd(
        b.name(),
        &["value", "--selector", "#secret", "--equals", "hunter2"],
    );
    assert_eq!(code, 0, "the comparison still happens: {v}");
    assert_eq!(v["assertion"]["redacted"], true, "{v}");
    let printed = serde_json::to_string(&v).unwrap();
    assert!(
        !printed.contains("hunter2"),
        "the secret must not appear anywhere in the response: {printed}"
    );
    // Lengths separate "the mask reformatted it" from "empty".
    assert_eq!(v["assertion"]["actual_length"], 7, "{v}");

    let (v, code) = assert_cmd(
        b.name(),
        &["value", "--selector", "#secret", "--equals", "wrong"],
    );
    assert_eq!(code, 2, "{v}");
    let printed = serde_json::to_string(&v).unwrap();
    assert!(
        !printed.contains("hunter2"),
        "not even on the failure path: {printed}"
    );
}

/// `assert value` on an element with no `value` property is refused and names `assert text`.
#[test]
fn a_value_assertion_on_something_that_holds_no_value_is_refused() {
    let b = TestBrowser::new("assert-novalue");
    if !open(b.name(), "assert_page.html") {
        return;
    }
    let (v, code) = assert_cmd(
        b.name(),
        &["value", "--selector", "#editable", "--equals", "typed here"],
    );
    assert_eq!(code, 1, "{v}");
    let err = v["error"].as_str().unwrap_or_default();
    assert!(err.contains("no value property"), "{v}");
    assert!(
        err.contains("assert text"),
        "the refusal names what to use instead: {v}"
    );
    // The text of that same element does hold.
    let (v, code) = assert_cmd(
        b.name(),
        &[
            "text",
            "--selector",
            "#editable",
            "--contains",
            "typed here",
        ],
    );
    assert_eq!(code, 0, "{v}");
}

/// An assertion inside a batch has no exit code of its own, so `held` rides on `ok`;
/// `--stop-on-error` stops at the first one (and makes the process itself exit 1, which
/// `tests/cli_contract_tests.rs` pins).
#[test]
fn batch_stops_at_the_first_failed_assertion_only_when_asked() {
    let b = TestBrowser::new("assert-batch");
    if !open(b.name(), "assert_page.html") {
        return;
    }
    let commands = r#"[{"cmd":"assert","what":"exists","selector":".ghost"},{"cmd":"assert","what":"exists","selector":".row","min":3}]"#;

    // Default: every command runs.
    let out = Command::new(common::binary())
        .args(["--browser", b.name(), "--verdict", "off", "--json", "batch"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(commands.as_bytes())?;
            child.wait_with_output()
        })
        .expect("run batch");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    assert_eq!(
        v["ok"], false,
        "one failed assertion makes the batch not ok: {v}"
    );
    assert_eq!(
        v["results"].as_array().map(Vec::len),
        Some(2),
        "both commands ran: {v}"
    );
    assert!(v.get("stopped_at").is_none(), "nothing was skipped: {v}");

    // Opt in, and the second command never runs.
    let out = Command::new(common::binary())
        .args([
            "--browser",
            b.name(),
            "--verdict",
            "off",
            "--json",
            "batch",
            "--stop-on-error",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(commands.as_bytes())?;
            child.wait_with_output()
        })
        .expect("run batch");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    assert_eq!(
        v["results"].as_array().map(Vec::len),
        Some(1),
        "it stopped: {v}"
    );
    assert_eq!(v["stopped_at"], 0, "{v}");
    assert_eq!(v["skipped"], 1, "{v}");
    assert_eq!(v["results"][0]["assertion"]["held"], false, "{v}");
}

#[test]
fn an_assertion_reports_no_verdict_and_no_change() {
    let b = TestBrowser::new("assert-no-verdict");
    if !open(b.name(), "assert_page.html") {
        return;
    }
    // No `--verdict off` here: reporting is on by default and the read must still stay silent.
    let (out, code) = run_cli(&[
        "--browser",
        b.name(),
        "--json",
        "assert",
        "exists",
        "--selector",
        ".row",
    ]);
    let v: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
    assert_eq!(code, 0, "{v}");
    for absent in ["verdict", "verdict_reason", "changed", "delta"] {
        assert!(
            v.get(absent).is_none(),
            "an assertion is a read, so it carries no {absent}: {v}"
        );
    }
}

/// In text mode a failed assertion goes to stderr and stdout stays empty.
#[test]
fn text_mode_puts_a_failed_assertion_on_stderr() {
    let b = TestBrowser::new("assert-stderr");
    if !open(b.name(), "assert_page.html") {
        return;
    }
    let out = Command::new(common::binary())
        .args([
            "--browser",
            b.name(),
            "assert",
            "url",
            "--equals",
            "https://example.com/",
        ])
        .output()
        .expect("run assert");
    assert_eq!(out.status.code(), Some(2));
    assert!(
        out.stdout.is_empty(),
        "stdout must stay clean: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("did NOT hold"), "{stderr}");
    assert!(stderr.contains("hint:"), "{stderr}");
}
