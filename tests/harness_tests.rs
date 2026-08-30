//! Tests for the test harness itself.
//!
//! Two guards against a green run that asserted nothing: a missing precondition fails under
//! `CHROME_AGENT_REQUIRE_CHROME`, and a missing fixture always fails.
//! Also holds the source scanners that enforce the one isolation mechanism.

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
    // The env lookup is not exercised: the crate forbids `unsafe` and `set_var` is unsafe in
    // edition 2024. Only the parse is pinned.
    assert!(!common::require_from(None), "unset means skips are allowed");
    assert!(common::require_from(Some("1")), "set means skips are fatal");
    assert!(!common::require_from(Some("0")), "an explicit 0 opts back out");
    assert!(!common::require_from(Some("")), "an empty value opts back out");
}

#[test]
#[should_panic(expected = "fixture does not exist")]
fn a_missing_fixture_is_never_a_usable_url() {
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

/// Every `.rs` file under `dir`, at any depth, pushed as `(path relative to the crate root,
/// contents)`. Recursive because a hard-coded list of directories stops being true the next time
/// a module is split out: `src/hints/` was created by one and went unscanned, and so would any
/// directory added tomorrow.
fn collect_rs(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let name = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((name, std::fs::read_to_string(&path).unwrap_or_default()));
        }
    }
}

/// Every Rust source of the suite and of the crate, so a file added later — in a directory
/// invented later — is scanned for free.
fn sources() -> Vec<(String, String)> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    for dir in ["tests", "src"] {
        collect_rs(&root, &root.join(dir), &mut out);
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    // The repo holds ~130 `.rs` files across the two trees. A walk that broke would find far
    // fewer, and the point of the number is to make that failure loud rather than green.
    assert!(
        out.len() > 100,
        "the scan found {} sources, so the walk is broken and it is proving nothing",
        out.len()
    );
    out
}

/// This file, which spells out every pattern the rules below forbid and so cannot be scanned
/// for them.
const SCANNER: &str = "tests/harness_tests.rs";

/// Whether line `n` is exempt: the marker is on it, or on one of the 3 lines above it.
fn exempt(lines: &[&str], n: usize) -> bool {
    let from = n.saturating_sub(3);
    lines[from..=n].iter().any(|line| line.contains("isolation-exempt:"))
}

/// A hard-coded `--browser` name is a browser two concurrent runs share.
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
            // Two spellings of one name: `"--browser", "literal"`, and a literal bound to a
            // variable then passed on — `let browser = "test-webmcp-list";`.
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

/// A fixed path under the temp directory is the same collision as a fixed browser name.
/// Scans `src/` too, since a unit test hit it first.
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
            // The dash matters: `common::binary()` pushes `"chrome-agent"`, which is the
            // executable, not a file two runs would share.
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

/// The binary under test is resolved once, in `common::binary()`.
///
/// Forty-five suites carried a private copy, and they were not equivalent: most popped twice
/// unconditionally, one popped `deps/` only when it was there. That is the one thing every
/// suite depends on, and it had forty-five spellings — the same defect the rules above catch
/// for browser names and temp paths.
#[test]
fn no_test_resolves_the_binary_under_test_itself() {
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
            // The name AND the mechanism: a copy called something else still resolves the
            // path from the test executable, which is what may not be spelled twice.
            let named = line.trim_start().starts_with("fn binary(");
            let hand_rolled = line.contains("current_exe()");
            if named || hand_rolled {
                offenders.push(format!("{name}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "the binary under test comes from `common::binary()`, which handles the `deps/` \
         directory the copies disagreed about:\n{}",
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
    let path = common::temp_path("iso", "jsonl");
    assert!(path.starts_with(std::env::temp_dir()), "{}", path.display());
    assert!(path.to_string_lossy().ends_with(".jsonl"), "{}", path.display());
    assert_ne!(path, common::temp_path("iso", "jsonl"));
}
