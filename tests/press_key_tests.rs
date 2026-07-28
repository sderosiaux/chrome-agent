use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn binary() -> String {
    let mut path = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
    path.push("chrome-agent");
    path.to_string_lossy().into_owned()
}

fn fixture_url(name: &str) -> String {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures");
    path.push(name);
    format!("file://{}", path.display())
}

fn run_cli(args: &[&str]) -> (String, i32) {
    let output = Command::new(binary()).args(args).output().expect("Failed to run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
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
        if Command::new("which").arg(candidate).output().is_ok_and(|o| o.status.success()) {
            return true;
        }
    }
    false
}

struct TestBrowser(&'static str);
impl Drop for TestBrowser {
    fn drop(&mut self) {
        let _ = run_cli(&["--browser", self.0, "close", "--purge"]);
    }
}

fn open_and_focus(browser: &str) -> bool {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return false;
    }
    let (_, code) = run_cli(&["--browser", browser, "goto", &fixture_url("press_keys.html")]);
    if code != 0 {
        eprintln!("SKIP: goto failed");
        return false;
    }
    let (_, code) = run_cli(&["--browser", browser, "eval", "document.getElementById('i').focus(); 1"]);
    code == 0
}

/// A printable key has to type. Without `text` on the CDP event the page sees a keydown
/// and nothing is inserted, so the command reported success and left the field empty.
#[test]
fn pressing_a_printable_character_types_it() {
    let b = TestBrowser("press-char");
    if !open_and_focus(b.0) {
        return;
    }
    for key in ["h", "i"] {
        let (out, code) = run_cli(&["--browser", b.0, "--verdict", "off", "--json", "press", key]);
        assert_eq!(code, 0, "press {key} should succeed: {out}");
    }
    let (value, _) = run_cli(&["--browser", b.0, "eval", "document.getElementById('i').value"]);
    assert_eq!(value.trim().trim_matches('"'), "hi", "the characters should have been typed");
}

/// An unmapped key name used to go out with virtual key code 0, which no handler reads as
/// a key, and the command still reported success.
#[test]
fn an_unknown_key_name_is_refused_rather_than_sent_as_nothing() {
    let b = TestBrowser("press-unknown");
    if !open_and_focus(b.0) {
        return;
    }
    let (out, code) = run_cli(&["--browser", b.0, "--verdict", "off", "--json", "press", "Zorglub"]);
    let v: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
    assert_ne!(code, 0, "an unknown key should fail: {v}");
    assert!(
        v["error"].as_str().unwrap_or_default().contains("Unknown key"),
        "and say which one: {v}"
    );
}

/// Navigation keys that were missing entirely used to fall into the same hole.
#[test]
fn navigation_keys_reach_the_page() {
    let b = TestBrowser("press-nav");
    if !open_and_focus(b.0) {
        return;
    }
    for key in ["Home", "End", "PageDown", "F5"] {
        let (out, code) = run_cli(&["--browser", b.0, "--verdict", "off", "--json", "press", key]);
        assert_eq!(code, 0, "press {key} should succeed: {out}");
    }
    let (log, _) = run_cli(&["--browser", b.0, "eval", "document.getElementById('log').textContent"]);
    for key in ["Home", "End", "PageDown", "F5"] {
        assert!(log.contains(key), "the page should have seen {key}: {log}");
    }
}

/// `'.'` is ASCII 46, which is also VK_DELETE. Deriving a virtual key code from the
/// character's byte therefore turned `press .` into a delete: verified, a field holding
/// "XYZ" with the caret at 0 became "YZ", reported as success.
#[test]
fn punctuation_types_instead_of_deleting() {
    let b = TestBrowser("press-punct");
    if !open_and_focus(b.0) {
        return;
    }
    let (_, code) = run_cli(&[
        "--browser", b.0, "eval",
        "const i=document.getElementById('i'); i.value='XYZ'; i.focus(); i.setSelectionRange(0,0); 1",
    ]);
    assert_eq!(code, 0);
    let (out, code) = run_cli(&["--browser", b.0, "--verdict", "off", "--json", "press", "."]);
    assert_eq!(code, 0, "{out}");
    let (value, _) = run_cli(&["--browser", b.0, "eval", "document.getElementById('i').value"]);
    assert_eq!(
        value.trim().trim_matches('"'),
        ".XYZ",
        "the character must be inserted, and nothing deleted"
    );
}

/// ``Input.insertText` goes to whatever holds focus. With focus on BODY it goes nowhere,
/// and the message was built from the request rather than from the page.
#[test]
fn typing_with_nothing_focused_is_refused() {
    let b = TestBrowser("type-nofocus");
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let (_, code) = run_cli(&["--browser", b.0, "goto", &fixture_url("press_keys.html")]);
    if code != 0 {
        return;
    }
    let (out, code) = run_cli(&["--browser", b.0, "--verdict", "off", "--json", "type", "hello"]);
    let v: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
    assert_ne!(code, 0, "typing into nothing should fail: {v}");
    assert!(
        v["error"].as_str().unwrap_or_default().contains("focus"),
        "and say why: {v}"
    );
}
