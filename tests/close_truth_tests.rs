//! `close` says what it did, including when it could not find out.
//!
//! The unit tests in `src/kill.rs` pin the classifier; this pins the whole command against a
//! real Chrome, because the fact that matters is not a word in a match arm — it is what
//! `sessions.json` still holds afterwards. A `close` that reports "pid was no longer
//! running" and drops the entry over a Chrome that is very much running produces an orphan
//! nothing can name: `status` does not list it, `close` has no pid to look up, and only
//! `close --orphans` (which reads the process table this machine just failed to read) could
//! ever find it.

mod common;

use std::process::Command;

/// A `ps` that refuses the question, in a directory of its own.
///
/// This is the busybox door: the applet exists and does not implement `-p <pid> -o comm=`,
/// so it exits non-zero with empty output — an `Ok` from `Command::output()`, which is why
/// only the "could not spawn" door was ever checked. Written as a script rather than
/// simulated, so the test exercises the same `output()` call production does.
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

/// A directory with no `ps` in it at all: the distroless door, which is exactly the audience
/// a fully static musl binary is built for.
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

    // The profile of the browser about to be closed. `--purge` is passed on one of the two
    // attempts below precisely because it must NOT be honoured there: a close that closed
    // nothing has no business deleting the directory a live Chrome is writing into.
    let profile = std::env::var("HOME")
        .map(|home| std::path::PathBuf::from(home).join(".chrome-agent/browsers").join(browser.name()))
        .expect("HOME");
    assert!(profile.exists(), "the launch should have created {}", profile.display());

    // Both doors, in turn. Neither may signal, and neither may leave the store without the
    // entry — so the second attempt still has a browser to find.
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
            // Not attempted, and said so. Attempting it would also have "worked" as far as
            // the directory is concerned — a live Chrome writes its state back and
            // `purge_profile` gives up after its eight retries — but it spends ~20 s doing
            // it and reports the failure as an error rather than as the refusal it is. The
            // reason string is what separates the two, so the reason is what is asserted.
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

    // The proof that the entry survived, and that the browser did too: one more `close`,
    // this time with a process table, has something to close.
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
