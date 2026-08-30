//! `select` reads the selection back through the same 60 ms observation window as fill/check.
//! A selection a controlled component reverts inside that window must be refused, not reported
//! as made.

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

/// The fixture snaps the selection back on the task queue.
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
