//! `close` says what it did, and the guard behind it is not the caller's to move.
//!
//! `src/kill.rs` unit-tests the classifier; this pins the whole command against a real
//! Chrome, because what matters is which process gets signalled.

mod common;

use std::process::Command;

/// A directory holding a `ps` that answers `sleep` for every pid, plus a `kill` that signals
/// nothing. If either were consulted, `close` would refuse the kill as a recycled pid and then
/// drop the entry — a running browser made unreachable by an environment variable.
fn lying_tools_dir() -> std::path::PathBuf {
    let dir = common::temp_path("close-lying-tools", "d");
    std::fs::create_dir_all(&dir).expect("the stubs' own directory");
    for (name, body) in [
        ("ps", "#!/bin/sh\necho sleep\n"),
        ("kill", "#!/bin/sh\nexit 0\n"),
    ] {
        let stub = dir.join(name);
        std::fs::write(&stub, body).expect("write the stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        }
    }
    dir
}

/// The guard that decides whether `close` may signal used to resolve `ps` through the inherited
/// `PATH`, so whatever set that variable decided the answer — for a check whose whole purpose is
/// to stop a recycled pid being killed. It reads `/proc/<pid>/comm` or `/bin/ps` now, and this
/// pins that a `PATH` holding a hostile `ps` and `kill` changes neither the verdict nor the act.
///
/// The converse case — a system where the tools genuinely are absent, so `close` keeps the entry
/// it could not check — is no longer reachable from a test, since nothing a test may write moves
/// `/bin`. `kill.rs`'s unit tests cover it at the classifier, where it is decided.
#[test]
fn a_hostile_path_does_not_reach_the_guard_that_decides_the_kill() {
    if !common::browser_ready() {
        return;
    }
    let browser = common::TestBrowser::new("close-truth");
    let launched = Command::new(common::binary())
        .args([
            "--browser",
            browser.name(),
            "goto",
            &common::fixture_url("press_keys.html"),
        ])
        .output()
        .expect("launch a browser to close");
    assert!(
        launched.status.success(),
        "{}",
        String::from_utf8_lossy(&launched.stderr)
    );

    let profile = std::env::var("HOME")
        .map(|home| {
            std::path::PathBuf::from(home)
                .join(".chrome-agent/browsers")
                .join(browser.name())
        })
        .expect("HOME");
    assert!(
        profile.exists(),
        "the launch should have created {}",
        profile.display()
    );

    let tools = lying_tools_dir();
    let out = Command::new(common::binary())
        .args(["--browser", browser.name(), "close", "--json"])
        .env("PATH", &tools)
        .output()
        .expect("run close with a hostile PATH");
    let _ = std::fs::remove_dir_all(&tools);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let response: serde_json::Value =
        serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{e}: {stdout}"));

    assert_eq!(response["ok"], true, "the command ran: {stdout}");
    assert_eq!(
        response["signalled"], true,
        "a `ps` on PATH answered for the guard, so the browser was left alive and its entry \
         dropped — the exact outcome the guard exists to prevent: {stdout}"
    );
    assert_eq!(
        response["exited"], true,
        "the stub `kill` was run instead of the real one, so nothing was signalled: {stdout}"
    );
    let message = response["message"].as_str().unwrap_or_default();
    assert!(message.contains("Closed"), "{message}");
    assert!(
        !message.contains("another process"),
        "the stub `ps` classified a real Chrome as a recycled pid: {message}"
    );
}

/// A `close` that waited for the exit removes the port file it waited for.
///
/// `cmd_close` deleted `browsers_dir()/<name>/DevToolsActivePort`, one directory above the file
/// `browser::browser_profile_dir` writes, so the documented removal had never removed anything
/// — and a stale port file is what lets the next command on the same name handshake with a
/// Chrome that is gone. Only the path was wrong, which is exactly the class of bug no assertion
/// on the response can catch.
#[test]
fn a_close_that_waited_for_the_exit_removes_the_port_file() {
    if !common::browser_ready() {
        return;
    }
    let browser = common::TestBrowser::new("close-port-file");
    let launched = Command::new(common::binary())
        .args([
            "--browser",
            browser.name(),
            "goto",
            &common::fixture_url("press_keys.html"),
        ])
        .output()
        .expect("launch a browser to close");
    assert!(
        launched.status.success(),
        "{}",
        String::from_utf8_lossy(&launched.stderr)
    );

    let port_file = std::path::PathBuf::from(std::env::var("HOME").expect("HOME"))
        .join(".chrome-agent/browsers")
        .join(browser.name())
        .join("chromium-profile/DevToolsActivePort");
    assert!(
        port_file.exists(),
        "the launch writes it here, and a test asserting on a path Chrome never uses proves \
         nothing: {}",
        port_file.display()
    );

    let out = Command::new(common::binary())
        .args(["--browser", browser.name(), "close", "--json"])
        .output()
        .expect("close");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let response: serde_json::Value =
        serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{e}: {stdout}"));
    assert_eq!(response["signalled"], true, "{stdout}");
    assert_eq!(response["exited"], true, "{stdout}");
    assert!(
        !port_file.exists(),
        "the browser exited and its port file outlived it: {}",
        port_file.display()
    );
}
