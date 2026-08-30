//! Where a failure goes, and what it costs to read it wrong.
//!
//! Two claims in CLAUDE.md had one verb's worth of evidence each. "`--json` mode — errors exit
//! 1 with `{"ok":false}` on stdout" and "Only `assert` ever returns `2`" are claims about the
//! whole verb list, and an agent that pipes stdout into a JSON parser breaks on the first verb
//! that disagrees. This sweeps them across every failure path a caller can reach from one
//! loaded page, and pins the two shapes side by side: an error carries `error`, a claim that
//! did not hold carries `assertion` and exits 2 instead.

mod common;

use std::process::Command;

use common::{TestBrowser, binary, browser_ready, fixture_path, fixture_url};

/// Run one CLI invocation, returning `(stdout, stderr, exit code)`.
fn run(browser: &str, args: &[String]) -> (String, String, i32) {
    let mut full = vec!["--browser".to_string(), browser.to_string()];
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

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_string()).collect()
}

/// Open the fixture every case below fails against. `false` means Chrome is absent.
fn open(browser: &str) -> bool {
    if !browser_ready() {
        return false;
    }
    let (_, stderr, code) = run(
        browser,
        &argv(&["--json", "goto", &fixture_url("assert_page.html")]),
    );
    assert_eq!(code, 0, "goto failed: {stderr}");
    true
}

/// One failing invocation per verb that can fail on a loaded page, without its `--json`.
///
/// `diff` is first on purpose: it fails only while no snapshot is stored, and nothing else in
/// the list stores one.
fn failing_cases() -> Vec<Vec<String>> {
    let existing_file = fixture_path("upload_form.html").display().to_string();
    let mut cases = vec![
        argv(&["diff"]),
        argv(&["click", "--selector", "#nope"]),
        argv(&["click", "--selector", "???"]),
        argv(&["dblclick", "--selector", "#nope"]),
        argv(&["fill", "--selector", "#nope", "hello"]),
        argv(&["check", "--selector", "#nope"]),
        argv(&["select", "--selector", "#nope", "xyz"]),
        argv(&["text", "--selector", "#nope"]),
        argv(&["eval", "throw new Error('boom')"]),
        argv(&["hover", "n99999"]),
        argv(&["scroll", "n99999"]),
        argv(&["download", "--uid", "n99999"]),
        argv(&["screenshot", "--uid", "n99999"]),
        argv(&["frame", "#nope"]),
        argv(&["type", "hello"]),
        argv(&["extract"]),
        argv(&["read"]),
        argv(&["wait", "text", "zzz-never-on-this-page", "--timeout", "1"]),
    ];
    cases.push(argv(&["upload", "--selector", "#nope", &existing_file]));
    cases
}

/// `--json` and a failure: one JSON object on stdout, `ok:false`, exit 1 — for every verb, not
/// for the one that happened to be tested.
#[test]
fn every_failure_under_json_is_one_ok_false_object_on_stdout_and_exits_one() {
    let browser = TestBrowser::new("error-channel-json");
    if !open(browser.name()) {
        return;
    }
    for case in failing_cases() {
        let mut args = case.clone();
        args.push("--json".to_string());
        let (stdout, stderr, code) = run(browser.name(), &args);
        let label = case.join(" ");

        assert_eq!(
            code, 1,
            "{label}: exit {code} (2 is reserved for assert)\n{stdout}{stderr}"
        );
        let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(
            lines.len(),
            1,
            "{label}: stdout was not one JSON line: {stdout:?}"
        );
        let value: serde_json::Value = serde_json::from_str(lines[0])
            .unwrap_or_else(|e| panic!("{label}: stdout is not JSON ({e}): {}", lines[0]));
        assert_eq!(value["ok"], serde_json::json!(false), "{label}: {value}");
        assert!(
            value["error"].as_str().is_some_and(|e| !e.is_empty()),
            "{label}: an error a caller cannot read is not an error: {value}"
        );
        assert!(!stderr.contains("panic"), "{label}: {stderr}");
    }
}

/// Without `--json`, the same failures leave stdout empty. A caller capturing stdout must not
/// find half an answer there.
#[test]
fn the_same_failures_write_nothing_to_stdout_in_text_mode() {
    let browser = TestBrowser::new("error-channel-text");
    if !open(browser.name()) {
        return;
    }
    for case in failing_cases() {
        let (stdout, stderr, code) = run(browser.name(), &case);
        let label = case.join(" ");
        assert_eq!(code, 1, "{label}: exit {code}");
        assert!(
            stdout.trim().is_empty(),
            "{label}: stdout carried {stdout:?}"
        );
        assert!(
            stderr.starts_with("error: "),
            "{label}: stderr does not name the failure: {stderr:?}"
        );
    }
}

/// The one verb that exits 2, and the shape that goes with it: no `error`, an `assertion`
/// object saying what was compared. An agent that reads `error` to decide "this failed" would
/// see nothing here, which is why the exit code carries it instead.
#[test]
fn only_a_claim_that_did_not_hold_exits_two_and_it_reports_an_assertion_not_an_error() {
    let browser = TestBrowser::new("error-channel-assert");
    if !open(browser.name()) {
        return;
    }
    let (stdout, stderr, code) = run(
        browser.name(),
        &argv(&["assert", "exists", "--selector", "#nope", "--json"]),
    );
    assert_eq!(
        code, 2,
        "a claim that did not hold exits 2: {stdout}{stderr}"
    );
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).expect("assert JSON");
    assert_eq!(value["ok"], serde_json::json!(false), "{value}");
    assert!(
        value.get("error").is_none(),
        "an unheld claim is not an error: {value}"
    );
    assert_eq!(
        value["assertion"]["held"],
        serde_json::json!(false),
        "{value}"
    );
    assert_eq!(
        value["assertion"]["kind"],
        serde_json::json!("exists"),
        "{value}"
    );

    // A claim it could not check is an error like any other, and exits 1.
    let (stdout, _, code) = run(
        browser.name(),
        &argv(&[
            "assert", "value", "--uid", "n99999", "--equals", "x", "--json",
        ]),
    );
    assert_eq!(
        code, 1,
        "an unanswerable claim is not a failed one: {stdout}"
    );
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).expect("assert JSON");
    assert!(value["error"].as_str().is_some(), "{value}");
    assert!(
        value.get("assertion").is_none(),
        "nothing was compared: {value}"
    );
}

/// A `batch` whose whole input is unusable answers at the top level, not per command. The two
/// shapes are what a caller distinguishes on: `results` present means every command was seen and
/// each carries its own `ok`; `results` absent means none of them ran.
#[test]
fn a_batch_whose_whole_input_is_wrong_answers_without_a_results_array() {
    let browser = TestBrowser::new("error-channel-batch");
    if !open(browser.name()) {
        return;
    }
    for (input, expected) in [
        ("not json at all", "Invalid JSON:"),
        (
            "{\"cmd\":\"tabs\"}",
            "batch: expected a JSON array of commands",
        ),
        ("[]", "batch: empty command array"),
    ] {
        let mut child = Command::new(binary())
            .args(["--browser", browser.name(), "--json", "batch"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn batch");
        {
            use std::io::Write as _;
            child
                .stdin
                .take()
                .expect("stdin")
                .write_all(input.as_bytes())
                .expect("write");
        }
        let out = child.wait_with_output().expect("batch output");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert_eq!(out.status.code(), Some(1), "{input:?}: {stdout}");
        let value: serde_json::Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("{input:?}: {e}: {stdout}"));
        assert_eq!(value["ok"], serde_json::json!(false), "{value}");
        assert!(
            value["error"]
                .as_str()
                .unwrap_or_default()
                .contains(expected),
            "{input:?}: {value}"
        );
        assert!(
            value.get("results").is_none(),
            "nothing ran, so there is nothing to report per command: {value}"
        );
    }
}

/// A usage error is exit 1, not clap's 2, and it stays off stdout in both modes — the same
/// contract as a runtime failure, reached before any browser exists.
#[test]
fn a_usage_error_exits_one_and_leaves_stdout_empty_in_both_modes() {
    let browser = TestBrowser::new("error-channel-usage");
    for extra in [Vec::new(), argv(&["--json"])] {
        let mut args = argv(&["click", "--not-a-flag"]);
        args.extend(extra.clone());
        let (stdout, stderr, code) = run(browser.name(), &args);
        assert_eq!(code, 1, "clap's usage exit must be remapped: {stderr}");
        assert!(stdout.trim().is_empty(), "usage text on stdout: {stdout:?}");
        assert!(stderr.contains("error:"), "{stderr:?}");
    }
}
