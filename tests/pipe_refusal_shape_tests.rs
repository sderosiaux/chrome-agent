//! The shape a pipe session answers a line it cannot run with.
//!
//! Pipe mode has no exit code: a caller branches on the JSON of every line, including the ones
//! that never reached the page. Three things were unpinned end to end — the wording of each
//! refusal, that a refusal is one line and not zero or two, and that the session survives it.
//! A pipe that dies on a malformed line loses every command queued behind it, and a caller
//! that reads one answer per command would then pair every later response with the wrong
//! request.

mod common;

use std::io::Write as _;
use std::process::{Command, Stdio};

use common::{TestBrowser, binary, browser_ready, fixture_url};

/// Feed `lines` to one pipe session, return its stdout lines parsed as JSON.
///
/// # Panics
/// When a line of stdout is not JSON: the mode's whole contract is one JSON object per line.
fn run_pipe(browser: &str, lines: &[&str]) -> Vec<serde_json::Value> {
    let mut child = Command::new(binary())
        .args(["--browser", browser, "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pipe");
    let input = format!("{}\n", lines.join("\n"));
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("collect pipe output");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON: {l} ({e})")))
        .collect()
}

/// Every line the dispatcher refuses, with the exact text a caller reads.
const REFUSALS: &[(&str, &str)] = &[
    ("this is not json", "Invalid JSON:"),
    ("{\"cmd\":\"frobnicate\"}", "Unknown command: frobnicate"),
    ("{}", "Missing \"cmd\" field"),
    ("{\"cmd\":\"goto\"}", "goto: missing \"url\""),
    (
        "{\"cmd\":\"fill\",\"selector\":\"#a\"}",
        "fill: missing \"value\"",
    ),
    ("{\"cmd\":\"eval\"}", "eval: missing \"expression\""),
    ("{\"cmd\":\"type\"}", "type: missing \"text\""),
    ("{\"cmd\":\"press\"}", "press: missing \"key\""),
    ("{\"cmd\":\"scroll\"}", "scroll: missing \"target\""),
    (
        "{\"cmd\":\"wait\",\"what\":\"text\"}",
        "wait: missing \"pattern\"",
    ),
    ("{\"cmd\":\"frame\"}", "frame: missing \"target\""),
    ("{\"cmd\":\"batch\"}", "batch: missing \"commands\" array"),
];

/// One answer per line, each naming the command and the field, and the session still runs after
/// all of them.
#[test]
fn every_line_a_pipe_cannot_run_is_answered_and_none_of_them_ends_the_session() {
    if !browser_ready() {
        return;
    }
    let browser = TestBrowser::new("pipe-refusal-shape");
    let page = fixture_url("assert_page.html");
    let good = format!("{{\"cmd\":\"goto\",\"url\":\"{page}\"}}");

    let mut lines: Vec<&str> = REFUSALS.iter().map(|(line, _)| *line).collect();
    lines.push(&good);

    let responses = run_pipe(browser.name(), &lines);

    assert_eq!(
        responses.len(),
        lines.len(),
        "one response per line, so a caller can pair them: {responses:#?}"
    );
    for (i, (line, expected)) in REFUSALS.iter().enumerate() {
        let r = &responses[i];
        assert_eq!(r["ok"], serde_json::json!(false), "{line} → {r}");
        let error = r["error"]
            .as_str()
            .unwrap_or_else(|| panic!("no error string: {r}"));
        assert!(
            error.contains(expected),
            "{line} answered {error:?}, wanted {expected:?}"
        );
    }
    let last = responses.last().expect("the good command's answer");
    assert_eq!(
        last["ok"],
        serde_json::json!(true),
        "the session must outlive every refusal above: {last}"
    );
    assert_eq!(last["url"], serde_json::json!(page));
}

/// A refusal claims nothing about the page. `ok:false` plus an error, never a verdict, never a
/// change report — those words are what a caller reads as "the action ran".
#[test]
fn a_refused_line_never_carries_a_verdict_or_a_change() {
    if !browser_ready() {
        return;
    }
    let browser = TestBrowser::new("pipe-refusal-silent");
    let responses = run_pipe(
        browser.name(),
        &[
            "{\"cmd\":\"frobnicate\"}",
            "{\"cmd\":\"press\"}",
            "not json",
        ],
    );
    assert_eq!(responses.len(), 3);
    for r in &responses {
        assert_eq!(r["ok"], serde_json::json!(false), "{r}");
        for claim in ["verdict", "verdict_reason", "changed", "next", "message"] {
            assert!(r.get(claim).is_none(), "a refusal claimed {claim}: {r}");
        }
    }
}

/// `network-idle` is the one condition that needs no pattern; the refusal above proves the
/// others do. Pinned together so a change to one is measured against the other.
#[test]
fn network_idle_is_the_one_wait_that_needs_no_pattern() {
    if !browser_ready() {
        return;
    }
    let browser = TestBrowser::new("pipe-wait-pattern");
    let page = fixture_url("assert_page.html");
    let goto = format!(r#"{{"cmd":"goto","url":"{page}"}}"#);
    let responses = run_pipe(
        browser.name(),
        &[
            &goto,
            "{\"cmd\":\"wait\",\"what\":\"network-idle\"}",
            "{\"cmd\":\"wait\",\"what\":\"selector\"}",
        ],
    );
    assert_eq!(responses.len(), 3);
    assert_eq!(
        responses[0]["ok"],
        serde_json::json!(true),
        "{}",
        responses[0]
    );
    assert_eq!(responses[0]["url"], serde_json::json!(page));
    assert_eq!(
        responses[1]["ok"],
        serde_json::json!(true),
        "{}",
        responses[1]
    );
    assert_eq!(
        responses[2]["ok"],
        serde_json::json!(false),
        "{}",
        responses[2]
    );
    assert!(
        responses[2]["error"]
            .as_str()
            .unwrap_or_default()
            .contains("missing \"pattern\""),
        "{}",
        responses[2]
    );
}
