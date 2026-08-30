//! The second reading is not optional.
//!
//! `hit_test::aim` used to promote the FIRST probe to `Settle::Converged` whenever the point was
//! already inside the viewport, which made the settle loop's condition immediately false. The
//! viewport answers "can a pointer reach it", never "has it stopped": an element animating inside
//! the viewport was aimed at where it no longer was, and `classify` called that a `target_hit`.

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

fn open(browser: &str, fixture: &str) -> bool {
    if !common::browser_ready() {
        return false;
    }
    let url = common::fixture_url(fixture);
    let (_, code) = run_cli(&["--browser", browser, "goto", &url]);
    if code != 0 {
        return common::unavailable(&format!("goto {fixture} failed"));
    }
    let (_, code) = run_cli(&["--browser", browser, "--json", "inspect"]);
    if code != 0 {
        return common::unavailable(&format!("inspect {fixture} failed"));
    }
    true
}

fn eval(browser: &str, expression: &str) -> Value {
    let (stdout, code) = run_cli(&["--browser", browser, "--json", "eval", expression]);
    assert_eq!(code, 0, "eval failed: {stdout}");
    let v: Value = serde_json::from_str(&stdout).expect("JSON eval response");
    v["result"].clone()
}

fn click(browser: &str, args: &[&str]) -> Value {
    let mut argv = vec!["--browser", browser, "--json", "click"];
    argv.extend_from_slice(args);
    let (stdout, code) = run_cli(&argv);
    assert_eq!(code, 0, "click failed: {stdout}");
    serde_json::from_str(&stdout).expect("JSON click response")
}

/// The element never leaves the viewport, so the old code took one reading, called it settled and
/// dispatched at a coordinate the button had already left.
#[test]
fn a_target_moving_inside_the_viewport_is_refused_rather_than_aimed_at() {
    let b = TestBrowser::new("aim-settle-moving");
    if !open(b.name(), "moving_target_inside_viewport.html") {
        return;
    }
    let _ = eval(b.name(), "window.receiver = null; 1");
    let response = click(b.name(), &["--selector", "#runner"]);

    assert_eq!(
        response["delivery"], "not_settled",
        "a point still moving cannot be claimed, viewport or not: {response}"
    );
    assert_eq!(response["verdict_reason"], "scroll_not_settled", "{response}");
    assert_eq!(
        response["dispatched"],
        Value::Bool(false),
        "nothing may be sent at a point that was never confirmed: {response}"
    );
    assert_eq!(
        eval(b.name(), "window.receiver"),
        Value::Null,
        "the page must agree no event arrived"
    );
}

/// The control, on the same page and the same viewport: a still element still converges, so the
/// second reading costs a round trip and no behaviour.
#[test]
fn a_still_target_on_the_same_page_still_converges_and_lands() {
    let b = TestBrowser::new("aim-settle-still");
    if !open(b.name(), "moving_target_inside_viewport.html") {
        return;
    }
    let _ = eval(b.name(), "window.receiver = null; 1");
    let response = click(b.name(), &["--selector", "#anchor"]);

    assert_eq!(response["delivery"], "target_hit", "{response}");
    assert_eq!(
        eval(b.name(), "window.receiver"),
        Value::String("anchor".into()),
        "the click still lands: {response}"
    );
}
