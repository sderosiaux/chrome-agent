//! End-to-end regressions for capabilities exercised by Scout's controlled
//! Browser Lab. These use only local fixtures and never touch production sites.

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;

fn binary() -> PathBuf {
    let mut path = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    path.push("chrome-agent");
    path
}

fn chrome_available() -> bool {
    let candidates = if cfg!(target_os = "macos") {
        vec!["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"]
    } else {
        vec!["google-chrome", "chromium"]
    };
    candidates.iter().any(|candidate| {
        Path::new(candidate).exists()
            || Command::new("which")
                .arg(candidate)
                .output()
                .is_ok_and(|output| output.status.success())
    })
}

fn fixture_url(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    format!("file://{}", path.to_string_lossy())
}

fn run(browser: &str, args: &[&str]) -> Output {
    Command::new(binary())
        .args(["--browser", browser])
        .args(args)
        .output()
        .expect("run chrome-agent")
}

struct BrowserGuard(String);

impl BrowserGuard {
    fn new(browser: &str) -> Self {
        Self(browser.to_string())
    }
}

impl Drop for BrowserGuard {
    fn drop(&mut self) {
        let _ = run(&self.0, &["close", "--purge"]);
    }
}

fn run_pipe(browser: &str, commands: &[Value], timeout: Duration) -> Vec<Value> {
    let mut child = Command::new(binary())
        .args(["--browser", browser, "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn chrome-agent pipe");

    {
        let mut stdin = child.stdin.take().expect("pipe stdin");
        for command in commands {
            writeln!(stdin, "{}", serde_json::to_string(command).unwrap()).unwrap();
        }
    }

    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().expect("poll chrome-agent pipe").is_some() {
            let output = child.wait_with_output().expect("collect pipe output");
            assert!(
                output.status.success(),
                "pipe failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| serde_json::from_str(line).expect("JSON pipe response"))
                .collect();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().expect("collect timed-out pipe output");
            panic!(
                "pipe timed out after {timeout:?}; stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

struct RedirectServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl RedirectServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                        let mut request = [0_u8; 4096];
                        let size = stream.read(&mut request).unwrap_or(0);
                        let first_line = String::from_utf8_lossy(&request[..size])
                            .lines()
                            .next()
                            .unwrap_or("")
                            .to_string();
                        let path = first_line.split_whitespace().nth(1).unwrap_or("/");
                        let response = if path == "/start" {
                            b"HTTP/1.1 302 Found\r\nLocation: /settled\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
                        } else if path == "/settled" {
                            let body = b"<!doctype html><title>Settled page</title><p>redirect complete</p>";
                            let headers = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            [headers.as_bytes(), body].concat()
                        } else {
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
                        };
                        let _ = stream.write_all(&response);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("redirect fixture accept failed: {error}"),
                }
            }
        });
        Self {
            addr,
            stop,
            thread: Some(thread),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

impl Drop for RedirectServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

#[test]
fn goto_reports_the_settled_redirect_url() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let server = RedirectServer::start();
    let browser = format!("test-settled-url-{}", std::process::id());
    let _guard = BrowserGuard::new(&browser);
    let start_url = server.url("/start");
    let output = run(&browser, &["--json", "goto", &start_url]);
    assert!(
        output.status.success(),
        "goto failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).expect("JSON goto response");
    assert_eq!(response["url"], server.url("/settled"));
    assert_eq!(response["title"], "Settled page");
}

#[test]
fn frame_can_switch_from_an_iframe_into_a_nested_iframe() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let browser = format!("test-nested-frame-{}", std::process::id());
    let _guard = BrowserGuard::new(&browser);
    let responses = run_pipe(
        &browser,
        &[
            serde_json::json!({"cmd": "goto", "url": fixture_url("frame_nested_parent.html")}),
            serde_json::json!({"cmd": "frame", "target": "#outer-frame"}),
            serde_json::json!({"cmd": "frame", "target": "#nested-frame"}),
            serde_json::json!({"cmd": "eval", "expression": "document.querySelector('#grandchild-marker').textContent"}),
        ],
        Duration::from_secs(30),
    );
    assert_eq!(responses.len(), 4, "responses: {responses:?}");
    assert_eq!(responses[2]["ok"], true, "nested frame switch: {:?}", responses[2]);
    assert_eq!(responses[3]["result"], "NESTED GRANDCHILD CONTENT");
}

#[test]
fn selector_click_auto_accepts_native_alert_without_hanging_pipe() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let browser = format!("test-dialog-click-{}", std::process::id());
    let _guard = BrowserGuard::new(&browser);
    let responses = run_pipe(
        &browser,
        &[
            serde_json::json!({"cmd": "goto", "url": fixture_url("dialog_click.html")}),
            serde_json::json!({"cmd": "click", "selector": "#alert-button"}),
            serde_json::json!({"cmd": "eval", "expression": "window.dialogHandled === true"}),
        ],
        Duration::from_secs(30),
    );
    assert_eq!(responses.len(), 3, "responses: {responses:?}");
    assert_eq!(responses[1]["ok"], true, "alert click: {:?}", responses[1]);
    assert_eq!(responses[2]["result"], true);
}
