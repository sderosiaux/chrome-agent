//! Recording a macro from a real session, replaying it, and stopping on a guard that fails.
//! Distillation itself is pure and unit-tested in `src/macros_record.rs`.

use std::process::{Command, Stdio};

use serde_json::{Value, json};

mod common;
use common::TestBrowser;

fn run_cli(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(common::binary())
        .args(args)
        .output()
        .expect("run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

/// Feed a pipe session and hand back one parsed response per line.
fn run_pipe(browser: &str, commands: &[Value]) -> Vec<Value> {
    use std::io::Write as _;
    let mut child = Command::new(common::binary())
        .args(["--browser", browser, "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pipe");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for cmd in commands {
            writeln!(stdin, "{cmd}").expect("write");
        }
    }
    let output = child.wait_with_output().expect("pipe output");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("not JSON ({e}): {line}")))
        .collect()
}

/// A macro file this test owns, removed when it ends.
struct TestMacro(String);

impl TestMacro {
    fn new(label: &str) -> Self {
        Self(common::unique_name(label))
    }
    fn name(&self) -> &str {
        &self.0
    }
    fn path(&self) -> std::path::PathBuf {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .expect("HOME");
        home.join(".chrome-agent")
            .join("macros")
            .join(format!("{}.json", self.0))
    }
    /// Write one by hand, so a test can exercise `macro run` without the recorder.
    fn write(&self, body: Value) {
        let path = self.path();
        std::fs::create_dir_all(path.parent().expect("parent")).expect("macro dir");
        let mut file = body;
        file["name"] = json!(self.0);
        std::fs::write(&path, serde_json::to_string_pretty(&file).unwrap()).expect("write macro");
    }
}

impl Drop for TestMacro {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.path());
    }
}

/// Navigate, inspect, one failing click, then three that work. Returns the recording path.
fn record_a_session(browser: &str, fixture: &str) -> std::path::PathBuf {
    let record = common::temp_path("macro-session", "jsonl");
    let url = common::fixture_url(fixture);
    let with_record = |mut cmd: Value| {
        cmd["_record"] = json!(record.to_string_lossy());
        cmd
    };
    let responses = run_pipe(
        browser,
        &[
            with_record(json!({"cmd": "goto", "url": url})),
            with_record(json!({"cmd": "inspect"})),
            with_record(json!({"cmd": "click", "selector": "#not-a-thing"})),
            with_record(json!({"cmd": "click", "selector": "[data-test=billing]"})),
            with_record(json!({"cmd": "fill", "selector": "#email", "value": "ada@example.com"})),
            with_record(json!({"cmd": "click", "selector": "#confirm"})),
        ],
    );
    assert_eq!(
        responses.len(),
        6,
        "one response per command: {responses:?}"
    );
    assert_eq!(
        responses[2]["ok"], false,
        "the dead end really failed: {}",
        responses[2]
    );
    assert_eq!(
        responses[5]["ok"], true,
        "the task really worked: {}",
        responses[5]
    );
    record
}

/// The whitelist on a real session: what the file keeps and what it refuses to keep.
#[test]
fn a_recorded_macro_keeps_the_path_and_none_of_the_numbers() {
    let browser = TestBrowser::new("macro-record");
    if !common::browser_ready() {
        return;
    }
    let macro_file = TestMacro::new("macro-record");
    let record = record_a_session(browser.name(), "macro_cancel_flow.html");

    let (stdout, stderr, code) = run_cli(&[
        "--json",
        "macro",
        "record",
        macro_file.name(),
        "--from-recording",
        &record.to_string_lossy(),
    ]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let report: Value = serde_json::from_str(&stdout).expect("JSON report");
    assert_eq!(report["steps"], 4, "goto, click, fill, click: {report}");
    assert!(report["refused"].as_array().unwrap().is_empty(), "{report}");
    // The exploration and the dead end are dropped, each with a reason.
    let dropped = report["dropped"].as_array().expect("dropped");
    assert_eq!(dropped.len(), 2, "{report}");
    assert!(
        dropped
            .iter()
            .any(|d| d["reason"].as_str().unwrap().contains("reads the page"))
    );
    assert!(
        dropped
            .iter()
            .any(|d| d["reason"].as_str().unwrap().contains("failed"))
    );

    let text = std::fs::read_to_string(macro_file.path()).expect("the macro file");
    let parsed: Value = serde_json::from_str(&text).expect("valid JSON");
    assert_eq!(parsed["steps"].as_array().unwrap().len(), 4);
    assert_eq!(parsed["steps"][1]["expect"]["delivery"], "target_hit");
    assert_eq!(parsed["steps"][1]["expect"]["verdict"], "changed");
    assert_eq!(parsed["steps"][2]["expect"]["verbatim"], true);

    // The blacklist: each of these would break the macro on a page that still works.
    for forbidden in [
        "verdict_reason",
        "\"added\"",
        "\"removed\"",
        "observed_after_ms",
        "delta",
        "uid",
    ] {
        assert!(
            !text.contains(forbidden),
            "{forbidden} reached the file:\n{text}"
        );
    }
}

#[test]
fn a_recorded_macro_runs_again_and_its_guards_hold() {
    let browser = TestBrowser::new("macro-replay");
    if !common::browser_ready() {
        return;
    }
    let macro_file = TestMacro::new("macro-replay");
    let record = record_a_session(browser.name(), "macro_cancel_flow.html");
    let (_, _, code) = run_cli(&[
        "--json",
        "macro",
        "record",
        macro_file.name(),
        "--from-recording",
        &record.to_string_lossy(),
    ]);
    assert_eq!(code, 0);

    let runner = TestBrowser::new("macro-replay-run");
    let (stdout, stderr, code) = run_cli(&[
        "--browser",
        runner.name(),
        "--json",
        "macro",
        "run",
        macro_file.name(),
    ]);
    assert_eq!(code, 0, "the macro did not replay: {stdout}{stderr}");
    let report: Value = serde_json::from_str(&stdout).expect("JSON report");
    assert_eq!(report["ok"], true, "{report}");
    assert_eq!(report["steps_run"], 4, "{report}");
    assert_eq!(
        report["unguarded_steps"], 0,
        "every step promised something: {report}"
    );

    let (state, _, _) = run_cli(&[
        "--browser",
        runner.name(),
        "--json",
        "eval",
        "document.getElementById('result').className",
    ]);
    let state: Value = serde_json::from_str(&state).expect("JSON");
    assert_eq!(
        state["result"], "on",
        "the confirmation section is showing: {state}"
    );
}

#[test]
fn a_guard_that_does_not_hold_stops_the_run_where_it_failed() {
    let browser = TestBrowser::new("macro-stop");
    if !common::browser_ready() {
        return;
    }
    let macro_file = TestMacro::new("macro-stop");
    let url = common::fixture_url("macro_cancel_flow.html");
    // Step 1 clicks a heading, so `verdict: changed` cannot hold; the fill must not run.
    macro_file.write(json!({
        "name": "placeholder",
        "steps": [
            {"do": {"cmd": "goto", "url": url}, "expect": {"url_matches": "macro_cancel_flow"}},
            {"do": {"cmd": "click", "selector": "h1"}, "expect": {"verdict": "changed"}},
            {"do": {"cmd": "fill", "selector": "#email", "value": "must-not-run@example.com"},
             "expect": {"verbatim": true}}
        ]
    }));

    let (stdout, stderr, code) = run_cli(&[
        "--browser",
        browser.name(),
        "--json",
        "macro",
        "run",
        macro_file.name(),
    ]);
    // 2, not 1: the guard RAN and the page disagreed. Same claim class as a failed assertion,
    // and the opposite recovery from "the browser never started".
    assert_eq!(
        code, 2,
        "a guard that did not hold is exit 2: {stdout}{stderr}"
    );
    let report: Value = serde_json::from_str(&stdout).expect("JSON report on stdout");
    assert_eq!(report["ok"], false);
    assert_eq!(
        report["stopped_by"], "guard",
        "the claim class is in the data: {report}"
    );
    assert_eq!(
        report["stopped_at"], 1,
        "the failing step is named: {report}"
    );
    assert_eq!(report["guard"], "verdict");
    assert_eq!(report["expected"], "changed");
    assert_ne!(report["observed"], "changed", "{report}");
    assert!(
        report["next"].is_string(),
        "the action's own branch is carried: {report}"
    );
    assert!(
        report["stop"]
            .as_str()
            .unwrap_or_default()
            .contains("did not happen"),
        "the report says the rest did not run: {report}"
    );

    let (state, _, _) = run_cli(&[
        "--browser",
        browser.name(),
        "--json",
        "eval",
        "document.getElementById('email').value",
    ]);
    let state: Value = serde_json::from_str(&state).expect("JSON");
    assert_eq!(
        state["result"], "",
        "the step after the failure was not run: {state}"
    );
}

/// A page guard is the same claim class as a response guard, so it exits the same way — and the
/// text mode of the same stop keeps stdout empty, like `assert`.
#[test]
fn a_page_guard_that_does_not_hold_also_exits_two() {
    let browser = TestBrowser::new("macro-page-guard");
    if !common::browser_ready() {
        return;
    }
    let macro_file = TestMacro::new("macro-page-guard");
    let url = common::fixture_url("macro_cancel_flow.html");
    // The navigation succeeds and lands nowhere near this path, so the guard is answered and
    // answered `no`.
    macro_file.write(json!({
        "name": "placeholder",
        "steps": [
            {"do": {"cmd": "goto", "url": url}, "expect": {"url_matches": "not-this-page-at-all"}}
        ]
    }));

    let (stdout, stderr, code) = run_cli(&[
        "--browser",
        browser.name(),
        "--json",
        "macro",
        "run",
        macro_file.name(),
    ]);
    assert_eq!(code, 2, "the page was read and disagreed: {stdout}{stderr}");
    let report: Value = serde_json::from_str(&stdout).expect("JSON report on stdout");
    assert_eq!(report["ok"], false);
    assert_eq!(report["stopped_by"], "guard", "{report}");
    assert_eq!(report["guard"], "url_matches", "{report}");
    assert_eq!(report["stopped_at"], 0, "{report}");
    assert!(
        report["error"]
            .as_str()
            .unwrap_or_default()
            .contains("did not hold"),
        "the sentence names the claim, not an operational failure: {report}"
    );

    // Without --json the same stop is exit 2 with the report on stderr and stdout empty, so a
    // shell pipeline can branch on the code alone.
    let (stdout, stderr, code) = run_cli(&[
        "--browser",
        browser.name(),
        "macro",
        "run",
        macro_file.name(),
    ]);
    assert_eq!(code, 2, "{stdout}{stderr}");
    assert!(
        stdout.trim().is_empty(),
        "stdout carries nothing in text mode: {stdout}"
    );
    assert!(
        stderr.contains("url_matches"),
        "the guard is named on stderr: {stderr}"
    );
}

/// A step that never ran is an operational failure, and stays exit 1 — the code that also means
/// "the browser never started". No guard was answered, so nothing is claimed about the page.
#[test]
fn a_step_that_could_not_run_is_an_error_and_still_exits_one() {
    let browser = TestBrowser::new("macro-step-error");
    if !common::browser_ready() {
        return;
    }
    let macro_file = TestMacro::new("macro-step-error");
    let url = common::fixture_url("macro_cancel_flow.html");
    macro_file.write(json!({
        "name": "placeholder",
        "steps": [
            {"do": {"cmd": "goto", "url": url}, "expect": {"url_matches": "macro_cancel_flow"}},
            {"do": {"cmd": "click", "selector": "#no-such-element"}, "expect": {"verdict": "changed"}},
            {"do": {"cmd": "fill", "selector": "#email", "value": "must-not-run@example.com"},
             "expect": {"verbatim": true}}
        ]
    }));

    let (stdout, stderr, code) = run_cli(&[
        "--browser",
        browser.name(),
        "--json",
        "macro",
        "run",
        macro_file.name(),
    ]);
    assert_eq!(
        code, 1,
        "the step failed; nothing was compared: {stdout}{stderr}"
    );
    let report: Value = serde_json::from_str(&stdout).expect("JSON report on stdout");
    assert_eq!(report["ok"], false);
    assert_eq!(report["stopped_by"], "error", "{report}");
    assert_eq!(report["stopped_at"], 1, "{report}");
    assert!(
        report.get("guard").is_none(),
        "no guard was reached: {report}"
    );
    assert!(
        report["error"]
            .as_str()
            .unwrap_or_default()
            .contains("did not run"),
        "the sentence says the step never ran: {report}"
    );

    let (state, _, _) = run_cli(&[
        "--browser",
        browser.name(),
        "--json",
        "eval",
        "document.getElementById('email').value",
    ]);
    let state: Value = serde_json::from_str(&state).expect("JSON");
    assert_eq!(
        state["result"], "",
        "the step after the failure was not run: {state}"
    );
}

/// The macro file itself is operational: it never reaches a guard, so it never reaches 2.
#[test]
fn a_macro_that_cannot_be_read_exits_one() {
    let browser = TestBrowser::new("macro-unreadable");
    if !common::browser_ready() {
        return;
    }
    let missing = common::unique_name("macro-no-such");
    let (stdout, stderr, code) = run_cli(&[
        "--browser",
        browser.name(),
        "--json",
        "macro",
        "run",
        &missing,
    ]);
    assert_eq!(
        code, 1,
        "a missing macro is an error, never a claim: {stdout}{stderr}"
    );
    let report: Value = serde_json::from_str(&stdout).expect("JSON on stdout");
    assert_eq!(report["ok"], false, "{report}");
    assert!(
        report["stopped_by"].is_null(),
        "it never got as far as a step: {report}"
    );
    assert!(
        report["error"]
            .as_str()
            .unwrap_or_default()
            .contains(&missing),
        "{report}"
    );

    // Malformed rather than missing: still 1.
    let broken = TestMacro::new("macro-malformed");
    let path = broken.path();
    std::fs::create_dir_all(path.parent().expect("parent")).expect("macro dir");
    std::fs::write(&path, "{ not json").expect("write");
    let (stdout, stderr, code) = run_cli(&[
        "--browser",
        browser.name(),
        "--json",
        "macro",
        "run",
        broken.name(),
    ]);
    assert_eq!(
        code, 1,
        "a malformed macro is an error too: {stdout}{stderr}"
    );
}

/// A secret is never stored, so a run without it stops before touching the page.
#[test]
fn a_missing_secret_refuses_before_the_browser_is_touched() {
    let browser = TestBrowser::new("macro-secret");
    if !common::browser_ready() {
        return;
    }
    let macro_file = TestMacro::new("macro-secret");
    let url = common::fixture_url("macro_cancel_flow.html");
    macro_file.write(json!({
        "name": "placeholder",
        "params": {"card": {"required": true, "secret": true}},
        "steps": [
            {"do": {"cmd": "goto", "url": url}, "expect": {}},
            {"do": {"cmd": "fill", "selector": "#card", "value": "{{card}}"},
             "expect": {"verbatim": true}}
        ]
    }));

    let (stdout, stderr, code) = run_cli(&[
        "--browser",
        browser.name(),
        "--json",
        "macro",
        "run",
        macro_file.name(),
    ]);
    // 1, not 2: no guard was ever evaluated, so this is not a claim about the page.
    assert_eq!(code, 1, "{stdout}{stderr}");
    let said = format!("{stdout}{stderr}");
    assert!(
        said.contains("card"),
        "the missing parameter is named: {said}"
    );
    assert!(
        said.contains("never stored"),
        "and why it cannot be defaulted: {said}"
    );
    assert!(
        said.contains("--var card="),
        "with the command that fixes it: {said}"
    );

    let (state, _, _) = run_cli(&[
        "--browser",
        browser.name(),
        "--json",
        "eval",
        "location.href",
    ]);
    let state: Value = serde_json::from_str(&state).unwrap_or_else(|_| json!({}));
    assert!(
        !state["result"]
            .as_str()
            .unwrap_or_default()
            .contains("macro_cancel_flow"),
        "the macro navigated before finding out it could not finish: {state}"
    );
}

/// A card number is a secret by `element::SECRET_FIELD`.
#[test]
fn a_recorded_secret_becomes_a_parameter_and_never_a_value() {
    let browser = TestBrowser::new("macro-secret-record");
    if !common::browser_ready() {
        return;
    }
    let macro_file = TestMacro::new("macro-secret-record");
    let record = common::temp_path("macro-secret-session", "jsonl");
    let url = common::fixture_url("macro_cancel_flow.html");
    let with_record = |mut cmd: Value| {
        cmd["_record"] = json!(record.to_string_lossy());
        cmd
    };
    let responses = run_pipe(
        browser.name(),
        &[
            with_record(json!({"cmd": "goto", "url": url})),
            with_record(json!({"cmd": "click", "selector": "[data-test=billing]"})),
            with_record(json!({"cmd": "fill", "selector": "#card", "value": "4111111111111111"})),
        ],
    );
    assert_eq!(responses[2]["ok"], true, "{}", responses[2]);
    assert_eq!(
        responses[2]["value"]["redacted"], true,
        "the fill knew it was a secret: {}",
        responses[2]
    );

    let (stdout, stderr, code) = run_cli(&[
        "--json",
        "macro",
        "record",
        macro_file.name(),
        "--from-recording",
        &record.to_string_lossy(),
    ]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let text = std::fs::read_to_string(macro_file.path()).expect("the macro file");
    assert!(
        !text.contains("4111"),
        "the card number reached the file:\n{text}"
    );
    assert!(
        text.contains("{{card}}"),
        "the value became a parameter:\n{text}"
    );
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["params"]["card"]["secret"], true, "{text}");
    // The guard is the read-back: a secret is verified, never printed.
    assert_eq!(parsed["steps"][2]["expect"]["verbatim"], true, "{text}");
}

/// A step aimed by uid reports no role and no name, so its locator comes from the snapshot.
#[test]
fn a_step_aimed_by_uid_is_recorded_by_role_and_name_and_runs_again() {
    let browser = TestBrowser::new("macro-uid");
    if !common::browser_ready() {
        return;
    }
    let macro_file = TestMacro::new("macro-uid");
    let record = common::temp_path("macro-uid-session", "jsonl");
    let url = common::fixture_url("macro_cancel_flow.html");
    let with_record = |mut cmd: Value| {
        cmd["_record"] = json!(record.to_string_lossy());
        cmd
    };

    // Navigate and inspect: a uid only exists after a snapshot.
    let first = run_pipe(
        browser.name(),
        &[
            with_record(json!({"cmd": "goto", "url": url})),
            with_record(json!({"cmd": "inspect"})),
        ],
    );
    let snapshot = first[1]["snapshot"].as_str().expect("a snapshot");
    let uid = snapshot
        .lines()
        .find(|line| line.contains("Manage billing"))
        .and_then(|line| line.trim_start().strip_prefix("uid="))
        .and_then(|rest| rest.split_whitespace().next())
        .expect("the button's uid")
        .to_string();

    let clicked = run_pipe(
        browser.name(),
        &[
            with_record(json!({"cmd": "inspect"})),
            with_record(json!({"cmd": "click", "uid": uid})),
        ],
    );
    assert_eq!(clicked[1]["ok"], true, "{}", clicked[1]);
    assert!(
        clicked[1]["role"].is_null(),
        "the uid path reports no role — that is the premise"
    );

    let (stdout, stderr, code) = run_cli(&[
        "--json",
        "macro",
        "record",
        macro_file.name(),
        "--from-recording",
        &record.to_string_lossy(),
    ]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let report: Value = serde_json::from_str(&stdout).expect("JSON");
    assert!(
        report["refused"].as_array().unwrap().is_empty(),
        "not refused any more: {report}"
    );

    let text = std::fs::read_to_string(macro_file.path()).expect("the macro");
    assert!(
        !text.contains("\"uid\""),
        "no uid survives into the file:\n{text}"
    );
    let parsed: Value = serde_json::from_str(&text).unwrap();
    let last = parsed["steps"].as_array().unwrap().last().unwrap();
    assert_eq!(last["do"]["role"], "button", "{text}");
    assert_eq!(last["do"]["name"], "Manage billing", "{text}");

    let runner = TestBrowser::new("macro-uid-run");
    let (stdout, stderr, code) = run_cli(&[
        "--browser",
        runner.name(),
        "--json",
        "macro",
        "run",
        macro_file.name(),
    ]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let run: Value = serde_json::from_str(&stdout).expect("JSON");
    assert_eq!(run["ok"], true, "{run}");
    let (state, _, _) = run_cli(&[
        "--browser",
        runner.name(),
        "--json",
        "eval",
        "document.getElementById('billing-panel').className",
    ]);
    let state: Value = serde_json::from_str(&state).expect("JSON");
    assert_eq!(
        state["result"], "on",
        "the click really landed by role+name: {state}"
    );
}
