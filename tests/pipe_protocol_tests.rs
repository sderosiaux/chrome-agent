//! The pipe protocol has a type, and the type is what answers.
//!
//! Every dispatcher used to hand-decode `serde_json::Value`, so an unknown key was ignored:
//! `{"cmd":"click","uidd":"n1"}` answered `click: provide "uid", "selector", or "xy"` — a
//! refusal naming a problem the caller did not have. `pipe_command::PipeCommand` is one
//! `deny_unknown_fields` struct per verb; these run it through a real session, because the
//! parse being right in a unit test says nothing about which code path the pipe reaches.
//!
//! `back` is here for the second reason: it and `forward` are now one `history_step`, and the
//! end of the stack is read from `Page.getNavigationHistory` before anything is dispatched.

mod common;

use std::io::{BufRead as _, Write as _};
use std::process::{Command, Stdio};
use std::time::Instant;

use serde_json::{Value, json};

use common::{TestBrowser, binary, browser_ready, fixture_url};

/// A live pipe session: one command in, one response out, on one connection.
struct PipeSession {
    child: std::process::Child,
    responses: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
}

impl PipeSession {
    fn start(browser: &str) -> Self {
        let mut child = Command::new(binary())
            .args(["--browser", browser, "pipe"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn pipe");
        let stdout = child.stdout.take().expect("pipe stdout");
        Self {
            child,
            responses: std::io::BufReader::new(stdout).lines(),
        }
    }

    fn send(&mut self, cmd: &Value) -> Value {
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

fn error_of(response: &Value) -> String {
    response["error"]
        .as_str()
        .unwrap_or_else(|| panic!("no error string: {response}"))
        .to_string()
}

/// Every spelling the hand-written match accepted is still accepted, and reaches the same verb.
///
/// The composite verbs are the ones with aliases, and each one's alias set was a `|` arm in a
/// match that a typed enum could silently have dropped.
#[test]
fn every_alias_the_dispatcher_accepted_still_runs() {
    if !browser_ready() {
        return;
    }
    let browser = TestBrowser::new("pipe-protocol-aliases");
    let mut session = PipeSession::start(browser.name());
    let page = fixture_url("upload_form.html");
    assert_eq!(
        session.send(&json!({"cmd": "goto", "url": page}))["ok"],
        json!(true)
    );

    // Reaches its dispatcher under all three spellings: an empty `pairs` fills nothing and
    // succeeds, which is enough to prove the arm was found.
    for spelling in ["fill_form", "fill-form", "fillform"] {
        let r = session.send(&json!({"cmd": spelling, "pairs": []}));
        assert_eq!(r["ok"], json!(true), "{spelling} → {r}");
        assert_eq!(r["message"], json!("Filled 0 fields"), "{spelling} → {r}");
    }
    // No native WebMCP on most Chrome installs and no polyfill on this fixture, so what proves
    // the arm was reached is that the page answered, not the protocol.
    for spelling in ["webmcp_list", "webmcp-list"] {
        let r = session.send(&json!({"cmd": spelling}));
        let reached = r["ok"] == json!(true) || error_of(&r).contains("modelContext");
        assert!(reached, "{spelling} → {r}");
    }
    // Readability rejects this fixture, which is fine: the refusal is the reader's and proves
    // the navigation ran under both spellings.
    for spelling in ["navigate_and_read", "navigate-and-read"] {
        let r = session.send(&json!({"cmd": spelling, "url": page}));
        let reached = r["ok"] == json!(true) || error_of(&r).contains("not an article");
        assert!(reached, "{spelling} → {r}");
    }
    for spelling in ["fill_and_submit", "fill-and-submit"] {
        // No fields and a submit that matches nothing: the refusal proves the arm was reached,
        // and it is the submit's, not the protocol's.
        let r = session.send(&json!({"cmd": spelling, "fields": [], "submit": "#no-such-button"}));
        assert_eq!(r["ok"], json!(false), "{spelling} → {r}");
        let error = error_of(&r);
        assert!(!error.contains("Unknown command"), "{spelling} → {error}");
        assert!(!error.contains("unknown field"), "{spelling} → {error}");
    }
    for spelling in ["webmcp_call", "webmcp-call"] {
        let r = session.send(&json!({"cmd": spelling, "name": "no-such-tool"}));
        let error = error_of(&r);
        assert!(!error.contains("Unknown command"), "{spelling} → {error}");
    }
}

/// A key the command does not take is an error naming the key. This is the whole point: it used
/// to be dropped on the way in, and the command then complained about something else.
#[test]
fn an_unknown_key_is_refused_by_name_and_the_session_survives_it() {
    if !browser_ready() {
        return;
    }
    let browser = TestBrowser::new("pipe-protocol-unknown-key");
    let mut session = PipeSession::start(browser.name());

    for (cmd, key) in [
        (json!({"cmd": "click", "uidd": "n1"}), "uidd"),
        (
            json!({"cmd": "goto", "url": "about:blank", "inspekt": true}),
            "inspekt",
        ),
        // A verb that takes nothing still refuses one.
        (json!({"cmd": "back", "delta": -1}), "delta"),
        // `desired` is `check`'s alone; `uncheck` already decided.
        (
            json!({"cmd": "uncheck", "uid": "n1", "desired": true}),
            "desired",
        ),
        // Nested: a pair inside `fill-form` is checked too.
        (
            json!({"cmd": "fill_form", "pairs": [{"uid": "n1", "valeur": "x"}]}),
            "valeur",
        ),
    ] {
        let r = session.send(&cmd);
        assert_eq!(r["ok"], json!(false), "{cmd} → {r}");
        let error = error_of(&r);
        assert!(
            error.contains(key),
            "{cmd} answered {error:?}, which never names {key:?}"
        );
        // A refusal claims nothing about the page.
        assert!(
            r.get("verdict").is_none() && r.get("changed").is_none(),
            "{r}"
        );
    }

    // The session outlived all of them.
    let page = fixture_url("assert_page.html");
    assert_eq!(
        session.send(&json!({"cmd": "goto", "url": page}))["ok"],
        json!(true)
    );
}

/// A required field that is absent names the command and the field, in the words it always used;
/// one that is present with the wrong type names both too, where serde alone names neither.
#[test]
fn a_missing_or_mistyped_field_names_the_command_and_the_field() {
    if !browser_ready() {
        return;
    }
    let browser = TestBrowser::new("pipe-protocol-fields");
    let mut session = PipeSession::start(browser.name());

    for (cmd, expected) in [
        (json!({"cmd": "goto"}), "goto: missing \"url\""),
        (
            json!({"cmd": "fill", "selector": "#a"}),
            "fill: missing \"value\"",
        ),
        (json!({"cmd": "press"}), "press: missing \"key\""),
        (json!({"cmd": "batch"}), "batch: missing \"commands\" array"),
    ] {
        let r = session.send(&cmd);
        assert_eq!(r["ok"], json!(false), "{cmd} → {r}");
        assert!(
            error_of(&r).contains(expected),
            "{cmd} answered {r}, wanted {expected:?}"
        );
    }

    let r = session.send(&json!({"cmd": "fill", "selector": "#a", "value": 42}));
    let error = error_of(&r);
    assert!(
        error.starts_with("fill: \"value\":"),
        "the field must be named: {error}"
    );
}

#[test]
fn invalid_policy_and_ambiguous_target_are_refused_without_dispatch() {
    if !browser_ready() {
        return;
    }
    let browser = TestBrowser::new("pipe-protocol-no-ambiguous-dispatch");
    let mut session = PipeSession::start(browser.name());
    assert_eq!(
        session.send(&json!({
            "cmd": "goto",
            "url": fixture_url("intercept_actionable_overlay.html")
        }))["ok"],
        true
    );

    let invalid = session.send(&json!({
        "cmd": "click",
        "selector": "#target",
        "on_intercept": "refuze"
    }));
    assert_eq!(invalid["ok"], false, "{invalid}");
    assert!(error_of(&invalid).contains("Unknown --on-intercept value"));
    let receiver = session.send(&json!({"cmd": "eval", "expression": "window.receiver || null"}));
    assert!(
        receiver["result"].is_null(),
        "the invalid command dispatched: {receiver}"
    );

    assert_eq!(
        session.send(&json!({
            "cmd": "goto",
            "url": fixture_url("checkable_kinds.html")
        }))["ok"],
        true
    );
    let ambiguous = session.send(&json!({
        "cmd": "click",
        "uid": "n999999",
        "selector": "#native"
    }));
    assert_eq!(ambiguous["ok"], false, "{ambiguous}");
    assert!(error_of(&ambiguous).contains("exactly one"));
    let checked = session.send(&json!({
        "cmd": "eval",
        "expression": "document.getElementById('native').checked"
    }));
    assert_eq!(
        checked["result"], false,
        "the ambiguous command acted: {checked}"
    );
}

#[test]
fn navigate_and_read_clears_uids_even_when_readability_refuses() {
    if !browser_ready() {
        return;
    }
    let browser = TestBrowser::new("pipe-navigate-read-clears-uids");
    let mut session = PipeSession::start(browser.name());
    assert_eq!(
        session.send(&json!({
            "cmd": "goto",
            "url": fixture_url("checkable_kinds.html")
        }))["ok"],
        true
    );
    let inspected = session.send(&json!({"cmd": "inspect"}));
    let stale_uid = inspected["snapshot"]
        .as_str()
        .and_then(|snapshot| {
            snapshot.lines().find_map(|line| {
                line.trim_start()
                    .strip_prefix("uid=")
                    .and_then(|rest| rest.split_whitespace().next())
            })
        })
        .unwrap_or_else(|| panic!("no uid in {inspected}"))
        .to_string();

    let navigation = session.send(&json!({
        "cmd": "navigate_and_read",
        "url": fixture_url("upload_form.html")
    }));
    assert_eq!(
        navigation["ok"], false,
        "fixture should not be an article: {navigation}"
    );
    let stale = session.send(&json!({"cmd": "click", "uid": stale_uid}));
    assert_eq!(stale["ok"], false, "{stale}");
    assert!(
        error_of(&stale).contains("not found. Run 'chrome-agent inspect'"),
        "the old document's uid map survived navigation: {stale}"
    );
}

/// `back` at the start of the history stack did not move, and says so.
///
/// It used to fire `history.back()` blind and wait five seconds for a `Page.loadEventFired` that
/// never comes, then answer `{"ok":true,"title":"New Tab"}` — byte-identical to a real back.
/// `forward` had the boundary guard; `back` did not. Both now read it from
/// `Page.getNavigationHistory` before dispatching anything.
#[test]
fn back_at_the_start_of_history_says_it_did_not_move_and_does_not_wait_for_it() {
    if !browser_ready() {
        return;
    }
    let browser = TestBrowser::new("pipe-protocol-back-edge");
    let mut session = PipeSession::start(browser.name());

    let started = Instant::now();
    let r = session.send(&json!({"cmd": "back"}));
    let elapsed = started.elapsed();

    assert_eq!(
        r["ok"],
        json!(true),
        "a stated non-event is not an error: {r}"
    );
    assert_eq!(r["message"], json!("Already at first history entry"), "{r}");
    assert_eq!(r["title"], json!(""), "{r}");
    assert!(
        r.get("url").is_none(),
        "nothing was navigated to, so no url: {r}"
    );
    assert!(
        elapsed.as_secs() < 3,
        "the boundary is read, not waited for: {:.2}s",
        elapsed.as_secs_f64()
    );
}

/// One `history_step` behind both verbs: each reports where it landed, as `goto` does.
#[test]
fn back_and_forward_report_the_url_they_landed_on() {
    if !browser_ready() {
        return;
    }
    let browser = TestBrowser::new("pipe-protocol-history-url");
    let mut session = PipeSession::start(browser.name());
    let first = fixture_url("assert_page.html");
    let second = fixture_url("upload_form.html");
    for url in [&first, &second] {
        assert_eq!(
            session.send(&json!({"cmd": "goto", "url": url}))["ok"],
            json!(true)
        );
    }

    let back = session.send(&json!({"cmd": "back"}));
    assert_eq!(back["ok"], json!(true), "{back}");
    assert_eq!(
        back["url"],
        json!(first),
        "back must name where it landed: {back}"
    );

    let forward = session.send(&json!({"cmd": "forward"}));
    assert_eq!(forward["ok"], json!(true), "{forward}");
    assert_eq!(forward["url"], json!(second), "{forward}");

    // And past the last entry it is the same stated non-event `back` gives at the other end.
    let past = session.send(&json!({"cmd": "forward"}));
    assert_eq!(
        past["message"],
        json!("Already at last history entry"),
        "{past}"
    );
    assert!(past.get("url").is_none(), "{past}");
}
