//! The accessibility snapshot must not print a value the response redacts.
//!
//! Chrome masks a `type=password` by itself; the case that needs redaction is a card number or
//! one-time code in a `type=text` field, secret only because of its `autocomplete` attribute.

use std::process::Command;

use serde_json::Value;

mod common;
use common::TestBrowser;

/// Every string the fixture holds in a secret field. None may appear in any output.
const SECRETS: &[&str] = &["4111111111111111", "4242424242424242", "7391", "903214", "hunter2secret"];

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

fn assert_no_secret(label: &str, text: &str) {
    for secret in SECRETS {
        assert!(
            !text.contains(secret),
            "{label} printed a secret ({secret}):\n{text}"
        );
    }
}

/// The fixture's fields are pre-filled, so `inspect` alone can leak a card number.
#[test]
fn inspect_names_every_secret_field_without_printing_one() {
    let b = TestBrowser::new("secret-inspect");
    if !open(b.name(), "snapshot_secret_values.html") {
        return;
    }
    let (text, code) = run_cli(&["--browser", b.name(), "inspect"]);
    assert_eq!(code, 0, "{text}");
    assert_no_secret("inspect", &text);

    for label in ["Card number", "Security code", "One-time code", "Password", "Note for the courier"] {
        assert!(text.contains(label), "the field must still be named ({label}):\n{text}");
    }
    // Four secret fields in the fixture; each keeps a value token saying it was withheld.
    assert_eq!(
        text.matches("value=\"<redacted>\"").count(),
        4,
        "one marker per secret field:\n{text}"
    );
    assert!(
        text.contains("value=\"leave at the door\""),
        "an ordinary value must survive:\n{text}"
    );
}

/// A mutating command quotes changed tree lines in `delta`, the second place a secret escapes.
#[test]
fn an_action_delta_never_quotes_a_secret() {
    let b = TestBrowser::new("secret-delta");
    if !open(b.name(), "snapshot_secret_values.html") {
        return;
    }
    let (base, _) = run_cli(&["--browser", b.name(), "inspect"]);
    assert_no_secret("baseline inspect", &base);

    // A fill that replaces the card number, so the delta holds both numbers.
    let v = json_cli(b.name(), &["fill", "--selector", "#card", "4242424242424242"]);
    assert_no_secret("fill response", &v.to_string());
    assert_eq!(v["value"]["redacted"], true, "the fill's own report agrees: {v}");
    assert_eq!(v["value"]["verbatim"], true, "and the write landed: {v}");

    let clicked = json_cli(b.name(), &["click", "--selector", "#pay-submit"]);
    assert_no_secret("click response", &clicked.to_string());
    assert!(
        clicked["delta"].as_str().unwrap_or_default().contains("paid"),
        "the change it did cause is still reported: {clicked}"
    );
}

/// The marker is fixed: a length or hash would make every secret field look changed.
#[test]
fn two_snapshots_of_an_unchanged_secret_compare_equal() {
    let b = TestBrowser::new("secret-stable");
    if !open(b.name(), "snapshot_secret_values.html") {
        return;
    }
    let (first, _) = run_cli(&["--browser", b.name(), "inspect"]);
    let v = json_cli(b.name(), &["diff"]);
    assert_no_secret("diff", &v.to_string());
    assert_eq!(v["changed"], 0, "nothing moved between the two reads: {v}");
    assert_eq!(v["added"], 0, "{v}");
    assert_eq!(v["removed"], 0, "{v}");
    assert!(
        v["diff"].as_str().unwrap_or_default().contains("No changes detected"),
        "and it says so: {v}"
    );

    let (second, _) = run_cli(&["--browser", b.name(), "inspect"]);
    assert_eq!(first, second, "two reads of the same page must render identically");
}

/// The accepted trade-off: a replaced secret compares equal, since both sides render the same
/// marker. A secret that DISAPPEARS stays visible, because an empty value emits no token.
#[test]
fn a_changed_secret_is_invisible_to_the_diff_but_a_lost_one_is_not() {
    let b = TestBrowser::new("secret-tradeoff");
    if !open(b.name(), "snapshot_secret_values.html") {
        return;
    }
    run_cli(&["--browser", b.name(), "inspect"]);
    let v = json_cli(b.name(), &["fill", "--selector", "#card", "4242424242424242"]);
    let delta = v["delta"].as_str().unwrap_or_default();
    assert!(
        !delta.contains("uid=n2 ") || !delta.contains("value="),
        "the changed secret is not reported as a value change: {v}"
    );

    // The loss half, on the fixture built for it.
    let b2 = TestBrowser::new("secret-tradeoff-lost");
    if !open(b2.name(), "form_value_secret_lost_on_submit.html") {
        return;
    }
    json_cli(b2.name(), &["fill", "--selector", "#card", "4111111111111111"]);
    let lost = json_cli(b2.name(), &["click", "--selector", "#pay-submit"]);
    assert_no_secret("values_lost response", &lost.to_string());
    let entries = lost["values_lost"].as_array().unwrap_or_else(|| panic!("no values_lost: {lost}"));
    assert!(
        entries.iter().any(|e| e["redacted"] == true),
        "a lost secret is still named, and still redacted: {lost}"
    );
}
