//! End-to-end regressions for capabilities exercised by Scout's controlled
//! Browser Lab. These use only local fixtures and never touch production sites.

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;

mod common;

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

/// Answer one connection: `/start` redirects to `/settled`, `/settled` is the destination.
///
/// A request that never arrives gets no response at all. Replying 404 to a speculative
/// connection is what made this test flaky: `read` returning 0 (timeout, or a partial first
/// line) fell through to the 404 branch, and the navigation reported
/// `net::ERR_HTTP_RESPONSE_CODE_FAILURE` — an error about the fixture, dressed as an error
/// about `goto`.
fn serve(mut stream: std::net::TcpStream) {
    // On macOS/BSD an accepted socket inherits the listener's O_NONBLOCK, so `read` returns
    // WouldBlock the instant the request has not landed yet. That is the flakiness: the
    // handler saw an empty request and answered 404 to the navigation itself.
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    // Read until the headers end rather than trusting a single read to deliver them.
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => request.extend_from_slice(&chunk[..n]),
        }
    }
    let first_line = String::from_utf8_lossy(&request).lines().next().unwrap_or("").to_string();
    let Some(path) = first_line.split_whitespace().nth(1) else {
        return; // no request line: a preconnect, not a page load
    };
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

impl RedirectServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let mut workers: Vec<JoinHandle<()>> = Vec::new();
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    // One thread per connection. Chrome opens speculative sockets that
                    // carry no request; serving in the accept loop makes the next request
                    // wait out this one's 2s read timeout (measured: 2.01s).
                    Ok((stream, _)) => workers.push(std::thread::spawn(move || serve(stream))),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("redirect fixture accept failed: {error}"),
                }
            }
            for worker in workers {
                let _ = worker.join();
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
    if !common::browser_ready() {
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

/// The fixture server must not turn a browser's speculative connection into an answer on
/// the navigation. Chrome preconnects: it opens a socket and may send nothing on it. The
/// first version of this server accepted in a single loop and replied 404 whenever the read
/// came back empty, so `goto` reported `net::ERR_HTTP_RESPONSE_CODE_FAILURE` — a fixture bug
/// wearing a `goto` bug's clothes. Seen twice under load, never on demand, which is why the
/// property is pinned here instead of by re-running the browser test.
#[test]
fn the_fixture_server_serves_a_request_made_behind_a_silent_connection() {
    use std::net::TcpStream;

    let server = RedirectServer::start();

    // A connection that never sends a request line, held open for the whole exchange.
    let silent = TcpStream::connect(server.addr).expect("preconnect");

    let mut real = TcpStream::connect(server.addr).expect("request connection");
    real.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(real, "GET /start HTTP/1.1\r\nHost: localhost\r\n\r\n").expect("send request");

    let started = Instant::now();
    let mut response = String::new();
    real.read_to_string(&mut response).expect("read response");
    let waited = started.elapsed();
    drop(silent);

    assert!(
        response.starts_with("HTTP/1.1 302"),
        "a real request must not wait behind a silent one, got: {response:?}"
    );
    assert!(response.contains("Location: /settled"), "got: {response:?}");
    // The bound is the server's own 2s read timeout: serving connections in the accept loop
    // makes this request wait for the silent one to time out. Loopback answers in ~1ms.
    assert!(
        waited < Duration::from_secs(1),
        "the request queued behind the silent connection: waited {waited:?}"
    );
}

/// A connection that carries no request gets no response. It used to get a 404, and when
/// that connection was the navigation's own, `goto` failed with an HTTP status error.
#[test]
fn the_fixture_server_answers_nothing_to_a_connection_that_sends_nothing() {
    use std::net::TcpStream;

    let server = RedirectServer::start();
    let mut silent = TcpStream::connect(server.addr).expect("preconnect");
    silent.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

    let mut response = String::new();
    let _ = silent.read_to_string(&mut response);

    assert!(
        response.is_empty(),
        "a preconnect must not be answered, got: {response:?}"
    );
}

#[test]
fn frame_can_switch_from_an_iframe_into_a_nested_iframe() {
    if !common::browser_ready() {
        return;
    }
    let browser = format!("test-nested-frame-{}", std::process::id());
    let _guard = BrowserGuard::new(&browser);
    let responses = run_pipe(
        &browser,
        &[
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("frame_nested_parent.html")}),
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
    if !common::browser_ready() {
        return;
    }
    let browser = format!("test-dialog-click-{}", std::process::id());
    let _guard = BrowserGuard::new(&browser);
    let responses = run_pipe(
        &browser,
        &[
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("dialog_click.html")}),
            serde_json::json!({"cmd": "click", "selector": "#alert-button"}),
            serde_json::json!({"cmd": "eval", "expression": "window.dialogHandled === true"}),
        ],
        Duration::from_secs(30),
    );
    assert_eq!(responses.len(), 3, "responses: {responses:?}");
    assert_eq!(responses[1]["ok"], true, "alert click: {:?}", responses[1]);
    assert_eq!(responses[2]["result"], true);
}
