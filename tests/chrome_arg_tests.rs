//! `--chrome-arg` end-to-end: does the flag reach the real Chrome process, not just the
//! `Vec<String>` `managed_launch_args` builds (that half is a pure unit test in `browser.rs`).
//!
//! The motivating case — `--enable-features=WebMCP,WebMCPTesting` making
//! `document.modelContext` observable — is demonstrated manually below rather than pinned as
//! an assertion: it depends on the installed Chrome shipping that experimental flag at all
//! (verified against Chrome 152 while building this feature; a CI runner's managed Chromium
//! is a different, independently-pinned build that may or may not carry it), so asserting on
//! it would fail for a reason that has nothing to do with whether `--chrome-arg` works. What
//! this suite pins instead is version-independent: the exact string chrome-agent was asked to
//! pass showing up in the real OS process's argv, read back with `ps` rather than trusted from
//! this tool's own output.

use serde_json::Value;
use std::process::Command;

mod common;

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

struct BrowserGuard(&'static str);

impl Drop for BrowserGuard {
    fn drop(&mut self) {
        let _ = Command::new(binary())
            .args(["--browser", self.0, "close", "--purge"])
            .output();
    }
}

fn status_pid(browser: &str) -> Option<u32> {
    let output = Command::new(binary())
        .args(["--browser", browser, "status", "--json"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).ok()?;
    v["browsers"]
        .as_array()?
        .iter()
        .find(|b| b["name"] == browser)?["pid"]
        .as_u64()
        .map(|p| p as u32)
}

/// The real OS process's command line, not this tool's own record of what it asked for.
#[cfg(unix)]
fn process_argv(pid: u32) -> String {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .expect("ps must be available to read the real process argv");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[cfg(unix)]
#[test]
fn chrome_arg_reaches_the_real_chrome_process() {
    if !common::browser_ready() {
        return;
    }

    let browser = "test-chrome-arg";
    let _guard = BrowserGuard(browser);
    let url = common::fixture_url("assert_page.html");
    let output = Command::new(binary())
        .args([
            "--browser",
            browser,
            "--chrome-arg",
            "--enable-features=WebMCP,WebMCPTesting",
            "goto",
            &url,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "chrome-agent failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let pid = status_pid(browser).expect("launched browser has a pid on a live session entry");
    let argv = process_argv(pid);
    assert!(
        argv.contains("--enable-features=WebMCP,WebMCPTesting"),
        "the requested --chrome-arg did not reach the real Chrome process argv: {argv}"
    );

    // A follow-up command that omits --chrome-arg reconnects to the SAME process rather than
    // relaunching without it — the inherit-when-omitted rule, proven against the live pid
    // rather than the session file's record of it.
    let follow_up = Command::new(binary())
        .args(["--browser", browser, "eval", "1+1", "--json"])
        .output()
        .unwrap();
    assert!(follow_up.status.success());
    let follow_up_pid = status_pid(browser).expect("browser still has a pid after a follow-up");
    assert_eq!(follow_up_pid, pid, "omitting --chrome-arg relaunched the browser");
}

#[cfg(unix)]
#[test]
fn a_conflicting_chrome_arg_is_refused_rather_than_relaunching() {
    if !common::browser_ready() {
        return;
    }

    let browser = "test-chrome-arg-conflict";
    let _guard = BrowserGuard(browser);
    let url = common::fixture_url("assert_page.html");
    let first = Command::new(binary())
        .args([
            "--browser",
            browser,
            "--chrome-arg",
            "--disable-features=Translate",
            "goto",
            &url,
        ])
        .output()
        .unwrap();
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    let pid = status_pid(browser).expect("launched browser has a pid");

    let conflicting = Command::new(binary())
        .args([
            "--browser",
            browser,
            "--chrome-arg",
            "--disable-features=Translate,PasswordImport",
            "eval",
            "1+1",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!conflicting.status.success(), "a different --chrome-arg must be refused, not silently applied");
    // --json puts the error on stdout (`{"ok":false,...}`), not stderr.
    let stdout = String::from_utf8_lossy(&conflicting.stdout);
    assert!(stdout.contains("different --chrome-arg flags"), "{stdout}");

    // The refusal must not have torn down the running browser to get there.
    let still_pid = status_pid(browser).expect("the refused command must not have killed the browser");
    assert_eq!(still_pid, pid, "a refused --chrome-arg relaunched the browser anyway");
}
