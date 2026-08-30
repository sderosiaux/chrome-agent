use std::process::Command;
use std::time::{Duration, Instant};

mod common;
use common::TestBrowser;

fn run_cli(args: &[&str]) -> (String, i32) {
    let output = Command::new(common::binary())
        .args(args)
        .output()
        .expect("Failed to run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

/// The settle probe has a ceiling, so a page that never goes quiet cannot hold `goto` open.
#[test]
fn goto_returns_on_a_page_that_never_stops_mutating() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("settle-ticker");
    let url = common::fixture_url("goto_ticker.html");
    // Warm the browser so the measurement covers navigation, not Chrome startup.
    let _ = run_cli(&[
        "--browser",
        b.name(),
        "goto",
        &common::fixture_url("extract_cards.html"),
    ]);

    let started = Instant::now();
    let (_, code) = run_cli(&["--browser", b.name(), "goto", &url]);
    let elapsed = started.elapsed();

    assert_eq!(code, 0, "goto should succeed on a mutating page");
    assert!(
        elapsed < Duration::from_secs(15),
        "goto took {elapsed:?} on a continuously mutating page; the settle probe has no ceiling"
    );
}

/// The quiet window starts immediately, so a static page pays no flat wait.
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
