//! `~/.chrome-agent/history.jsonl` was the one file this tool writes with no `set_permissions`,
//! and it stores every URL a session navigated to — permanently, and with the query string that
//! carries an OAuth `?code=`, a password-reset token or a pre-signed signature.
//!
//! The file is global (one per home directory), so these tests assert only about facts a
//! concurrent run cannot change: the mode is narrowed by every append, and a token this test
//! alone invented never appears in it.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::Command;

mod common;
use common::TestBrowser;

fn history_path() -> PathBuf {
    store_dir().join("history.jsonl")
}

/// The same resolver the binary uses, so the test cannot look in a different home.
fn store_dir() -> PathBuf {
    dirs::home_dir()
        .expect("a home directory")
        .join(".chrome-agent")
}

/// Navigate once, so the binary appends a history entry. `false` when Chrome is unavailable.
fn navigate(label: &str, url: &str) -> bool {
    if !common::browser_ready() {
        return false;
    }
    let guard = TestBrowser::new(label);
    let output = Command::new(common::binary())
        .args(["--browser", guard.name(), "goto", url])
        .output()
        .expect("run chrome-agent");
    assert!(
        output.status.success(),
        "goto failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

/// The mode is set on every append, not only at creation, so a file left 0644 by a run that
/// predates this is narrowed the next time anything navigates.
#[test]
fn the_history_file_is_not_world_readable() {
    let path = history_path();
    // Widen it if it exists: the assertion is that the append NARROWS it, not that it happened
    // to be private already.
    if path.exists() {
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
    }
    if !navigate("history-perms", &common::fixture_url("press_keys.html")) {
        return;
    }
    let mode = std::fs::metadata(&path)
        .unwrap_or_else(|e| panic!("no history at {}: {e}", path.display()))
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o600,
        "history holds every URL a session visited, forever; got {mode:o}"
    );
}

/// The directory holds the session store, the daemon socket and this file. Only
/// `session::save_to` used to set its mode, so a first run whose command errored before the save
/// left it at whatever the umask allowed.
#[test]
fn the_store_directory_is_not_world_readable() {
    if !navigate("history-dir-perms", &common::fixture_url("press_keys.html")) {
        return;
    }
    let dir = store_dir();
    let mode = std::fs::metadata(&dir)
        .expect("the store directory")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700, "got {mode:o}");
}

/// The credential is in the query string. A `file://` URL carries one just as an OAuth callback
/// does, and the token here is unique to this run, so nothing else can have written it.
#[test]
fn a_query_string_is_never_written_down() {
    let token = common::unique_name("tok");
    let url = format!(
        "{}?code={token}&state=xyz",
        common::fixture_url("press_keys.html")
    );
    if !navigate("history-query", &url) {
        return;
    }
    let contents = std::fs::read_to_string(history_path()).expect("history file");
    assert!(
        !contents.contains(&token),
        "the single-use credential was stored permanently: {contents:?}"
    );
    assert!(
        contents.contains("press_keys.html\u{2026}"),
        "the navigation is still recorded, marked as truncated: {contents}"
    );
}

/// And `history` itself prints what was stored: the path, so the entry stays recognisable.
#[test]
fn history_still_reports_the_navigation_it_truncated() {
    let token = common::unique_name("tok");
    let url = format!("{}?code={token}", common::fixture_url("press_keys.html"));
    if !navigate("history-report", &url) {
        return;
    }
    let output = Command::new(common::binary())
        .args(["history", "--filter", "press_keys.html"])
        .output()
        .expect("run chrome-agent");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("press_keys.html"),
        "the entry is unreadable: {stdout}"
    );
    assert!(
        !stdout.contains(&token),
        "the token reached stdout: {stdout}"
    );
}
