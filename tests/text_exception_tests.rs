//! A throwing page getter must not read as "this element has no text".
//!
//! The uid branch of `text` was the one `Runtime.callFunctionOn` site in the codebase
//! that never checked `exceptionDetails`: a JS exception during the read came back as
//! `""` with `ok:true` — indistinguishable from a genuinely empty element. Every other
//! call site (element.rs) refuses on exception.

use std::process::Command;

use serde_json::Value;

mod common;
use common::TestBrowser;

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

#[test]
fn a_throwing_text_read_is_an_error_not_an_empty_string() {
    let b = TestBrowser::new("text-exception");
    if !common::browser_ready() {
        return;
    }
    let url = common::fixture_url("smooth_scroll_click.html");
    let (_, code) = run_cli(&["--browser", b.name(), "goto", &url]);
    if code != 0 {
        common::unavailable("goto smooth_scroll_click.html failed");
        return;
    }

    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "inspect"]);
    assert_eq!(code, 0, "{stdout}");
    let v: Value = serde_json::from_str(&stdout).expect("inspect JSON");
    let snapshot = v["snapshot"].as_str().expect("snapshot text");
    let uid = snapshot
        .lines()
        .find_map(|l| {
            let l = l.trim();
            (l.contains("button") && l.contains("Click me"))
                .then(|| l.strip_prefix("uid=")?.split(' ').next())?
        })
        .expect("the target button's uid");

    // Make the element's text read throw, the way a hostile or broken page can.
    let sabotage = "Object.defineProperty(document.getElementById('target'), 'innerText', \
                    {get() { throw new Error('boom'); }}); 1";
    let (stdout, code) = run_cli(&["--browser", b.name(), "eval", sabotage]);
    assert_eq!(code, 0, "{stdout}");

    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "text", uid]);
    assert_ne!(
        code, 0,
        "a JS exception during the read must be an error, not an empty string: {stdout}"
    );
    assert!(
        stdout.contains("boom") || stdout.to_lowercase().contains("error"),
        "the error should surface what the page threw: {stdout}"
    );
}
