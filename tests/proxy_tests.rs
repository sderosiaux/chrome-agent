use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, Instant};

mod common;
use common::TestBrowser;

/// Answer one proxied connection with the fixture page. Called on its own thread: Chrome opens
/// speculative sockets that carry no request, and a managed Chrome also routes its own
/// background traffic through the proxy.
fn serve(mut stream: std::net::TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut request = [0_u8; 8192];
    let _ = stream.read(&mut request);
    let body = "<html><title>Scout proxy fixture</title><main>proxied</main></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

#[test]
fn managed_browser_routes_navigation_through_proxy() {
    if !common::browser_ready() {
        return;
    }

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    // Not joined: the 15s deadline bounds it, and joining would add that to a green run.
    let _server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut workers = Vec::new();
        while Instant::now() < deadline {
            match listener.accept() {
                // One thread per connection: serving in the accept loop makes the real
                // connection wait out a speculative socket's 2s read timeout.
                Ok((stream, _)) => workers.push(std::thread::spawn(move || serve(stream))),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => panic!("proxy accept failed: {error}"),
            }
        }
        for worker in workers {
            let _ = worker.join();
        }
    });

    let guard = TestBrowser::new("test-managed-proxy");
    let browser = guard.name();
    let output = Command::new(common::binary())
        .args([
            "--browser",
            browser,
            "--proxy-server",
            &proxy,
            "--json",
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

    // Asserted on the PAGE, not on a request line: `.invalid` is reserved by RFC 6761 and
    // resolves nowhere, so a document carrying the fixture's title can only have come from this
    // proxy. `goto` alone proves nothing — it reports where it landed, not what served it.
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "goto did not answer JSON ({e}): {:?}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    assert_eq!(
        response["title"], "Scout proxy fixture",
        "the page did not come from the proxy: {response}"
    );
}
