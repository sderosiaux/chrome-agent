use std::process::Command;

mod common;

/// Get the path to the built binary.
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

/// Run chrome-agent with args and return (stdout, stderr, `exit_code`).
fn run_cli(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(binary())
        .args(args)
        .output()
        .expect("Failed to run chrome-agent");

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
    assert!(stdout.contains("assert"));
    assert!(stdout.contains("emulate"));
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
    assert!(stdout.contains("--proxy-server"));
    assert!(stdout.contains("--headed"));
    assert!(stdout.contains("--timeout"));
    assert!(stdout.contains("--ignore-https-errors"));
    assert!(stdout.contains("--page"));
}

/// A global flag parses on either side of the subcommand.
///
/// `chrome-agent fill --selector "#micro" "x" --json` used to fail with a raw clap error and the
/// tip "to pass '--json' as a value, use '-- --json'" — advice for a different problem, on the
/// most natural way to reach for the flag, and on the caller's first attempt. `CHROME_AGENT_PARSE_ONLY`
/// returns the moment clap has spoken, so this is clap's verdict and no browser is launched.
#[test]
fn a_global_flag_is_accepted_on_either_side_of_the_verb() {
    let parses = |args: &[&str]| {
        let output = Command::new(binary())
            .args(args)
            .env("CHROME_AGENT_PARSE_ONLY", "1")
            .output()
            .expect("run chrome-agent");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stderr).lines().next().unwrap_or("").to_string(),
        )
    };
    let cases: &[(&[&str], &[&str])] = &[
        (&["--json", "fill", "--selector", "#micro", "x"], &["fill", "--selector", "#micro", "x", "--json"]),
        (&["--json", "click", "n1"], &["click", "n1", "--json"]),
        (&["--json", "inspect"], &["inspect", "--json"]),
        (&["--verdict", "off", "click", "n1"], &["click", "n1", "--verdict", "off"]),
        (&["--browser", "a7", "eval", "1"], &["eval", "1", "--browser", "a7"]),
    ];
    for (before, after) in cases {
        let (ok, err) = parses(before);
        assert!(ok, "the documented order stopped working: {before:?} -> {err}");
        let (ok, err) = parses(after);
        assert!(ok, "a global flag after the verb must parse: {after:?} -> {err}");
    }
}

/// The two flags that cannot be global, and why: `wait`/`download` declare their own
/// `--timeout`, and the twelve action commands their own `--max-depth`. A global arg propagates
/// into every subcommand, so sharing an id with one is a duplicate-argument panic at startup.
/// Both positions still parse, each meaning its own thing, and `run.rs` resolves them with
/// `local.or(global)`.
#[test]
fn the_two_locally_redeclared_flags_still_work_in_both_positions() {
    for args in [
        vec!["wait", "selector", ".x", "--timeout", "5"],
        vec!["--timeout", "5", "click", "n1"],
        vec!["click", "n1", "--max-depth", "2"],
        vec!["--max-depth", "2", "click", "n1"],
    ] {
        let output = Command::new(binary())
            .args(&args)
            .env("CHROME_AGENT_PARSE_ONLY", "1")
            .output()
            .expect("run chrome-agent");
        assert!(
            output.status.success(),
            "{args:?} -> {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// The two flags that must precede the verb now say so, in the caller's own words.
///
/// `chrome-agent click n1 --timeout 5` is rejected on purpose — `wait` and `download` declare
/// their own `--timeout` with their own defaults, so the global one cannot propagate into every
/// subcommand. The harm was never the rule; it was clap's answer to it, `tip: to pass
/// '--timeout' as a value, use '-- --timeout'`, which is advice for escaping a literal string
/// nobody meant to pass.
#[test]
fn a_flag_that_must_precede_the_verb_says_so_instead_of_offering_to_escape_it() {
    for (args, flag, moved) in [
        (vec!["click", "n1", "--timeout", "5"], "--timeout", "chrome-agent --timeout 5 click n1"),
        (vec!["text", "--max-depth", "2"], "--max-depth", "chrome-agent --max-depth 2 text"),
    ] {
        let (_, stderr, code) = run_cli(&args);
        assert_eq!(code, 1, "{args:?} should still be a usage error: {stderr}");
        assert!(
            stderr.contains(&format!("hint: {flag} is read before the verb")),
            "{args:?} did not state the rule: {stderr}"
        );
        assert!(
            stderr.contains(&format!("`{moved}`")),
            "{args:?} did not name the working invocation: {stderr}"
        );
        assert!(
            !stderr.contains(&format!("-- {flag}")),
            "clap's escape-it tip is back for {args:?}: {stderr}"
        );
        assert!(
            !stderr.contains("as a value"),
            "clap's escape-it tip is back for {args:?}: {stderr}"
        );

        // The strongest form of the hint contract: the command it hands back has to run. A hint
        // that names an invocation the parser also rejects is worse than no hint.
        let suggested: Vec<&str> = moved.split_whitespace().skip(1).collect();
        let output = Command::new(binary())
            .args(&suggested)
            .env("CHROME_AGENT_PARSE_ONLY", "1")
            .output()
            .expect("run chrome-agent");
        assert!(
            output.status.success(),
            "the hint for {args:?} names an invocation that does not parse: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// And an unrelated usage error keeps clap's own wording, tip included: this rewrite covers the
/// one error clap gets wrong, not its output in general.
#[test]
fn an_unrelated_usage_error_is_left_to_clap() {
    let (_, stderr, code) = run_cli(&["click", "n1", "--nonsense"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("unexpected argument '--nonsense'"), "{stderr}");
    assert!(stderr.contains("-- --nonsense"), "clap's tip should survive here: {stderr}");
    assert!(!stderr.contains("read before the verb"), "{stderr}");
}

#[test]
fn version_flag() {
    let (stdout, _, code) = run_cli(&["--version"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("chrome-agent"));
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
    assert!(stdout.contains("Click an element"));
    // The one-liner is what a token-conscious agent reads instead of the 26 KB `--help`, so it
    // has to carry the guarantee: a click that reports success may still have been taken by
    // something stacked above the target, and the response is where that shows.
    assert!(stdout.contains("who received the event"), "{stdout}");
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

/// RAII guard: closes browser on drop (even on panic).
struct TestBrowser(String);

impl TestBrowser {
    /// Unique per process. A fixed name means two concurrent runs of this suite drive the same
    /// browser: one navigates while the other clicks a uid from its own snapshot, and both fail
    /// with "Node with given id does not belong to the document". CLAUDE.md documents the
    /// hazard — `--browser <unique>` per agent — and the suites have to obey it too.
    fn new(label: &str) -> Self {
        Self(format!("{label}-{}", std::process::id()))
    }
    fn name(&self) -> &str {
        &self.0
    }
}

impl Drop for TestBrowser {
    fn drop(&mut self) {
        let _ = run_cli(&["--browser", &self.0, "close", "--purge"]);
    }
}

#[test]
fn headed_goto_and_eval() {
    if !common::browser_ready() {
        return;
    }

    let b = TestBrowser::new("test-integration");

    // Navigate
    let (stdout, stderr, code) = run_cli(&[
        "--browser",
        b.name(),
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
        b.name(),
        "eval",
        "document.title",
    ]);

    if code == 0 {
        assert!(
            stdout.contains("Example Domain") || stdout.contains("example"),
            "eval output: {stdout}"
        );
    }
}

#[test]
fn dblclick_selector_fires_real_double_click() {
    if !common::browser_ready() {
        return;
    }

    let b = TestBrowser::new("test-dblclick-selector");

    // Fixture page: the button counts `click` vs `dblclick` events separately.
    // Written to a temp file and loaded via file:// (avoids data:-URL encoding).
    let html = "<!doctype html><html><body><button id=\"b\" \
        onclick=\"window.__c=(window.__c||0)+1\" \
        ondblclick=\"window.__d=(window.__d||0)+1\">x</button></body></html>";
    let mut path = std::env::temp_dir();
    path.push("chrome-agent-dblclick-selector-test.html");
    std::fs::write(&path, html).expect("write fixture");
    let url = format!("file://{}", path.display());

    let (_, stderr, code) = run_cli(&["--browser", b.name(), "goto", &url]);
    if code != 0 {
        let _ = std::fs::remove_file(&path);
        common::unavailable(&format!("goto dblclick fixture failed: {stderr}"));
        return;
    }

    let (_, _, code) = run_cli(&["--browser", b.name(), "dblclick", "--selector", "#b"]);
    assert_eq!(code, 0, "dblclick --selector should succeed");

    // The whole point of the fix: a selector double-click must fire `dblclick`,
    // not just a single `click`. Pre-fix (click_selector → el.click()) left __d=0.
    let (stdout, _, code) = run_cli(&["--browser", b.name(), "eval", "String(window.__d||0)"]);
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 0, "eval should succeed");
    assert!(
        stdout.contains('1'),
        "dblclick event must have fired once (window.__d), got: {stdout}"
    );
}

#[test]
fn headed_inspect_returns_uids() {
    if !common::browser_ready() {
        return;
    }

    let b = TestBrowser::new("test-inspect");

    let (_, _, code) = run_cli(&["--browser", b.name(), "goto", "https://example.com"]);

    if code != 0 {
        common::unavailable("goto https://example.com failed");
        return;
    }

    let (stdout, _, code) = run_cli(&["--browser", b.name(), "inspect"]);

    if code == 0 {
        assert!(stdout.contains("uid="), "inspect should contain uid=N: {stdout}");
    }
}

#[test]
fn headed_screenshot_returns_path() {
    if !common::browser_ready() {
        return;
    }

    let b = TestBrowser::new("test-screenshot");

    let (_, _, code) = run_cli(&["--browser", b.name(), "goto", "https://example.com"]);

    if code != 0 {
        common::unavailable("goto https://example.com failed");
        return;
    }

    let (stdout, _, code) = run_cli(&["--browser", b.name(), "screenshot"]);

    if code == 0 {
        assert!(
            stdout.contains(".png") && stdout.contains(".chrome-agent/tmp/"),
            "screenshot should return a file path: {stdout}"
        );
        let path = stdout.trim();
        assert!(
            std::path::Path::new(path).exists(),
            "Screenshot file should exist at {path}"
        );
    }
}

#[test]
fn headed_tabs_lists_pages() {
    if !common::browser_ready() {
        return;
    }

    let b = TestBrowser::new("test-tabs");

    let (_, _, code) = run_cli(&["--browser", b.name(), "goto", "https://example.com"]);

    if code != 0 {
        common::unavailable("goto https://example.com failed");
        return;
    }

    let (stdout, _, code) = run_cli(&["--browser", b.name(), "tabs"]);

    if code == 0 {
        assert!(
            stdout.contains("TARGET_ID") || stdout.contains("example.com"),
            "tabs output: {stdout}"
        );
    }
}
