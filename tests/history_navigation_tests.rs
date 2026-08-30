//! `back` and `forward` at the ends of the history stack.
//!
//! `forward` past the last entry is the only branch of either verb that says it did nothing,
//! and nothing pinned its wording or that all three modes share it. `mode_parity_tests` drives
//! `forward` only where it has somewhere to go.
//!
//! `back` carries no `url` in its response — unlike `goto` and `forward` — so the only way to
//! prove it moved is to read the page afterwards, which is what the second test does.

mod common;

use std::io::Write as _;
use std::process::{Command, Stdio};

use serde_json::Value;

use common::{TestBrowser, binary, browser_ready, fixture_url};

fn run(browser: &str, args: &[&str]) -> (String, String, i32) {
    let mut full = vec!["--browser", browser];
    full.extend_from_slice(args);
    let out = Command::new(binary())
        .args(&full)
        .output()
        .expect("run chrome-agent");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Feed lines to `pipe` (one command per line) or a JSON array to `batch`.
fn run_stdin(browser: &str, args: &[&str], input: &str) -> String {
    let mut full = vec!["--browser", browser];
    full.extend_from_slice(args);
    let mut child = Command::new(binary())
        .args(&full)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("output");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A forward with nowhere to go is a stated non-event, in the same words in every mode. It is
/// `ok:true` — nothing failed — so the message is the only thing telling a caller the page did
/// not move, and an empty `title` is not evidence of anything.
#[test]
fn forward_past_the_last_entry_says_so_in_all_three_modes() {
    if !browser_ready() {
        return;
    }
    let browser = TestBrowser::new("history-forward-end");
    let page = fixture_url("assert_page.html");
    let (_, stderr, code) = run(browser.name(), &["--json", "goto", &page]);
    assert_eq!(code, 0, "{stderr}");

    let (stdout, _, code) = run(browser.name(), &["--json", "forward"]);
    assert_eq!(code, 0, "a stated non-event is not an error: {stdout}");
    let cli: Value = serde_json::from_str(stdout.trim()).expect("CLI JSON");
    assert_eq!(cli["ok"], serde_json::json!(true), "{cli}");
    assert_eq!(
        cli["message"],
        serde_json::json!("Already at last history entry"),
        "{cli}"
    );
    assert_eq!(cli["title"], serde_json::json!(""), "{cli}");
    assert!(
        cli.get("url").is_none(),
        "no navigation happened, so no url: {cli}"
    );

    let piped = run_stdin(browser.name(), &["pipe"], "{\"cmd\":\"forward\"}\n");
    let pipe_value: Value =
        serde_json::from_str(piped.lines().last().unwrap_or_default()).expect("pipe JSON");
    assert_eq!(pipe_value, cli, "pipe must answer what the CLI answers");

    let batched = run_stdin(
        browser.name(),
        &["--json", "batch"],
        "[{\"cmd\":\"forward\"}]",
    );
    let batch_value: Value = serde_json::from_str(batched.trim()).expect("batch JSON");
    assert_eq!(
        batch_value["results"][0], cli,
        "batch must answer what the CLI answers"
    );
}

/// `back` moves the document even though its response does not name where it landed. Read the
/// page rather than the response, then `forward` back to where the pair started.
#[test]
fn back_lands_on_the_previous_document_and_forward_returns_to_the_later_one() {
    if !browser_ready() {
        return;
    }
    let browser = TestBrowser::new("history-round-trip");
    let first = fixture_url("assert_page.html");
    let second = fixture_url("upload_form.html");
    for url in [&first, &second] {
        let (_, stderr, code) = run(browser.name(), &["--json", "goto", url]);
        assert_eq!(code, 0, "{stderr}");
    }

    let (stdout, _, code) = run(browser.name(), &["--json", "back"]);
    assert_eq!(code, 0, "{stdout}");
    let back: Value = serde_json::from_str(stdout.trim()).expect("back JSON");
    assert_eq!(back["ok"], serde_json::json!(true), "{back}");

    // The response has no `url`, so the page is the evidence.
    let (stdout, _, code) = run(browser.name(), &["--json", "eval", "location.href"]);
    assert_eq!(code, 0, "{stdout}");
    let here: Value = serde_json::from_str(stdout.trim()).expect("eval JSON");
    assert_eq!(
        here["result"],
        serde_json::json!(first),
        "back did not land on the first page: {here}"
    );

    let (stdout, _, code) = run(browser.name(), &["--json", "forward"]);
    assert_eq!(code, 0, "{stdout}");
    let fwd: Value = serde_json::from_str(stdout.trim()).expect("forward JSON");
    assert_eq!(fwd["url"], serde_json::json!(second), "{fwd}");
}
