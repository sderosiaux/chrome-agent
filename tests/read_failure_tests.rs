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

struct TestBrowser(String);
impl TestBrowser {
    /// Unique per process. A fixed name means two concurrent runs of this suite drive the same
    /// browser: one navigates while the other clicks a uid from its own snapshot, and both fail
    /// with "Node with given id does not belong to the document". CLAUDE.md documents the
    /// hazard — `--browser <unique>` per agent — and the suites have to obey it too.
    fn new(label: &str) -> Self {
        Self(format!("{label}-{}", std::process::id()))
    }
    fn name(&self) -> &str {
        &self.0
    }
}
impl Drop for TestBrowser {
    fn drop(&mut self) {
        let _ = run_cli(&["--browser", &self.0, "close", "--purge"]);
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
    let b = TestBrowser::new("read-failure-cli");
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

/// The pair where the verdict and the next step have different subjects.
///
/// A fill's read-back happens inside the action, on the field, so it survives a failed page read:
/// the verdict is `changed / value_kept` and it is true. But `proceed` would mean carrying on
/// against a page nobody has seen, so `next` answers `inspect` instead — the one place `next`
/// deliberately diverges from what the verdict word implies. The blindness that `read_failed`
/// used to carry in the verdict is carried by `next` and the hint now that the Group A rung
/// outranks it.
#[test]
fn a_confirmed_write_on_a_page_that_could_not_be_read_says_inspect() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("read-failure-fill");
    let url = common::fixture_url("blocks_after_fill.html");
    let (_, code) = run_cli(&["--browser", b.name(), "goto", &url]);
    if code != 0 {
        common::unavailable("goto blocks_after_fill.html failed");
        return;
    }
    let (_, code) = run_cli(&["--browser", b.name(), "inspect"]);
    assert_eq!(code, 0, "baseline");
    let (stdout, code) = run_cli(&[
        "--browser", b.name(), "--timeout", "2", "--json", "fill", "--selector", "#slow",
        "ada@example.com",
    ]);
    let v: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}"));

    assert_eq!(code, 0, "the write landed: {v}");
    assert_eq!(v["value"]["verbatim"], true, "read back on the field itself: {v}");
    assert_eq!(v["verdict"], "changed", "so the verdict is not an admission of ignorance: {v}");
    assert_eq!(v["verdict_reason"], "value_kept", "{v}");
    assert!(v["changed"].is_null(), "and yet nothing was compared: {v}");
    assert_eq!(v["next"], "inspect", "carrying on while blind is the one refusal: {v}");
    let hint = v["verdict_hint"].as_str().unwrap_or_default();
    assert!(hint.contains("what else moved"), "the hint names what is unknown: {v}");
    assert!(hint.contains("inspect"), "and the command that resolves it: {v}");

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
    // Unique per process: a fixed name lets a second concurrent run of this suite drive the
    // same browser and clobber this one's page.
    let browser = format!("read-failure-pipe-{}", std::process::id());
    let mut child = Command::new(binary())
        .args(["--browser", &browser, "--timeout", "2", "pipe"])
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
    let _ = run_cli(&["--browser", &browser, "close", "--purge"]);
}

/// A failure in the action itself is still a failure — the policy is about the read only.
#[test]
fn an_action_that_did_not_happen_is_still_an_error() {
    let b = TestBrowser::new("read-failure-real");
    if !open_busy_page(b.name()) {
        return;
    }
    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "click", "--selector", "#missing"]);
    assert_ne!(code, 0, "{stdout}");
    assert!(stdout.contains("\"ok\":false"), "{stdout}");
}
