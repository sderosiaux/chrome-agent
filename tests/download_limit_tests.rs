use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

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
    ["google-chrome", "chromium"].iter().any(|candidate| {
        Path::new(candidate).exists()
            || Command::new("which")
                .arg(candidate)
                .output()
                .is_ok_and(|output| output.status.success())
    })
}

fn run(browser: &str, args: &[&str]) -> Output {
    Command::new(binary())
        .args(["--browser", browser])
        .args(args)
        .output()
        .expect("run chrome-agent")
}

struct BrowserGuard(String);

impl Drop for BrowserGuard {
    fn drop(&mut self) {
        let _ = run(&self.0, &["close", "--purge"]);
    }
}

struct FixtureServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FixtureServer {
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
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut request = [0_u8; 8192];
                        let size = stream.read(&mut request).unwrap_or(0);
                        let first_line = String::from_utf8_lossy(&request[..size])
                            .lines()
                            .next()
                            .unwrap_or("")
                            .to_string();
                        let path = first_line.split_whitespace().nth(1).unwrap_or("/");
                        let response = match path {
                            "/" => http_response("text/html", b"<html><main>download fixture</main></html>"),
                            "/declared" => http_response("application/octet-stream", b"12345678901"),
                            "/streamed" => {
                                b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n12345678901".to_vec()
                            }
                            "/exact" => http_response("application/octet-stream", b"1234567890"),
                            _ => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                        };
                        let _ = stream.write_all(&response);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("fixture accept failed: {error}"),
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

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn http_response(content_type: &str, body: &[u8]) -> Vec<u8> {
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    [headers.as_bytes(), body].concat()
}

fn unique_temp_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "chrome-agent-download-limit-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn download_rejects_declared_and_streamed_overflow_without_writing_a_file() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }

    let server = FixtureServer::start();
    let browser = format!("test-download-limit-{}", std::process::id());
    let _browser_guard = BrowserGuard(browser.clone());
    let temp_dir = unique_temp_dir();

    let root = server.url("/");
    let goto = run(&browser, &["goto", &root]);
    assert!(
        goto.status.success(),
        "goto failed: {}",
        String::from_utf8_lossy(&goto.stderr)
    );

    for path in ["/declared", "/streamed"] {
        let output_path = temp_dir.join(path.trim_start_matches('/'));
        let url = server.url(path);
        let output = run(
            &browser,
            &[
                "download",
                &url,
                "--out",
                output_path.to_str().unwrap(),
                "--max-bytes",
                "10",
            ],
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "oversized {path} unexpectedly succeeded");
        assert!(
            stderr.contains("download exceeded 10 bytes"),
            "unexpected error for {path}: {stderr}"
        );
        assert!(!stderr.contains("12345678901"), "response bytes leaked in error output");
        assert!(!output_path.exists(), "rejected download wrote {output_path:?}");
    }

    let exact_path = temp_dir.join("exact");
    let exact_url = server.url("/exact");
    let exact = run(
        &browser,
        &[
            "download",
            &exact_url,
            "--out",
            exact_path.to_str().unwrap(),
            "--max-bytes",
            "10",
        ],
    );
    assert!(
        exact.status.success(),
        "exact-limit download failed: {}",
        String::from_utf8_lossy(&exact.stderr)
    );
    assert_eq!(std::fs::read(&exact_path).unwrap(), b"1234567890");

    std::fs::remove_dir_all(temp_dir).unwrap();
}
