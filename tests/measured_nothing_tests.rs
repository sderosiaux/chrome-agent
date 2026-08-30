//! Two commands that answered as if they had measured something.
//!
//! `console` read `window.__chrome_agent_console || []`, so a page where the interceptor had
//! never been installed came back with the same empty list as a page that had simply not
//! logged — and "No console messages captured" is a statement about the page, which an agent
//! uses to conclude the page is healthy. `--stealth` discarded all four of its CDP results, so
//! a patch that did not land was invisible; and a patch that does not land produces exactly
//! the failure the flag exists to prevent, in the worst form — `landed.serving: "challenge"`
//! and a hint recommending `--connect`, attributing to the site a cause that is ours.
//!
//! Both halves report on stderr, never stdout, so the `--json` contract is untouched: a field
//! beside the list, never an error in place of it.

use std::io::Write as _;
use std::process::{Command, Stdio};

use serde_json::Value;

mod common;
use common::{binary, TestBrowser};

/// Run a pipe session and return (stdout lines as JSON, raw stderr).
///
/// Pipe mode is what makes the console half observable at all: the interceptor is injected
/// once per CONNECTION, and every CLI invocation opens a fresh one, so a bare `chrome-agent
/// console` would re-install the buffer a previous command had removed.
fn run_pipe(browser: &str, commands: &[Value]) -> (Vec<Value>, String) {
    let mut child = Command::new(binary())
        .args(["--browser", browser, "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn chrome-agent pipe");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for cmd in commands {
            writeln!(stdin, "{}", serde_json::to_string(cmd).unwrap()).unwrap();
        }
    }
    let output = child.wait_with_output().expect("wait pipe");
    let lines = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
        .collect();
    (lines, String::from_utf8_lossy(&output.stderr).into_owned())
}

fn run_cli(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(binary()).args(args).output().expect("run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

// ---------------------------------------------------------------------------
// console: the two silences
// ---------------------------------------------------------------------------

/// The archetype: the page had a listener, then it did not, and the two reads used to be
/// byte-identical.
#[test]
fn a_missing_interceptor_is_reported_instead_of_read_as_a_quiet_page() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("console-not-installed");
    let url = common::fixture_url("webdriver_locked.html");
    let (out, stderr) = run_pipe(
        b.name(),
        &[
            serde_json::json!({"cmd": "goto", "url": url}),
            serde_json::json!({"cmd": "eval", "expression": "console.log('hello'); 1"}),
            serde_json::json!({"cmd": "console"}),
            serde_json::json!({"cmd": "eval", "expression": "delete window.__chrome_agent_console; 1"}),
            serde_json::json!({"cmd": "console"}),
        ],
    );
    assert_eq!(out.len(), 5, "one response per command: {out:?}");

    // With the interceptor in place the read is a real measurement.
    let listening = &out[2];
    assert_eq!(listening["ok"], true, "{listening}");
    assert_eq!(listening["installed"], true, "{listening}");
    assert_eq!(listening["messages"][0]["message"], "hello", "{listening}");

    // With it gone the list is empty for a reason that has nothing to do with the page.
    let blind = &out[4];
    assert_eq!(blind["ok"], true, "a read command still read: {blind}");
    assert_eq!(
        blind["installed"], false,
        "the field is the whole point: the same empty list, told apart: {blind}"
    );
    assert_eq!(blind["messages"], serde_json::json!([]), "the list stays a list: {blind}");
    assert!(
        stderr.contains("console interceptor not installed on this page"),
        "the absence must be stated, not left as an empty list: {stderr}"
    );
    assert!(
        stderr.contains("window.__chrome_agent_console is undefined"),
        "and it must name what it measured: {stderr}"
    );
    // Exactly one of the two reads was blind. stderr is the whole session's, so the count is
    // what separates "the blind read warned" from "every read warns".
    assert_eq!(
        stderr.matches("console interceptor not installed on this page").count(),
        1,
        "the read that DID measure something must not warn: {stderr}"
    );
}

/// What it must NOT say. The measurement is "nothing was listening"; "the page logged things
/// you missed" is a different claim and this tool cannot make it.
#[test]
fn the_warning_claims_nothing_about_what_was_missed() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("console-no-overclaim");
    let url = common::fixture_url("webdriver_locked.html");
    let (_, stderr) = run_pipe(
        b.name(),
        &[
            serde_json::json!({"cmd": "goto", "url": url}),
            serde_json::json!({"cmd": "eval", "expression": "delete window.__chrome_agent_console; 1"}),
            serde_json::json!({"cmd": "console"}),
        ],
    );
    let warning = stderr
        .lines()
        .find(|l| l.contains("interceptor not installed"))
        .unwrap_or_else(|| panic!("expected the warning: {stderr}"));
    let lowered = warning.to_lowercase();
    for forbidden in ["missed", "lost", "logged messages", "were captured elsewhere"] {
        assert!(
            !lowered.contains(forbidden),
            "the warning must not claim what the page did: {warning}"
        );
    }
}

/// `--clear` used to ASSIGN an empty array, which on a page with no interceptor CREATED the
/// buffer — so the next read would have probed `installed: true` on a page where nothing is
/// listening. The fix that reports the absence would have been undone by the fix's own
/// neighbour.
#[test]
fn clearing_a_page_with_no_interceptor_does_not_manufacture_one() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("console-clear-no-buffer");
    let url = common::fixture_url("webdriver_locked.html");
    let (out, stderr) = run_pipe(
        b.name(),
        &[
            serde_json::json!({"cmd": "goto", "url": url}),
            serde_json::json!({"cmd": "eval", "expression": "delete window.__chrome_agent_console; 1"}),
            serde_json::json!({"cmd": "console", "clear": true}),
            serde_json::json!({"cmd": "eval", "expression": "typeof window.__chrome_agent_console"}),
            serde_json::json!({"cmd": "console"}),
        ],
    );
    assert_eq!(
        out[3]["result"], "undefined",
        "the clear must not create the buffer it had nothing to clear: {:?}",
        out[3]
    );
    assert!(
        stderr.contains("nothing to clear"),
        "and it says so rather than reporting a clear it did not do: {stderr}"
    );
    // The second read is still blind, which is the point of the previous assertion.
    assert_eq!(
        stderr.matches("console interceptor not installed on this page").count(),
        2,
        "both reads were blind and both said so: {stderr}"
    );
}

/// The `--json` contract: a field beside the list, never an error in place of it. A blind read
/// is still `ok:true` with a `messages` array, because it is a read command and a missing
/// listener does not stop it reading.
#[test]
fn a_blind_read_stays_a_successful_read() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("console-json-shape");
    let url = common::fixture_url("webdriver_locked.html");
    let (out, _) = run_pipe(
        b.name(),
        &[
            serde_json::json!({"cmd": "goto", "url": url}),
            serde_json::json!({"cmd": "eval", "expression": "delete window.__chrome_agent_console; 1"}),
            serde_json::json!({"cmd": "console", "level": "error", "limit": 5}),
        ],
    );
    let blind = &out[2];
    assert_eq!(blind["ok"], true, "{blind}");
    assert!(blind["messages"].is_array(), "{blind}");
    assert!(blind["error"].is_null(), "not an error: {blind}");
    assert_eq!(blind["installed"], false, "a field beside the list: {blind}");

    // And the response says nothing about what the page might have logged. The measurement is
    // "nothing was listening"; anything counting or naming what was missed would be invented.
    let obj = blind.as_object().expect("an object");
    let keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        vec!["installed", "messages", "ok"],
        "no field may infer beyond the measurement: {blind}"
    );
}

/// The CLI carries the same field, and reports the ordinary case honestly: a page that logged
/// nothing while something WAS listening is a measurement, and it must not read as a failure.
#[test]
fn the_cli_reports_a_real_measurement_as_installed() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("console-cli-installed");
    let url = common::fixture_url("focus_after_click.html");
    let (_, _, code) = run_cli(&["--browser", b.name(), "goto", &url]);
    assert_eq!(code, 0, "fixture must load");
    let (_, _, code) = run_cli(&["--browser", b.name(), "eval", "console.log('salut')"]);
    assert_eq!(code, 0);

    let (stdout, _, code) = run_cli(&["--browser", b.name(), "--json", "console"]);
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{e}: {stdout}"));
    assert_eq!(v["installed"], true, "{v}");
    assert_eq!(v["messages"][0]["message"], "salut", "{v}");

    // Text mode must be readable on its own: the two silences are told apart without --json.
    let (text, _, code) = run_cli(&["--browser", b.name(), "console", "--clear"]);
    assert_eq!(code, 0);
    assert!(text.contains("LOG: salut"), "{text}");
    let (text, _, code) = run_cli(&["--browser", b.name(), "console"]);
    assert_eq!(code, 0);
    assert_eq!(
        text.trim(),
        "No console messages captured.",
        "a listener that heard nothing says exactly that, and says nothing else"
    );
}

// ---------------------------------------------------------------------------
// stealth: a patch that did not land
// ---------------------------------------------------------------------------

/// `webdriver_locked.html` freezes `navigator.webdriver` with `configurable: false`, so the
/// "patch the already-loaded page" step throws. Chrome answers that evaluation with `Ok` and
/// an `exceptionDetails`, which is why discarding the result hid it so well.
#[test]
fn a_stealth_patch_that_did_not_land_is_named_on_stderr() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("stealth-patch-failed");
    let url = common::fixture_url("webdriver_locked.html");
    let (_, _, code) = run_cli(&["--browser", b.name(), "goto", &url]);
    assert_eq!(code, 0, "fixture must load");

    let (stdout, stderr, code) = run_cli(&["--browser", b.name(), "--stealth", "--json", "console"]);
    assert_eq!(code, 0, "a failed patch does not fail the command: {stderr}");

    assert!(
        stderr.contains("stealth patch not applied"),
        "the failure must be stated: {stderr}"
    );
    assert!(
        stderr.contains("navigator.webdriver"),
        "and it must name WHICH of the four failed — they are four different fingerprints: {stderr}"
    );
    assert!(
        stderr.contains("Cannot redefine property"),
        "with the reason Chrome gave: {stderr}"
    );

    // stdout is the --json contract and saw none of it.
    let v: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout must stay clean JSON ({e}): {stdout}"));
    assert_eq!(v["ok"], true, "{v}");
    assert!(
        !stdout.contains("stealth patch"),
        "the warning must not reach stdout: {stdout}"
    );
}

/// The control. A warning that fires on a healthy page is a warning nobody reads.
#[test]
fn a_stealth_session_on_an_ordinary_page_warns_about_nothing() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("stealth-patch-ok");
    let url = common::fixture_url("focus_after_click.html");
    let (_, _, code) = run_cli(&["--browser", b.name(), "--stealth", "goto", &url]);
    assert_eq!(code, 0, "fixture must load");

    let (_, stderr, code) = run_cli(&["--browser", b.name(), "--stealth", "--json", "console"]);
    assert_eq!(code, 0);
    assert!(
        !stderr.contains("stealth patch not applied"),
        "all four landed here: {stderr}"
    );
}
