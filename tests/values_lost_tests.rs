//! `values_lost`: what an action destroyed, as a field rather than as delta prose.
//!
//! The rule under test is that a response claims the requested state unless a FIELD denies it,
//! so a `form.reset()` that empties a filled field must show up as more than a delta line.

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

fn json_cli(browser: &str, args: &[&str]) -> Value {
    let mut full: Vec<&str> = vec!["--browser", browser, "--json"];
    full.extend_from_slice(args);
    let (stdout, code) = run_cli(&full);
    assert_eq!(code, 0, "command should succeed: {stdout}");
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}"))
}

/// The fixture's submit handler sets a status AND calls `form.reset()`.
#[test]
fn a_submit_that_resets_the_form_names_the_value_it_destroyed() {
    let b = TestBrowser::new("lost-reset");
    if !open(b.name(), "form_value_reset_on_submit.html") {
        return;
    }
    let filled = json_cli(b.name(), &["fill", "--selector", "#email", "hello@example.com"]);
    assert_eq!(filled["value"]["verbatim"], true, "the fill has to land first: {filled}");
    assert!(
        filled["values_lost"].is_null(),
        "a fill that landed destroyed nothing: {filled}"
    );

    let v = json_cli(b.name(), &["click", "--selector", "#submit"]);
    let lost = v["values_lost"].as_array().unwrap_or_else(|| panic!("no values_lost: {v}"));
    assert_eq!(lost.len(), 1, "{v}");
    assert!(!lost[0]["uid"].as_str().unwrap_or_default().is_empty(), "{v}");
    assert_eq!(lost[0]["role"], "textbox", "{v}");
    assert_eq!(lost[0]["name"], "Email", "{v}");
    assert_eq!(lost[0]["was"], "hello@example.com", "{v}");
    assert_eq!(v["verdict"], "changed", "the page did move: {v}");
    assert_eq!(v["verdict_reason"], "values_lost", "{v}");
    assert!(
        v["verdict_hint"].as_str().unwrap_or_default().contains("cleared itself"),
        "the hint states the ambiguity rather than declaring failure: {v}"
    );

    let (truth, _) = run_cli(&["--browser", b.name(), "eval", "document.getElementById('email').value"]);
    assert_eq!(truth.trim(), "\"\"", "the field is empty: {truth}");
}

#[test]
fn a_submit_that_keeps_the_value_reports_no_loss() {
    let b = TestBrowser::new("lost-keep");
    if !open(b.name(), "form_value_secret_lost_on_submit.html") {
        return;
    }
    json_cli(b.name(), &["fill", "--selector", "#search", "kafka"]);
    let v = json_cli(b.name(), &["click", "--selector", "#keep-submit"]);
    assert!(v["values_lost"].is_null(), "nothing was lost: {v}");
    assert_eq!(v["verdict"], "changed", "{v}");
    assert_eq!(v["verdict_reason"], "tree_delta", "and the reason stays the plain one: {v}");
}

/// A lost secret is named without being printed. The card field is `type=text`, so the tree
/// reports its value verbatim and only `autocomplete` marks it secret.
#[test]
fn a_lost_secret_is_named_but_never_printed() {
    let b = TestBrowser::new("lost-secret");
    if !open(b.name(), "form_value_secret_lost_on_submit.html") {
        return;
    }
    json_cli(b.name(), &["fill", "--selector", "#card", "4111111111111111"]);
    json_cli(b.name(), &["fill", "--selector", "#pw", "topsecret123"]);
    json_cli(b.name(), &["fill", "--selector", "#note", "gift wrap"]);
    let v = json_cli(b.name(), &["click", "--selector", "#pay-submit"]);

    let lost = v["values_lost"].as_array().unwrap_or_else(|| panic!("no values_lost: {v}"));
    let by_name = |name: &str| {
        lost.iter()
            .find(|e| e["name"] == name)
            .unwrap_or_else(|| panic!("no lost value named {name}: {v}"))
            .clone()
    };

    for secret in ["Card number", "Password"] {
        let entry = by_name(secret);
        assert_eq!(entry["redacted"], true, "{secret} must be redacted: {entry}");
        assert!(entry["was"].is_null(), "{secret} must not carry its value: {entry}");
        // Not a length either: for a password the tree reports the mask's length, not the value's.
        assert!(entry["was_length"].is_null(), "{secret} must not carry a length: {entry}");
    }
    assert_eq!(by_name("Note")["was"], "gift wrap", "{v}");

    assert!(
        !v["values_lost"].to_string().contains("4111111111111111"),
        "the card number must not appear in values_lost: {v}"
    );
    assert!(
        !v["values_lost"].to_string().contains("topsecret123"),
        "the password must not appear in values_lost: {v}"
    );
}

#[test]
fn pipe_reports_the_lost_value_the_way_the_cli_does() {
    if !common::browser_ready() {
        return;
    }
    let url = common::fixture_url("form_value_reset_on_submit.html");
    let script = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": url}),
        serde_json::json!({"cmd": "fill", "selector": "#email", "value": "hello@example.com"}),
        serde_json::json!({"cmd": "click", "selector": "#submit"}),
    );
    // Unique per process: a fixed name would let a concurrent run clobber this one's page.
    let guard = TestBrowser::new("lost-pipe");
    let browser = guard.name().to_string();
    let mut child = Command::new(common::binary())
        .args(["--browser", &browser, "pipe"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn pipe");
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("pipe output");
    let stdout = String::from_utf8_lossy(&out.stdout);

    let last: Value = stdout
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
        .expect("a click response");
    assert_eq!(last["verdict_reason"], "values_lost", "{last}");
    assert_eq!(last["values_lost"][0]["was"], "hello@example.com", "{last}");
}
