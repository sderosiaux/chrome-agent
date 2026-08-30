//! The console interceptor's in-page buffer is bounded in both dimensions.
//!
//! The `console[level]` wrapper capped the array at 200 entries; the `error` and
//! `unhandledrejection` listeners right below it pushed with no cap at all and no length clamp.
//! A page throwing in a loop grew the array without bound, and `console::run` then pulls the
//! whole thing back in one `JSON.stringify`.

use std::process::Command;

use serde_json::Value;

mod common;
use common::TestBrowser;

fn run_cli(args: &[&str]) -> (String, i32) {
    let output = Command::new(common::binary())
        .args(args)
        .output()
        .expect("run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

/// Load the flooding fixture. `None` when Chrome is unavailable.
fn flood(browser: &str) -> Option<()> {
    if !common::browser_ready() {
        return None;
    }
    let url = common::fixture_url("console_flood.html");
    let (out, code) = run_cli(&["--browser", browser, "goto", &url]);
    if code != 0 {
        common::unavailable(&format!("goto console_flood.html failed: {out}"));
        return None;
    }
    Some(())
}

/// Read a JS expression off the page as JSON.
fn eval(browser: &str, expression: &str) -> Value {
    let (out, code) = run_cli(&["--browser", browser, "--json", "eval", expression]);
    assert_eq!(code, 0, "eval failed: {out}");
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("not JSON ({e}): {out}"))
}

/// 400 uncaught errors against a 200-entry buffer. Before the fix the count was 402.
#[test]
fn uncaught_errors_do_not_grow_the_buffer_past_the_cap() {
    let b = TestBrowser::new("console-flood-count");
    if flood(b.name()).is_none() {
        return;
    }
    let v = eval(b.name(), "window.__chrome_agent_console.length");
    let count = v["result"]
        .as_u64()
        .or_else(|| v["value"].as_u64())
        .unwrap_or_else(|| panic!("the buffer length is not on the response: {v}"));
    assert!(
        count > 0,
        "nothing was captured, so this proves nothing: {v}"
    );
    assert!(
        count <= 200,
        "the error listener pushed past the cap: {count}"
    );
}

/// The cap bounds the COUNT and says nothing about one entry, which a stack trace or a data URL
/// blows through on its own.
#[test]
fn one_enormous_message_is_clamped_and_says_how_much_was_dropped() {
    let b = TestBrowser::new("console-flood-length");
    if flood(b.name()).is_none() {
        return;
    }
    let v = eval(
        b.name(),
        "Math.max(...window.__chrome_agent_console.map(e => e.message.length))",
    );
    let longest = v["result"]
        .as_u64()
        .or_else(|| v["value"].as_u64())
        .unwrap_or_else(|| panic!("the longest message is not on the response: {v}"));
    assert!(
        longest <= 2_100,
        "a 50,000-character message came back whole ({longest} chars)"
    );

    let (out, code) = run_cli(&["--browser", b.name(), "console", "--limit", "500"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("BIGSTART"),
        "the message is still reported, just cut: {out}"
    );
    assert!(
        !out.contains("BIGEND"),
        "the tail came through, so nothing was clamped"
    );
    assert!(
        out.contains("chars)"),
        "a clamped message must say how much it dropped: {out}"
    );
}

/// `JSON.stringify` throws on a circular object. The wrapper used to call it unguarded, so a
/// page logging one broke its own `console.log` — and the interceptor is what did that to it.
#[test]
fn a_circular_object_does_not_break_the_page_s_console() {
    let b = TestBrowser::new("console-flood-circular");
    if flood(b.name()).is_none() {
        return;
    }
    let (out, code) = run_cli(&["--browser", b.name(), "console", "--limit", "500"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("[object Object]") || out.contains("circular"),
        "the circular log was dropped instead of falling back to String(): {out}"
    );
}
