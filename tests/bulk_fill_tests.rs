//! `fill-form` and `fill_and_submit` report `value:{requested,actual,verbatim}` per field,
//! like a single `fill`. For `fill_and_submit` it is the only witness: the change report
//! runs after the submit, by which time the form has moved on.

use std::io::Write as _;
use std::process::{Command, Stdio};

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

/// Run a pipe script. The browser is owned by a `common::TestBrowser` and closed on return.
fn run_pipe(label: &str, script: &str) -> Vec<Value> {
    let guard = TestBrowser::new(label);
    let mut child = Command::new(common::binary())
        .args(["--browser", guard.name(), "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pipe");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("pipe output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let responses: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
        .collect();
    assert!(!responses.is_empty(), "pipe produced nothing");
    responses
}

/// The phone mask rewrites the value, and the per-field report says so.
#[test]
fn fill_form_reports_what_each_field_kept() {
    if !common::browser_ready() {
        return;
    }
    let url = common::fixture_url("multi_field_form.html");
    let script = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": url}),
        serde_json::json!({"cmd": "inspect"}),
        serde_json::json!({"cmd": "fill_and_submit", "fields": [
            {"selector": "#name", "value": "Ada Lovelace"},
            {"selector": "#phone", "value": "5551234567"},
        ], "submit": "#submit"}),
    );
    let responses = run_pipe("bulk-fill-submit", &script);
    let last = responses.last().expect("a fill_and_submit response");

    let values = last["values"].as_array().unwrap_or_else(|| panic!("no per-field report: {last}"));
    assert_eq!(values.len(), 2, "one entry per field: {last}");

    let name = values.iter().find(|v| v["selector"] == "#name").expect("the name field");
    assert_eq!(name["value"]["actual"], "Ada Lovelace", "{name}");
    assert_eq!(name["value"]["verbatim"], true, "{name}");

    let phone = values.iter().find(|v| v["selector"] == "#phone").expect("the phone field");
    assert_eq!(
        phone["value"]["verbatim"], false,
        "the mask rewrote it, and after the submit nothing else can tell you: {phone}"
    );
    assert_eq!(phone["value"]["actual"], "(555) 123-4567", "{phone}");
}

#[test]
fn fill_form_by_uid_reports_each_outcome() {
    if !common::browser_ready() {
        return;
    }
    let url = common::fixture_url("multi_field_form.html");
    let probe = "JSON.stringify([...document.querySelectorAll('#name,#phone')].map(e => e.id))";
    let script = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": url}),
        serde_json::json!({"cmd": "inspect"}),
        serde_json::json!({"cmd": "eval", "expression": probe}),
    );
    let responses = run_pipe("bulk-fill-probe", &script);
    let snapshot = responses[1]["snapshot"].as_str().unwrap_or_default().to_string();
    let uid_of = |name: &str| -> String {
        snapshot
            .lines()
            .find(|l| l.contains("textbox") && l.contains(name))
            .and_then(|l| l.trim_start().strip_prefix("uid="))
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or_else(|| panic!("no uid for {name} in: {snapshot}"))
            .to_string()
    };
    let phone_uid = uid_of("Phone");

    let script = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": url}),
        serde_json::json!({"cmd": "inspect"}),
        serde_json::json!({"cmd": "fill_form", "pairs": [{"uid": phone_uid, "value": "5551234567"}]}),
    );
    let responses = run_pipe("bulk-fill-uid", &script);
    let last = responses.last().expect("a fill_form response");
    let values = last["values"].as_array().unwrap_or_else(|| panic!("no per-field report: {last}"));
    assert_eq!(values.len(), 1, "{last}");
    assert_eq!(values[0]["uid"], phone_uid, "{last}");
    assert_eq!(values[0]["value"]["verbatim"], false, "{last}");
}

#[test]
fn the_cli_fill_form_reports_each_outcome_too() {
    let b = TestBrowser::new("bulk-fill-cli");
    if !common::browser_ready() {
        return;
    }
    let url = common::fixture_url("multi_field_form.html");
    let (_, code) = run_cli(&["--browser", b.name(), "goto", &url]);
    if code != 0 {
        common::unavailable("goto multi_field_form.html failed");
        return;
    }
    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "inspect"]);
    assert_eq!(code, 0, "{stdout}");
    let snapshot: Value = serde_json::from_str(&stdout).expect("JSON inspect");
    let text = snapshot["snapshot"].as_str().unwrap_or_default();
    let phone_uid = text
        .lines()
        .find(|l| l.contains("textbox") && l.contains("Phone"))
        .and_then(|l| l.trim_start().strip_prefix("uid="))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no phone uid in: {text}"))
        .to_string();

    let pair = format!("{phone_uid}=5551234567");
    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "fill-form", &pair]);
    assert_eq!(code, 0, "{stdout}");
    let v: Value = serde_json::from_str(&stdout).expect("JSON fill-form");
    let values = v["values"].as_array().unwrap_or_else(|| panic!("no per-field report: {v}"));
    assert_eq!(values[0]["value"]["verbatim"], false, "{v}");
}

/// A secret in a bulk fill is redacted exactly as in a single one.
#[test]
fn a_password_in_a_bulk_fill_is_still_redacted() {
    if !common::browser_ready() {
        return;
    }
    let url = common::fixture_url("form_value_password.html");
    let script = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": url}),
        serde_json::json!({"cmd": "inspect"}),
        serde_json::json!({"cmd": "fill_and_submit", "fields": [
            {"selector": "#p", "value": "hunter2-not-in-the-log"},
        ], "submit": "#p"}),
    );
    let responses = run_pipe("bulk-fill-secret", &script);
    let whole = serde_json::to_string(&responses).unwrap_or_default();
    assert!(
        !whole.contains("hunter2-not-in-the-log"),
        "a bulk fill must not be a way around redaction: {whole}"
    );
    let last = responses.last().expect("a response");
    let values = last["values"].as_array().unwrap_or_else(|| panic!("no per-field report: {last}"));
    assert_eq!(values[0]["value"]["redacted"], true, "{last}");
}
