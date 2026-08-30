//! Shared test harness: Chrome/fixture preconditions, per-test isolation.
//!
//! A missing Chrome skips locally. With `CHROME_AGENT_REQUIRE_CHROME=1` (set in CI) the same
//! condition panics instead, so a green run cannot mean "nothing ran".

#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;

/// Environment variable CI sets to turn every skip into a failure.
pub const REQUIRE_ENV: &str = "CHROME_AGENT_REQUIRE_CHROME";

/// Whether a raw `REQUIRE_ENV` value demands a real browser run. Split from the lookup so it
/// is testable without mutating the environment.
#[must_use]
pub fn require_from(value: Option<&str>) -> bool {
    matches!(value, Some(v) if v != "0" && !v.is_empty())
}

#[must_use]
pub fn require_chrome() -> bool {
    require_from(std::env::var(REQUIRE_ENV).ok().as_deref())
}

/// Report a precondition the test cannot meet: `false` so the caller returns early and the
/// test passes as a skip, unless a browser run was required, in which case it panics.
pub fn unavailable(reason: &str) -> bool {
    unavailable_with(require_chrome(), reason)
}

/// `unavailable` with the policy passed in.
///
/// # Panics
/// When `require` is set.
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
        if Command::new("which")
            .arg(candidate)
            .output()
            .is_ok_and(|o| o.status.success())
        {
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
/// When the fixture is missing: a `file://` URL to a deleted one loads an error page.
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

// Isolation between concurrent test processes.

/// The binary under test, resolved from the test executable's own location.
///
/// The one resolver: every suite used to carry a copy, and they disagreed — most popped twice
/// unconditionally, one popped `deps/` only when it was there. The conditional form is the
/// correct one and is what survives here: a test executable cargo places directly in
/// `target/<profile>/` rather than in `target/<profile>/deps/` walks one directory too far
/// under the unconditional pop, and then names a `chrome-agent` that does not exist.
///
/// NOTE: `cargo test --test X` does not always rebuild it. Run `cargo build` between two
/// states of `src/` or the suite measures the previous build.
#[must_use]
pub fn binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("test binary path");
    path.pop(); // the test executable's own directory
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("chrome-agent");
    path
}

/// A name no other process is using, and no later run of this one will reuse. The pid
/// separates concurrent `cargo test` processes; the counter separates tests inside one, which
/// the harness runs on parallel threads. Readable so it can be found in `chrome-agent status`.
#[must_use]
pub fn unique_name(label: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{label}-{}-{n}", std::process::id())
}

/// A browser this test owns, closed and purged on `Drop`, including on panic.
///
/// The one mechanism for browser isolation: a hard-coded `--browser` name lets two concurrent
/// runs drive one browser. `Drop` rather than a trailing `close`, which a panicking assertion
/// skips, leaking a Chrome and its ~14 MB profile.
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

/// A temporary file path this test owns. Same rule as the browser name: a fixed path lets a
/// concurrent run rewrite or unlink the file between another run's write and its read.
#[must_use]
pub fn temp_path(label: &str, extension: &str) -> PathBuf {
    std::env::temp_dir().join(format!("chrome-agent-{}.{extension}", unique_name(label)))
}
