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
