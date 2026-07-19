//! E2E tests for the `frame` command: switching execution context into an
//! iframe must scope subsequent `eval` and `inspect` to that frame (issue #8).
//!
//! These drive a single `chrome-agent pipe` process (one persistent CDP
//! connection) so we exercise the real in-process frame-binding state, not
//! cross-process behavior.

use std::io::Write as _;
use std::process::{Command, Stdio};

use serde_json::Value;

/// Path to the built binary (sibling of the test binary).
fn binary() -> String {
    let mut path = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    path.push("chrome-agent");
    path.to_string_lossy().into_owned()
}

fn chrome_available() -> bool {
    let candidates = if cfg!(target_os = "macos") {
        vec!["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"]
    } else {
        vec!["google-chrome", "chromium"]
    };
    for candidate in candidates {
        if std::path::Path::new(candidate).exists() {
            return true;
        }
        if Command::new("which")
            .arg(candidate)
            .output()
            .is_ok_and(|o| o.status.success())
        {
            return true;
        }
    }
    false
}

/// `file://` URL for a fixture in `tests/fixtures/`.
fn fixture_url(name: &str) -> String {
    let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures");
    path.push(name);
    format!("file://{}", path.to_string_lossy())
}

/// Run `chrome-agent pipe` feeding the given JSON command lines on stdin,
/// return one parsed `Value` per output line (in order).
fn run_pipe(browser: &str, commands: &[Value]) -> Vec<Value> {
    let mut child = Command::new(binary())
        .args(["--browser", browser, "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn chrome-agent pipe");

    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for cmd in commands {
            writeln!(stdin, "{}", serde_json::to_string(cmd).unwrap()).unwrap();
        }
    }

    let output = child.wait_with_output().expect("wait pipe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).unwrap_or_else(|_| Value::String(l.to_string())))
        .collect()
}

/// Close the browser session (best-effort cleanup).
fn close(browser: &str) {
    let _ = Command::new(binary())
        .args(["--browser", browser, "close"])
        .output();
}

// ---------------------------------------------------------------------------
// Positive: frame switch binds subsequent commands to the iframe
// ---------------------------------------------------------------------------

#[test]
fn frame_switch_scopes_eval_location_to_iframe() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let browser = "test-frame-eval-loc";
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "frame", "target": "iframe"}),
            serde_json::json!({"cmd": "eval", "expression": "location.href"}),
        ],
    );
    close(browser);

    assert_eq!(responses.len(), 3, "expected 3 responses: {responses:?}");
    assert_eq!(responses[1]["ok"], Value::Bool(true), "frame switch: {:?}", responses[1]);

    let href = responses[2]["result"].as_str().unwrap_or_default();
    assert!(
        href.contains("frame_child.html"),
        "after frame switch, eval location.href must be the iframe URL, got: {href:?}"
    );
    assert!(
        !href.contains("frame_parent.html"),
        "eval location.href must NOT be the parent URL, got: {href:?}"
    );
}

#[test]
fn frame_switch_scopes_eval_dom_to_iframe() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let browser = "test-frame-eval-dom";
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "frame", "target": "iframe"}),
            serde_json::json!({"cmd": "eval", "expression": "document.querySelector('#child-marker').textContent"}),
        ],
    );
    close(browser);

    assert_eq!(responses.len(), 3, "responses: {responses:?}");
    let text = responses[2]["result"].as_str().unwrap_or_default();
    assert!(
        text.contains("child-only-marker-xyz"),
        "eval in iframe must see the iframe DOM, got: {:?}",
        responses[2]
    );
}

#[test]
fn frame_switch_scopes_inspect_to_iframe() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let browser = "test-frame-inspect";
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "frame", "target": "iframe"}),
            serde_json::json!({"cmd": "inspect"}),
        ],
    );
    close(browser);

    assert_eq!(responses.len(), 3, "responses: {responses:?}");
    let snap = responses[2]["snapshot"].as_str().unwrap_or_default();
    assert!(
        snap.contains("CHILD FRAME CONTENT"),
        "inspect after frame switch must show iframe content, got: {snap:?}"
    );
    assert!(
        !snap.contains("PARENT PAGE CONTENT"),
        "inspect after frame switch must NOT show parent content, got: {snap:?}"
    );
}

// ---------------------------------------------------------------------------
// Positive: switching back to main restores the top document
// ---------------------------------------------------------------------------

#[test]
fn frame_main_switches_back_to_top_document() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let browser = "test-frame-main-back";
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "frame", "target": "iframe"}),
            serde_json::json!({"cmd": "frame", "target": "main"}),
            serde_json::json!({"cmd": "eval", "expression": "location.href"}),
        ],
    );
    close(browser);

    assert_eq!(responses.len(), 4, "responses: {responses:?}");
    let href = responses[3]["result"].as_str().unwrap_or_default();
    assert!(
        href.contains("frame_parent.html"),
        "after 'frame main', eval location.href must be the parent URL again, got: {href:?}"
    );
}

// ---------------------------------------------------------------------------
// Positive: navigation resets frame binding (no stale context)
// ---------------------------------------------------------------------------

#[test]
fn navigation_resets_frame_binding() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let browser = "test-frame-nav-reset";
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "frame", "target": "iframe"}),
            serde_json::json!({"cmd": "goto", "url": fixture_url("frame_child.html")}),
            serde_json::json!({"cmd": "eval", "expression": "location.href"}),
        ],
    );
    close(browser);

    assert_eq!(responses.len(), 4, "responses: {responses:?}");
    // After re-navigating the top page, eval must run against the top document
    // (the iframe context is stale/gone), not error out with a dead context.
    assert_eq!(
        responses[3]["ok"],
        Value::Bool(true),
        "eval after navigation must succeed (frame binding reset), got: {:?}",
        responses[3]
    );
    let href = responses[3]["result"].as_str().unwrap_or_default();
    assert!(
        href.contains("frame_child.html"),
        "eval after navigation targets the newly loaded top document, got: {href:?}"
    );
}

// ---------------------------------------------------------------------------
// Negatives: error paths must stay ok:false with clear messages
// ---------------------------------------------------------------------------

#[test]
fn frame_on_non_iframe_element_errors() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let browser = "test-frame-non-iframe";
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "frame", "target": "h1"}),
        ],
    );
    close(browser);

    assert_eq!(responses.len(), 2, "responses: {responses:?}");
    assert_eq!(responses[1]["ok"], Value::Bool(false), "{:?}", responses[1]);
    let err = responses[1]["error"].as_str().unwrap_or_default();
    assert!(
        err.contains("not an <iframe>") || err.to_lowercase().contains("iframe"),
        "expected 'not an <iframe>' error, got: {err:?}"
    );
}

#[test]
fn frame_on_missing_selector_errors() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let browser = "test-frame-missing";
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "frame", "target": ".does-not-exist"}),
        ],
    );
    close(browser);

    assert_eq!(responses.len(), 2, "responses: {responses:?}");
    assert_eq!(responses[1]["ok"], Value::Bool(false), "{:?}", responses[1]);
    let err = responses[1]["error"].as_str().unwrap_or_default();
    assert!(
        err.contains("No element matches"),
        "expected 'No element matches' error, got: {err:?}"
    );
}

#[test]
fn frame_missing_target_field_errors() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let browser = "test-frame-no-target";
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "frame"}),
        ],
    );
    close(browser);

    assert_eq!(responses.len(), 2, "responses: {responses:?}");
    assert_eq!(responses[1]["ok"], Value::Bool(false), "{:?}", responses[1]);
    let err = responses[1]["error"].as_str().unwrap_or_default();
    assert!(err.contains("target"), "expected missing-target error, got: {err:?}");
}

// ---------------------------------------------------------------------------
// Control / regression: without a frame switch, eval + inspect target top doc
// ---------------------------------------------------------------------------

#[test]
fn without_frame_switch_eval_targets_top_document() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let browser = "test-frame-control-eval";
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "eval", "expression": "location.href"}),
        ],
    );
    close(browser);

    assert_eq!(responses.len(), 2, "responses: {responses:?}");
    let href = responses[1]["result"].as_str().unwrap_or_default();
    assert!(
        href.contains("frame_parent.html"),
        "without frame switch, eval must target the top document, got: {href:?}"
    );
}

#[test]
fn without_frame_switch_inspect_shows_parent() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let browser = "test-frame-control-inspect";
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": fixture_url("frame_parent.html")}),
            serde_json::json!({"cmd": "inspect"}),
        ],
    );
    close(browser);

    assert_eq!(responses.len(), 2, "responses: {responses:?}");
    let snap = responses[1]["snapshot"].as_str().unwrap_or_default();
    assert!(
        snap.contains("PARENT PAGE CONTENT"),
        "without frame switch, inspect must show the parent page, got: {snap:?}"
    );
}
