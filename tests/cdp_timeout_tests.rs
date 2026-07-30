//! No in-page promise can wedge the tool forever.
//!
//! `CdpClient::call` awaited its response channel with no deadline, so an evaluation that
//! never resolved left the command hanging with no error, no output and no recovery — in
//! pipe mode, for the rest of the session. Chrome was fine; the caller simply never heard
//! back. The reachable instance was `inspect --limit`, whose scroll probe re-armed a 400ms
//! debounce on every mutation and therefore never fired on a page that mutates forever.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

mod common;

/// Generous enough that a real answer always beats it, short enough that a hang is caught.
const HANG_LIMIT: Duration = Duration::from_secs(45);

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

/// Run and fail the test rather than the suite if the command hangs.
fn run_bounded(args: &[&str]) -> (String, i32, Duration) {
    let started = Instant::now();
    let mut child = Command::new(binary())
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn chrome-agent");
    loop {
        if let Some(status) = child.try_wait().expect("poll chrome-agent") {
            let output = child.wait_with_output().expect("collect output");
            return (
                String::from_utf8_lossy(&output.stdout).to_string(),
                status.code().unwrap_or(-1),
                started.elapsed(),
            );
        }
        if started.elapsed() > HANG_LIMIT {
            let _ = child.kill();
            let _ = child.wait();
            panic!("command hung for more than {HANG_LIMIT:?}: {args:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
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

/// A promise that never resolves is the simplest form of the defect: `eval` awaits it, and
/// the response channel had no deadline behind it.
#[test]
fn a_promise_that_never_resolves_becomes_an_error_not_a_hang() {
    let b = TestBrowser::new("cdp-timeout-eval");
    if !open(b.name(), "verdict_states.html") {
        return;
    }
    let (stdout, code, elapsed) = run_bounded(&[
        "--browser", b.name(), "--timeout", "5", "--json", "eval", "new Promise(() => {})",
    ]);
    assert_ne!(code, 0, "a command that never got an answer is not a success: {stdout}");
    assert!(
        stdout.contains("timed out") || stdout.contains("timeout"),
        "the error must say what happened: {stdout}"
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "the deadline should be the one asked for, not a multiple: {elapsed:?}"
    );
}

/// The caller's own `--timeout` is the deadline, so a short one fails fast.
#[test]
fn the_deadline_is_the_one_the_caller_asked_for() {
    let b = TestBrowser::new("cdp-timeout-short");
    if !open(b.name(), "verdict_states.html") {
        return;
    }
    let (_, _, elapsed) = run_bounded(&[
        "--browser", b.name(), "--timeout", "3", "--json", "eval", "new Promise(() => {})",
    ]);
    assert!(
        elapsed >= Duration::from_secs(3),
        "it must actually wait the deadline: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(20),
        "and not much beyond it: {elapsed:?}"
    );
}

/// `inspect --limit` scrolls, then waits for mutations to stop. Its debounce re-armed on
/// every mutation with no ceiling, so a page that mutates forever never let it return.
#[test]
fn inspect_limit_returns_on_a_page_that_never_stops_mutating() {
    let b = TestBrowser::new("cdp-timeout-ticker");
    if !open(b.name(), "goto_ticker.html") {
        return;
    }
    // The limit has to exceed what the page holds, or the collector returns before it ever
    // reaches the scroll probe — which is why an earlier version of this test passed
    // against the unfixed code.
    let (stdout, code, _) = run_bounded(&["--browser", b.name(), "--timeout", "10", "--json", "inspect", "--limit", "500"]);
    assert_eq!(code, 0, "the page is alive, not broken: {stdout}");
    assert!(stdout.contains("snapshot"), "{stdout}");
}

/// An ordinary command must not pay for the deadline.
#[test]
fn a_normal_command_is_unaffected() {
    let b = TestBrowser::new("cdp-timeout-normal");
    if !open(b.name(), "verdict_states.html") {
        return;
    }
    let (stdout, code, elapsed) = run_bounded(&["--browser", b.name(), "--timeout", "5", "--json", "eval", "1 + 1"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains("\"result\":2"), "{stdout}");
    assert!(elapsed < Duration::from_secs(5), "no deadline was reached: {elapsed:?}");
}
