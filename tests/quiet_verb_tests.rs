//! The four verbs no suite drove end to end: `console`, `history`, `pdf`, `tabs`.
//!
//! `console` had unit tests for its formatter and one integration test for the case where
//! nothing was listening — nothing proved it reports what a page actually logged, or that
//! `--level` and `--clear` do what their help says. `history` and `pdf` had no test at all;
//! `tabs` only appeared as a smoke check that it exits 0.
//!
//! `pdf --filename` reduces its argument to a basename under `~/.chrome-agent/tmp`, so the
//! only truthful answer to "where is my file" is the `path` in the response. That is the thing
//! pinned here: the reported path is where the bytes are.

mod common;

use std::process::Command;

use common::{TestBrowser, binary, browser_ready, temp_path, unique_name};

/// Run one CLI invocation, returning `(stdout, stderr, exit code)`.
fn run(browser: &str, args: &[&str]) -> (String, String, i32) {
    let mut full = vec!["--browser", browser];
    full.extend_from_slice(args);
    let out = Command::new(binary())
        .args(&full)
        .output()
        .expect("run chrome-agent");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn json_of(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}"))
}

/// A page this test owns, at a path no other run can guess. Returns `(path, file:// url)`.
fn own_page(label: &str, body: &str) -> (std::path::PathBuf, String) {
    let path = temp_path(label, "html");
    std::fs::write(&path, body).expect("write page");
    let url = format!("file://{}", path.display());
    (path, url)
}

const NOISY_PAGE: &str = "<!doctype html><title>Noisy page</title>\
<script>console.log('log-line');console.warn('warn-line');console.error('error-line');</script>\
<p>content</p>";

/// What the page logged, at the level it logged it — and `--level` as a selection, not a cap.
#[test]
fn console_reports_each_level_and_filters_to_one() {
    if !browser_ready() {
        return;
    }
    let browser = TestBrowser::new("quiet-console-levels");
    let (path, url) = own_page("quiet-console-levels", NOISY_PAGE);
    let (_, stderr, code) = run(browser.name(), &["--json", "goto", &url]);
    assert_eq!(code, 0, "{stderr}");

    let (stdout, _, code) = run(browser.name(), &["console", "--json"]);
    assert_eq!(code, 0);
    let all = json_of(&stdout);
    assert_eq!(all["installed"], serde_json::json!(true), "{all}");
    let messages = all["messages"].as_array().expect("messages array").clone();
    let seen: Vec<(&str, &str)> = messages
        .iter()
        .map(|m| {
            (
                m["level"].as_str().unwrap_or(""),
                m["message"].as_str().unwrap_or(""),
            )
        })
        .collect();
    for want in [
        ("log", "log-line"),
        ("warn", "warn-line"),
        ("error", "error-line"),
    ] {
        assert!(seen.contains(&want), "{want:?} missing from {seen:?}");
    }

    let (stdout, _, code) = run(browser.name(), &["console", "--level", "error", "--json"]);
    assert_eq!(code, 0);
    let errors = json_of(&stdout);
    let levels: Vec<&str> = errors["messages"]
        .as_array()
        .expect("messages array")
        .iter()
        .map(|m| m["level"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(levels, vec!["error"], "--level selected {levels:?}");

    let _ = std::fs::remove_file(path);
}

/// `--clear` reads first and clears after: the messages it drops are the ones it just returned,
/// so a caller never loses a line by asking for both in one call.
#[test]
fn console_clear_returns_what_it_removes() {
    if !browser_ready() {
        return;
    }
    let browser = TestBrowser::new("quiet-console-clear");
    let (path, url) = own_page("quiet-console-clear", NOISY_PAGE);
    let (_, stderr, code) = run(browser.name(), &["--json", "goto", &url]);
    assert_eq!(code, 0, "{stderr}");

    let (stdout, _, _) = run(browser.name(), &["console", "--clear", "--json"]);
    let cleared = json_of(&stdout);
    assert_eq!(
        cleared["messages"].as_array().map(Vec::len),
        Some(3),
        "the clearing read still answers with what was there: {cleared}"
    );

    let (stdout, _, _) = run(browser.name(), &["console", "--json"]);
    let after = json_of(&stdout);
    assert_eq!(after["messages"], serde_json::json!([]), "{after}");
    assert_eq!(
        after["installed"],
        serde_json::json!(true),
        "clearing the messages must not read as a page nothing was listening to: {after}"
    );

    let _ = std::fs::remove_file(path);
}

/// A navigation reaches `history`, and a filter that matches nothing is an empty answer rather
/// than an error — the two are different facts and both exit 0.
#[test]
fn history_records_the_navigation_and_an_empty_filter_result_is_not_a_failure() {
    if !browser_ready() {
        return;
    }
    let browser = TestBrowser::new("quiet-history");
    let (path, url) = own_page(
        "quiet-history",
        "<!doctype html><title>Recorded page</title><p>x</p>",
    );
    // The file name is unique to this test, so the filter selects only what this test wrote —
    // `history.jsonl` is one file for every browser on the machine.
    let token = path
        .file_stem()
        .expect("stem")
        .to_string_lossy()
        .into_owned();

    let (_, stderr, code) = run(browser.name(), &["--json", "goto", &url]);
    assert_eq!(code, 0, "{stderr}");

    let (stdout, _, code) = run(browser.name(), &["history", "--filter", &token, "--json"]);
    assert_eq!(code, 0);
    let value = json_of(&stdout);
    let entries = value["entries"].as_array().expect("entries").clone();
    assert_eq!(entries.len(), 1, "{value}");
    assert_eq!(entries[0]["url"], serde_json::json!(url), "{value}");
    assert_eq!(
        entries[0]["title"],
        serde_json::json!("Recorded page"),
        "{value}"
    );
    assert_eq!(entries[0]["page"], serde_json::json!("default"), "{value}");

    let (text, _, code) = run(browser.name(), &["history", "--filter", &token]);
    assert_eq!(code, 0);
    assert!(
        text.contains(&url) && text.contains("Recorded page"),
        "{text:?}"
    );

    let miss = format!("{token}-nothing-matches-this");
    let (stdout, _, code) = run(browser.name(), &["history", "--filter", &miss, "--json"]);
    assert_eq!(code, 0, "an empty history is not an error");
    assert_eq!(json_of(&stdout)["entries"], serde_json::json!([]));
    let (text, _, code) = run(browser.name(), &["history", "--filter", &miss]);
    assert_eq!(code, 0);
    assert!(text.contains("No history entries found."), "{text:?}");

    let _ = std::fs::remove_file(path);
}

/// The `path` in the response is where the bytes are — including when `--filename` named a
/// directory that does not exist, which is reduced to its basename rather than refused.
#[test]
fn pdf_writes_a_real_pdf_at_the_path_it_reports() {
    if !browser_ready() {
        return;
    }
    let browser = TestBrowser::new("quiet-pdf");
    let (page, url) = own_page(
        "quiet-pdf",
        "<!doctype html><title>Printable</title><p>ink</p>",
    );
    let (_, stderr, code) = run(browser.name(), &["--json", "goto", &url]);
    assert_eq!(code, 0, "{stderr}");

    let name = unique_name("quiet-pdf-out");
    let asked = format!("/no/such/directory/{name}");
    let (stdout, _, code) = run(browser.name(), &["pdf", "--filename", &asked, "--json"]);
    assert_eq!(code, 0, "{stdout}");
    let value = json_of(&stdout);
    let written = std::path::PathBuf::from(
        value["path"]
            .as_str()
            .unwrap_or_else(|| panic!("no path: {value}")),
    );

    assert_ne!(
        written,
        std::path::PathBuf::from(&asked),
        "the directory was not honoured"
    );
    assert_eq!(
        written
            .file_name()
            .map(std::ffi::OsStr::to_string_lossy)
            .as_deref(),
        Some(format!("{name}.pdf").as_str()),
        "the basename survives and the extension is forced: {}",
        written.display()
    );
    let bytes = std::fs::read(&written).unwrap_or_else(|e| {
        panic!(
            "the reported path holds no file ({e}): {}",
            written.display()
        )
    });
    assert!(
        bytes.starts_with(b"%PDF-"),
        "not a PDF: {:?}",
        &bytes[..bytes.len().min(8)]
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&written)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "a printed page is private: {mode:o}");
    }

    let _ = std::fs::remove_file(&written);
    let _ = std::fs::remove_file(page);
}

/// `tabs` names the managed page by the name this tool gave it, which is what makes `--page`
/// addressable at all.
#[test]
fn tabs_names_the_managed_page_and_its_url() {
    if !browser_ready() {
        return;
    }
    let browser = TestBrowser::new("quiet-tabs");
    let (path, url) = own_page(
        "quiet-tabs",
        "<!doctype html><title>One tab</title><p>x</p>",
    );
    let (_, stderr, code) = run(browser.name(), &["--json", "goto", &url]);
    assert_eq!(code, 0, "{stderr}");

    let (stdout, _, code) = run(browser.name(), &["tabs", "--json"]);
    assert_eq!(code, 0);
    let value = json_of(&stdout);
    let tabs = value["tabs"].as_array().expect("tabs array").clone();
    let mine = tabs
        .iter()
        .find(|t| t["url"] == serde_json::json!(url))
        .unwrap_or_else(|| panic!("the page this test opened is not listed: {value}"));
    assert_eq!(mine["page"], serde_json::json!("default"), "{value}");
    assert_eq!(mine["title"], serde_json::json!("One tab"), "{value}");
    assert!(
        mine["id"].as_str().is_some_and(|id| !id.is_empty()),
        "a tab with no target id cannot be addressed: {value}"
    );

    let _ = std::fs::remove_file(path);
}
