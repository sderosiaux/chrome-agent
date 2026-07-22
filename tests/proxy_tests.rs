use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn binary() -> String {
    let mut path = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    path.push("chrome-agent");
    path.to_string_lossy().into_owned()
}

fn chrome_available() -> bool {
    ["google-chrome", "chromium"].iter().any(|candidate| {
        std::path::Path::new(candidate).exists()
            || Command::new("which")
                .arg(candidate)
                .output()
                .is_ok_and(|output| output.status.success())
    })
}

struct BrowserGuard(&'static str);

impl Drop for BrowserGuard {
    fn drop(&mut self) {
        let _ = Command::new(binary())
            .args(["--browser", self.0, "close", "--purge"])
            .output();
    }
}

#[test]
fn managed_browser_routes_navigation_through_proxy() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    let (sender, receiver) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut request = [0_u8; 8192];
                    let size = stream.read(&mut request).unwrap_or(0);
                    let text = String::from_utf8_lossy(&request[..size]);
                    let first_line = text.lines().next().unwrap_or("").to_string();
                    let body = "<html><title>Scout proxy fixture</title><main>proxied</main></html>";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                    if first_line.contains("scout-proxy.invalid") {
                        let _ = sender.send(first_line);
                        return;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => panic!("proxy accept failed: {error}"),
            }
        }
    });

    let browser = "test-managed-proxy";
    let _guard = BrowserGuard(browser);
    let output = Command::new(binary())
        .args([
            "--browser",
            browser,
            "--proxy-server",
            &proxy,
            "goto",
            "http://scout-proxy.invalid/",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "chrome-agent failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request_line = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(request_line.starts_with("GET http://scout-proxy.invalid/"));
    server.join().unwrap();
}
