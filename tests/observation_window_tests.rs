//! Every read-back path waits the same 60 ms window and reports `observed_after_ms`.
//!
//! Persistence cannot be promised — a page can revert at any time — so what is asserted is a
//! bounded observation reported with its bound.

use std::process::Command;

use serde_json::Value;

mod common;
use common::TestBrowser;

/// The window every read-back waits before looking. Must match `element::READ_BACK_MS`.
const WINDOW_MS: u64 = 60;

fn run_cli(args: &[&str]) -> (String, i32) {
    let output = Command::new(common::binary())
        .args(args)
        .output()
        .expect("Failed to run chrome-agent");
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
    true
}

fn fill(browser: &str, selector: &str, value: &str) -> Value {
    let (stdout, _) = run_cli(&[
        "--browser",
        browser,
        "--json",
        "fill",
        "--selector",
        selector,
        value,
    ]);
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}"))
}

/// A revert one microtask after the write is invisible to a same-evaluation read.
#[test]
fn a_value_reverted_on_the_microtask_queue_is_not_reported_as_kept() {
    let b = TestBrowser::new("window-microtask");
    if !open(b.name(), "form_value_microtask_revert.html") {
        return;
    }
    let v = fill(b.name(), "#micro", "coupon-123");
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(
        v["value"]["actual"], "",
        "the page threw the value away before the read window closed: {v}"
    );
    assert_eq!(
        v["value"]["verbatim"], false,
        "so it was not kept verbatim: {v}"
    );
}

#[test]
fn a_fill_reports_the_window_it_observed() {
    let b = TestBrowser::new("window-fill-declared");
    if !open(b.name(), "form_value_plain_input.html") {
        return;
    }
    let v = fill(b.name(), "#plain", "hello");
    assert_eq!(v["value"]["observed_after_ms"], WINDOW_MS, "{v}");
    assert_eq!(v["value"]["verbatim"], true, "a plain input keeps it: {v}");
}

/// A revert past the window is invisible to the read-back, so the claim stays scoped to when
/// it was made.
#[test]
fn a_revert_past_the_window_is_still_bounded_by_a_stated_time() {
    let b = TestBrowser::new("window-late");
    if !open(b.name(), "form_value_late_revert.html") {
        return;
    }
    let v = fill(b.name(), "#late", "vanishes");
    // 400ms > 60ms: the read-back legitimately sees the value.
    assert_eq!(v["value"]["actual"], "vanishes", "{v}");
    assert_eq!(
        v["value"]["observed_after_ms"], WINDOW_MS,
        "the claim is scoped to when it was made, not to the future: {v}"
    );

    // Polled, not read once: the round trip out of `fill` and back into `eval` does not
    // reliably outlast the fixture's 400ms timer on a loaded machine.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let last;
    loop {
        let (stdout, code) = run_cli(&[
            "--browser",
            b.name(),
            "--json",
            "eval",
            "document.querySelector('#late').value",
        ]);
        assert_eq!(code, 0, "{stdout}");
        let current: Value = serde_json::from_str(&stdout).expect("JSON eval");
        if current["result"] == "" || std::time::Instant::now() >= deadline {
            last = current;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert_eq!(
        last["result"], "",
        "the fixture never reverted, so it cannot demonstrate a change past the window: {last}"
    );
}

#[test]
fn check_reports_the_same_window_as_fill() {
    let b = TestBrowser::new("window-check");
    if !open(b.name(), "checkable_kinds.html") {
        return;
    }
    let (stdout, code) = run_cli(&[
        "--browser",
        b.name(),
        "--json",
        "check",
        "--selector",
        "#native",
    ]);
    assert_eq!(code, 0, "{stdout}");
    let v: Value = serde_json::from_str(&stdout).expect("JSON check response");
    assert_eq!(v["observed_after_ms"], WINDOW_MS, "{v}");
}

#[test]
fn check_by_uid_reports_the_window_too() {
    let b = TestBrowser::new("window-check-uid");
    if !open(b.name(), "checkable_kinds.html") {
        return;
    }
    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "inspect"]);
    assert_eq!(code, 0, "{stdout}");
    let snapshot: Value = serde_json::from_str(&stdout).expect("JSON inspect");
    let text = snapshot["snapshot"].as_str().unwrap_or_default();
    let uid = text
        .lines()
        .find(|l| l.contains("checkbox"))
        .and_then(|l| l.trim_start().strip_prefix("uid="))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no checkbox uid in: {text}"))
        .to_string();

    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "check", &uid]);
    assert_eq!(code, 0, "{stdout}");
    let v: Value = serde_json::from_str(&stdout).expect("JSON check response");
    assert_eq!(
        v["observed_after_ms"], WINDOW_MS,
        "the uid path used to wait for however long a CDP round trip took: {v}"
    );
}
