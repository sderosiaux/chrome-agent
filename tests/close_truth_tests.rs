//! `close` says what it did, including when it could not read the process table.
//!
//! `src/kill.rs` unit-tests the classifier; this pins the whole command against a real
//! Chrome, because what matters is that the session entry and the browser both survive.

mod common;

use std::process::Command;

/// A `ps` that exists and exits non-zero with empty output — the busybox case, which is an
/// `Ok` from `Command::output()` rather than a spawn failure.
fn refusing_ps_dir() -> std::path::PathBuf {
    let dir = common::temp_path("close-ps-refuses", "d");
    std::fs::create_dir_all(&dir).expect("the stub's own directory");
    let stub = dir.join("ps");
    std::fs::write(&stub, "#!/bin/sh\nexit 1\n").expect("write the stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    }
    dir
}

/// A directory with no `ps` at all: the distroless case.
fn absent_ps_dir() -> std::path::PathBuf {
    let dir = common::temp_path("close-ps-absent", "d");
    std::fs::create_dir_all(&dir).expect("the empty directory");
    dir
}

#[test]
fn a_close_that_cannot_read_the_process_table_keeps_the_browser_it_cannot_check() {
    if !common::browser_ready() {
        return;
    }
    let browser = common::TestBrowser::new("close-truth");
    let launched = Command::new(common::binary())
        .args(["--browser", browser.name(), "goto", &common::fixture_url("press_keys.html")])
        .output()
        .expect("launch a browser to close");
    assert!(launched.status.success(), "{}", String::from_utf8_lossy(&launched.stderr));

    // `--purge` is passed on one of the two attempts below because it must NOT be honoured:
    // a close that closed nothing must not delete a live Chrome's profile.
    let profile = std::env::var("HOME")
        .map(|home| std::path::PathBuf::from(home).join(".chrome-agent/browsers").join(browser.name()))
        .expect("HOME");
    assert!(profile.exists(), "the launch should have created {}", profile.display());

    // Both cases in turn. Neither may signal, and neither may drop the session entry.
    for (path_dir, purge) in [(refusing_ps_dir(), true), (absent_ps_dir(), false)] {
        let mut args = vec!["--browser", browser.name(), "close", "--json"];
        if purge {
            args.push("--purge");
        }
        let out = Command::new(common::binary())
            .args(&args)
            .env("PATH", &path_dir)
            .output()
            .expect("run close with a process table that will not answer");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let response: serde_json::Value =
            serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{e}: {stdout}"));

        assert_eq!(response["ok"], true, "the command ran: {stdout}");
        assert_eq!(
            response["signalled"], false,
            "nothing was signalled, because nothing was checked: {stdout}"
        );
        let message = response["message"].as_str().unwrap_or_default();
        assert!(!message.contains("Closed"), "nothing was closed: {message}");
        assert!(
            !message.contains("no longer running"),
            "this is the sentence the whole lot exists to remove — it is a claim about a \
             process this invocation never looked at: {message}"
        );
        assert!(message.contains("may still be running"), "{message}");
        if purge {
            // The purge must be refused, not attempted: an attempt would spend ~20 s in
            // `purge_profile`'s retries and report an error rather than a refusal. The
            // reason string is what separates the two, so the reason is asserted.
            assert!(
                message.contains("nothing was closed"),
                "--purge on a close that closed nothing must be refused, not attempted: {message}"
            );
        }
        assert!(
            profile.exists(),
            "the profile of a browser this command did not close was deleted under it: {}",
            profile.display()
        );
        let _ = std::fs::remove_dir_all(&path_dir);
    }

    // The entry and the browser both survived: one more `close`, with a process table this
    // time, has something to close.
    let out = Command::new(common::binary())
        .args(["--browser", browser.name(), "close", "--json"])
        .output()
        .expect("close for real");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let response: serde_json::Value =
        serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{e}: {stdout}"));
    assert_eq!(
        response["signalled"], true,
        "the entry was dropped by the close that could not check it, so the running Chrome \
         it named is now unreachable: {stdout}"
    );
    assert!(
        response["message"].as_str().unwrap_or_default().contains("Closed"),
        "{stdout}"
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
        .args(["--browser", browser.name(), "goto", &common::fixture_url("press_keys.html")])
        .output()
        .expect("launch a browser to close");
    assert!(launched.status.success(), "{}", String::from_utf8_lossy(&launched.stderr));

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
