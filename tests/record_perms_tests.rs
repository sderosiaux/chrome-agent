//! A recording is as sensitive as the session it recorded.
//!
//! A pipe command carrying `_record` writes it and its response to that file, which
//! includes the values
//! that passed through a fill — among them the ones redacted on stdout precisely because
//! they are secrets. Screenshot, pdf, download and the session store all chmod 0600; the
//! recording was created with whatever the umask allowed, typically 0644, world-readable on
//! a shared machine.

#![cfg(unix)]

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::process::{Command, Stdio};

mod common;

fn binary() -> String {
    let mut path = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
    path.push("chrome-agent");
    path.to_string_lossy().into_owned()
}

fn run_cli(args: &[&str]) -> i32 {
    Command::new(binary())
        .args(args)
        .output()
        .expect("Failed to run chrome-agent")
        .status
        .code()
        .unwrap_or(-1)
}

fn temp_path(name: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("chrome-agent-{name}-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

fn record_a_session(browser: &str, path: &std::path::Path) -> bool {
    if !common::browser_ready() {
        return false;
    }
    let url = common::fixture_url("verdict_states.html");
    let script = format!(
        "{}\n",
        serde_json::json!({"cmd": "goto", "url": url, "_record": path.to_string_lossy()})
    );
    let mut child = Command::new(binary())
        .args(["--browser", browser, "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pipe");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let _ = child.wait();
    let _ = run_cli(&["--browser", browser, "close", "--purge"]);
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

/// Appending to an existing recording must not widen it back either.
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
///
/// `start_recording` and `log_entry` both return Result, and the pipe loop discarded
/// both with `let _ =`. The response for the command was `ok:true` and stdout was
/// indistinguishable from a session that was actually being written — so an agent
/// finishes a long run, goes to `replay` it, and finds nothing there.
#[test]
fn an_unwritable_record_path_is_reported_not_swallowed() {
    if !common::browser_ready() {
        return;
    }
    let bad = std::env::temp_dir()
        .join(format!("chrome-agent-no-such-dir-{}", std::process::id()))
        .join("session.jsonl");
    let url = common::fixture_url("verdict_states.html");
    let script = format!(
        "{}\n",
        serde_json::json!({"cmd": "goto", "url": url, "_record": bad.to_string_lossy()})
    );
    // Unique per process: a fixed name lets a second concurrent run of this suite drive the
    // same browser and clobber this one's page.
    let browser = format!("record-unwritable-{}", std::process::id());
    let mut child = Command::new(binary())
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

    // The browser stays up here on purpose: the next check reads the page it is showing.
    let output = Command::new(binary())
        .args(["--browser", &browser, "--json", "eval", "location.href"])
        .output()
        .expect("read the page location");
    let location = String::from_utf8_lossy(&output.stdout).to_string();
    let _ = run_cli(&["--browser", &browser, "close", "--purge"]);

    assert!(
        stdout.contains("recording"),
        "the response must say the recording could not be written: {stdout}"
    );
    assert!(
        !stdout.contains("\"ok\":true"),
        "the refused command must not also report a successful navigation: {stdout}"
    );

    // And the navigation genuinely did not happen. This is the deliberate half of the
    // trade: the caller asked for a recorded goto, and an unrecorded one is not that.
    // The refusal is per command and loud, so an agent learns on its first line rather
    // than at replay time — but a bad path stops the session's work, not just its log.
    assert!(
        location.contains("\"ok\":true"),
        "the browser should still be reachable for this check: {location}"
    );
    assert!(
        !location.contains("verdict_states.html"),
        "the refused goto must not have navigated anyway: {location}"
    );
}
