//! Scripted protocol/executor regressions. These do not measure autonomous discovery.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

mod common;
#[path = "common/macros.rs"]
mod macros;
use common::TestBrowser;
use macros::{TestMacro, run_cli};

struct Journal(std::path::PathBuf);
impl Journal {
    fn new() -> Self {
        Self(common::temp_path("discovery", "json"))
    }
    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }
    fn read(&self) -> Value {
        serde_json::from_slice(&std::fs::read(&self.0).unwrap()).unwrap()
    }
}
impl Drop for Journal {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(format!("{}.lock", self.path()));
    }
}

struct Candidate(TestMacro);
impl Candidate {
    fn new() -> Self {
        Self(TestMacro::new("discovered"))
    }
}
impl Drop for Candidate {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(format!("{}.lock", self.0.path().display()));
    }
}

fn cli(browser: &TestBrowser, args: &[&str], expected: i32) -> Value {
    let mut full = vec!["--json", "--browser", browser.name()];
    full.extend_from_slice(args);
    let (stdout, stderr, code) = run_cli(&full);
    assert_eq!(code, expected, "{full:?}\n{stdout}{stderr}");
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{e}: {stdout}{stderr}"))
}

fn start(browser: &TestBrowser, journal: &Journal, url: &str, budget: &str) -> Value {
    cli(
        browser,
        &[
            "discover",
            "start",
            journal.path(),
            "--goal",
            "Collect the edition's headlines",
            "--url",
            url,
            "--inputs",
            &json!({"url":url,"title":"Alpha edition"}).to_string(),
            "--max-commands",
            budget,
        ],
        0,
    )
}

fn proposal(id: &str, revision: u64, command: Value, checks: Vec<Value>) -> Value {
    let mut value = json!({"id":id,"revision":revision,"reason":"Observe the requested edition",
        "hypotheses":["Headlines belong to the displayed edition"],"unknowns":["Other editions remain untested"]});
    value["command"] = command;
    value["checks"] = Value::Array(checks);
    value
}

fn step(browser: &TestBrowser, journal: &Journal, proposal: &Value, expected: i32) -> Value {
    cli(
        browser,
        &[
            "discover",
            "step",
            journal.path(),
            "--proposal",
            &proposal.to_string(),
        ],
        expected,
    )
}

fn opening() -> Value {
    proposal(
        "open",
        0,
        json!({"cmd":"goto","url":"{{url}}"}),
        vec![json!({"cmd":"assert","what":"url","equals":"{{url}}"})],
    )
}

#[test]
fn invalid_proposals_and_corrupt_history_fail_offline_without_debiting_budget() {
    let browser = TestBrowser::new("discovery-offline");
    let journal = Journal::new();
    let url = "https://example.com/news";
    let initial = start(&browser, &journal, url, "20");
    assert_eq!(initial["discovery"]["revision"], 0);
    assert_eq!(initial["model_usage"], Value::Null);
    let original = std::fs::read(&journal.0).unwrap();
    for command in [
        json!({"cmd":"text"}),
        json!({"cmd":"goto","url":"https://other.example/news"}),
        json!({"cmd":"goto","url":"https://example.com/other"}),
        json!({"cmd":"goto","url":"https://example.com:invalid/news"}),
        json!({"cmd":"goto","url":"https://user:password@example.com/news"}),
        json!({"cmd":"goto","url":"file:///tmp/private"}),
        json!({"cmd":"goto","url":"{{undeclared}}"}),
        json!({"cmd":"goto","url":url,"typo":true}),
        json!({"cmd":"eval","expression":"fetch('/write',{method:'POST'})"}),
        json!({"cmd":"click","selector":"button"}),
        json!({"cmd":"batch","commands":[{"cmd":"goto","url":url}]}),
    ] {
        step(
            &browser,
            &journal,
            &proposal("invalid", 0, command, vec![]),
            1,
        );
        assert_eq!(std::fs::read(&journal.0).unwrap(), original);
    }
    for check in [
        json!({"cmd":"text"}),
        json!({"cmd":"assert","what":"text","matches":"["}),
    ] {
        step(
            &browser,
            &journal,
            &proposal("invalid", 0, json!({"cmd":"goto","url":url}), vec![check]),
            1,
        );
    }
    let mut stale = opening();
    stale["revision"] = json!(1);
    assert!(
        step(&browser, &journal, &stale, 1)["error"]
            .as_str()
            .unwrap()
            .contains("revision")
    );
    cli(
        &browser,
        &[
            "discover",
            "start",
            journal.path(),
            "--goal",
            "overwrite",
            "--url",
            url,
        ],
        1,
    );
    assert_eq!(std::fs::read(&journal.0).unwrap(), original);
    let mut corrupt = journal.read();
    corrupt["reserved_commands"] = json!(1);
    std::fs::write(&journal.0, corrupt.to_string()).unwrap();
    cli(&browser, &["discover", "show", journal.path()], 1);
    let sessions = std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
        .join(".chrome-agent/sessions.json");
    assert!(
        !std::fs::read_to_string(sessions)
            .unwrap_or_default()
            .contains(browser.name())
    );
}

#[cfg(unix)]
#[test]
fn state_is_private_and_symlinks_and_concurrent_writers_are_refused() {
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    let browser = TestBrowser::new("discovery-files");
    let journal = Journal::new();
    start(&browser, &journal, "http://example.com/", "1");
    assert_eq!(
        std::fs::metadata(&journal.0).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let alias = Journal::new();
    symlink(&journal.0, &alias.0).unwrap();
    cli(&browser, &["discover", "show", alias.path()], 1);
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(format!("{}.lock", journal.path()))
        .unwrap();
    lock.try_lock().unwrap();
    assert!(
        step(&browser, &journal, &opening(), 1)["error"]
            .as_str()
            .unwrap()
            .contains("busy")
    );
    cli(&browser, &["discover", "show", journal.path()], 0);
    assert_eq!(journal.read()["revision"], 0);
}

/// A real HTTP site with two independently specified editions. The oracle reads the response
/// and fixture request log, not the discovery outcome label.
struct Site {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Site {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let redirect = format!("{}/alpha", base.replace("127.0.0.1", "localhost"));
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut bytes = [0; 4096];
                let Ok(n) = stream.read(&mut bytes) else {
                    continue;
                };
                let request = String::from_utf8_lossy(&bytes[..n]);
                let route = request.split_whitespace().nth(1).unwrap_or("/");
                log.lock().unwrap().push(route.to_string());
                let (title, headlines) = if route == "/beta" {
                    ("Beta edition", ["Harbor reopens", "Moon mission launches"])
                } else {
                    ("Alpha edition", ["Library opens", "New rail service"])
                };
                let large = if route == "/large" {
                    "x".repeat(70000)
                } else {
                    String::new()
                };
                let body = format!(
                    "<!doctype html><title>{title}</title><main><h1>{title}</h1><ul><li><a href='/first'>{}</a></li><li><a href='/second'>{}</a></li></ul><p id='large'>{large}</p></main>",
                    headlines[0], headlines[1]
                );
                let response = if route == "/redirect" {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: {redirect}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self {
            base,
            requests,
            stop,
            thread: Some(thread),
        }
    }
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
    fn hits(&self, path: &str) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|p| *p == path)
            .count()
    }
}
impl Drop for Site {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

#[test]
fn fresh_process_continues_observations_and_reuses_a_candidate_with_new_inputs() {
    if !common::browser_ready() {
        return;
    }
    let site = Site::new();
    let browser = TestBrowser::new("discovery-learn");
    let journal = Journal::new();
    start(&browser, &journal, &site.url("/alpha"), "12");
    let opened = step(&browser, &journal, &opening(), 0);
    assert_eq!(opened["revision"], 2);
    assert_eq!(site.hits("/alpha"), 1);
    let repeat = step(&browser, &journal, &opening(), 0);
    assert_eq!(repeat["replayed"], true);
    assert_eq!(repeat["experiment"], opened["experiment"]);
    assert_eq!(site.hits("/alpha"), 1);
    let mut changed = opening();
    changed["reason"] = json!("Different request");
    step(&browser, &journal, &changed, 1);
    let failed = proposal(
        "wrong",
        2,
        json!({"cmd":"assert","what":"text","selector":"h1","contains":"Wrong edition"}),
        vec![],
    );
    assert_eq!(step(&browser, &journal, &failed, 2)["outcome"], "not_held");
    let shown = cli(&browser, &["discover", "show", journal.path()], 0);
    assert_eq!(
        shown["discovery"]["experiments"][1]["results"][0]["response"]["assertion"]["held"],
        false
    );
    // The continuation needs the file and task inputs only; each helper spawns a fresh process.
    let revision = shown["discovery"]["revision"].as_u64().unwrap();
    let read = proposal(
        "headlines",
        revision,
        json!({"cmd":"text","selector":"ul"}),
        vec![json!({"cmd":"assert","what":"text","selector":"h1","contains":"{{title}}"})],
    );
    let collected = step(&browser, &journal, &read, 0);
    assert_eq!(
        collected["experiment"]["results"][0]["response"]["text"],
        "Library opens\nNew rail service"
    );
    assert_eq!(
        collected["experiment"]["proposal"]["unknowns"][0],
        "Other editions remain untested"
    );
    let candidate = Candidate::new();
    for ids in ["open,wrong", "headlines,open", "open,open"] {
        cli(
            &browser,
            &[
                "discover",
                "export",
                journal.path(),
                "--name",
                candidate.0.name(),
                "--steps",
                ids,
            ],
            1,
        );
    }
    let exported = cli(
        &browser,
        &[
            "discover",
            "export",
            journal.path(),
            "--name",
            candidate.0.name(),
            "--steps",
            "open,headlines",
        ],
        0,
    );
    assert_eq!(exported["status"], "candidate");
    let occupied = Candidate::new();
    occupied.0.write(json!({"steps":[{"do":{"cmd":"text"}}]}));
    let occupied_bytes = std::fs::read(occupied.0.path()).unwrap();
    cli(
        &browser,
        &[
            "discover",
            "export",
            journal.path(),
            "--name",
            occupied.0.name(),
            "--steps",
            "open,headlines",
        ],
        1,
    );
    assert_eq!(std::fs::read(occupied.0.path()).unwrap(), occupied_bytes);
    let original = std::fs::read(candidate.0.path()).unwrap();
    let macro_json: Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(macro_json["params"]["title"]["required"], true);
    assert!(!String::from_utf8_lossy(&original).contains("Alpha edition"));
    assert!(!String::from_utf8_lossy(&original).contains("Library opens"));
    cli(
        &browser,
        &[
            "discover",
            "export",
            journal.path(),
            "--name",
            candidate.0.name(),
            "--steps",
            "open,headlines",
        ],
        1,
    );
    assert_eq!(std::fs::read(candidate.0.path()).unwrap(), original);
    let fresh = TestBrowser::new("discovery-reuse");
    let url = format!("url={}", site.url("/beta"));
    let replay = cli(
        &fresh,
        &[
            "macro",
            "run",
            candidate.0.name(),
            "--var",
            &url,
            "--var",
            "title=Beta edition",
        ],
        0,
    );
    assert_eq!(
        replay["steps"][2]["result"]["text"],
        "Harbor reopens\nMoon mission launches"
    );
    let wrong = cli(
        &fresh,
        &[
            "macro",
            "run",
            candidate.0.name(),
            "--var",
            &url,
            "--var",
            "title=Alpha edition",
        ],
        2,
    );
    assert_eq!(wrong["result"]["assertion"]["held"], false);
}

#[test]
fn budget_deadline_redirect_and_oversized_observations_remain_explicit() {
    if !common::browser_ready() {
        return;
    }
    let site = Site::new();
    let browser = TestBrowser::new("discovery-limits");
    let journal = Journal::new();
    start(&browser, &journal, &site.url("/alpha"), "2");
    step(&browser, &journal, &opening(), 0);
    let next = proposal("read", 2, json!({"cmd":"text"}), vec![]);
    assert!(
        step(&browser, &journal, &next, 1)["error"]
            .as_str()
            .unwrap()
            .contains("budget")
    );
    assert_eq!(journal.read()["revision"], 2);
    assert_eq!(
        step(&browser, &journal, &opening(), 0)["remaining_commands"],
        0
    );
    let expired = Journal::new();
    start(&browser, &expired, &site.url("/alpha"), "10");
    let mut state = expired.read();
    state["created_ms"] = json!(1);
    state["deadline_ms"] = json!(2);
    std::fs::write(&expired.0, state.to_string()).unwrap();
    assert!(
        step(&browser, &expired, &opening(), 1)["error"]
            .as_str()
            .unwrap()
            .contains("deadline")
    );
    assert_eq!(site.hits("/alpha"), 1);
    let large = Journal::new();
    start(&browser, &large, &site.url("/large"), "10");
    step(&browser, &large, &opening(), 0);
    let oversized = step(&browser, &large, &next, 1);
    assert_eq!(oversized["outcome"], "uncertain");
    assert!(
        oversized["experiment"]["error"]
            .as_str()
            .unwrap()
            .contains("64 KiB")
    );
    assert_eq!(oversized["experiment"]["results"], json!([]));
    let narrow = proposal("narrow", 4, json!({"cmd":"text","selector":"h1"}), vec![]);
    assert_eq!(
        step(&browser, &large, &narrow, 0)["experiment"]["results"][0]["response"]["text"],
        "Alpha edition"
    );
    let candidate = Candidate::new();
    cli(
        &browser,
        &[
            "discover",
            "export",
            large.path(),
            "--name",
            candidate.0.name(),
            "--steps",
            "open,narrow",
        ],
        1,
    );
    let redirected = Journal::new();
    start(&browser, &redirected, &site.url("/redirect"), "10");
    let result = step(&browser, &redirected, &opening(), 1);
    assert_eq!(result["outcome"], "error");
    assert_eq!(result["experiment"]["results"].as_array().unwrap().len(), 1);
    let result = step(&browser, &redirected, &next, 1);
    assert_eq!(result["experiment"]["results"], json!([]));
    // Another caller can move the shared page. A new read must inspect the actual URL.
    assert_eq!(
        macros::run_pipe(
            browser.name(),
            &[json!({"cmd":"goto","url":site.url("/beta").replace("127.0.0.1", "localhost")})]
        )[0]["ok"],
        true
    );
    let outside = proposal("outside", 6, json!({"cmd":"text"}), vec![]);
    assert_eq!(
        step(&browser, &large, &outside, 1)["experiment"]["results"],
        json!([])
    );
    let timed = Journal::new();
    cli(
        &browser,
        &[
            "discover",
            "start",
            timed.path(),
            "--goal",
            "Bound a pending assertion",
            "--url",
            &site.url("/alpha"),
            "--within",
            "1",
        ],
        0,
    );
    let slow = proposal(
        "slow",
        0,
        json!({"cmd":"goto","url":site.url("/alpha")}),
        vec![json!({"cmd":"assert","what":"exists","selector":"#never","within":30})],
    );
    let began = Instant::now();
    assert_eq!(step(&browser, &timed, &slow, 1)["outcome"], "uncertain");
    assert!(began.elapsed() < Duration::from_secs(5));
    assert_eq!(step(&browser, &timed, &slow, 1)["replayed"], true);
}

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !condition() {
        assert!(Instant::now() < deadline, "condition timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn process_loss_preserves_uncertainty_and_never_repeats_the_reserved_experiment() {
    if !common::browser_ready() {
        return;
    }
    let site = Site::new();
    let browser = TestBrowser::new("discovery-crash");
    let journal = Journal::new();
    start(&browser, &journal, &site.url("/alpha"), "10");
    let blocked = proposal(
        "blocked",
        0,
        json!({"cmd":"goto","url":"{{url}}"}),
        vec![json!({"cmd":"assert","what":"exists","selector":"#never","within":30})],
    );
    let mut child = Running(
        Command::new(common::binary())
            .args([
                "--browser",
                browser.name(),
                "--json",
                "discover",
                "step",
                journal.path(),
                "--proposal",
                &blocked.to_string(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_until(|| journal.read()["revision"] == 1 && site.hits("/alpha") == 1);
    let pending = cli(&browser, &["discover", "show", journal.path()], 0);
    assert_eq!(pending["unresolved_experiments"], json!(["blocked"]));
    assert!(
        step(&browser, &journal, &blocked, 1)["error"]
            .as_str()
            .unwrap()
            .contains("busy")
    );
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let retry = step(&browser, &journal, &blocked, 1);
    assert_eq!(retry["replayed"], true);
    assert_eq!(retry["outcome"], "uncertain");
    assert_eq!(retry["experiment"]["state"], "pending");
    assert_eq!(site.hits("/alpha"), 1);
    assert_eq!(retry["remaining_commands"], 8);
    let observe = proposal("recover", 1, json!({"cmd":"text","selector":"h1"}), vec![]);
    assert_eq!(step(&browser, &journal, &observe, 0)["revision"], 3);
    let candidate = Candidate::new();
    cli(
        &browser,
        &[
            "discover",
            "export",
            journal.path(),
            "--name",
            candidate.0.name(),
            "--steps",
            "blocked,recover",
        ],
        1,
    );
    assert_eq!(
        cli(&browser, &["discover", "show", journal.path()], 0)["unresolved_experiments"],
        json!(["blocked"])
    );
}
