//! The output and exit-code contract of the CLI, on the points where it was broken.
//!
//! 1. `check`/`uncheck`/`upload` took a uid AND a `--selector` and silently acted on the
//!    selector, while `click`/`fill`/`select`/`dblclick` refuse the same pair. All nine verbs
//!    now declare a clap `ArgGroup`, so the refusal is a usage error on stderr and no browser is
//!    opened for it — the wording and the exit code are pinned in
//!    `cli_tests::an_invalid_invocation_is_refused_before_a_browser_is_resolved`. What is left
//!    here is the fact that made the old refusal worth having: a refused invocation must not
//!    have acted on the target it discarded.
//! 2. Every `--json` response funnels through one writer, which answered a serialization
//!    failure with an empty line (unit-tested in `src/run_helpers.rs`, since a `Value` cannot
//!    be made to fail from out here).
//! 3. The CLI `batch` exited 0 even when `--stop-on-error` cut the run short, and printed raw
//!    JSON in text mode.

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

/// The page is the evidence: whatever the message says, a refused invocation must leave the
/// target it discarded alone. Before the fix `check n1 --selector "#native"` checked `#native`
/// and never mentioned the uid.
#[test]
fn a_refused_target_pair_never_acts_on_the_selector_it_discarded() {
    let b = TestBrowser::new("contract-target-pair");
    if !open(b.name(), "checkable_kinds.html") {
        return;
    }
    // `#native` starts unchecked, `#native_on` starts checked, so acting on either is visible.
    for (args, selector, state) in [
        (
            vec!["check", "n1", "--selector", "#native"],
            "#native",
            "--unchecked",
        ),
        (
            vec!["uncheck", "n1", "--selector", "#native_on"],
            "#native_on",
            "--checked",
        ),
    ] {
        let (stdout, stderr, code) = run(b.name(), &args);
        assert_eq!(code, 1, "stdout={stdout} stderr={stderr}");
        assert!(
            stderr.contains("cannot be used with"),
            "clap's group refusal, not a message from after the browser was opened: {stderr}"
        );
        assert!(
            stdout.trim().is_empty(),
            "a usage error belongs on stderr: {stdout}"
        );

        let (stdout, stderr, code) = run(
            b.name(),
            &["assert", "state", "--selector", selector, state],
        );
        assert_eq!(
            code, 0,
            "the discarded selector was acted on anyway: {stdout} {stderr}"
        );
    }
}

/// Stated rather than hidden: a malformed invocation no longer answers `{"ok":false}` on
/// stdout under `--json`. It is a clap usage error on stderr, exit 1, like every missing
/// positional and every `assert` group already was.
#[test]
fn a_usage_refusal_under_json_is_on_stderr_not_stdout() {
    let b = TestBrowser::new("contract-usage-json");
    if !open(b.name(), "checkable_kinds.html") {
        return;
    }
    let (stdout, stderr, code) = run(
        b.name(),
        &["--json", "check", "n1", "--selector", "#native"],
    );
    assert_eq!(code, 1, "stdout={stdout} stderr={stderr}");
    assert!(
        stdout.trim().is_empty(),
        "nothing on stdout for a usage error: {stdout}"
    );
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

// ---------------------------------------------------------------------------
// 3. A batch that stopped on an error says so in its exit code
// ---------------------------------------------------------------------------

/// A uid no snapshot ever produced: the first command fails, deterministically and without
/// depending on the page.
const FAILING: &str = r#"[{"cmd":"click","uid":"n999999"},{"cmd":"text","selector":"h1"}]"#;

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
    assert_eq!(
        v["ok"], false,
        "a failed command still makes it not ok: {stdout}"
    );
    assert_eq!(v["results"].as_array().map(Vec::len), Some(2), "{stdout}");
    assert!(
        v.get("stopped_at").is_none(),
        "nothing was skipped: {stdout}"
    );
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
