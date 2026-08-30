use std::process::Command;

mod common;
use common::TestBrowser;

fn run_cli(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(common::binary()).args(args).output().expect("Failed to run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

fn goto(browser: &str, fixture: &str) -> bool {
    let url = common::fixture_url(fixture);
    let (_, stderr, code) = run_cli(&["--browser", browser, "goto", &url]);
    if code != 0 {
        return common::unavailable(&format!("goto {fixture} failed: {stderr}"));
    }
    true
}

fn diff_json(browser: &str) -> Option<serde_json::Value> {
    let (stdout, _, _) = run_cli(&["--browser", browser, "--json", "diff"]);
    serde_json::from_str(&stdout).ok()
}

// ─── diff across a navigation ───
//
// backendNodeId counters overlap between documents, so matching uids line by line pairs
// unrelated elements across a navigation and reports them as changed.

#[test]
fn diff_reports_document_change_instead_of_pairing_unrelated_uids() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("diff-nav");
    if !goto(b.name(), "extract_cards.html") {
        return;
    }
    let (_, _, code) = run_cli(&["--browser", b.name(), "inspect"]);
    assert_eq!(code, 0, "inspect should succeed");

    if !goto(b.name(), "extract_hn_subtext.html") {
        return;
    }
    let json = diff_json(b.name()).expect("diff should emit JSON");

    assert_eq!(
        json["document_changed"], true,
        "diff after navigating to a different document must say so, got {json}"
    );
    assert_eq!(json["changed"], 0, "no element can be 'changed' across two different documents, got {json}");
}

#[test]
fn diff_on_the_same_document_still_reports_changes() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("diff-same");
    if !goto(b.name(), "extract_cards.html") {
        return;
    }
    let (_, _, code) = run_cli(&["--browser", b.name(), "inspect"]);
    assert_eq!(code, 0, "inspect should succeed");

    // Mutate the live document without navigating.
    let (_, _, code) = run_cli(&[
        "--browser",
        b.name(),
        "eval",
        "document.body.insertAdjacentHTML('beforeend', '<h2>Freshly added heading</h2>'); 1",
    ]);
    assert_eq!(code, 0, "eval should succeed");

    let json = diff_json(b.name()).expect("diff should emit JSON");
    assert_eq!(json["document_changed"], false, "same document, got {json}");
    assert!(
        json["added"].as_u64().unwrap_or(0) >= 1,
        "the injected heading should show up as added, got {json}"
    );
}
