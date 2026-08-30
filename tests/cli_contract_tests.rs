//! The output and exit-code contract of the CLI, on the three points where it was broken.
//!
//! 1. `check`/`uncheck`/`upload` took a uid AND a `--selector` and silently acted on the
//!    selector, while `click`/`fill`/`select`/`dblclick` refuse the same pair.
//! 2. Every `--json` response funnels through one writer, which answered a serialization
//!    failure with an empty line (unit-tested in `src/run_helpers.rs`, since a `Value` cannot
//!    be made to fail from out here).
//! 3. The CLI `batch` exited 0 even when `--stop-on-error` cut the run short, and printed raw
//!    JSON in text mode.
//!
//! The refusals in (1) live in `run::run`'s second match, which runs AFTER the browser is
//! resolved — so `CHROME_AGENT_PARSE_ONLY` returns before reaching them and these tests own a
//! real browser. Moving them into clap (`ArgGroup`, the way `download` does it) would make
//! them parse-time; it would also give them clap's wording instead of the sibling commands'.

mod common;

use std::io::Write as _;
use std::process::{Command, Stdio};

use common::{TestBrowser, binary, browser_ready, fixture_url};

/// Run one CLI invocation against `browser`, returning `(stdout, stderr, exit code)`.
fn run(browser: &str, args: &[&str]) -> (String, String, i32) {
    let mut full = vec!["--browser", browser];
    full.extend_from_slice(args);
    let out = Command::new(binary())
        .args(&full)
        .output()
        .expect("run chrome-agent");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Feed a JSON array to `batch` on stdin.
fn run_batch(browser: &str, args: &[&str], commands: &str) -> (String, String, i32) {
    let mut full = vec!["--browser", browser];
    full.extend_from_slice(args);
    let mut child = Command::new(binary())
        .args(&full)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn chrome-agent batch");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(commands.as_bytes())
        .expect("write commands");
    let out = child.wait_with_output().expect("collect batch output");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Open a fixture on this test's own browser. `false` means the precondition was not met.
fn open(browser: &str, fixture: &str) -> bool {
    if !browser_ready() {
        return false;
    }
    let (_, stderr, code) = run(browser, &["--json", "goto", &fixture_url(fixture)]);
    assert_eq!(code, 0, "goto failed: {stderr}");
    true
}

// ---------------------------------------------------------------------------
// 1. Two ways to name one target is a refusal, not a ranking
// ---------------------------------------------------------------------------

#[test]
fn check_refuses_a_uid_and_a_selector_together() {
    let b = TestBrowser::new("contract-check-both");
    if !open(b.name(), "checkable_kinds.html") {
        return;
    }
    // `#native` starts unchecked. Before the fix this checked it and never mentioned the uid.
    let (stdout, stderr, code) = run(b.name(), &["check", "n1", "--selector", "#native"]);
    assert_eq!(code, 1, "stdout={stdout} stderr={stderr}");
    assert!(
        stderr.contains("Only one of uid or --selector can be provided."),
        "the same wording click/fill/select/dblclick already use: {stderr}"
    );

    let (stdout, stderr, code) =
        run(b.name(), &["assert", "state", "--selector", "#native", "--unchecked"]);
    assert_eq!(
        code, 0,
        "a refused invocation must not have acted on the selector it discarded: {stdout} {stderr}"
    );
}

#[test]
fn uncheck_refuses_a_uid_and_a_selector_together() {
    let b = TestBrowser::new("contract-uncheck-both");
    if !open(b.name(), "checkable_kinds.html") {
        return;
    }
    // `#native_on` starts checked, so acting on the selector would be visible.
    let (stdout, stderr, code) = run(b.name(), &["uncheck", "n1", "--selector", "#native_on"]);
    assert_eq!(code, 1, "stdout={stdout} stderr={stderr}");
    assert!(
        stderr.contains("Only one of uid or --selector can be provided."),
        "{stderr}"
    );

    let (stdout, stderr, code) =
        run(b.name(), &["assert", "state", "--selector", "#native_on", "--checked"]);
    assert_eq!(
        code, 0,
        "the discarded selector was left alone: {stdout} {stderr}"
    );
}

#[test]
fn upload_refuses_a_uid_and_a_selector_together() {
    let b = TestBrowser::new("contract-upload-both");
    if !open(b.name(), "checkable_kinds.html") {
        return;
    }
    let file = common::temp_path("contract-upload", "txt");
    std::fs::write(&file, b"payload").expect("write the upload source");
    let path = file.to_string_lossy().into_owned();
    let (stdout, stderr, code) = run(
        b.name(),
        &["upload", &path, "--uid", "n1", "--selector", "#native"],
    );
    let _ = std::fs::remove_file(&file);
    assert_eq!(code, 1, "stdout={stdout} stderr={stderr}");
    assert!(
        stderr.contains("Only one of --uid or --selector can be provided."),
        "`upload` names its uid with a flag, so the wording matches fill/select: {stderr}"
    );
}

#[test]
fn a_refusal_under_json_is_ok_false_on_stdout() {
    let b = TestBrowser::new("contract-check-both-json");
    if !open(b.name(), "checkable_kinds.html") {
        return;
    }
    let (stdout, stderr, code) =
        run(b.name(), &["--json", "check", "n1", "--selector", "#native"]);
    assert_eq!(code, 1, "stdout={stdout} stderr={stderr}");
    let v: serde_json::Value =
        serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{e}: {stdout}"));
    assert_eq!(v["ok"], false, "{stdout}");
    assert!(
        v["error"].as_str().unwrap_or_default().contains("Only one of"),
        "{stdout}"
    );
}

// ---------------------------------------------------------------------------
// 3. A batch that stopped on an error says so in its exit code
// ---------------------------------------------------------------------------

/// A uid no snapshot ever produced: the first command fails, deterministically and without
/// depending on the page.
const FAILING: &str =
    r#"[{"cmd":"click","uid":"n999999"},{"cmd":"text","selector":"h1"}]"#;

#[test]
fn a_batch_stopped_on_an_error_exits_1() {
    let b = TestBrowser::new("contract-batch-stopped");
    if !open(b.name(), "checkable_kinds.html") {
        return;
    }
    let (stdout, stderr, code) =
        run_batch(b.name(), &["--json", "batch", "--stop-on-error"], FAILING);
    assert_eq!(
        code, 1,
        "1 (an error), never 2 — 2 is reserved for an assertion that did not hold: \
         stdout={stdout} stderr={stderr}"
    );
    let v: serde_json::Value =
        serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{e}: {stdout}"));
    assert_eq!(v["ok"], false, "{stdout}");
    assert_eq!(v["stopped_at"], 0, "{stdout}");
    assert_eq!(v["skipped"], 1, "{stdout}");
    assert_eq!(
        v["results"].as_array().map(Vec::len),
        Some(1),
        "the exit code did not cost the caller the response: {stdout}"
    );
    assert_eq!(
        stdout.lines().filter(|l| !l.trim().is_empty()).count(),
        1,
        "one invocation, one response line: {stdout}"
    );
}

#[test]
fn a_batch_that_ran_every_command_still_exits_0() {
    let b = TestBrowser::new("contract-batch-ran-all");
    if !open(b.name(), "checkable_kinds.html") {
        return;
    }
    // No --stop-on-error: the batch did what it was asked, and `ok` carries the failure.
    let (stdout, stderr, code) = run_batch(b.name(), &["--json", "batch"], FAILING);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    let v: serde_json::Value =
        serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{e}: {stdout}"));
    assert_eq!(v["ok"], false, "a failed command still makes it not ok: {stdout}");
    assert_eq!(v["results"].as_array().map(Vec::len), Some(2), "{stdout}");
    assert!(v.get("stopped_at").is_none(), "nothing was skipped: {stdout}");
}

#[test]
fn batch_prints_json_only_when_json_was_asked_for() {
    let b = TestBrowser::new("contract-batch-text");
    if !open(b.name(), "checkable_kinds.html") {
        return;
    }
    let (stdout, stderr, code) = run_batch(b.name(), &["batch", "--stop-on-error"], FAILING);
    assert_eq!(code, 1, "stdout={stdout} stderr={stderr}");
    assert!(
        serde_json::from_str::<serde_json::Value>(stdout.trim()).is_err(),
        "text mode was getting the raw JSON object: {stdout}"
    );
    assert!(stdout.contains("[0] error"), "{stdout}");
    assert!(
        stdout.contains("stopped at command 0 — 1 skipped"),
        "text mode says what --stop-on-error did: {stdout}"
    );
}
