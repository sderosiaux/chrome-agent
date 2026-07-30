//! The read-back is evidence whichever verb performed it.
//!
//! `fill` reports `changed / value_kept` on a fresh session: the read on the handle it wrote to
//! is measured on this action's own target, so it stands whether or not the page could be
//! compared (rung 11 of the ladder in `src/verdict.rs`). `select` and `check`/`uncheck` perform
//! the same measurement — they set a state, dispatch, wait through `READ_BACK_MS` and re-read —
//! and used to report the window and nothing else, so the classifier saw no postcondition at
//! all and answered `unknown / no_baseline` for an action whose own target had been measured.
//!
//! Same class of evidence honoured for one verb and discarded for two others is the asymmetry
//! the verdict module exists to remove, so these tests compare the three verbs directly rather
//! than asserting a literal per verb.

use std::process::Command;

use serde_json::Value;

mod common;

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
    /// Unique per process AND per case: a verdict of `no_baseline` is only reachable on a
    /// session that has never stored a snapshot, so every case here needs its own browser
    /// rather than its own `goto` — `goto` deliberately keeps `last_snapshot`.
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

/// The asymmetry, stated as a comparison: whatever `fill` claims for a write the page kept,
/// `select` and `check` claim for a state the page kept, and the evidence is in the same field.
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
    // One vocabulary, not three: the postcondition reader in `pipe_report` reads ONE key, and a
    // second key for the same idea is how select and check came to be classified as silent.
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

/// The one case that must NOT claim `value_kept`: the element already held the state, so
/// nothing was dispatched and there is no write of ours to have been kept. Claiming the rung
/// there would be a claim about a click that never happened.
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

/// `uncheck` is the same measurement in the other direction, and reports the state it read.
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

/// A refusal is still a refusal: this change is about what a SUCCESS reports.
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

/// The window survived the new `value` object in text mode.
///
/// `render::observation_line` used to skip itself whenever the response carried a `value`
/// field, on the reasoning that the value line names its own window. But that line prints
/// NOTHING when the state was kept — so the moment check and select acquired a `value`, every
/// successful one silently stopped saying when it had looked.
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

/// A dropdown that names a secret reports the lengths a secret fill reports, and the option
/// text nowhere — not in `value`, not in the message. Contrived markup, deliberately: it is
/// the only way to reach `element::SECRET_FIELD` on a `<select>`, and the redaction has to hold
/// wherever that predicate does or it holds by luck.
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
    // Still classifiable from the lengths alone: a secret must never be the silent case.
    assert_eq!(out["verdict_reason"], "value_kept", "{out}");

    // And it is the ELEMENT that is secret, not the page: an ordinary dropdown beside it still
    // reports its option text. Worth pinning, because the two mechanisms are easy to confuse —
    // `snapshot_secret` also scrubs any node echoing a secret's value, so a control sharing
    // option labels with the secret select would have come back redacted for a different reason.
    let (stdout, code) =
        run_cli(&["--browser", b.name(), "--json", "select", "d", "--selector", "#plain"]);
    assert_eq!(code, 0, "{stdout}");
    let plain: Value = serde_json::from_str(&stdout).expect("JSON response");
    assert_eq!(plain["value"]["requested"], "Delta", "{plain}");
    assert_eq!(plain["value"]["actual"], "Delta", "{plain}");
    assert!(plain["value"]["redacted"].is_null(), "{plain}");
}
