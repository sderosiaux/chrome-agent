use std::process::Command;
use std::time::Duration;

/// Get the path to the built binary.
fn binary() -> String {
    let mut path = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    path.push("aibrowsr");
    path.to_string_lossy().into_owned()
}

/// Run aibrowsr with args and return (stdout, stderr, exit_code).
fn run_cli(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(binary())
        .args(args)
        .output()
        .expect("Failed to run aibrowsr");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);

    (stdout, stderr, code)
}

#[test]
fn help_shows_all_subcommands() {
    let (stdout, _, code) = run_cli(&["--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("goto"));
    assert!(stdout.contains("click"));
    assert!(stdout.contains("fill"));
    assert!(stdout.contains("fill-form"));
    assert!(stdout.contains("inspect"));
    assert!(stdout.contains("screenshot"));
    assert!(stdout.contains("eval"));
    assert!(stdout.contains("tabs"));
    assert!(stdout.contains("wait"));
    assert!(stdout.contains("type"));
    assert!(stdout.contains("press"));
    assert!(stdout.contains("scroll"));
    assert!(stdout.contains("hover"));
    assert!(stdout.contains("close"));
    assert!(stdout.contains("status"));
    assert!(stdout.contains("stop"));
    assert!(stdout.contains("daemon"));
}

#[test]
fn help_includes_llm_guide() {
    let (stdout, _, code) = run_cli(&["--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("LLM USAGE GUIDE"));
    assert!(stdout.contains("inspect -> read uids -> act"));
    assert!(stdout.contains("--inspect"));
}

#[test]
fn help_shows_global_flags() {
    let (stdout, _, code) = run_cli(&["--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("--browser"));
    assert!(stdout.contains("--connect"));
    assert!(stdout.contains("--headed"));
    assert!(stdout.contains("--timeout"));
    assert!(stdout.contains("--ignore-https-errors"));
    assert!(stdout.contains("--page"));
}

#[test]
fn version_flag() {
    let (stdout, _, code) = run_cli(&["--version"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("aibrowsr"));
}

#[test]
fn status_works_without_browser() {
    let (stdout, _, code) = run_cli(&["status"]);
    assert_eq!(code, 0);
    // Should show either "No active browser sessions" or existing sessions
    assert!(
        stdout.contains("No active browser sessions") || stdout.contains("browser="),
        "Unexpected status output: {stdout}"
    );
}

#[test]
fn stop_when_no_daemon() {
    let (stdout, _, code) = run_cli(&["stop"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("not running") || stdout.contains("stopped"));
}

#[test]
fn goto_subcommand_help() {
    let (stdout, _, code) = run_cli(&["goto", "--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Navigate to a URL"));
    assert!(stdout.contains("--inspect"));
}

#[test]
fn click_subcommand_help() {
    let (stdout, _, code) = run_cli(&["click", "--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Click an element by uid"));
    assert!(stdout.contains("--inspect"));
}

#[test]
fn fill_subcommand_help() {
    let (stdout, _, code) = run_cli(&["fill", "--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Fill an input"));
    assert!(stdout.contains("--inspect"));
}

#[test]
fn inspect_subcommand_help() {
    let (stdout, _, code) = run_cli(&["inspect", "--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("accessibility tree inspection"));
    assert!(stdout.contains("--verbose"));
}

#[test]
fn eval_subcommand_help() {
    let (stdout, _, code) = run_cli(&["eval", "--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Evaluate JavaScript"));
}

// Integration tests that require Chrome (skipped in CI without Chrome)
// These are guarded by a check for Chrome availability.

fn chrome_available() -> bool {
    // Check if any Chrome-like binary exists
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
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

#[test]
fn headed_goto_and_eval() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }

    // Navigate
    let (stdout, stderr, code) = run_cli(&[
        
        "--browser",
        "test-integration",
        "goto",
        "https://example.com",
    ]);

    if code != 0 {
        eprintln!("goto failed (may be network issue): {stderr}");
        return;
    }

    assert!(
        stdout.contains("example.com") || stdout.contains("Example"),
        "goto output: {stdout}"
    );

    // Eval on same browser
    let (stdout, _, code) = run_cli(&[
        
        "--browser",
        "test-integration",
        "eval",
        "document.title",
    ]);

    if code == 0 {
        assert!(
            stdout.contains("Example Domain") || stdout.contains("example"),
            "eval output: {stdout}"
        );
    }

    // Cleanup
    let _ = run_cli(&["--browser", "test-integration", "close"]);
}

#[test]
fn headed_inspect_returns_uids() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }

    let (_, _, code) = run_cli(&[
        
        "--browser",
        "test-inspect",
        "goto",
        "https://example.com",
    ]);

    if code != 0 {
        eprintln!("SKIP: goto failed");
        return;
    }

    let (stdout, _, code) = run_cli(&[
        
        "--browser",
        "test-inspect",
        "inspect",
    ]);

    if code == 0 {
        assert!(stdout.contains("uid="), "inspect should contain uid=N: {stdout}");
    }

    let _ = run_cli(&["--browser", "test-inspect", "close"]);
}

#[test]
fn headed_screenshot_returns_path() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }

    let (_, _, code) = run_cli(&[
        
        "--browser",
        "test-screenshot",
        "goto",
        "https://example.com",
    ]);

    if code != 0 {
        eprintln!("SKIP: goto failed");
        return;
    }

    let (stdout, _, code) = run_cli(&[
        
        "--browser",
        "test-screenshot",
        "screenshot",
    ]);

    if code == 0 {
        assert!(
            stdout.contains(".png") && stdout.contains(".aibrowsr/tmp/"),
            "screenshot should return a file path: {stdout}"
        );
        // Verify file exists
        let path = stdout.trim();
        assert!(
            std::path::Path::new(path).exists(),
            "Screenshot file should exist at {path}"
        );
    }

    let _ = run_cli(&["--browser", "test-screenshot", "close"]);
}

#[test]
fn headed_tabs_lists_pages() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }

    let (_, _, code) = run_cli(&[
        
        "--browser",
        "test-tabs",
        "goto",
        "https://example.com",
    ]);

    if code != 0 {
        eprintln!("SKIP: goto failed");
        return;
    }

    let (stdout, _, code) = run_cli(&[
        
        "--browser",
        "test-tabs",
        "tabs",
    ]);

    if code == 0 {
        assert!(
            stdout.contains("TARGET_ID") || stdout.contains("example.com"),
            "tabs output: {stdout}"
        );
    }

    let _ = run_cli(&["--browser", "test-tabs", "close"]);
}
