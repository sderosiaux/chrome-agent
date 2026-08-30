//! A macro is a path that worked once, and a guard is what makes replaying it worth anything.
//!
//! The two halves are tested apart on purpose. Distillation is pure and lives in unit tests
//! (`src/macros_record.rs`) where a frozen response proves what the whitelist keeps; here is
//! what only a browser can answer: that the file records a real session, that running it holds
//! its guards on the same page, and — the one that matters — that a guard which cannot hold
//! stops the run instead of letting it finish.

use std::process::{Command, Stdio};

use serde_json::{json, Value};

mod common;
use common::TestBrowser;

fn run_cli(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(common::binary()).args(args).output().expect("run chrome-agent");
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
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from).expect("HOME");
        home.join(".chrome-agent").join("macros").join(format!("{}.json", self.0))
    }
    /// Write one by hand. The format is the product; a test that could not write one would be
    /// testing the recorder only.
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

/// Drive the fixture the way an agent would the first time: navigate, look, get one thing
/// wrong, then do the three things that work. Returns the recording path.
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
    assert_eq!(responses.len(), 6, "one response per command: {responses:?}");
    assert_eq!(responses[2]["ok"], false, "the dead end really failed: {}", responses[2]);
    assert_eq!(responses[5]["ok"], true, "the task really worked: {}", responses[5]);
    record
}

/// The whitelist, on a real session rather than a frozen response: what the file keeps, and —
/// louder — what it refuses to keep.
#[test]
fn a_recorded_macro_keeps_the_path_and_none_of_the_numbers() {
    let browser = TestBrowser::new("macro-record");
    if !common::browser_ready() {
        return;
    }
    let macro_file = TestMacro::new("macro-record");
    let record = record_a_session(browser.name(), "macro_cancel_flow.html");

    let (stdout, stderr, code) = run_cli(&[
        "--json", "macro", "record", macro_file.name(),
        "--from-recording", &record.to_string_lossy(),
    ]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let report: Value = serde_json::from_str(&stdout).expect("JSON report");
    assert_eq!(report["steps"], 4, "goto, click, fill, click: {report}");
    assert!(report["refused"].as_array().unwrap().is_empty(), "{report}");
    // The exploration and the dead end are gone, and each says why.
    let dropped = report["dropped"].as_array().expect("dropped");
    assert_eq!(dropped.len(), 2, "{report}");
    assert!(dropped.iter().any(|d| d["reason"].as_str().unwrap().contains("reads the page")));
    assert!(dropped.iter().any(|d| d["reason"].as_str().unwrap().contains("failed")));

    let text = std::fs::read_to_string(macro_file.path()).expect("the macro file");
    let parsed: Value = serde_json::from_str(&text).expect("valid JSON");
    assert_eq!(parsed["steps"].as_array().unwrap().len(), 4);
    assert_eq!(parsed["steps"][1]["expect"]["delivery"], "target_hit");
    assert_eq!(parsed["steps"][1]["expect"]["verdict"], "changed");
    assert_eq!(parsed["steps"][2]["expect"]["verbatim"], true);

    // The blacklist. These are the fields a response carries in quantity, and every one of them
    // would make the macro break on a page that still works.
    for forbidden in ["verdict_reason", "\"added\"", "\"removed\"", "observed_after_ms", "delta", "uid"] {
        assert!(!text.contains(forbidden), "{forbidden} reached the file:\n{text}");
    }
}

/// The same session, run again from the file: every guard holds on the page it was recorded on.
#[test]
fn a_recorded_macro_runs_again_and_its_guards_hold() {
    let browser = TestBrowser::new("macro-replay");
    if !common::browser_ready() {
        return;
    }
    let macro_file = TestMacro::new("macro-replay");
    let record = record_a_session(browser.name(), "macro_cancel_flow.html");
    let (_, _, code) = run_cli(&[
        "--json", "macro", "record", macro_file.name(),
        "--from-recording", &record.to_string_lossy(),
    ]);
    assert_eq!(code, 0);

    let runner = TestBrowser::new("macro-replay-run");
    let (stdout, stderr, code) = run_cli(&[
        "--browser", runner.name(), "--json", "macro", "run", macro_file.name(),
    ]);
    assert_eq!(code, 0, "the macro did not replay: {stdout}{stderr}");
    let report: Value = serde_json::from_str(&stdout).expect("JSON report");
    assert_eq!(report["ok"], true, "{report}");
    assert_eq!(report["steps_run"], 4, "{report}");
    assert_eq!(report["unguarded_steps"], 0, "every step promised something: {report}");

    // The page really is at the end of the task, not merely at the end of the file.
    let (state, _, _) = run_cli(&[
        "--browser", runner.name(), "--json", "eval",
        "document.getElementById('result').className",
    ]);
    let state: Value = serde_json::from_str(&state).expect("JSON");
    assert_eq!(state["result"], "on", "the confirmation section is showing: {state}");
}

/// The one that decides whether any of this is worth having: a guard that cannot hold stops
/// the run, and the steps after it do not happen.
#[test]
fn a_guard_that_does_not_hold_stops_the_run_where_it_failed() {
    let browser = TestBrowser::new("macro-stop");
    if !common::browser_ready() {
        return;
    }
    let macro_file = TestMacro::new("macro-stop");
    let url = common::fixture_url("macro_cancel_flow.html");
    // Step 1 clicks a heading, which changes nothing: `verdict: changed` cannot hold. Step 2
    // would fill the email field, and must never run.
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
        "--browser", browser.name(), "--json", "macro", "run", macro_file.name(),
    ]);
    assert_ne!(code, 0, "a stopped macro is a failure: {stdout}{stderr}");
    let report: Value = serde_json::from_str(&stdout).expect("JSON report on stdout");
    assert_eq!(report["ok"], false);
    assert_eq!(report["stopped_at"], 1, "the failing step is named: {report}");
    assert_eq!(report["guard"], "verdict");
    assert_eq!(report["expected"], "changed");
    assert_ne!(report["observed"], "changed", "{report}");
    assert!(report["next"].is_string(), "the action's own branch is carried: {report}");
    assert!(
        report["stop"].as_str().unwrap_or_default().contains("did not happen"),
        "the report says the rest did not run: {report}"
    );

    // And it really did not run: the field the third step would have filled is untouched.
    let (state, _, _) = run_cli(&[
        "--browser", browser.name(), "--json", "eval",
        "document.getElementById('email').value",
    ]);
    let state: Value = serde_json::from_str(&state).expect("JSON");
    assert_eq!(state["result"], "", "the step after the failure was not run: {state}");
}

/// A secret is declared and never stored, so a run without it stops before touching the page.
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
        "--browser", browser.name(), "--json", "macro", "run", macro_file.name(),
    ]);
    assert_ne!(code, 0, "{stdout}");
    let said = format!("{stdout}{stderr}");
    assert!(said.contains("card"), "the missing parameter is named: {said}");
    assert!(said.contains("never stored"), "and why it cannot be defaulted: {said}");
    assert!(said.contains("--var card="), "with the command that fixes it: {said}");

    // Nothing ran: the first step would have navigated, and the browser has no page from it.
    let (state, _, _) = run_cli(&["--browser", browser.name(), "--json", "eval", "location.href"]);
    let state: Value = serde_json::from_str(&state).unwrap_or_else(|_| json!({}));
    assert!(
        !state["result"].as_str().unwrap_or_default().contains("macro_cancel_flow"),
        "the macro navigated before finding out it could not finish: {state}"
    );
}

/// A card number is a secret by `element::SECRET_FIELD`, so recording a session that filled one
/// declares a parameter and writes no value.
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
    assert_eq!(responses[2]["value"]["redacted"], true, "the fill knew it was a secret: {}", responses[2]);

    let (stdout, stderr, code) = run_cli(&[
        "--json", "macro", "record", macro_file.name(),
        "--from-recording", &record.to_string_lossy(),
    ]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let text = std::fs::read_to_string(macro_file.path()).expect("the macro file");
    assert!(!text.contains("4111"), "the card number reached the file:\n{text}");
    assert!(text.contains("{{card}}"), "the value became a parameter:\n{text}");
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["params"]["card"]["secret"], true, "{text}");
    // And the guard is still the read-back: a secret is verified, never printed.
    assert_eq!(parsed["steps"][2]["expect"]["verbatim"], true, "{text}");
}

/// The design's second preference, which only exists because the first end-to-end recording
/// showed it never firing: a step aimed by uid reports no role and no name, so without the
/// snapshot it was refused and only selector-aimed steps were recordable.
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

    // Navigate and look, exactly as an agent does before it can name a uid at all.
    let first = run_pipe(
        browser.name(),
        &[with_record(json!({"cmd": "goto", "url": url})), with_record(json!({"cmd": "inspect"}))],
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
        &[with_record(json!({"cmd": "inspect"})), with_record(json!({"cmd": "click", "uid": uid}))],
    );
    assert_eq!(clicked[1]["ok"], true, "{}", clicked[1]);
    assert!(clicked[1]["role"].is_null(), "the uid path reports no role — that is the premise");

    let (stdout, stderr, code) = run_cli(&[
        "--json", "macro", "record", macro_file.name(),
        "--from-recording", &record.to_string_lossy(),
    ]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let report: Value = serde_json::from_str(&stdout).expect("JSON");
    assert!(report["refused"].as_array().unwrap().is_empty(), "not refused any more: {report}");

    let text = std::fs::read_to_string(macro_file.path()).expect("the macro");
    assert!(!text.contains("\"uid\""), "no uid survives into the file:\n{text}");
    let parsed: Value = serde_json::from_str(&text).unwrap();
    let last = parsed["steps"].as_array().unwrap().last().unwrap();
    assert_eq!(last["do"]["role"], "button", "{text}");
    assert_eq!(last["do"]["name"], "Manage billing", "{text}");

    // And it resolves again on a fresh page, which is the half a locator is for.
    let runner = TestBrowser::new("macro-uid-run");
    let (stdout, stderr, code) =
        run_cli(&["--browser", runner.name(), "--json", "macro", "run", macro_file.name()]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let run: Value = serde_json::from_str(&stdout).expect("JSON");
    assert_eq!(run["ok"], true, "{run}");
    let (state, _, _) = run_cli(&[
        "--browser", runner.name(), "--json", "eval",
        "document.getElementById('billing-panel').className",
    ]);
    let state: Value = serde_json::from_str(&state).expect("JSON");
    assert_eq!(state["result"], "on", "the click really landed by role+name: {state}");
}
