//! A value the response redacts, printed by the snapshot on the line above.
//!
//! `fill` redacts a secret because the response "reaches stdout, the agent transcript and any
//! `--record` file". The accessibility snapshot reaches all three by the same route and printed
//! the same value verbatim: `inspect` prints it, and every action report quotes those lines back
//! inside `delta`. Chrome masks a `type=password` in the tree, which is what hid the leak — the
//! half that matters is a card number or a one-time code in a `type=text` field, secret only
//! because its `autocomplete` attribute says so.

use std::process::Command;

use serde_json::Value;

mod common;

/// Every string the fixture holds in a secret field, plus the digits it echoes into a
/// `generic` node. None of these may appear in any output.
const SECRETS: &[&str] = &["4111111111111111", "4242424242424242", "7391", "903214", "hunter2secret"];

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

struct TestBrowser(String);
impl TestBrowser {
    /// Unique per process: a fixed name means two concurrent runs drive the same browser and
    /// each invalidates the other's uids.
    fn new(label: &str) -> Self {
        Self(format!("{label}-{}", std::process::id()))
    }
    fn name(&self) -> &str {
        &self.0
    }
}
impl Drop for TestBrowser {
    fn drop(&mut self) {
        let _ = run_cli(&["--browser", &self.0, "close", "--purge"]);
    }
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

/// `inspect` is where the leak is widest: the fields are pre-filled, so no action is needed for
/// a card number to reach stdout.
#[test]
fn inspect_names_every_secret_field_without_printing_one() {
    let b = TestBrowser::new("secret-inspect");
    if !open(b.name(), "snapshot_secret_values.html") {
        return;
    }
    let (text, code) = run_cli(&["--browser", b.name(), "inspect"]);
    assert_eq!(code, 0, "{text}");
    assert_no_secret("inspect", &text);

    // The node, its uid, its role and its label are what an agent aims by — all still there.
    for label in ["Card number", "Security code", "One-time code", "Password", "Note for the courier"] {
        assert!(text.contains(label), "the field must still be named ({label}):\n{text}");
    }
    // Four secret fields plus Chrome's editable-content child inside each: the value token is
    // present and states that it was withheld, rather than disappearing.
    assert_eq!(
        text.matches("value=\"<redacted>\"").count(),
        4,
        "one marker per secret field:\n{text}"
    );
    // And the ordinary field is printed, which is the point of the field.
    assert!(
        text.contains("value=\"leave at the door\""),
        "an ordinary value must survive:\n{text}"
    );
}

/// The other consumer of the same text. Every mutating command re-reads the page and quotes
/// the changed lines in `delta` — the response that says `{"redacted": true}` about what it
/// wrote used to print the same digits three lines later.
#[test]
fn an_action_delta_never_quotes_a_secret() {
    let b = TestBrowser::new("secret-delta");
    if !open(b.name(), "snapshot_secret_values.html") {
        return;
    }
    // Baseline, so the next action has something to compare against.
    let (base, _) = run_cli(&["--browser", b.name(), "inspect"]);
    assert_no_secret("baseline inspect", &base);

    // A fill that REPLACES the card number: before the fix this produced three delta lines
    // carrying both the old and the new number (the field, Chrome's editable child, and the
    // page's own echo of it).
    let v = json_cli(b.name(), &["fill", "--selector", "#card", "4242424242424242"]);
    assert_no_secret("fill response", &v.to_string());
    assert_eq!(v["value"]["redacted"], true, "the fill's own report agrees: {v}");
    assert_eq!(v["value"]["verbatim"], true, "and the write landed: {v}");

    // A click elsewhere: its delta still walks past every secret field.
    let clicked = json_cli(b.name(), &["click", "--selector", "#pay-submit"]);
    assert_no_secret("click response", &clicked.to_string());
    assert!(
        clicked["delta"].as_str().unwrap_or_default().contains("paid"),
        "the change it did cause is still reported: {clicked}"
    );
}

/// The marker has to be fixed, not derived. A marker carrying a length or a hash would make
/// every secret field on the page look changed on every action.
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

    // Same text, character for character — the marker does not depend on what it hides.
    let (second, _) = run_cli(&["--browser", b.name(), "inspect"]);
    assert_eq!(first, second, "two reads of the same page must render identically");
}

/// The trade-off, pinned so it cannot change silently: a secret whose value the page really
/// replaced now compares equal, because both sides render the same marker. What is NOT lost is
/// a secret that disappears — the `value=` token stops being emitted, which is how
/// `values_lost` still finds it.
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

    // The loss half, on the fixture built for it: emptying a secret field is still visible,
    // because an empty value emits no token at all.
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
