//! `select` reads the selection back through the same observation window as fill/check.
//!
//! It used to set `selectedIndex`, dispatch `change`, and return the option text from the
//! same synchronous script — before a controlled component (React/MUI validation) had any
//! chance to revert it. "Selected \"Beta\"" on a select the page had already snapped back
//! to Alpha is a silent wrong answer: the agent submits the form believing a different
//! option is chosen than what the page holds.

use std::process::Command;

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

fn open(browser: &str) -> bool {
    if !common::browser_ready() {
        return false;
    }
    let url = common::fixture_url("select_controlled_revert.html");
    let (_, code) = run_cli(&["--browser", browser, "goto", &url]);
    if code != 0 {
        return common::unavailable("goto select_controlled_revert.html failed");
    }
    true
}

/// The page snaps the selection back on the task queue. Claiming "Selected" there is
/// the wrong answer; the command must refuse, like check does when a click is rejected.
#[test]
fn a_reverted_selection_is_not_reported_as_selected() {
    let b = TestBrowser::new("select-revert");
    if !open(b.name()) {
        return;
    }
    let (stdout, code) = run_cli(&[
        "--browser", b.name(), "--json", "select", "b", "--selector", "#controlled",
    ]);
    assert_ne!(
        code, 0,
        "the page reverted the selection; reporting success is a silent wrong answer: {stdout}"
    );
    assert!(
        stdout.contains("revert"),
        "the error must say the page reverted it: {stdout}"
    );
}

/// A selection that sticks reports the window it was observed through, like fill/check.
#[test]
fn a_kept_selection_reports_its_observation_window() {
    let b = TestBrowser::new("select-kept");
    if !open(b.name()) {
        return;
    }
    let (stdout, code) = run_cli(&[
        "--browser", b.name(), "--json", "select", "b", "--selector", "#plain",
    ]);
    assert_eq!(code, 0, "{stdout}");
    let v: Value = serde_json::from_str(&stdout).expect("JSON response");
    assert!(
        v["message"].as_str().unwrap_or_default().contains("Beta"),
        "the kept option text is the witness: {v}"
    );
    assert_eq!(
        v["observed_after_ms"], 60,
        "the read-back window must be stated, as fill and check do: {v}"
    );
}

/// The uid path makes the same promise as the selector path.
#[test]
fn the_uid_path_reads_back_too() {
    let b = TestBrowser::new("select-uid-revert");
    if !open(b.name()) {
        return;
    }
    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "inspect"]);
    assert_eq!(code, 0, "{stdout}");
    let v: Value = serde_json::from_str(&stdout).expect("inspect JSON");
    let snapshot = v["snapshot"].as_str().expect("snapshot text");
    // The controlled select renders first; take the first combobox uid.
    let uid = snapshot
        .lines()
        .find_map(|l| {
            let l = l.trim();
            l.contains("combobox").then(|| l.strip_prefix("uid=")?.split(' ').next())?
        })
        .expect("a combobox uid in the snapshot");

    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "select", "b", "--uid", uid]);
    assert_ne!(
        code, 0,
        "the uid path must also see the revert and refuse: {stdout}"
    );
    assert!(stdout.contains("revert"), "{stdout}");
}
