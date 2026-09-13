//! Ordinary scripts prove pagination completeness and reconcile a possibly committed write.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

mod common;
use common::TestBrowser;

fn example(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/verified_workflows")
        .join(file)
}

struct Server {
    child: Child,
    url: String,
    account: String,
}

impl Server {
    fn start(scenario: &str, account: &str) -> Self {
        let child = Command::new("python3")
            .arg(example("demo_server.py"))
            .args(["--scenario", scenario, "--account", account])
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut server = Self {
            child,
            url: String::new(),
            account: account.into(),
        };
        let stdout = server.child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = tx.send(line);
        });
        let line = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("server readiness deadline");
        let ready: Value = serde_json::from_str(&line).unwrap();
        server.url = ready["url"].as_str().unwrap().to_string();
        server
    }

    fn state(&self) -> Value {
        let address = self.url.strip_prefix("http://").unwrap();
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write!(stream, "GET /state HTTP/1.0\r\nHost: {address}\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = common::temp_path("workflow-files", "dir");
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(server: &Server, browser: &TestBrowser, script: &str, args: &[&str]) -> (Value, i32) {
    let mut child = Command::new("python3")
        .arg(example(script))
        .args([
            "--url",
            &server.url,
            "--account",
            &server.account,
            "--browser",
            browser.name(),
            "--timeout",
            "2",
        ])
        .arg("--binary")
        .arg(common::binary())
        .args(args)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        let _ = tx.send(bytes);
    });
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!("workflow exceeded test deadline: {script}, {output:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    let bytes = rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let report =
        serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{e}: {bytes:?}, {output:?}"));
    (report, output.status.code().unwrap_or(-1))
}

fn draft(
    server: &Server,
    browser: &TestBrowser,
    journal: &std::path::Path,
    extra: &[&str],
) -> (Value, i32) {
    let journal = journal.to_string_lossy();
    let mut args = vec![
        "--reference",
        "run-1",
        "--title",
        "Rapport été \"Q3\" \\ {{literal}}",
        "--amount",
        "12.50",
        "--journal",
        &journal,
    ];
    args.extend_from_slice(extra);
    run(server, browser, "create_draft.py", &args)
}

#[test]
fn collection_returns_the_expected_identities_across_pages_and_inputs() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("workflow-collection-inputs");
    for account in ["acme", "beta"] {
        let server = Server::start("normal", account);
        for (period, ids, amounts) in [
            (
                "2026-08",
                vec!["INV-801", "INV-802", "INV-803", "INV-804", "INV-805"],
                vec![1250, 725, 3100, 450, 999],
            ),
            (
                "2026-09",
                vec!["INV-901", "INV-902", "INV-903"],
                vec![4200, 350, 875],
            ),
        ] {
            let (report, code) = run(
                &server,
                &browser,
                "collect_invoices.py",
                &["--period", period],
            );
            assert_eq!(code, 0, "{report}");
            assert_eq!(report["complete"], true);
            let rows = report["outputs"]["items"].as_array().unwrap();
            assert_eq!(
                rows.iter()
                    .map(|r| r["id"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                ids
            );
            for (row, amount) in rows.iter().zip(amounts) {
                assert_eq!(row["account"], account);
                assert_eq!(row["period"], period);
                assert_eq!(row["currency"], "EUR");
                assert_eq!(row["amount_cents"], amount);
            }
        }
    }
    for scenario in ["overlap", "delayed", "empty"] {
        let server = Server::start(scenario, "acme");
        let (report, code) = run(
            &server,
            &browser,
            "collect_invoices.py",
            &["--period", "2026-08"],
        );
        assert_eq!(code, 0, "{scenario}: {report}");
        assert_eq!(
            report["outputs"]["items"].as_array().unwrap().len(),
            if scenario == "empty" { 0 } else { 5 }
        );
        if scenario == "overlap" {
            assert_eq!(report["duplicates"], 2);
        }
    }
}

#[test]
fn incomplete_or_inconsistent_collections_are_explicit_and_bounded() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("workflow-collection-failures");
    for scenario in [
        "conflict",
        "repeat-page",
        "repeated-cursor",
        "no-progress",
        "changed-revision",
        "truncated",
        "wrong-account",
        "wrong-row-account",
        "invalid-amount",
        "expired-session",
        "disconnect",
    ] {
        let server = Server::start(scenario, "acme");
        let (report, code) = run(
            &server,
            &browser,
            "collect_invoices.py",
            &["--period", "2026-08"],
        );
        assert_ne!(code, 0, "{scenario}: {report}");
        assert_eq!(report["complete"], false, "{report}");
        let rows = report["outputs"]["items"].as_array().unwrap();
        assert!(rows.len() < 5, "{report}");
        assert!(
            rows.iter()
                .all(|r| r["account"] == "acme" && r["period"] == "2026-08")
        );
        assert!(
            server.state()["pages"].as_array().unwrap().len() <= 2,
            "{scenario}: loop continued"
        );
    }
    let server = Server::start("normal", "acme");
    let (report, code) = run(
        &server,
        &browser,
        "collect_invoices.py",
        &["--period", "2026-08", "--max-pages", "1"],
    );
    assert_eq!(code, 2, "{report}");
    assert_eq!(report["status"], "partial");
    assert_eq!(report["outputs"]["items"].as_array().unwrap().len(), 2);
    assert_eq!(server.state()["pages"].as_array().unwrap().len(), 1);
    let (report, code) = run(
        &server,
        &browser,
        "collect_invoices.py",
        &["--period", "2026-08", "--max-rows", "2"],
    );
    assert_eq!(code, 2, "{report}");
    assert_eq!(report["complete"], false);
    assert_eq!(report["outputs"]["items"], json!([]));
    assert_eq!(server.state()["pages"].as_array().unwrap().len(), 2);
}

#[test]
fn drafts_are_verified_and_reused_across_invocations_without_another_submit() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("workflow-draft-reuse");
    let files = Files::new();
    let server = Server::start("normal", "acme");
    let journal = files.0.join("draft.json");
    let (first, code) = draft(&server, &browser, &journal, &[]);
    assert_eq!(code, 0, "{first}");
    assert_eq!(first["creation_attempted"], true);
    assert_eq!(first["outputs"]["draft"]["id"], "DRAFT-1");
    assert_eq!(server.state()["drafts"][0], first["outputs"]["draft"]);
    assert_eq!(first["submission_commands"], 1);
    for file in [&journal, &files.0.join("another.json")] {
        let (report, code) = draft(&server, &browser, file, &[]);
        assert_eq!(code, 0, "{report}");
        assert_eq!(report["outputs"], first["outputs"]);
        assert_eq!(report["creation_attempted"], false);
        assert_eq!(report["submission_commands"], 0);
    }
    assert_eq!(server.state()["creations"].as_array().unwrap().len(), 1);
    let saved: Value = serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
    assert_eq!(saved["state"], "verified");
    assert_eq!(saved["record"], first["outputs"]["draft"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(&journal).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let (report, code) = draft(&server, &browser, &journal, &["--amount", "99.99"]);
    assert_eq!(code, 1, "{report}");
    assert_eq!(report["command_count"], 0);
    assert_eq!(server.state()["creations"].as_array().unwrap().len(), 1);
}

#[test]
fn lost_create_responses_are_reconciled_and_unresolved_writes_are_never_replayed() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("workflow-draft-loss");
    let files = Files::new();
    for scenario in [
        "lost-response",
        "delayed-commit",
        "missing-draft",
        "duplicate-draft",
        "wrong-fields",
        "lookup-unavailable",
    ] {
        let server = Server::start(scenario, "acme");
        let journal = files.0.join(format!("{scenario}.json"));
        let (report, code) = draft(&server, &browser, &journal, &[]);
        let verified = matches!(scenario, "lost-response" | "delayed-commit");
        assert_eq!(code == 0, verified, "{scenario}: {report}");
        assert_eq!(
            report["status"],
            if verified {
                "verified"
            } else if scenario == "wrong-fields" {
                "failed"
            } else {
                "uncertain"
            },
            "{report}"
        );
        assert_eq!(
            server.state()["creations"].as_array().unwrap().len(),
            1,
            "{report}"
        );
        if !verified {
            let (again, code) = draft(&server, &browser, &journal, &[]);
            assert_ne!(code, 0, "{again}");
            assert_eq!(again["creation_attempted"], false);
            assert_eq!(
                server.state()["creations"].as_array().unwrap().len(),
                1,
                "{again}"
            );
        }
    }
}

#[test]
fn a_later_run_can_find_a_late_commit_and_bad_context_never_creates() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("workflow-draft-later");
    let files = Files::new();
    let server = Server::start("late-commit", "acme");
    let journal = files.0.join("late.json");
    let (report, code) = draft(&server, &browser, &journal, &["--timeout", "1"]);
    assert_eq!(code, 1, "{report}");
    assert_eq!(report["status"], "uncertain");
    let deadline = Instant::now() + Duration::from_secs(6);
    while server.state()["drafts"].as_array().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "demo never committed");
        std::thread::sleep(Duration::from_millis(100));
    }
    let (report, code) = draft(&server, &browser, &journal, &[]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["creation_attempted"], false);
    assert_eq!(server.state()["creations"].as_array().unwrap().len(), 1);
    for scenario in ["wrong-account", "duplicate-submit"] {
        let server = Server::start(scenario, "acme");
        let (report, code) = draft(
            &server,
            &browser,
            &files.0.join(format!("{scenario}.json")),
            &[],
        );
        assert_ne!(code, 0, "{report}");
        assert_eq!(report["creation_attempted"], false);
        assert_eq!(server.state()["creations"], json!([]));
    }
}

#[test]
fn a_browser_transport_retry_is_not_mistaken_for_one_verified_creation() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("workflow-browser-retry");
    let files = Files::new();
    let server = Server::start("transport-retry", "acme");
    let (report, code) = draft(&server, &browser, &files.0.join("retry.json"), &[]);
    let received = server.state()["creations"].as_array().unwrap().len();
    assert_eq!(report["submission_commands"], 1, "{report}");
    assert!(received >= 1);
    // Chrome versions differ on retrying a POST whose connection closed before any headers.
    // Both outcomes must be reported according to records, not inferred from the one click.
    if received > 1 {
        assert_eq!(code, 1, "{report}");
        assert_eq!(report["status"], "uncertain");
        assert_eq!(report["outputs"], json!({}));
    } else {
        assert_eq!(code, 0, "{report}");
        assert_eq!(report["outputs"]["draft"]["id"], "DRAFT-1");
    }
}
