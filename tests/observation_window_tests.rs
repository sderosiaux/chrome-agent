//! Read-backs have one window, and they say what it was.
//!
//! Before this, `fill` read the value back synchronously (0ms), `check --selector` waited
//! 60ms, and `check <uid>` waited however long a CDP round trip happened to take. None of
//! the three said so, while CLAUDE.md and SKILL.md promised "the state is read back" with no
//! window stated — a promise that cannot be kept, since a page can revert at any time.
//!
//! What can be promised is a bounded observation, reported with its bound.

use std::process::Command;

use serde_json::Value;

mod common;

/// The window every read-back waits before looking. Must match `element::READ_BACK_MS`.
const WINDOW_MS: u64 = 60;

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

struct TestBrowser(&'static str);
impl TestBrowser {
    const fn name(&self) -> &str {
        self.0
    }
}
impl Drop for TestBrowser {
    fn drop(&mut self) {
        let _ = run_cli(&["--browser", self.0, "close", "--purge"]);
    }
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
    let (stdout, _) = run_cli(&["--browser", browser, "--json", "fill", "--selector", selector, value]);
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}"))
}

/// A revert one microtask after the write is invisible to a same-evaluation read, and the
/// old fill reported the requested value back as though the page had kept it.
#[test]
fn a_value_reverted_on_the_microtask_queue_is_not_reported_as_kept() {
    let b = TestBrowser("window-microtask");
    if !open(b.name(), "form_value_microtask_revert.html") {
        return;
    }
    let v = fill(b.name(), "#micro", "coupon-123");
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(
        v["value"]["actual"], "",
        "the page threw the value away before the read window closed: {v}"
    );
    assert_eq!(v["value"]["verbatim"], false, "so it was not kept verbatim: {v}");
}

/// Every read-back states the window it observed, because "the value is X" is only ever
/// true as of a moment.
#[test]
fn a_fill_reports_the_window_it_observed() {
    let b = TestBrowser("window-fill-declared");
    if !open(b.name(), "form_value_plain_input.html") {
        return;
    }
    let v = fill(b.name(), "#plain", "hello");
    assert_eq!(v["value"]["observed_after_ms"], WINDOW_MS, "{v}");
    assert_eq!(v["value"]["verbatim"], true, "a plain input keeps it: {v}");
}

/// A revert past the window must not be dressed up as an observation of persistence. The
/// tool cannot see it — what it can do is say when it looked.
#[test]
fn a_revert_past_the_window_is_still_bounded_by_a_stated_time() {
    let b = TestBrowser("window-late");
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

    // And the claim is indeed only about that moment.
    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "eval", "document.querySelector('#late').value"]);
    assert_eq!(code, 0, "{stdout}");
    let later: Value = serde_json::from_str(&stdout).expect("JSON eval");
    assert_eq!(later["result"], "", "the page did revert, later: {later}");
}

/// The three read-back paths used to disagree about how long to wait. They no longer do.
#[test]
fn check_reports_the_same_window_as_fill() {
    let b = TestBrowser("window-check");
    if !open(b.name(), "checkable_kinds.html") {
        return;
    }
    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "check", "--selector", "#native"]);
    assert_eq!(code, 0, "{stdout}");
    let v: Value = serde_json::from_str(&stdout).expect("JSON check response");
    assert_eq!(v["observed_after_ms"], WINDOW_MS, "{v}");
}

#[test]
fn check_by_uid_reports_the_window_too() {
    let b = TestBrowser("window-check-uid");
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
