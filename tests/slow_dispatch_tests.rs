//! What a pointer action spends its time on, and what it says about it.
//!
//! Two stalls reproduced offline: a subframe navigation arming a ten-second wait for
//! `Page.loadEventFired`, and Chrome answering `Input.dispatchMouseEvent` on a fixed 5 s timer
//! for a background tab. Assertions are wall-clock.

use std::process::Command;
use std::time::Instant;

use serde_json::Value;

mod common;
use common::TestBrowser;

/// The failures are 10.1 s and 5.1 s; a healthy local click measures 0.15 s.
const CEILING_SECS: f64 = 3.0;

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

/// The command, its response, and how long the whole invocation took.
fn timed(args: &[&str]) -> (Value, f64) {
    let started = Instant::now();
    let (stdout, code) = run_cli(args);
    let secs = started.elapsed().as_secs_f64();
    assert_eq!(code, 0, "{args:?} failed: {stdout}");
    (serde_json::from_str(&stdout).expect("JSON response"), secs)
}

/// A live pipe session: one command in, one response out, on one connection.
struct PipeSession {
    child: std::process::Child,
    responses: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
}

impl PipeSession {
    fn start(browser: &str) -> Self {
        use std::io::BufRead as _;
        use std::process::Stdio;
        let mut child = Command::new(common::binary())
            .args(["--browser", browser, "pipe"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn pipe");
        let stdout = child.stdout.take().expect("pipe stdout");
        Self {
            child,
            responses: std::io::BufReader::new(stdout).lines(),
        }
    }

    fn send(&mut self, cmd: &Value) -> Value {
        use std::io::Write as _;
        let stdin = self.child.stdin.as_mut().expect("pipe stdin");
        writeln!(stdin, "{cmd}").expect("write command");
        stdin.flush().expect("flush command");
        let line = self
            .responses
            .next()
            .expect("a response per command")
            .expect("readable");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad pipe line {line}: {e}"))
    }
}

impl Drop for PipeSession {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.wait();
    }
}

fn uid_for(browser: &str, needles: &[&str]) -> String {
    let (stdout, code) = run_cli(&["--browser", browser, "--json", "inspect"]);
    assert_eq!(code, 0, "{stdout}");
    let snapshot: Value = serde_json::from_str(&stdout).expect("JSON inspect");
    let text = snapshot["snapshot"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    text.lines()
        .find(|line| needles.iter().all(|n| line.contains(n)))
        .and_then(|line| line.trim_start().strip_prefix("uid="))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no uid matching {needles:?} in:\n{text}"))
        .to_string()
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

#[test]
fn a_click_that_only_navigates_a_subframe_does_not_wait_for_a_load() {
    let b = TestBrowser::new("slow-subframe");
    if !open(b.name(), "click_spawns_subframe.html") {
        return;
    }
    // Warm the session: the first invocation pays for the connection.
    let _ = run_cli(&["--browser", b.name(), "--json", "eval", "1"]);

    let (response, secs) = timed(&[
        "--browser",
        b.name(),
        "--verdict",
        "off",
        "--json",
        "click",
        "--selector",
        "#go",
    ]);
    assert!(
        secs < CEILING_SECS,
        "a click that spawns a tracking iframe waited {secs:.2}s for a load event that cannot \
         fire: {response}"
    );
    assert!(
        response["waited_ms"].is_null(),
        "nothing was waited for, so nothing may be reported: {response}"
    );

    // The click really was delivered — otherwise this test would pass by not clicking.
    let (state, _) = timed(&[
        "--browser",
        b.name(),
        "--json",
        "eval",
        "document.getElementById('state').textContent",
    ]);
    assert_eq!(state["result"], "tracked", "the handler ran: {state}");
    let (url, _) = timed(&["--browser", b.name(), "--json", "eval", "location.pathname"]);
    assert!(
        url["result"]
            .as_str()
            .unwrap_or_default()
            .ends_with("click_spawns_subframe.html"),
        "the top frame never navigated: {url}"
    );
}

#[test]
fn a_click_that_navigates_the_page_still_waits_and_says_how_long() {
    let b = TestBrowser::new("slow-navigates");
    if !open(b.name(), "click_navigates_away.html") {
        return;
    }
    let _ = run_cli(&["--browser", b.name(), "--json", "eval", "1"]);

    let (response, secs) = timed(&[
        "--browser",
        b.name(),
        "--verdict",
        "off",
        "--json",
        "click",
        "--selector",
        "#leave",
    ]);
    assert!(
        secs < CEILING_SECS,
        "a local navigation took {secs:.2}s: {response}"
    );
    assert!(
        response["waited_ms"].is_u64(),
        "the action waited for a load, and a wait a caller paid for is a wait it may read: \
         {response}"
    );

    let (url, _) = timed(&["--browser", b.name(), "--json", "eval", "location.pathname"]);
    assert!(
        url["result"]
            .as_str()
            .unwrap_or_default()
            .ends_with("click_spawns_subframe.html"),
        "the click did navigate the top frame: {url}"
    );
}

/// Pipe reuses one connection, so `waited_ms` is cleared per action: a click that waited for
/// nothing must not report the previous click's wait.
#[test]
fn a_wait_belongs_to_the_action_that_paid_it_and_to_no_other() {
    let b = TestBrowser::new("slow-pipe");
    if !common::browser_ready() {
        return;
    }
    let navigates = common::fixture_url("click_navigates_away.html");
    let mut session = PipeSession::start(b.name());
    session.send(&serde_json::json!({"cmd": "goto", "url": navigates}));
    session.send(&serde_json::json!({"cmd": "inspect"}));

    // No per-command `"verdict"` key: it was never read, and the protocol now says so by
    // refusing it. `--verdict` is a session flag, and this test does not need it off.
    let navigated = session.send(&serde_json::json!({"cmd": "click", "selector": "#leave"}));
    assert!(
        navigated["waited_ms"].is_u64(),
        "the navigation was waited for: {navigated}"
    );

    // On the page it landed on, this click spawns a subframe and waits for nothing.
    session.send(&serde_json::json!({"cmd": "inspect"}));
    let tracked = session.send(&serde_json::json!({"cmd": "click", "selector": "#go"}));
    assert!(
        tracked["waited_ms"].is_null(),
        "the previous command's wait is not this command's: {tracked}"
    );
}

/// A second page backgrounds the first, and Chrome then answers its pointer events on a 5 s
/// timer. `--page` is how an agent reaches that state without doing anything unusual.
#[test]
fn a_pointer_action_on_a_backgrounded_page_is_not_charged_five_seconds() {
    let b = TestBrowser::new("slow-background");
    if !open(b.name(), "click_overlay.html") {
        return;
    }
    // Creating a second page activates it, so the page under test goes to the background.
    let second = common::fixture_url("click_navigates_away.html");
    let (out, code) = run_cli(&["--browser", b.name(), "--page", "second", "goto", &second]);
    assert_eq!(code, 0, "a second page could not be opened: {out}");

    let (hidden, _) = timed(&[
        "--browser",
        b.name(),
        "--json",
        "eval",
        "document.visibilityState",
    ]);
    assert_eq!(
        hidden["result"], "hidden",
        "the page under test must be backgrounded: {hidden}"
    );

    // `hover` is one mouse event: a click sends two and the second rides on the first's hit
    // test, which can hide the stall.
    let uid = uid_for(b.name(), &["button"]);
    let (response, secs) = timed(&[
        "--browser",
        b.name(),
        "--verdict",
        "off",
        "--json",
        "hover",
        &uid,
    ]);
    assert!(
        secs < CEILING_SECS,
        "a hover on a backgrounded page took {secs:.2}s — the pointer path did not bring it \
         forward: {response}"
    );

    // A keyboard event was never affected and must not have acquired the foregrounding either.
    let (_, keys) = timed(&[
        "--browser",
        b.name(),
        "--verdict",
        "off",
        "--json",
        "press",
        "Escape",
    ]);
    assert!(keys < CEILING_SECS, "press took {keys:.2}s");
}
