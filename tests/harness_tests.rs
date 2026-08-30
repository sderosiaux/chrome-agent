//! Tests for the test harness itself.
//!
//! The suite's worst failure mode was never a wrong assertion — it was a green run that
//! asserted nothing. These tests pin the two guards that make that impossible: a missing
//! precondition fails under `CHROME_AGENT_REQUIRE_CHROME`, and a missing fixture always does.

mod common;

#[test]
fn a_skip_is_a_failure_when_a_browser_run_is_required() {
    let panicked = std::panic::catch_unwind(|| common::unavailable_with(true, "Chrome not found"));
    let payload = panicked.expect_err("a required browser run must not be skippable");
    let message = payload
        .downcast_ref::<String>()
        .map_or_else(|| String::from("<non-string panic>"), Clone::clone);
    assert!(
        message.contains("Chrome not found"),
        "the panic must name the unmet precondition, got: {message}"
    );
    assert!(
        message.contains(common::REQUIRE_ENV),
        "the panic must name the variable that made it fatal, got: {message}"
    );
}

#[test]
fn a_skip_stays_a_skip_when_no_browser_run_is_required() {
    assert!(
        !common::unavailable_with(false, "Chrome not found"),
        "without the variable a missing Chrome skips, and the caller returns early"
    );
}

#[test]
fn the_require_flag_reads_its_value_not_just_its_presence() {
    // The env lookup itself is not exercised here: the crate forbids `unsafe`, and
    // `set_var` is unsafe in edition 2024. What is worth pinning is the parse — an
    // exported-but-disabled `CHROME_AGENT_REQUIRE_CHROME=0` must not make CI fatal.
    assert!(!common::require_from(None), "unset means skips are allowed");
    assert!(common::require_from(Some("1")), "set means skips are fatal");
    assert!(!common::require_from(Some("0")), "an explicit 0 opts back out");
    assert!(!common::require_from(Some("")), "an empty value opts back out");
}

#[test]
#[should_panic(expected = "fixture does not exist")]
fn a_missing_fixture_is_never_a_usable_url() {
    // Before this guard, `file://…/deleted.html` produced an error page and every test that
    // loaded it returned early on a "goto failed" skip.
    let _ = common::fixture_url("this_fixture_was_deleted.html");
}

#[test]
fn a_present_fixture_resolves_to_a_file_url() {
    let url = common::fixture_url("press_keys.html");
    assert!(url.starts_with("file:///"), "got {url}");
    assert!(url.ends_with("/tests/fixtures/press_keys.html"), "got {url}");
}

// ---------------------------------------------------------------------------
// One isolation mechanism, enforced on the sources
// ---------------------------------------------------------------------------

/// Every Rust source of the suite and of the crate, so a file added later is scanned for free.
fn sources() -> Vec<(String, String)> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    for dir in ["tests", "src", "src/cdp", "src/commands"] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "rs") {
                let name = format!("{dir}/{}", path.file_name().unwrap_or_default().to_string_lossy());
                out.push((name, std::fs::read_to_string(&path).unwrap_or_default()));
            }
        }
    }
    assert!(out.len() > 30, "the scan found almost nothing, so it is proving nothing");
    out
}

/// This file, which spells out every pattern the rules below forbid and so cannot be scanned
/// for them.
const SCANNER: &str = "tests/harness_tests.rs";

/// Whether line `n` is exempt: the marker is on it, or in the comment written directly above
/// it. A reason on its own line is how a comment is normally written, and a rule that only
/// accepted the marker inline would push authors to write worse comments to satisfy it.
fn exempt(lines: &[&str], n: usize) -> bool {
    let from = n.saturating_sub(3);
    lines[from..=n].iter().any(|line| line.contains("isolation-exempt:"))
}

/// A hard-coded `--browser` name is a browser two concurrent runs share.
///
/// Measured before this rule existed, by running the whole suite twice at once from two
/// directories: `action_report_tests` died with `transport: transport closed` on the browser
/// named `pipe-bootstrap`, and `proxy_tests` timed out on `test-managed-proxy`. Neither had a
/// bug; each had a name.
#[test]
fn no_test_hard_codes_a_browser_name() {
    let mut offenders = Vec::new();
    for (name, text) in sources() {
        if !name.starts_with("tests/") || name == SCANNER {
            continue;
        }
        let lines: Vec<&str> = text.lines().collect();
        for (n, line) in lines.iter().enumerate() {
            if exempt(&lines, n) {
                continue;
            }
            // Two spellings of one name. The first is `"--browser", "literal"`. The second is a
            // literal bound to a variable and passed on, which is what six suites merged after
            // this rule was written actually did — `let browser = "test-webmcp-list";` — and it
            // walked straight past a rule that only looked at the flag. It was caught by the
            // one-implementation rule below instead, and only because each of them also carried
            // its own guard; a file that hard-coded a name and used the shared guard would have
            // passed both. That is the hole this second clause closes.
            let after_flag = line
                .split("\"--browser\",")
                .nth(1)
                .is_some_and(|rest| rest.trim_start().starts_with('"'));
            let bound_to_a_literal = line.trim_start().starts_with("let browser")
                && line.contains('=')
                && line.split('=').nth(1).is_some_and(|rest| rest.trim_start().starts_with('"'));
            if after_flag || bound_to_a_literal {
                offenders.push(format!("{name}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a browser name belongs to `common::TestBrowser`, which makes it unique per process \
         AND per test:\n{}",
        offenders.join("\n")
    );
}

/// One mechanism, not several. A second implementation is how the first one stops being true.
#[test]
fn the_uniqueness_rule_has_exactly_one_implementation() {
    let mut offenders = Vec::new();
    for (name, text) in sources() {
        if !name.starts_with("tests/") || name == "tests/common/mod.rs" || name == SCANNER {
            continue;
        }
        let lines: Vec<&str> = text.lines().collect();
        for (n, line) in lines.iter().enumerate() {
            if exempt(&lines, n) {
                continue;
            }
            let hand_rolled = line.contains("std::process::id()")
                || line.contains("struct TestBrowser")
                || line.contains("struct BrowserGuard");
            if hand_rolled {
                offenders.push(format!("{name}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "use `common::TestBrowser` / `common::unique_name` / `common::temp_path` rather than \
         spelling the rule again:\n{}",
        offenders.join("\n")
    );
}

/// A fixed path under the temp directory is the same collision as a fixed browser name, and it
/// bit a unit test rather than an integration one: two runs of `src/browser.rs`'s
/// `read_devtools_active_port_parses_correctly` shared `/tmp/chrome-agent_test_devtools`, and
/// one `remove_dir_all` deleted the file the other was about to read (`left: None`).
#[test]
fn no_source_writes_to_a_fixed_temporary_path() {
    let mut offenders = Vec::new();
    for (name, text) in sources() {
        if name == SCANNER {
            continue;
        }
        let lines: Vec<&str> = text.lines().collect();
        for (n, line) in lines.iter().enumerate() {
            if exempt(&lines, n) {
                continue;
            }
            // The dash matters: every suite's `binary()` helper pushes `"chrome-agent"`, which
            // is the executable, not a file two runs would share.
            if line.contains("temp_dir().join(\"") || line.contains("path.push(\"chrome-agent-") {
                offenders.push(format!("{name}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a temporary path a second process can guess is a shared file:\n{}",
        offenders.join("\n")
    );
}

/// The name has to be unique per call, not merely per process: the harness runs tests on
/// parallel threads, so one pid covers several tests at once.
#[test]
fn a_unique_name_differs_from_the_last_one_and_carries_the_pid() {
    let first = common::unique_name("iso");
    let second = common::unique_name("iso");
    assert_ne!(first, second, "two names in one process collided: {first}");
    let pid = std::process::id().to_string();
    assert!(first.starts_with("iso-"), "{first}");
    assert!(first.contains(&pid), "the pid separates concurrent runs: {first}");
    // A path built from it is unique too, and lands in the temp directory rather than the repo.
    let path = common::temp_path("iso", "jsonl");
    assert!(path.starts_with(std::env::temp_dir()), "{}", path.display());
    assert!(path.to_string_lossy().ends_with(".jsonl"), "{}", path.display());
    assert_ne!(path, common::temp_path("iso", "jsonl"));
}
