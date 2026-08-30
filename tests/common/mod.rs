//! Shared test harness.
//!
//! Every browser test in this suite used to open with the same twelve lines: find Chrome,
//! `eprintln!("SKIP: …")`, `return`. A bare `return` inside `#[test]` is a pass, so a machine
//! without Chrome — or a fixture deleted by mistake — turned ~57 of 68 tests into green
//! no-ops. This module makes that failure loud when it matters and quiet when it doesn't:
//! locally a missing Chrome still skips, but with `CHROME_AGENT_REQUIRE_CHROME=1` (set in CI)
//! the same condition panics.

#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;

/// Environment variable CI sets to turn every skip into a failure.
pub const REQUIRE_ENV: &str = "CHROME_AGENT_REQUIRE_CHROME";

/// Whether a raw `REQUIRE_ENV` value demands a real browser run. Split from the lookup so
/// the decision is testable without mutating the process environment (`set_var` is unsafe
/// in edition 2024 and this crate forbids `unsafe`).
#[must_use]
pub fn require_from(value: Option<&str>) -> bool {
    matches!(value, Some(v) if v != "0" && !v.is_empty())
}

/// Whether the caller demands a real browser run (CI) or tolerates a skip (a laptop
/// without Chrome).
#[must_use]
pub fn require_chrome() -> bool {
    require_from(std::env::var(REQUIRE_ENV).ok().as_deref())
}

/// Report a precondition the test cannot meet. Returns `false` (the caller then returns and
/// the test passes as a skip) unless a browser run was required, in which case it panics.
///
/// The pure form is `unavailable_with`; this is the environment-reading wrapper.
pub fn unavailable(reason: &str) -> bool {
    unavailable_with(require_chrome(), reason)
}

/// `unavailable` with the policy passed in.
///
/// # Panics
/// When `require` is set — that is the point: a CI run that silently skips every browser
/// test reports the same green as one that ran them.
pub fn unavailable_with(require: bool, reason: &str) -> bool {
    assert!(
        !require,
        "{REQUIRE_ENV} is set, so this test may not be skipped: {reason}"
    );
    eprintln!("SKIP: {reason}");
    false
}

/// True when a Chrome binary exists on this machine.
#[must_use]
pub fn chrome_available() -> bool {
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

/// `true` when the test may proceed. Skips (or fails, under `REQUIRE_ENV`) otherwise.
#[must_use]
pub fn browser_ready() -> bool {
    if chrome_available() {
        return true;
    }
    unavailable("Chrome not found")
}

/// Absolute path of a fixture, asserted to exist.
///
/// # Panics
/// When the fixture is missing. Deleting a fixture used to leave the tests that load it
/// green: `file://…/gone.html` navigates to an error page and every later assertion was
/// guarded by an early return.
#[must_use]
pub fn fixture_path(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures");
    path.push(name);
    assert!(
        path.exists(),
        "fixture does not exist: {} — a test that navigates to a missing file:// URL cannot fail honestly",
        path.display()
    );
    path
}

/// `file://` URL of a fixture, asserted to exist.
#[must_use]
pub fn fixture_url(name: &str) -> String {
    format!("file://{}", fixture_path(name).display())
}

// ---------------------------------------------------------------------------
// Isolation between concurrent test processes
// ---------------------------------------------------------------------------

/// The binary under test, resolved from the test executable's own location.
///
/// Every suite had its own copy of this, and every copy was identical. It lives here now for
/// the same reason [`TestBrowser`] does: the thing that must not drift is the thing that
/// several files spell the same way.
///
/// One trap it cannot remove, so it is written down instead: `cargo test --test X` does not
/// always rebuild this binary. An A/B that edits `src/` and re-runs one suite can measure the
/// PREVIOUS build and read as a regression that is not there. Run `cargo build` between the
/// two states.
#[must_use]
pub fn binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("test binary path");
    path.pop(); // deps/
    path.pop();
    path.push("chrome-agent");
    path
}

/// A name no other process is using, and no later run of this one will reuse.
///
/// Two ingredients, and both are needed. The pid separates concurrent processes — two
/// `cargo test` runs, which is the normal regime on a machine with several worktrees. The
/// counter separates tests INSIDE one process: the harness runs them on parallel threads, so
/// two tests that happen to pass the same label would otherwise drive one browser, and the
/// first to finish would `close --purge` it under the second.
///
/// Not a random number: a name that appears in a failure message is worth being able to find
/// again in `chrome-agent status` while the run is still going.
#[must_use]
pub fn unique_name(label: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{label}-{}-{n}", std::process::id())
}

/// A browser this test owns, closed and purged when the test ends — including on panic.
///
/// This is the ONE mechanism. Twenty-four suites carried a byte-identical copy of it and five
/// did not, and those five were the ones that failed: a fixed `--browser` name means two
/// concurrent runs drive ONE browser, and the first to finish closes it under the second.
/// Measured, before this existed, by running the whole suite twice at once from two
/// directories: `action_report_tests` died with `transport: transport closed` on the browser
/// named `pipe-bootstrap`, and `proxy_tests` timed out on `test-managed-proxy`. Both had
/// hard-coded their name; neither had a bug.
///
/// RAII, and that matters as much as the name: a plain `close` statement at the end of a
/// helper is skipped when an assertion panics, which leaks a Chrome and a ~14 MB profile
/// directory per failure. `Drop` runs on the unwind.
pub struct TestBrowser(String);

impl TestBrowser {
    #[must_use]
    pub fn new(label: &str) -> Self {
        Self(unique_name(label))
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.0
    }
}

impl Drop for TestBrowser {
    fn drop(&mut self) {
        let _ = std::process::Command::new(binary())
            .args(["--browser", &self.0, "close", "--purge"])
            .output();
    }
}

/// A temporary file path this test owns.
///
/// The same rule as the browser name, for the same reason: two suites wrote
/// `/tmp/chrome-agent-<fixed>.jsonl` and `/tmp/chrome-agent-dblclick-selector-test.html`, so a
/// concurrent run could rewrite or unlink the file between another run's write and its read.
#[must_use]
pub fn temp_path(label: &str, extension: &str) -> PathBuf {
    std::env::temp_dir().join(format!("chrome-agent-{}.{extension}", unique_name(label)))
}
