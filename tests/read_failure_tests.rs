//! A failed post-action read does not turn a landed action into a failure.
//!
//! The CLI propagated the read error with `?`, so a click that had already been delivered
//! came back as `ok:false`. The natural response to that is to click again — the one
//! outcome an agent cannot recover from, since the second click is real. `pipe_dispatch`
//! stated the opposite policy in a comment and followed it; the two modes disagreed about
//! the same event.
//!
//! The fixture pins the main thread after the click returns, so CDP — which needs that
//! thread — cannot answer the read inside a short `--timeout`. The action is delivered and
//! the observation of it is not, on purpose.

use std::io::Write as _;
use std::process::{Command, Stdio};

use serde_json::Value;

mod common;

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

fn open_busy_page(browser: &str) -> bool {
    if !common::browser_ready() {
        return false;
    }
    let url = common::fixture_url("blocks_after_click.html");
    let (_, code) = run_cli(&["--browser", browser, "goto", &url]);
    if code != 0 {
        return common::unavailable("goto blocks_after_click.html failed");
    }
    let (_, code) = run_cli(&["--browser", browser, "inspect"]);
    assert_eq!(code, 0, "baseline");
    true
}

/// The click landed. Whether we managed to look afterwards is a different question, and
/// answering it with `ok:false` invites the agent to do the whole thing again.
#[test]
fn a_click_that_landed_is_not_reported_as_failed_because_the_read_timed_out() {
    let b = TestBrowser("read-failure-cli");
    if !open_busy_page(b.name()) {
        return;
    }
    let (stdout, code) = run_cli(&[
        "--browser", b.name(), "--timeout", "2", "--json", "click", "--selector", "#block",
    ]);
    let v: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}"));

    assert_eq!(code, 0, "the action succeeded: {v}");
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["verdict"], "unknown", "and the report says why it cannot say more: {v}");
    assert_eq!(v["verdict_reason"], "read_failed", "{v}");
    assert!(v["changed"].is_null(), "nothing was compared: {v}");

    // The page is still busy; give it back before the guard tries to close it.
    std::thread::sleep(std::time::Duration::from_secs(7));
}

/// Both modes describe the same event the same way.
#[test]
fn pipe_and_cli_agree_when_the_read_fails() {
    if !common::browser_ready() {
        return;
    }
    let url = common::fixture_url("blocks_after_click.html");
    let script = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": url}),
        serde_json::json!({"cmd": "inspect"}),
        serde_json::json!({"cmd": "click", "selector": "#block"}),
    );
    let mut child = Command::new(binary())
        .args(["--browser", "read-failure-pipe", "--timeout", "2", "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pipe");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("pipe output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last: Value = stdout
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("JSON"))
        .expect("a click response");

    assert_eq!(last["ok"], true, "{last}");
    assert_eq!(last["verdict"], "unknown", "{last}");
    assert_eq!(last["verdict_reason"], "read_failed", "{last}");

    std::thread::sleep(std::time::Duration::from_secs(7));
    let _ = run_cli(&["--browser", "read-failure-pipe", "close", "--purge"]);
}

/// A failure in the action itself is still a failure — the policy is about the read only.
#[test]
fn an_action_that_did_not_happen_is_still_an_error() {
    let b = TestBrowser("read-failure-real");
    if !open_busy_page(b.name()) {
        return;
    }
    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "click", "--selector", "#missing"]);
    assert_ne!(code, 0, "{stdout}");
    assert!(stdout.contains("\"ok\":false"), "{stdout}");
}
