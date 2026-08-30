//! `--chrome-arg` end-to-end: the flag reaches the real Chrome process argv, read back with
//! `ps`. Building the arg vector is a unit test in `browser.rs`.
//!
//! Deliberately version-independent: what the flag DOES (e.g. `--enable-features=WebMCP`)
//! depends on the installed Chrome shipping it, so only its presence in argv is asserted.

use serde_json::Value;
use std::process::Command;

mod common;
use common::TestBrowser;


fn status_pid(browser: &str) -> Option<u32> {
    let output = Command::new(common::binary())
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

    let guard = TestBrowser::new("test-chrome-arg");
    let browser = guard.name();
    let url = common::fixture_url("assert_page.html");
    let output = Command::new(common::binary())
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

    // Inherit-when-omitted: a follow-up without --chrome-arg reconnects to the same pid.
    let follow_up = Command::new(common::binary())
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

    let guard = TestBrowser::new("test-chrome-arg-conflict");
    let browser = guard.name();
    let url = common::fixture_url("assert_page.html");
    let first = Command::new(common::binary())
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

    let conflicting = Command::new(common::binary())
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

    // The refusal must not tear down the running browser.
    let still_pid = status_pid(browser).expect("the refused command must not have killed the browser");
    assert_eq!(still_pid, pid, "a refused --chrome-arg relaunched the browser anyway");
}
