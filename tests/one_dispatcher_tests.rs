//! One dispatcher behind pipe, pipe `batch` and CLI `batch`.
//!
//! `pipe::dispatch` and `pipe_dispatch::dispatch_single` were the same ~36-arm match written
//! twice, and they had drifted: only the pipe's copy carried a `"batch"` arm, so a `batch`
//! nested inside a `batch` answered `Unknown command: batch` while the identical object sent
//! straight to the pipe ran. The two tests here pin the two facts that survived the merge — the
//! nesting works and answers the same thing, and a command that FAILS after waiting no longer
//! hands its `waited_ms` to whatever runs next on the same connection.

use std::io::{BufRead as _, Write as _};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

mod common;
use common::TestBrowser;

/// A live pipe session: one command in, one response out, on one connection.
struct PipeSession {
    child: std::process::Child,
    responses: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
}

impl PipeSession {
    fn start(browser: &str) -> Self {
        let mut child = Command::new(common::binary())
            .args(["--browser", browser, "pipe"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn pipe");
        let stdout = child.stdout.take().expect("pipe stdout");
        Self { child, responses: std::io::BufReader::new(stdout).lines() }
    }

    fn send(&mut self, cmd: &Value) -> Value {
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

/// The CLI front end for the same loop: a JSON array on stdin.
fn run_cli_batch(browser: &str, commands_json: &str) -> Value {
    let mut child = Command::new(common::binary())
        .args(["--browser", browser, "--json", "batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn batch");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(commands_json.as_bytes())
        .expect("write batch input");
    let output = child.wait_with_output().expect("batch output");
    serde_json::from_slice(&output.stdout).expect("batch JSON")
}

#[test]
fn a_batch_nested_in_a_batch_answers_what_the_pipe_answers() {
    let b = TestBrowser::new("one-dispatcher-nested");
    if !common::browser_ready() {
        return;
    }
    let inner = json!({"cmd": "batch", "commands": [{"cmd": "eval", "expression": "6 * 7"}]});

    let mut session = PipeSession::start(b.name());
    session.send(&json!({"cmd": "goto", "url": common::fixture_url("extract_cards.html")}));

    // The same object, sent to the pipe and then wrapped in a batch of one.
    let direct = session.send(&inner);
    assert_eq!(direct["ok"], true, "a batch issued straight to the pipe runs: {direct}");
    assert_eq!(direct["results"][0]["result"], 42, "{direct}");

    let nested = session.send(&json!({"cmd": "batch", "commands": [inner.clone()]}));
    let inner_response = &nested["results"][0];
    assert_eq!(
        inner_response, &direct,
        "the same command must answer the same thing whichever dispatcher reached it; a second \
         copy of the match is how `batch` came to exist in one and not the other: {nested}"
    );

    // And through the CLI front end, which shares the loop but not the process.
    let cli = run_cli_batch(b.name(), &json!([inner]).to_string());
    assert_eq!(
        cli["results"][0]["results"][0]["result"], 42,
        "CLI batch runs the same nested batch: {cli}"
    );
}

/// `settle_wait` lives on the connection and is only ever TAKEN by `attach_verdict_for`, which
/// a failed command never reaches. Pipe reuses one connection, so the wait was still in the slot
/// when the next command settled its verdict and reported it as its own.
#[test]
fn a_failed_command_does_not_hand_its_wait_to_the_next_one() {
    let b = TestBrowser::new("one-dispatcher-wait");
    if !common::browser_ready() {
        return;
    }
    let mut session = PipeSession::start(b.name());
    let opened = session.send(&json!({
        "cmd": "goto", "url": common::fixture_url("checkbox_navigates_away.html")
    }));
    assert_eq!(opened["ok"], true, "{opened}");

    let snapshot = session.send(&json!({"cmd": "inspect"}));
    let uid = snapshot["snapshot"]
        .as_str()
        .unwrap_or_default()
        .lines()
        // On the ROLE, not on the line: the page's own title contains the word "checkbox", and
        // matching that returned the RootWebArea — a `check` that refuses before dispatching,
        // so the test passed against the unfixed binary.
        .find(|line| line.split_whitespace().nth(1) == Some("checkbox"))
        .and_then(|line| line.trim_start().strip_prefix("uid="))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no checkbox in:\n{snapshot}"))
        .to_string();

    // Dispatches, waits for the navigation its handler triggers, then fails its read-back
    // against a document that is gone.
    let failed = session.send(&json!({"cmd": "check", "uid": uid}));
    assert_eq!(
        failed["ok"], false,
        "the fixture exists to produce a failure AFTER the wait; without one this test proves \
         nothing: {failed}"
    );

    // `scroll` mutates the page (so it settles a verdict, where `waited_ms` is written) and
    // dispatches no input event (so it never calls `mark_dispatch`, which clears the slot on
    // the way in). It is the one command that can inherit another's wait.
    let after = session.send(&json!({"cmd": "scroll", "target": "down"}));
    assert!(
        after["waited_ms"].is_null(),
        "this scroll waited for nothing; the wait belongs to the check that paid it and died: \
         {after}"
    );
}
