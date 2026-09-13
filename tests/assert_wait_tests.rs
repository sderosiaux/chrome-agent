//! Bounded observations through CLI, pipe, batch and recorded macro replay.

use std::io::Write as _;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

mod common;
#[path = "common/macros.rs"]
#[allow(dead_code)] // This suite uses the owned macro file and its bounded process runner.
mod macros;
use common::TestBrowser;
use macros::TestMacro;

struct Recording(std::path::PathBuf);
impl Drop for Recording {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn invoke(browser: &TestBrowser, args: &[&str], input: Option<&str>) -> Output {
    let mut child = Command::new(common::binary())
        .args(["--browser", browser.name(), "--verdict", "off", "--json"])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(input) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    } else {
        drop(child.stdin.take());
    }
    let started = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if started.elapsed() > Duration::from_secs(15) {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!("command exceeded its deadline: {args:?}, {output:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn parsed(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}: {output:?}"))
}

fn pipe(browser: &TestBrowser, commands: &[Value]) -> Vec<Value> {
    let input = commands
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let output = invoke(browser, &["pipe"], Some(&input));
    assert!(output.status.success(), "{output:?}");
    let responses: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), commands.len(), "{responses:?}");
    responses
}

fn goto() -> Value {
    json!({"cmd":"goto", "url":common::fixture_url("assert_wait.html")})
}

#[test]
fn every_assertion_kind_can_observe_a_delayed_result_without_another_click() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("assert-wait-kinds");
    for mut check in [
        json!({"cmd":"assert", "what":"value", "selector":"#quantity", "equals":"1"}),
        json!({"cmd":"assert", "what":"text", "selector":"#status", "matches":"^Saved$"}),
        json!({"cmd":"assert", "what":"state", "selector":"#export", "enabled":true}),
        json!({"cmd":"assert", "what":"state", "selector":"#checked", "checked":true}),
        json!({"cmd":"assert", "what":"exists", "selector":".row", "count":2}),
        json!({"cmd":"assert", "what":"url", "matches":"#saved$"}),
    ] {
        check["within"] = json!(3);
        let results = pipe(
            &browser,
            &[
                goto(),
                json!({"cmd":"click", "selector":"#add"}),
                check,
                json!({"cmd":"eval", "expression":"window.writes"}),
            ],
        );
        assert!(results.iter().all(|r| r["ok"] == true), "{results:?}");
        let wait = &results[2]["assertion"]["wait"];
        assert_eq!(wait["within_ms"], 3000);
        assert_eq!(wait["timed_out"], false);
        assert!(wait["observations"].as_u64().unwrap() > 1, "{results:?}");
        assert!(wait["elapsed_ms"].as_u64().unwrap() < 3000, "{results:?}");
        assert_eq!(results[3]["result"], 1, "one click per task: {results:?}");
    }
}

#[test]
fn cli_keeps_immediate_checks_and_reports_deadline_failure_with_the_last_value() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("assert-wait-cli");
    assert_eq!(pipe(&browser, &[goto()])[0]["ok"], true);
    let args = [
        "assert",
        "value",
        "--selector",
        "#quantity",
        "--equals",
        "1",
    ];
    let output = invoke(&browser, &args, None);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(parsed(&output)["assertion"].get("wait").is_none());

    let mut waiting = args.to_vec();
    waiting.extend(["--within", "1"]);
    let output = invoke(&browser, &waiting, None);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let result = parsed(&output);
    assert_eq!(result["assertion"]["actual"], "0");
    assert_eq!(result["assertion"]["expected"], "1");
    assert_eq!(result["assertion"]["wait"]["timed_out"], true);
    assert!(result["assertion"]["wait"]["elapsed_ms"].as_u64().unwrap() >= 1000);
    assert!(
        result["assertion"]["wait"]["observations"]
            .as_u64()
            .unwrap()
            > 1
    );

    // A true condition returns on its first observation, without paying the window.
    let output = invoke(
        &browser,
        &[
            "assert",
            "--within",
            "5",
            "value",
            "--selector",
            "#quantity",
            "--equals",
            "0",
        ],
        None,
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let result = parsed(&output);
    assert_eq!(result["assertion"]["wait"]["observations"], 1);
    assert!(result["assertion"]["wait"]["elapsed_ms"].as_u64().unwrap() < 1000);

    // A delayed true result also reaches the CLI's exit-0 path.
    assert!(
        invoke(&browser, &["eval", "begin(); true"], None)
            .status
            .success()
    );
    let output = invoke(
        &browser,
        &[
            "assert",
            "value",
            "--selector",
            "#quantity",
            "--equals",
            "1",
            "--within",
            "3",
        ],
        None,
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        parsed(&output)["assertion"]["wait"]["observations"]
            .as_u64()
            .unwrap()
            > 1
    );
}

#[test]
fn unreadable_targets_stay_errors_and_secret_observations_stay_redacted() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("assert-wait-errors");
    let results = pipe(
        &browser,
        &[
            goto(),
            json!({"cmd":"assert", "what":"value", "selector":"#missing", "equals":"1", "within":5}),
            json!({"cmd":"assert", "what":"value", "selector":"[", "equals":"1", "within":5}),
            json!({"cmd":"assert", "what":"value", "uid":"n999999", "equals":"1", "within":5}),
            json!({"cmd":"assert", "what":"value", "selector":"#password", "equals":"new-secret", "within":1}),
        ],
    );
    for result in &results[1..4] {
        assert!(result["error"].is_string(), "{result}");
        assert!(result.get("assertion").is_none(), "{result}");
        assert_eq!(result["wait"]["timed_out"], false);
        assert_eq!(result["wait"]["observations"], 0);
    }
    let secret = &results[4];
    assert_eq!(secret["assertion"]["held"], false);
    assert_eq!(secret["assertion"]["wait"]["timed_out"], true);
    assert!(!secret.to_string().contains("old-secret"), "{secret}");
    assert!(!secret.to_string().contains("new-secret"), "{secret}");
    let output = invoke(
        &browser,
        &[
            "assert",
            "state",
            "--selector",
            "#missing",
            "--enabled",
            "--within",
            "5",
        ],
        None,
    );
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(parsed(&output)["wait"]["timed_out"], false);
}

#[test]
fn a_blocked_read_is_bounded_and_preserves_earlier_evidence_without_claiming_false() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("assert-wait-blocked");
    let results = pipe(
        &browser,
        &[
            goto(),
            // Initial observations succeed. Then Chrome's main thread blocks. A finite
            // block lets the test inspect the same connection after its cancelled read returns.
            json!({"cmd":"eval", "expression":"window.blocks=0; setTimeout(() => { window.blocks++; const end=Date.now()+1800; while(Date.now()<end){} },450); true"}),
            json!({"cmd":"assert", "what":"value", "selector":"#quantity", "equals":"1", "within":1}),
            json!({"cmd":"eval", "expression":"({blocks:window.blocks,writes:window.writes})"}),
        ],
    );
    let failed = &results[2];
    assert_eq!(failed["ok"], false);
    assert!(
        failed["error"].as_str().unwrap().contains("deadline"),
        "{failed}"
    );
    assert!(failed.get("assertion").is_none(), "{failed}");
    assert_eq!(failed["wait"]["timed_out"], true);
    assert!(
        failed["wait"]["observations"].as_u64().unwrap() >= 1,
        "{failed}"
    );
    assert!(
        failed["wait"]["elapsed_ms"].as_u64().unwrap() < 1700,
        "{failed}"
    );
    assert_eq!(failed["last_observation"]["actual"], "0");
    assert_eq!(results[3]["result"]["writes"], 0);
    assert_eq!(
        results[3]["result"]["blocks"], 1,
        "the page blocked once: {results:?}"
    );
}

#[test]
fn batch_stops_after_expiry_and_macros_keep_the_authored_wait() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("assert-wait-modes");
    assert_eq!(pipe(&browser, &[goto()])[0]["ok"], true);
    let output = invoke(
        &browser,
        &["batch", "--stop-on-error"],
        Some(
            &json!([
                goto(),
                {"cmd":"assert", "what":"value", "selector":"#quantity", "equals":"1", "within":1},
                {"cmd":"click", "selector":"#add"}
            ])
            .to_string(),
        ),
    );
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(parsed(&output)["stopped_at"], 1);
    assert_eq!(
        pipe(
            &browser,
            &[json!({"cmd":"eval", "expression":"window.writes"})]
        )[0]["result"],
        0
    );

    let file = TestMacro::new("assert-wait-macro");
    let recording = Recording(common::temp_path("assert-wait-recording", "jsonl"));
    let mut commands = vec![
        goto(),
        json!({"cmd":"click", "selector":"#add"}),
        json!({"cmd":"assert", "what":"value", "selector":"#quantity", "equals":"1", "within":3}),
        json!({"cmd":"eval", "expression":"window.writes"}),
    ];
    for cmd in &mut commands {
        cmd["_record"] = json!(recording.0);
    }
    let results = pipe(&browser, &commands);
    assert!(results.iter().all(|r| r["ok"] == true), "{results:?}");
    let output = invoke(
        &browser,
        &[
            "macro",
            "record",
            file.name(),
            "--from",
            "0",
            "--from-recording",
            &recording.0.to_string_lossy(),
        ],
        None,
    );
    assert!(output.status.success(), "{output:?}");
    let recorded: Value = serde_json::from_slice(&std::fs::read(file.path()).unwrap()).unwrap();
    assert_eq!(recorded["steps"][2]["do"]["within"], 3, "{recorded}");
    let check = invoke(&browser, &["macro", "check", file.name()], None);
    assert!(check.status.success(), "{check:?}");
    let output = invoke(&browser, &["macro", "run", file.name()], None);
    assert!(output.status.success(), "{output:?}");
    let report = parsed(&output);
    assert_eq!(
        report["steps"][2]["result"]["assertion"]["wait"]["within_ms"], 3000,
        "{report}"
    );
    assert_eq!(report["steps"][3]["result"]["result"], 1, "{report}");

    for (check, code, stopped_by) in [
        (
            json!({"cmd":"assert", "what":"value", "selector":"#quantity", "equals":"2", "within":1}),
            2,
            "guard",
        ),
        (
            json!({"cmd":"assert", "what":"value", "selector":"#missing", "equals":"2", "within":1}),
            1,
            "error",
        ),
    ] {
        file.write(json!({"steps":[{"do":check}, {"do":{"cmd":"click", "selector":"#add"}}]}));
        let output = invoke(&browser, &["macro", "run", file.name()], None);
        assert_eq!(output.status.code(), Some(code), "{output:?}");
        assert_eq!(parsed(&output)["stopped_by"], stopped_by);
        assert_eq!(
            pipe(
                &browser,
                &[json!({"cmd":"eval", "expression":"window.writes"})]
            )[0]["result"],
            1
        );
    }
}
