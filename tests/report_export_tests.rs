//! A runnable task example: browser evidence, downloaded bytes, and an independent server oracle.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

mod common;
use common::TestBrowser;

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/report_export")
        .join(name)
}

fn python_ready() -> bool {
    if Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        true
    } else {
        common::unavailable("python3 is required for the report export example")
    }
}

struct Server {
    child: Child,
    url: String,
}

impl Server {
    fn start(account: &str, scenario: &str) -> Self {
        let child = Command::new("python3")
            .arg(example("demo_server.py"))
            .args(["--account", account, "--scenario", scenario])
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut server = Self {
            child,
            url: String::new(),
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
            .expect("demo server startup deadline");
        let ready: Value = serde_json::from_str(&line).expect("server readiness JSON");
        server.url = ready["url"].as_str().unwrap().to_string();
        server
    }

    fn exports(&self) -> Vec<Value> {
        let address = self
            .url
            .strip_prefix("http://")
            .unwrap()
            .trim_end_matches('/');
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write!(stream, "GET /state HTTP/1.0\r\nHost: {address}\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (_, body) = response.split_once("\r\n\r\n").unwrap();
        let state: Value = serde_json::from_str(body).unwrap();
        state["exports"].as_array().unwrap().clone()
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
        let path = common::temp_path("report-export", "dir");
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(
    server: &Server,
    browser: &TestBrowser,
    files: &Files,
    account: &str,
    period: &str,
    timeout: u32,
) -> (Value, i32, PathBuf) {
    let out = files
        .0
        .join(format!("{}.csv", common::unique_name("export")));
    let output = Command::new("python3")
        .arg(example("export_report.py"))
        .args([
            "--url",
            &server.url,
            "--account",
            account,
            "--period",
            period,
            "--browser",
            browser.name(),
            "--timeout",
            &timeout.to_string(),
        ])
        .arg("--binary")
        .arg(common::binary())
        .arg("--out")
        .arg(&out)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    let report =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}: {output:?}"));
    (report, output.status.code().unwrap_or(-1), out)
}

#[test]
fn the_examples_python_checks_and_protocol_tests_pass() {
    if !python_ready() {
        return;
    }
    let output = Command::new("python3")
        .args(["-m", "unittest", "discover", "-s", "tests/python", "-q"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn the_task_reuses_the_procedure_for_different_accounts_and_periods() {
    if !python_ready() || !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("report-export-inputs");
    let files = Files::new();
    for account in ["acme", "beta"] {
        let server = Server::start(account, "normal");
        for (period, ids) in [
            ("2026-08", vec!["INV-801", "INV-802"]),
            ("2026-09", vec!["INV-901", "INV-902", "INV-903"]),
        ] {
            let (report, code, out) = run(&server, &browser, &files, account, period, 3);
            assert_eq!(code, 0, "{report}");
            assert_eq!(report["status"], "verified");
            assert_eq!(report["outputs"]["file"], json!(out));
            assert_eq!(report["outputs"]["account"], account);
            assert_eq!(report["outputs"]["period"], period);
            assert_eq!(report["outputs"]["row_count"], ids.len());
            assert!(
                report["checks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|c| c["held"] == true),
                "{report}"
            );
            let csv = std::fs::read_to_string(&out).unwrap();
            let actual: Vec<&str> = csv
                .lines()
                .skip(1)
                .map(|row| row.split(',').nth(2).unwrap())
                .collect();
            assert_eq!(actual, ids);
            assert!(
                csv.lines()
                    .skip(1)
                    .all(|row| row.starts_with(&format!("{account},{period},")))
            );
            let exports = server.exports();
            assert_eq!(exports.last().unwrap()["invoice_ids"], json!(ids));
            assert_eq!(exports.last().unwrap()["period"], period);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                assert_eq!(
                    std::fs::metadata(&out).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
        assert_eq!(server.exports().len(), 2, "one export per invocation");
    }
}

#[test]
fn wrong_page_context_and_ambiguous_controls_stop_before_export() {
    if !python_ready() || !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("report-export-context");
    let files = Files::new();
    for scenario in [
        "wrong-account",
        "wrong-selection",
        "expired-session",
        "duplicate-export",
    ] {
        let server = Server::start("acme", scenario);
        let (report, code, out) = run(&server, &browser, &files, "acme", "2026-09", 2);
        let (expected_code, expected_status) = if scenario == "wrong-selection" {
            (1, "error") // select reports a reverted value as a command error.
        } else {
            (2, "failed")
        };
        assert_eq!(code, expected_code, "{scenario}: {report}");
        assert_eq!(report["status"], expected_status);
        assert_eq!(report["export_attempted"], false);
        assert!(
            server.exports().is_empty(),
            "{scenario}: an export was dispatched"
        );
        assert!(!out.exists());
    }
}

#[test]
fn wrong_or_incomplete_csv_never_becomes_the_published_result() {
    if !python_ready() || !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("report-export-content");
    let files = Files::new();
    for scenario in [
        "wrong-file-account",
        "wrong-file-period",
        "empty-file",
        "truncated-report",
    ] {
        let server = Server::start("acme", scenario);
        let (report, code, out) = run(&server, &browser, &files, "acme", "2026-09", 3);
        assert_eq!(code, 2, "{scenario}: {report}");
        assert_eq!(report["status"], "failed");
        assert_eq!(report["stopped_at"], "verify_file");
        assert_eq!(report["download"]["downloaded"], true);
        assert_eq!(report["outputs"], json!({}));
        assert!(!out.exists());
        assert!(PathBuf::from(report["artifact"]["file"].as_str().unwrap()).exists());
        assert_eq!(server.exports().len(), 1);
    }
}

#[test]
fn delayed_results_wait_and_lost_export_responses_never_trigger_a_retry() {
    if !python_ready() || !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("report-export-timing");
    let files = Files::new();
    let server = Server::start("acme", "delayed");
    let (report, code, out) = run(&server, &browser, &files, "acme", "2026-09", 3);
    assert_eq!(code, 0, "{report}");
    assert!(out.exists());
    assert_eq!(server.exports().len(), 1);
    for scenario in ["missing-download", "disconnect"] {
        let server = Server::start("acme", scenario);
        let (report, code, out) = run(&server, &browser, &files, "acme", "2026-09", 1);
        assert_eq!(code, 1, "{scenario}: {report}");
        assert_eq!(report["status"], "uncertain");
        assert_eq!(report["export_attempted"], true);
        assert_eq!(report["download"]["downloaded"], false);
        assert!(!out.exists());
        assert_eq!(
            server.exports().len(),
            1,
            "a lost response must not cause another export"
        );
    }
}
