//! `--timeout` applies to the browser-level client, not only the page client.
//!
//! No real Chrome: a fake browser answers /json/version, accepts the WebSocket handshake,
//! then never answers any CDP call.

use futures_util::StreamExt as _;
use std::process::Command;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

mod common;
use common::TestBrowser;

/// Serve one port: HTTP GETs get a /json/version answer pointing back at this port;
/// WebSocket upgrades complete and are then starved. The thread is detached.
fn spawn_starving_browser() -> std::net::SocketAddr {
    let (addr_tx, addr_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            addr_tx.send(addr).unwrap();
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut probe = [0_u8; 1024];
                    let Ok(n) = stream.peek(&mut probe).await else {
                        return;
                    };
                    let head = String::from_utf8_lossy(&probe[..n]);
                    if head.to_ascii_lowercase().contains("upgrade: websocket") {
                        // Read frames forever and never reply: every CDP call hangs.
                        let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                            return;
                        };
                        while ws.next().await.is_some() {}
                    } else {
                        // /json/version resolution.
                        let mut sink = [0_u8; 4096];
                        let _ = stream.read(&mut sink).await;
                        let addr = stream.local_addr().unwrap();
                        let body = format!(
                            "{{\"webSocketDebuggerUrl\":\"ws://{addr}/devtools/browser/fake\"}}"
                        );
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                    }
                });
            }
        });
    });
    addr_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("fake browser did not start")
}

#[test]
fn browser_level_calls_honor_the_timeout_flag() {
    let addr = spawn_starving_browser();
    let guard = TestBrowser::new("test-browser-timeout");
    let browser = guard.name();

    // Target resolution on the browser client is the first CDP call of the run, and the
    // fake never answers it. `goto` rather than `tabs`, which bails before connecting.
    let started = Instant::now();
    let output = Command::new(common::binary())
        .args([
            "--browser",
            browser,
            "--connect",
            &format!("http://{addr}"),
            "--timeout",
            "2",
            "--json",
            "goto",
            "about:blank",
        ])
        .output()
        .expect("run chrome-agent");
    let elapsed = started.elapsed();

    assert!(
        !output.status.success(),
        "a starved browser endpoint cannot yield a successful goto, got stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        elapsed < Duration::from_secs(15),
        "--timeout 2 was ignored by the browser-level client: command took {elapsed:?} \
         (the 30s DEFAULT_CALL_TIMEOUT is still in charge)"
    );
}

/// An HTTP peer that accepts a connection but never finishes its response. Own its thread so
/// even a regression killing the CLI does not leave a listener behind in the test process.
struct StalledHttp {
    address: std::net::SocketAddr,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl StalledHttp {
    fn start(partial_body: bool) -> Self {
        use std::io::{Read as _, Write as _};
        use std::sync::atomic::Ordering;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                if let Ok((mut stream, _)) = listener.accept() {
                    stream
                        .set_read_timeout(Some(Duration::from_millis(100)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let mut request = [0; 4096];
                    let _ = stream.read(&mut request);
                    if partial_body {
                        // Some of the same global budget is spent waiting for headers.
                        std::thread::sleep(Duration::from_millis(700));
                        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 999\r\nContent-Type: application/json\r\n\r\n{");
                    }
                    while !stopping.load(Ordering::Relaxed) {
                        if matches!(stream.read(&mut request), Ok(0)) {
                            return;
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Self {
            address,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for StalledHttp {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

#[test]
fn discovery_deadline_covers_silent_headers_and_the_whole_response() {
    for partial_body in [false, true] {
        let server = StalledHttp::start(partial_body);
        let browser = TestBrowser::new("http-discovery-timeout");
        let mut child = Command::new(common::binary())
            .args([
                "--browser",
                browser.name(),
                "--connect",
                &format!("http://{}", server.address),
                "--json",
                "goto",
                "about:blank",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        while child.try_wait().unwrap().is_none() {
            if started.elapsed() > Duration::from_secs(6) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("HTTP discovery outlived its 2s budget; partial body: {partial_body}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["ok"], false);
        assert!(
            report["error"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("global"),
            "{report}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
