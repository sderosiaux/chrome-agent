use std::path::PathBuf;
use std::process::Command;

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

fn run_cli(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(binary()).args(args).output().expect("Failed to run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
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
        let _ = run_cli(&["--browser", self.0, "close"]);
    }
}

fn goto(browser: &str, fixture: &str) -> bool {
    let url = fixture_url(fixture);
    let (_, stderr, code) = run_cli(&["--browser", browser, "goto", &url]);
    if code != 0 {
        eprintln!("SKIP: goto failed for {fixture}: {stderr}");
        return false;
    }
    true
}

fn diff_json(browser: &str) -> Option<serde_json::Value> {
    let (stdout, _, _) = run_cli(&["--browser", browser, "--json", "diff"]);
    serde_json::from_str(&stdout).ok()
}

// ─── diff across a navigation ───
//
// backendNodeId counters overlap between documents, so a naive line-by-line uid match
// pairs an element on page A with an unrelated element carrying the same uid on page B
// and reports it as "changed". Measured on real sites, that produced 328 bogus "~" lines
// and cost more tokens than simply re-inspecting the destination page.

#[test]
fn diff_reports_document_change_instead_of_pairing_unrelated_uids() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let b = TestBrowser::new("diff-nav");
    if !goto(b.name(), "extract_cards.html") {
        return;
    }
    let (_, _, code) = run_cli(&["--browser", b.name(), "inspect"]);
    assert_eq!(code, 0, "inspect should succeed");

    if !goto(b.name(), "extract_hn_subtext.html") {
        return;
    }
    let json = diff_json(b.name()).expect("diff should emit JSON");

    assert_eq!(
        json["document_changed"], true,
        "diff after navigating to a different document must say so, got {json}"
    );
    assert_eq!(json["changed"], 0, "no element can be 'changed' across two different documents, got {json}");
}

#[test]
fn diff_on_the_same_document_still_reports_changes() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let b = TestBrowser::new("diff-same");
    if !goto(b.name(), "extract_cards.html") {
        return;
    }
    let (_, _, code) = run_cli(&["--browser", b.name(), "inspect"]);
    assert_eq!(code, 0, "inspect should succeed");

    // Mutate the live document without navigating.
    let (_, _, code) = run_cli(&[
        "--browser",
        b.name(),
        "eval",
        "document.body.insertAdjacentHTML('beforeend', '<h2>Freshly added heading</h2>'); 1",
    ]);
    assert_eq!(code, 0, "eval should succeed");

    let json = diff_json(b.name()).expect("diff should emit JSON");
    assert_eq!(json["document_changed"], false, "same document, got {json}");
    assert!(
        json["added"].as_u64().unwrap_or(0) >= 1,
        "the injected heading should show up as added, got {json}"
    );
}
