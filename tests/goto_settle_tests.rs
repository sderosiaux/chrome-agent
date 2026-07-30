use std::process::Command;
use std::time::{Duration, Instant};

mod common;

fn binary() -> String {
    let mut path = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
    path.push("chrome-agent");
    path.to_string_lossy().into_owned()
}

fn run_cli(args: &[&str]) -> (String, i32) {
    let output = Command::new(binary()).args(args).output().expect("Failed to run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

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

/// The settle probe waits for the DOM to go quiet. A page that never goes quiet must not
/// hold the command open: measured against the previous implementation, whose deadline was
/// cleared by the first mutation, this never returned at all.
#[test]
fn goto_returns_on_a_page_that_never_stops_mutating() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("settle-ticker");
    let url = common::fixture_url("goto_ticker.html");
    // Warm the browser so the measurement covers navigation, not Chrome startup.
    let _ = run_cli(&["--browser", b.name(), "goto", &common::fixture_url("extract_cards.html")]);

    let started = Instant::now();
    let (_, code) = run_cli(&["--browser", b.name(), "goto", &url]);
    let elapsed = started.elapsed();

    assert_eq!(code, 0, "goto should succeed on a mutating page");
    assert!(
        elapsed < Duration::from_secs(15),
        "goto took {elapsed:?} on a continuously mutating page; the settle probe has no ceiling"
    );
}

/// A page where nothing moves should not be charged for waiting to find that out.
#[test]
fn goto_does_not_wait_the_full_budget_on_a_static_page() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("settle-static");
    let url = common::fixture_url("extract_cards.html");
    let _ = run_cli(&["--browser", b.name(), "goto", &url]);

    let started = Instant::now();
    let (_, code) = run_cli(&["--browser", b.name(), "goto", &url]);
    let elapsed = started.elapsed();

    assert_eq!(code, 0, "goto should succeed");
    assert!(
        elapsed < Duration::from_secs(2),
        "goto took {elapsed:?} on a static page; the quiet window should start immediately \
         rather than only after the first mutation"
    );
}
