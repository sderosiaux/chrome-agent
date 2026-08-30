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
