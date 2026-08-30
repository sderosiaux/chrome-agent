//! One pipe connection reads the tree of every document it navigates to.
//!
//! Written for `CdpClient::ensure_accessibility`, which sends `Accessibility.enable` once per
//! connection instead of once per snapshot (a snapshot is taken after every mutating action under
//! the default `--verdict auto`). It does NOT discriminate that change: measured by neutering
//! `ensure_accessibility` to a no-op and rebuilding, `Accessibility.getFullAXTree` answers with
//! no enable at all on Chrome 152, so the round trip the cache removes is not observable from
//! outside the process. What is left is still a contract worth holding: anything cached on the
//! connection — the enable today, a tree or a redaction tomorrow — must not survive a navigation
//! and answer for the wrong document.

use std::io::Write;
use std::process::Command;

use serde_json::{Value, json};

mod common;
use common::TestBrowser;

fn run_pipe(browser: &str, commands: &[Value]) -> Vec<Value> {
    let mut child = Command::new(common::binary())
        .args(["--browser", browser])
        .arg("pipe")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn chrome-agent pipe");
    {
        let stdin = child.stdin.as_mut().expect("pipe stdin");
        for cmd in commands {
            writeln!(stdin, "{cmd}").expect("write pipe command");
        }
    }
    let output = child.wait_with_output().expect("pipe run");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn tree(response: &Value) -> String {
    response
        .get("snapshot")
        .or_else(|| response.get("tree"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Three documents on one connection, the third back at the first: each reading is of the
/// document that was loaded. A per-connection cache that outlived its validity shows up here as
/// an empty or a stale reading.
#[test]
fn each_reading_on_one_connection_is_of_the_document_that_was_loaded() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("accessibility-enable-cache");
    let first = common::fixture_url("assert_page.html");
    let second = common::fixture_url("drag_list.html");

    let responses = run_pipe(
        browser.name(),
        &[
            json!({"cmd": "goto", "url": first}),
            json!({"cmd": "inspect"}),
            json!({"cmd": "goto", "url": second}),
            json!({"cmd": "inspect"}),
            json!({"cmd": "goto", "url": first}),
            json!({"cmd": "inspect"}),
        ],
    );
    assert_eq!(
        responses.len(),
        6,
        "one response per command: {responses:?}"
    );
    for (index, response) in responses.iter().enumerate() {
        assert_eq!(response["ok"], true, "command {index} failed: {response}");
    }

    let readings = [
        tree(&responses[1]),
        tree(&responses[3]),
        tree(&responses[5]),
    ];
    for (index, reading) in readings.iter().enumerate() {
        assert!(
            reading.contains("uid=n"),
            "reading {index} carries no row, so the tree was not read: {reading:?}"
        );
    }
    // Each reading is of the document that was loaded, not of the one the enable was sent on.
    assert!(readings[0].contains("Order 4815"), "{}", readings[0]);
    assert!(readings[1].contains("Drag fixture"), "{}", readings[1]);
    assert!(readings[2].contains("Order 4815"), "{}", readings[2]);
    assert!(
        !readings[1].contains("Order 4815"),
        "stale tree: {}",
        readings[1]
    );
}
