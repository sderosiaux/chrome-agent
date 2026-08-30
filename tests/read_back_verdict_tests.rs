//! The read-back is evidence whichever verb performed it: `fill`, `select` and `check`
//! all measure their own target and report `changed / value_kept` (rung 11 of the ladder in
//! `src/verdict.rs`). These tests compare the three verbs rather than asserting a literal each.

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

/// A fresh browser sitting on `fixture`, or `None` when there is no Chrome to drive.
fn fresh(label: &str, fixture: &str) -> Option<TestBrowser> {
    if !common::browser_ready() {
        return None;
    }
    let b = TestBrowser::new(label);
    let url = common::fixture_url(fixture);
    let (_, code) = run_cli(&["--browser", b.name(), "goto", &url]);
    if code != 0 {
        common::unavailable(&format!("goto {fixture} failed"));
        return None;
    }
    Some(b)
}

/// Run one action as the FIRST action of a session and return its response.
fn first_action(label: &str, args: &[&str]) -> Option<Value> {
    let b = fresh(label, "read_back_kinds.html")?;
    let mut argv = vec!["--browser", b.name(), "--json"];
    argv.extend_from_slice(args);
    let (stdout, code) = run_cli(&argv);
    assert_eq!(code, 0, "{stdout}");
    Some(serde_json::from_str(&stdout).expect("JSON response"))
}

/// What `fill` claims for a kept write, `select` and `check` claim for a kept state.
#[test]
fn fill_select_and_check_report_the_same_evidence_on_a_fresh_session() {
    let Some(filled) = first_action("rb-fill", &["fill", "--selector", "#text", "hello"]) else {
        return;
    };
    let selected =
        first_action("rb-select", &["select", "b", "--selector", "#dropdown"]).expect("a browser");
    let checked = first_action("rb-check", &["check", "--selector", "#box"]).expect("a browser");

    for (verb, out) in [("fill", &filled), ("select", &selected), ("check", &checked)] {
        assert_eq!(out["verdict"], "changed", "{verb}: {out}");
        assert_eq!(out["verdict_reason"], "value_kept", "{verb}: {out}");
        assert_eq!(
            out["value"]["verbatim"], true,
            "{verb} must carry the read-back the verdict was made from: {out}"
        );
        assert_eq!(out["next"], "proceed", "{verb}: {out}");
    }
    // One vocabulary: the postcondition reader in `pipe_report` reads exactly one key.
    for (verb, out) in [("select", &selected), ("check", &checked)] {
        assert!(
            out["value"]["requested"].is_string() && out["value"]["actual"].is_string(),
            "{verb} must say what it asked for and what it got: {out}"
        );
        assert_eq!(
            out["value"]["requested"], out["value"]["actual"],
            "{verb} claimed the state was kept: {out}"
        );
        assert_eq!(
            out["observed_after_ms"], 60,
            "{verb} must still state the window it looked through: {out}"
        );
    }
    assert_eq!(checked["value"]["actual"], "checked", "in the words the message uses: {checked}");
    assert_eq!(selected["value"]["actual"], "Beta", "the option the page held: {selected}");
}

/// The element already held the state, so nothing was dispatched and no write of ours could
/// have been kept.
#[test]
fn a_check_that_dispatched_nothing_claims_no_read_back() {
    let Some(out) = first_action("rb-already", &["check", "--selector", "#box_on"]) else {
        return;
    };
    assert!(out["value"].is_null(), "no postcondition without a post-action moment: {out}");
    assert!(
        out["observed_after_ms"].is_null(),
        "and no window either — nothing was observed after anything: {out}"
    );
    assert_ne!(out["verdict_reason"], "value_kept", "{out}");
    assert_eq!(out["verdict_reason"], "no_baseline", "the honest floor on a fresh session: {out}");
}

#[test]
fn uncheck_reports_the_state_it_read_back() {
    let Some(b) = fresh("rb-uncheck", "read_back_kinds.html") else {
        return;
    };
    let (stdout, code) =
        run_cli(&["--browser", b.name(), "--json", "uncheck", "--selector", "#box_on"]);
    assert_eq!(code, 0, "{stdout}");
    let out: Value = serde_json::from_str(&stdout).expect("JSON response");
    assert_eq!(out["value"]["requested"], "unchecked", "{out}");
    assert_eq!(out["value"]["actual"], "unchecked", "{out}");
    assert_eq!(out["verdict_reason"], "value_kept", "{out}");
}

#[test]
fn a_reverted_selection_still_refuses() {
    let Some(b) = fresh("rb-revert", "select_controlled_revert.html") else {
        return;
    };
    let (stdout, code) =
        run_cli(&["--browser", b.name(), "--json", "select", "b", "--selector", "#controlled"]);
    assert_ne!(code, 0, "a selection the page took away is not a selection: {stdout}");
    assert!(stdout.contains("revert"), "{stdout}");
    let out: Value = serde_json::from_str(&stdout).expect("JSON response");
    assert_eq!(out["ok"], false, "{out}");
    assert!(
        out["value"].is_null(),
        "a refusal carries no postcondition to be read as evidence: {out}"
    );
}

/// A kept state prints no `value:` line, so `render::observation_line` must print the window.
#[test]
fn a_kept_state_still_states_its_window_in_text_mode() {
    let Some(b) = fresh("rb-text", "read_back_kinds.html") else {
        return;
    };
    for args in [
        vec!["check", "--selector", "#box"],
        vec!["select", "b", "--selector", "#dropdown"],
    ] {
        let mut argv = vec!["--browser", b.name()];
        argv.extend_from_slice(&args);
        let (stdout, code) = run_cli(&argv);
        assert_eq!(code, 0, "{stdout}");
        assert!(
            stdout.contains("observed: 60 ms after the action"),
            "{args:?} must say when it looked: {stdout}"
        );
    }
}

/// A secret dropdown reports lengths and the option text nowhere. The markup is contrived on
/// purpose: it is the only way to reach `element::SECRET_FIELD` on a `<select>`.
#[test]
fn a_dropdown_naming_a_secret_reports_lengths_and_never_the_option() {
    let Some(b) = fresh("rb-secret", "select_secret_autocomplete.html") else {
        return;
    };
    let (stdout, code) =
        run_cli(&["--browser", b.name(), "--json", "select", "b", "--selector", "#secret"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(
        !stdout.contains("Sesame"),
        "the option text reaches stdout, the transcript and any recording: {stdout}"
    );
    let out: Value = serde_json::from_str(&stdout).expect("JSON response");
    assert_eq!(out["value"]["redacted"], true, "{out}");
    assert_eq!(out["value"]["requested_length"], 6, "{out}");
    assert_eq!(out["value"]["actual_length"], 6, "{out}");
    assert_eq!(out["verdict_reason"], "value_kept", "{out}");

    // It is the ELEMENT that is secret, not the page: a plain dropdown beside it still talks.
    let (stdout, code) =
        run_cli(&["--browser", b.name(), "--json", "select", "d", "--selector", "#plain"]);
    assert_eq!(code, 0, "{stdout}");
    let plain: Value = serde_json::from_str(&stdout).expect("JSON response");
    assert_eq!(plain["value"]["requested"], "Delta", "{plain}");
    assert_eq!(plain["value"]["actual"], "Delta", "{plain}");
    assert!(plain["value"]["redacted"].is_null(), "{plain}");
}
