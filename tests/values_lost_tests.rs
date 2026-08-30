//! What an action destroyed, as a field rather than as prose.
//!
//! The evidence was always in the delta: the `value=` token stops appearing on the field's line
//! after a `form.reset()`. But a diff line is prose. Scored by the rule this project applies to
//! everything else — "a response claims the requested state unless a FIELD denies it" — the
//! click on `form_value_reset_on_submit.html` said `ok:true` and `verdict:"changed"`, both true,
//! and never said the field the agent had just filled was empty again.

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

/// Open a fixture and take the baseline the change report needs.
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

/// S3. Fill a field, submit, and the handler sets a status AND calls `form.reset()`. Ground
/// truth afterwards is `{"email":"","status":"sent"}`: the click did something, and it also
/// destroyed what was typed. Both have to be on the response.
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
    // The verdict pair is not silent about it either.
    assert_eq!(v["verdict"], "changed", "the page did move: {v}");
    assert_eq!(v["verdict_reason"], "values_lost", "{v}");
    assert!(
        v["verdict_hint"].as_str().unwrap_or_default().contains("cleared itself"),
        "the hint states the ambiguity rather than declaring failure: {v}"
    );

    // And the page really is in that state, so the field is not describing a phantom.
    let (truth, _) = run_cli(&["--browser", b.name(), "eval", "document.getElementById('email').value"]);
    assert_eq!(truth.trim(), "\"\"", "the field is empty: {truth}");
}

/// The negative half. A report that fired on every click would pass the test above, so a
/// submit that KEEPS what was typed must emit nothing at all.
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

/// The value goes to stdout, into the agent transcript and into any recording, so a lost
/// secret is named without being printed — the same rule `fill` applies to what it wrote.
///
/// The `cc-number` field is the one that matters: it is `type=text`, so the accessibility tree
/// reports its value verbatim and only the `autocomplete` attribute says it is a secret.
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
        // Not a length either: the only length available comes from the accessibility tree,
        // which for a password is the length of Chrome's mask rather than of the value.
        assert!(entry["was_length"].is_null(), "{secret} must not carry a length: {entry}");
    }
    // An ordinary field still reports what it held — that is the point of the field.
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

/// Pipe settles its verdict in its own code path, and bench reads raw JSON from whichever
/// mode it drove. Two modes disagreeing about whether a submit ate the input is exactly the
/// kind of thing a re-score would catch late.
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
    // Unique per process: a fixed name lets a second concurrent run of this suite drive the
    // same browser and clobber this one's page.
    let guard = TestBrowser::new("lost-pipe");
    let browser = guard.name().to_string();
    let mut child = Command::new(binary())
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
