//! A recording is as sensitive as the session it recorded: it holds every value that passed
//! through a fill, including the ones redacted on stdout. Like screenshot, pdf, download and
//! the session store, it must be 0600 rather than whatever the umask allows.

#![cfg(unix)]

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::process::{Command, Stdio};

mod common;
use common::TestBrowser;


fn temp_path(name: &str) -> std::path::PathBuf {
    let path = common::temp_path(name, "jsonl");
    let _ = std::fs::remove_file(&path);
    path
}

/// `common::TestBrowser` makes the name unique and closes the session, panic included.
fn record_a_session(label: &str, path: &std::path::Path) -> bool {
    if !common::browser_ready() {
        return false;
    }
    let guard = TestBrowser::new(label);
    let browser = guard.name();
    let url = common::fixture_url("verdict_states.html");
    let script = format!(
        "{}\n",
        serde_json::json!({"cmd": "goto", "url": url, "_record": path.to_string_lossy()})
    );
    let mut child = Command::new(common::binary())
        .args(["--browser", browser, "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pipe");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let _ = child.wait();
    true
}

#[test]
fn a_recording_is_not_world_readable() {
    let path = temp_path("record-perms");
    if !record_a_session("record-perms", &path) {
        return;
    }
    let metadata = std::fs::metadata(&path).unwrap_or_else(|e| panic!("no recording at {}: {e}", path.display()));
    let mode = metadata.permissions().mode() & 0o777;
    let _ = std::fs::remove_file(&path);

    assert_eq!(
        mode, 0o600,
        "a recording holds every value that passed through the session, including the ones \
         redacted on stdout; got {mode:o}"
    );
}

#[test]
fn appending_to_an_existing_recording_keeps_it_private() {
    let path = temp_path("record-perms-append");
    std::fs::write(&path, "").expect("seed the file");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("widen it");

    if !record_a_session("record-perms-append", &path) {
        let _ = std::fs::remove_file(&path);
        return;
    }
    let metadata = std::fs::metadata(&path).expect("recording");
    let mode = metadata.permissions().mode() & 0o777;
    let _ = std::fs::remove_file(&path);

    assert_eq!(mode, 0o600, "an already-open recording is narrowed too, got {mode:o}");
}

/// An unwritable recording path must not read as a recorded session.
#[test]
fn an_unwritable_record_path_is_reported_not_swallowed() {
    if !common::browser_ready() {
        return;
    }
    let bad = common::temp_path("no-such-dir", "d").join("session.jsonl");
    let url = common::fixture_url("verdict_states.html");
    let script = format!(
        "{}\n",
        serde_json::json!({"cmd": "goto", "url": url, "_record": bad.to_string_lossy()})
    );
    // Unique per process: a fixed name would let a concurrent run clobber this one's page.
    let guard = TestBrowser::new("record-unwritable");
    let browser = guard.name().to_string();
    let mut child = Command::new(common::binary())
        .args(["--browser", &browser, "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pipe");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("pipe output");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();

    // The browser stays up on purpose: the next check reads the page it is showing.
    let output = Command::new(common::binary())
        .args(["--browser", &browser, "--json", "eval", "location.href"])
        .output()
        .expect("read the page location");
    let location = String::from_utf8_lossy(&output.stdout).to_string();

    assert!(
        stdout.contains("recording"),
        "the response must say the recording could not be written: {stdout}"
    );
    assert!(
        !stdout.contains("\"ok\":true"),
        "the refused command must not also report a successful navigation: {stdout}"
    );

    // The navigation did not happen, deliberately: the caller asked for a recorded goto, and
    // an unrecorded one is not that. A bad path stops the session's work, not just its log.
    assert!(
        location.contains("\"ok\":true"),
        "the browser should still be reachable for this check: {location}"
    );
    assert!(
        !location.contains("verdict_states.html"),
        "the refused goto must not have navigated anyway: {location}"
    );
}
