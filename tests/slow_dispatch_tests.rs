//! What a pointer action spends its time on, and what it says about it.
//!
//! Measured on shop.app, reproducible: `click --xy` took 10 051 ms while `inspect` on the same
//! page took 51 ms and a click at an inert coordinate took 140 ms. Ten seconds per click, with
//! nothing on the response to say why, and `--timeout` (30 s) never fires because the wait is
//! under it. Two independent causes, both reproduced offline here:
//!
//! 1. **Ours.** `wait_for_stabilization` armed a ten-second wait for `Page.loadEventFired` on
//!    ANY `Page.frameNavigated` — including a SUBFRAME's, for which that load event never
//!    fires. Clicking a product tile appends a tracking iframe, so every such click paid the
//!    full ceiling. `click_spawns_subframe.html` is that shape with no network.
//!
//! 2. **Chrome's.** A page that is not the foreground tab answers `Input.dispatchMouseEvent`
//!    on a fixed 5.00 s timer (5007, 5004, 5023 ms measured) while `Runtime.evaluate` on the
//!    same connection answers in 0–1 ms. Opening a second page backgrounds the first, which is
//!    what the second test below does with the tool's own `--page`.
//!
//! The assertions are wall-clock, and the thresholds are deliberately wide: the failures being
//! guarded against are 10.1 s and 5.1 s, so a 3 s ceiling separates them from any plausible
//! slowness on a local file without turning into a flake on a loaded machine.

use std::process::Command;
use std::time::Instant;

use serde_json::Value;

mod common;
use common::TestBrowser;

/// Wide enough that only the bug can trip it: the two failures are 10.1 s and 5.1 s, and a
/// healthy local click measures 0.15 s.
const CEILING_SECS: f64 = 3.0;

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

/// The command, its response, and how long the whole invocation took.
fn timed(args: &[&str]) -> (Value, f64) {
    let started = Instant::now();
    let (stdout, code) = run_cli(args);
    let secs = started.elapsed().as_secs_f64();
    assert_eq!(code, 0, "{args:?} failed: {stdout}");
    (serde_json::from_str(&stdout).expect("JSON response"), secs)
}

/// The uid the current snapshot gives the node whose line contains every needle.
/// A live pipe session: one command in, one response out, on ONE connection — which is what
/// makes the reset above worth pinning.
struct PipeSession {
    child: std::process::Child,
    responses: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
}

impl PipeSession {
    fn start(browser: &str) -> Self {
        use std::io::BufRead as _;
        use std::process::Stdio;
        let mut child = Command::new(binary())
            .args(["--browser", browser, "pipe"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn pipe");
        let stdout = child.stdout.take().expect("pipe stdout");
        Self { child, responses: std::io::BufReader::new(stdout).lines() }
    }

    fn send(&mut self, cmd: &Value) -> Value {
        use std::io::Write as _;
        let stdin = self.child.stdin.as_mut().expect("pipe stdin");
        writeln!(stdin, "{cmd}").expect("write command");
        stdin.flush().expect("flush command");
        let line = self.responses.next().expect("a response per command").expect("readable");
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
    let text = snapshot["snapshot"].as_str().unwrap_or_default().to_string();
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

/// The ten seconds. The click is delivered — the page's own state says so — and everything
/// after it is a wait for an event that cannot arrive.
#[test]
fn a_click_that_only_navigates_a_subframe_does_not_wait_for_a_load() {
    let b = TestBrowser::new("slow-subframe");
    if !open(b.name(), "click_spawns_subframe.html") {
        return;
    }
    // Warm: the first invocation of a session pays for the connection, and this test is about
    // what the click waits for, not about process start.
    let _ = run_cli(&["--browser", b.name(), "--json", "eval", "1"]);

    let (response, secs) = timed(&[
        "--browser", b.name(), "--verdict", "off", "--json", "click", "--selector", "#go",
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
        "--browser", b.name(), "--json", "eval",
        "document.getElementById('state').textContent",
    ]);
    assert_eq!(state["result"], "tracked", "the handler ran: {state}");
    // And the main document did not move, which is why waiting for its load was hopeless.
    let (url, _) = timed(&["--browser", b.name(), "--json", "eval", "location.pathname"]);
    assert!(
        url["result"].as_str().unwrap_or_default().ends_with("click_spawns_subframe.html"),
        "the top frame never navigated: {url}"
    );
}

/// The other half of the same rule: a navigation the tool CAN wait for is still waited for,
/// and now says how long it took. Removing the subframe wait must not remove this one.
#[test]
fn a_click_that_navigates_the_page_still_waits_and_says_how_long() {
    let b = TestBrowser::new("slow-navigates");
    if !open(b.name(), "click_navigates_away.html") {
        return;
    }
    let _ = run_cli(&["--browser", b.name(), "--json", "eval", "1"]);

    let (response, secs) = timed(&[
        "--browser", b.name(), "--verdict", "off", "--json", "click", "--selector", "#leave",
    ]);
    assert!(secs < CEILING_SECS, "a local navigation took {secs:.2}s: {response}");
    assert!(
        response["waited_ms"].is_u64(),
        "the action waited for a load, and a wait a caller paid for is a wait it may read: \
         {response}"
    );

    let (url, _) = timed(&["--browser", b.name(), "--json", "eval", "location.pathname"]);
    assert!(
        url["result"].as_str().unwrap_or_default().ends_with("click_spawns_subframe.html"),
        "the click did navigate the top frame: {url}"
    );
}

/// One connection, several commands: the wait belongs to the action that paid it.
///
/// Pipe and batch reuse a single connection, so a number recorded on it and merely read would
/// still be on the next response — a click that waited for nothing reporting the previous
/// click's ten seconds. It is cleared where the action starts and taken where it is read; this
/// is what proves both halves.
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

    // This one navigates the top frame, so it waits and says so.
    let navigated = session.send(&serde_json::json!({
        "cmd": "click", "selector": "#leave", "verdict": "off"
    }));
    assert!(navigated["waited_ms"].is_u64(), "the navigation was waited for: {navigated}");

    // This one, on the page it landed on, spawns a subframe and waits for nothing.
    session.send(&serde_json::json!({"cmd": "inspect"}));
    let tracked = session.send(&serde_json::json!({"cmd": "click", "selector": "#go"}));
    assert!(
        tracked["waited_ms"].is_null(),
        "the previous command's wait is not this command's: {tracked}"
    );
}

/// The five seconds. A second page in the same browser backgrounds the first, and every
/// pointer event on it then answers on Chrome's fixed timer. The tool's own `--page` is how
/// an agent reaches that state without doing anything unusual.
#[test]
fn a_pointer_action_on_a_backgrounded_page_is_not_charged_five_seconds() {
    let b = TestBrowser::new("slow-background");
    if !open(b.name(), "click_overlay.html") {
        return;
    }
    // A second page. Creating it activates it, so the page under test goes to the background:
    // `document.visibilityState` on it reads "hidden" from here on.
    let second = common::fixture_url("click_navigates_away.html");
    let (out, code) = run_cli(&["--browser", b.name(), "--page", "second", "goto", &second]);
    assert_eq!(code, 0, "a second page could not be opened: {out}");

    let (hidden, _) = timed(&["--browser", b.name(), "--json", "eval", "document.visibilityState"]);
    assert_eq!(hidden["result"], "hidden", "the page under test must be backgrounded: {hidden}");

    // `hover` is one mouse event and nothing else, which is the cleanest instrument here: a
    // click sends two, and on a static page the second one rides on the hit test the first
    // paid for, so a click can hide the stall that a hover cannot. It is also what was measured
    // in the field — `hover n101` answered in 5050 ms.
    let uid = uid_for(b.name(), &["button"]);
    let (response, secs) = timed(&[
        "--browser", b.name(), "--verdict", "off", "--json", "hover", &uid,
    ]);
    assert!(
        secs < CEILING_SECS,
        "a hover on a backgrounded page took {secs:.2}s — the pointer path did not bring it \
         forward: {response}"
    );

    // A keyboard event was never affected, and must not have acquired the state change either:
    // this pins the restraint, not just the fix.
    let (_, keys) = timed(&["--browser", b.name(), "--verdict", "off", "--json", "press", "Escape"]);
    assert!(keys < CEILING_SECS, "press took {keys:.2}s");
}
