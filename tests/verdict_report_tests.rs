//! Every mutating action says what it observed, and no two "I don't know"s look alike.
//!
//! The classifier itself is unit-tested in `src/verdict.rs`. What is pinned here is that
//! each case is actually reachable end to end, in both modes, with the same spelling.

use std::io::Write as _;
use std::process::{Command, Stdio};

use serde_json::Value;

mod common;
use common::TestBrowser;

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

/// Open the fixture and establish a baseline, so the next action is a comparison.
fn open_with_baseline(browser: &str) -> bool {
    if !common::browser_ready() {
        return false;
    }
    let url = common::fixture_url("verdict_states.html");
    let (_, code) = run_cli(&["--browser", browser, "goto", &url]);
    if code != 0 {
        return common::unavailable("goto verdict_states.html failed");
    }
    let (_, code) = run_cli(&["--browser", browser, "inspect"]);
    assert_eq!(code, 0, "inspect should establish the baseline");
    true
}

fn act(browser: &str, args: &[&str]) -> Value {
    let mut full: Vec<&str> = vec!["--browser", browser, "--json"];
    full.extend_from_slice(args);
    let (stdout, code) = run_cli(&full);
    assert_eq!(code, 0, "action should succeed: {stdout}");
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}"))
}

#[test]
fn an_action_that_moves_the_page_says_changed() {
    let b = TestBrowser::new("verdict-changed");
    if !open_with_baseline(b.name()) {
        return;
    }
    let v = act(b.name(), &["click", "--selector", "#add"]);
    assert_eq!(v["verdict"], "changed", "{v}");
    assert_eq!(v["verdict_reason"], "tree_delta", "{v}");
}

/// The case the report used to answer with silence, indistinguishable from three failures.
#[test]
fn an_action_that_moves_nothing_says_so_instead_of_going_quiet() {
    let b = TestBrowser::new("verdict-nodelta");
    if !open_with_baseline(b.name()) {
        return;
    }
    // The first press moves focus to the document; the second changes nothing at all.
    let first = act(b.name(), &["press", "ArrowDown"]);
    assert_eq!(first["verdict"], "changed", "focus moving is still the page reacting: {first}");
    assert_eq!(first["verdict_reason"], "focus_only", "{first}");
    let v = act(b.name(), &["press", "ArrowDown"]);
    assert_eq!(v["verdict"], "unchanged", "{v}");
    assert_eq!(v["verdict_reason"], "identical_tree", "{v}");
    assert!(
        v["verdict_hint"].as_str().unwrap_or_default().contains("overlay"),
        "an empty delta is not proof the action did nothing, and the hint must say so: {v}"
    );
}

/// `no_effect` is the spec's word for a much stronger claim (delivery proven, window quiet,
/// attribution clean). Nothing here measures that, so nothing may print it.
#[test]
fn an_empty_delta_never_claims_the_action_had_no_effect() {
    let b = TestBrowser::new("verdict-noeffect");
    if !open_with_baseline(b.name()) {
        return;
    }
    act(b.name(), &["press", "ArrowDown"]);
    let v = act(b.name(), &["press", "ArrowDown"]);
    assert_ne!(v["verdict"], "no_effect", "{v}");
    assert!(!v.to_string().contains("no_effect"), "{v}");
}

#[test]
fn an_action_that_replaces_the_document_says_navigated() {
    let b = TestBrowser::new("verdict-navigated");
    if !open_with_baseline(b.name()) {
        return;
    }
    let v = act(b.name(), &["click", "--selector", "#away"]);
    assert_eq!(v["verdict"], "navigated", "{v}");
    assert_eq!(v["verdict_reason"], "document_replaced", "{v}");
}

/// The first action of a session has nothing to compare against. That is not the same
/// statement as "the page did not move", and it no longer reads the same.
#[test]
fn the_first_action_of_a_session_says_it_had_no_baseline() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("verdict-nobaseline");
    let url = common::fixture_url("verdict_states.html");
    let (_, code) = run_cli(&["--browser", b.name(), "goto", &url]);
    if code != 0 {
        common::unavailable("goto verdict_states.html failed");
        return;
    }
    // No `inspect`: goto deliberately stores no baseline.
    let v = act(b.name(), &["click", "--selector", "#add"]);
    assert_eq!(v["verdict"], "unknown", "{v}");
    assert_eq!(v["verdict_reason"], "no_baseline", "{v}");
    assert!(
        v["verdict_hint"].as_str().unwrap_or_default().contains("inspect"),
        "an agent that cannot be told what happened must be told how to find out: {v}"
    );
}

/// Switching the report off is a decision, not an observation, and must not look like one.
#[test]
fn verdict_off_says_it_did_not_look() {
    let b = TestBrowser::new("verdict-off");
    if !open_with_baseline(b.name()) {
        return;
    }
    let v = act(b.name(), &["--verdict", "off", "click", "--selector", "#add"]);
    assert_eq!(v["verdict"], "not_checked", "{v}");
    assert_eq!(v["verdict_reason"], "reporting_disabled", "{v}");
    assert!(v["changed"].is_null(), "off still means no page read: {v}");
    assert!(v["delta"].is_null(), "{v}");
}

/// Two modes of the same tool disagreeing about what an action returns is the kind of
/// thing an agent discovers the hard way.
#[test]
fn pipe_spells_every_verdict_the_way_the_cli_does() {
    if !common::browser_ready() {
        return;
    }
    let url = common::fixture_url("verdict_states.html");
    let script = format!(
        "{}\n{}\n{}\n{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": url}),
        serde_json::json!({"cmd": "inspect"}),
        serde_json::json!({"cmd": "press", "key": "ArrowDown"}),
        serde_json::json!({"cmd": "press", "key": "ArrowDown"}),
        serde_json::json!({"cmd": "click", "selector": "#add"}),
    );
    let responses = run_pipe("verdict-pipe", &[], &script);

    let quiet = &responses[responses.len() - 2];
    assert_eq!(quiet["verdict"], "unchanged", "{quiet}");
    assert_eq!(quiet["verdict_reason"], "identical_tree", "{quiet}");

    let click = responses.last().expect("a click response");
    assert_eq!(click["verdict"], "changed", "{click}");
    assert_eq!(click["verdict_reason"], "tree_delta", "{click}");
}

#[test]
fn pipe_reports_the_missing_baseline_too() {
    if !common::browser_ready() {
        return;
    }
    let url = common::fixture_url("verdict_states.html");
    let script = format!(
        "{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": url}),
        serde_json::json!({"cmd": "click", "selector": "#add"}),
    );
    let responses = run_pipe("verdict-pipe-nobaseline", &[], &script);
    let click = responses.last().expect("a click response");
    assert_eq!(click["verdict"], "unknown", "{click}");
    assert_eq!(click["verdict_reason"], "no_baseline", "{click}");
}

#[test]
fn pipe_says_it_did_not_look_when_the_report_is_off() {
    if !common::browser_ready() {
        return;
    }
    let url = common::fixture_url("verdict_states.html");
    let script = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": url}),
        serde_json::json!({"cmd": "inspect"}),
        serde_json::json!({"cmd": "click", "selector": "#add"}),
    );
    let responses = run_pipe("verdict-pipe-off", &["--verdict", "off"], &script);
    let click = responses.last().expect("a click response");
    assert_eq!(click["verdict"], "not_checked", "{click}");
    assert_eq!(click["verdict_reason"], "reporting_disabled", "{click}");
    assert!(click["changed"].is_null(), "{click}");
}

/// Batch goes through `dispatch_single`, a third path to the same contract.
#[test]
fn batch_carries_the_verdict_as_well() {
    let b = TestBrowser::new("verdict-batch");
    if !open_with_baseline(b.name()) {
        return;
    }
    let script = serde_json::json!([
        {"cmd": "click", "selector": "#add"},
    ])
    .to_string();
    let mut child = Command::new(binary())
        .args(["--browser", b.name(), "batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn batch");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("batch output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"verdict\":\"changed\"") && stdout.contains("\"verdict_reason\":\"tree_delta\""),
        "batch must answer like the other two modes: {stdout}"
    );
}

/// The text mode reader is in the same position as the JSON one.
#[test]
fn the_text_output_carries_the_verdict_too() {
    let b = TestBrowser::new("verdict-text");
    if !open_with_baseline(b.name()) {
        return;
    }
    let _ = run_cli(&["--browser", b.name(), "press", "ArrowDown"]);
    let (stdout, code) = run_cli(&["--browser", b.name(), "press", "ArrowDown"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains("verdict: unchanged (identical_tree)"),
        "text mode must not be the quiet one: {stdout}"
    );
}

/// The sequence every demo and every quickstart uses, and the one where the artefact bites.
///
/// `goto` keeps `last_snapshot` while clearing `uid_map`, so the first action after a `goto`
/// that followed an `inspect` compares against the PREVIOUS page and the identity rung fires.
/// Measured before the fix: `delivery:"intercepted"` and `verdict:"navigated"` on the same
/// response, with the interception — the only thing the caller can act on — absent from the
/// verdict. A hit test is measured on this action's own target and does not need two trees to
/// be comparable, so it precedes the rung that says they are not.
#[test]
fn an_intercepted_click_says_so_on_the_first_action_after_a_goto() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("verdict-demo-sequence");
    // A snapshot of a DIFFERENT page, which is what makes the identity stale below.
    let (_, code) = run_cli(&["--browser", b.name(), "goto", &common::fixture_url("verdict_states.html")]);
    if code != 0 {
        common::unavailable("goto verdict_states.html failed");
        return;
    }
    let (_, code) = run_cli(&["--browser", b.name(), "inspect"]);
    assert_eq!(code, 0, "inspect should establish the baseline");
    let (_, code) = run_cli(&["--browser", b.name(), "goto", &common::fixture_url("click_overlay.html")]);
    assert_eq!(code, 0, "goto click_overlay.html");

    let v = act(b.name(), &["click", "--selector", "#target"]);
    assert_eq!(v["verdict"], "intercepted", "{v}");
    assert_eq!(v["verdict_reason"], "hit_test_receiver", "{v}");
    assert_eq!(v["intercepted_by"]["id"], "scrim", "and it names the receiver: {v}");
    // The navigation is not hidden, it is just not the verdict: the fields stay put.
    assert_eq!(
        v["changed"]["document_changed"], true,
        "the identity reading still rides on the response: {v}"
    );
}

/// The other half of that ordering. `target_hit` is not a verdict, it is the licence for
/// `no_effect` — a claim about a tree that stayed quiet — so on a replaced document the
/// navigation is still the answer.
#[test]
fn a_click_that_navigates_still_says_navigated() {
    let b = TestBrowser::new("verdict-nav-kept");
    if !open_with_baseline(b.name()) {
        return;
    }
    let v = act(b.name(), &["click", "--selector", "#away"]);
    assert_eq!(v["verdict"], "navigated", "{v}");
    assert_eq!(v["verdict_reason"], "document_replaced", "{v}");
}

/// Run a pipe script and return every JSON response.
/// The label names the browser; the name itself is unique and the session is closed
/// when this returns, panic included — see `common::TestBrowser`.
fn run_pipe(label: &str, extra: &[&str], script: &str) -> Vec<Value> {
    let guard = TestBrowser::new(label);
    let mut args: Vec<&str> = vec!["--browser", guard.name()];
    args.extend_from_slice(extra);
    args.push("pipe");
    let mut child = Command::new(binary())
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pipe");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("pipe output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let responses: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
        .collect();
    assert!(!responses.is_empty(), "pipe produced nothing");
    responses
}
