use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, Instant};

mod common;
use common::TestBrowser;

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

/// Answer one proxied connection with the fixture page.
///
/// One thread per connection, which is what `browser_lab_regressions`' fixture server already
/// does and says why: Chrome opens speculative sockets that carry no request, and serving them
/// in the accept loop makes the next connection wait out this one's read timeout. A managed
/// Chrome also sends its own background traffic through the proxy, so "the next connection" is
/// not a rare case here — it is most of them.
fn serve(mut stream: std::net::TcpStream) {
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
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
    // Not joined: the deadline bounds it, and waiting for it after the assertion has
    // passed would add fifteen seconds to a green run.
    let _server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut workers = Vec::new();
        while Instant::now() < deadline {
            match listener.accept() {
                // One thread per connection, which is what `browser_lab_regressions`'
                // fixture server already does and says why: Chrome opens speculative
                // sockets that carry no request, and serving them in the accept loop makes
                // the NEXT connection — the real one — wait out this one's read timeout.
                // Idle here, that cost nothing; with a second test process on the machine
                // Chrome preconnects more and gives up on the proxy first, so the page came
                // back as an error page, `goto` reported success on it (it reports where it
                // landed, not what served it), and the request line never arrived. Measured
                // under a concurrent suite: 7.40s and a failure, against 2.01s alone.
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
    let output = Command::new(binary())
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

    // The claim is asserted on the PAGE, not on a request line off a channel. `.invalid` is
    // reserved by RFC 6761 and resolves nowhere, so a document carrying the fixture's title
    // can only have come from this proxy — which is the whole claim, and it is proved by a
    // value on the response rather than by a race.
    //
    // What was there before: a `mpsc::channel` fed by the proxy thread, read with a two-second
    // budget. It passed on an idle machine and failed with a second test process running,
    // twice measured. The reason is visible once the proxy prints everything it receives: a
    // managed Chrome sends its OWN traffic through the proxy too — a time-sync GET, then
    // CONNECTs for clientservices, gstatic, accounts.google.com, nineteen connections in the
    // 5.4 s of one navigation — and the page's own request is one line among them, arriving
    // whenever the machine gets round to it. `goto` reports where it landed, not what served
    // it, so a success proved nothing on its own and the channel proved it unreliably.
    let response: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("goto did not answer JSON ({e}): {:?}", String::from_utf8_lossy(&output.stdout)));
    assert_eq!(
        response["title"], "Scout proxy fixture",
        "the page did not come from the proxy: {response}"
    );
}
