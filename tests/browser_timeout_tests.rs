//! `--timeout` applies to the browser-level client, not only the page client.
//!
//! Two `CdpClient`s are live per run: the page connection and the browser connection
//! (Target.* calls — tab resolution, `tabs`). Only the page client had
//! `set_call_timeout` wired from `--timeout`; the browser client stayed pinned to the
//! hardcoded 30s default, so a wedged browser endpoint ignored the caller's deadline
//! exactly where every run starts: resolving the page target.
//!
//! No real Chrome here — a fake "browser" answers /json/version, accepts the
//! WebSocket handshake, then never answers any CDP call.

use futures_util::StreamExt as _;
use std::process::Command;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

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

/// Serve one port: plain HTTP GETs receive a /json/version answer pointing back at
/// this port; WebSocket upgrades are completed and then starved — frames are read
/// and never answered. Runs until the process exits (thread is detached).
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
                        // Complete the handshake, then read frames forever and
                        // never reply: every CDP call on this socket hangs.
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

    // Target resolution (Target.getTargets on the browser client) is the first CDP
    // call of the run and the fake never answers it. With --timeout 2 the command
    // must fail within a few seconds — not after the 30s hardcoded default.
    // (`goto`, not `tabs`: tabs requires an existing session and bails before
    // connecting; goto is the path that establishes the connection.)
    let started = Instant::now();
    let output = Command::new(binary())
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
