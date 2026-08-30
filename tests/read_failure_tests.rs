//! A failed post-action read does not turn a landed action into a failure, in either mode.
//!
//! The fixtures pin the main thread after the action returns, so CDP cannot answer the read
//! inside a short `--timeout`: the action is delivered and the observation of it is not.

use std::io::Write as _;
use std::process::{Command, Stdio};

use serde_json::Value;

mod common;
use common::TestBrowser;

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

/// Give the page back to the browser before `TestBrowser::drop` tries to close it.
///
/// The fixtures pin the main thread for a fixed six seconds, and this used to be a flat
/// `sleep(7s)` tuned to that number: too short on a slower runner (the close then races a
/// wedged page) and pure waste on a faster one. Polling until the page answers is also the
/// stronger claim — it proves the block ended, which a sleep only assumes.
fn wait_until_the_page_answers_again(browser: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let (_, code) = run_cli(&["--browser", browser, "--timeout", "2", "eval", "1"]);
        if code == 0 {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the page never answered again: the main thread is still pinned 15s after the \
             action, so the fixture is no longer doing what this suite reads it as doing"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
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

#[test]
fn a_click_that_landed_is_not_reported_as_failed_because_the_read_timed_out() {
    let b = TestBrowser::new("read-failure-cli");
    if !open_busy_page(b.name()) {
        return;
    }
    let (stdout, code) = run_cli(&[
        "--browser",
        b.name(),
        "--timeout",
        "2",
        "--json",
        "click",
        "--selector",
        "#block",
    ]);
    let v: Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}"));

    assert_eq!(code, 0, "the action succeeded: {v}");
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(
        v["verdict"], "unknown",
        "and the report says why it cannot say more: {v}"
    );
    assert_eq!(v["verdict_reason"], "read_failed", "{v}");
    assert!(v["changed"].is_null(), "nothing was compared: {v}");

    wait_until_the_page_answers_again(b.name());
}

/// A fill's read-back is on the field, so it survives a failed page read: `changed /
/// value_kept`. `next` still answers `inspect`, the one place it diverges from the verdict.
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
        "--browser",
        b.name(),
        "--timeout",
        "2",
        "--json",
        "fill",
        "--selector",
        "#slow",
        "ada@example.com",
    ]);
    let v: Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}"));

    assert_eq!(code, 0, "the write landed: {v}");
    assert_eq!(
        v["value"]["verbatim"], true,
        "read back on the field itself: {v}"
    );
    assert_eq!(
        v["verdict"], "changed",
        "so the verdict is not an admission of ignorance: {v}"
    );
    assert_eq!(v["verdict_reason"], "value_kept", "{v}");
    assert!(v["changed"].is_null(), "and yet nothing was compared: {v}");
    assert_eq!(
        v["next"], "inspect",
        "carrying on while blind is the one refusal: {v}"
    );
    let hint = v["verdict_hint"].as_str().unwrap_or_default();
    assert!(
        hint.contains("what else moved"),
        "the hint names what is unknown: {v}"
    );
    assert!(
        hint.contains("inspect"),
        "and the command that resolves it: {v}"
    );

    wait_until_the_page_answers_again(b.name());
}

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
    // Unique per process: a fixed name would let a concurrent run clobber this one's page.
    let guard = TestBrowser::new("read-failure-pipe");
    let browser = guard.name().to_string();
    let mut child = Command::new(common::binary())
        .args(["--browser", &browser, "--timeout", "2", "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pipe");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
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

    wait_until_the_page_answers_again(&browser);
}

/// The policy covers the read only.
#[test]
fn an_action_that_did_not_happen_is_still_an_error() {
    let b = TestBrowser::new("read-failure-real");
    if !open_busy_page(b.name()) {
        return;
    }
    let (stdout, code) = run_cli(&[
        "--browser",
        b.name(),
        "--json",
        "click",
        "--selector",
        "#missing",
    ]);
    assert_ne!(code, 0, "{stdout}");
    assert!(stdout.contains("\"ok\":false"), "{stdout}");
}
