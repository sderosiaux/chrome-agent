use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

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
impl TestBrowser {
    const fn new(name: &'static str) -> Self {
        Self(name)
    }
    const fn name(&self) -> &str {
        self.0
    }
}
impl Drop for TestBrowser {
    fn drop(&mut self) {
        let _ = run_cli(&["--browser", self.0, "close", "--purge"]);
    }
}

/// The settle probe waits for the DOM to go quiet. A page that never goes quiet must not
/// hold the command open: measured against the previous implementation, whose deadline was
/// cleared by the first mutation, this never returned at all.
#[test]
fn goto_returns_on_a_page_that_never_stops_mutating() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let b = TestBrowser::new("settle-ticker");
    let url = fixture_url("goto_ticker.html");
    // Warm the browser so the measurement covers navigation, not Chrome startup.
    let _ = run_cli(&["--browser", b.name(), "goto", &fixture_url("extract_cards.html")]);

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
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let b = TestBrowser::new("settle-static");
    let url = fixture_url("extract_cards.html");
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
