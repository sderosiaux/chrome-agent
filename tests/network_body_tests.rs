//! `network --live --body`: a URL filter is an explicit selection and overrides the MIME
//! allowlist (issue #27), a binary body is counted rather than printed, and reaching the entry
//! limit does not abandon a selected body between response headers and `loadingFinished`.
//!
//! The server is a real `TcpListener` on a loopback port, fetched on an interval by a fixture
//! page so the requests fall inside the capture window.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::process::Command;

use serde_json::Value;

mod common;
use common::TestBrowser;

fn run_json(args: &[&str]) -> Value {
    let output = Command::new(common::binary())
        .args(args)
        .output()
        .expect("run chrome-agent");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert_eq!(
        output.status.code(),
        Some(0),
        "command failed: {args:?}\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("invalid JSON for {args:?}: {error}\nstdout: {stdout}"))
}

/// Serve `/config.yaml` (non-allowlisted MIME), `/blob.bin` (256 bytes, forcing
/// `base64Encoded: true`), and a JSON body delayed after its headers. CORS is open because the
/// fixture fetches from a `file://` origin.
fn spawn_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = [0u8; 2048];
            let Ok(n) = stream.read(&mut request) else {
                continue;
            };
            let request = String::from_utf8_lossy(&request[..n]);
            let (mime, body, delay): (&str, Vec<u8>, Option<std::time::Duration>) =
                if request.starts_with("GET /config.yaml") {
                    (
                        "application/yaml",
                        b"retries: 3\nname: chrome-agent-e2e\n".to_vec(),
                        None,
                    )
                } else if request.starts_with("GET /slow.json") {
                    (
                        "application/json",
                        br#"{"complete":true}"#.to_vec(),
                        Some(std::time::Duration::from_millis(750)),
                    )
                } else {
                    ("application/octet-stream", (0u8..=255).collect(), None)
                };
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.flush();
            if let Some(delay) = delay {
                std::thread::sleep(delay);
            }
            let _ = stream.write_all(&body);
        }
    });
    port
}

fn entry_for<'a>(response: &'a Value, path: &str) -> &'a Value {
    response["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["url"].as_str().unwrap_or("").contains(path))
        .unwrap_or_else(|| panic!("no captured entry for {path}: {response}"))
}

#[test]
fn a_filter_is_a_selection_and_a_binary_body_is_counted_not_printed() {
    if !common::browser_ready() {
        return;
    }
    let port = spawn_server();
    let guard = TestBrowser::new("network-body");
    let browser = guard.name().to_string();
    let probe = format!(
        "{}#http://127.0.0.1:{port}",
        common::fixture_url("network_body_probe.html")
    );

    assert_eq!(
        run_json(&["--browser", &browser, "--json", "goto", &probe])["ok"],
        true
    );

    // application/yaml is not on the allowlist, and the filter overrides it.
    let filtered = run_json(&[
        "--browser",
        &browser,
        "--json",
        "network",
        "--live",
        "6",
        "--body",
        "--filter",
        "config.yaml",
    ]);
    let yaml = entry_for(&filtered, "/config.yaml");
    assert_eq!(yaml["contentType"], "application/yaml");
    assert!(
        yaml["body"].as_str().unwrap_or("").contains("retries: 3"),
        "filtered yaml body was not captured: {yaml}"
    );

    // Filtered binary: selected, fetched, counted — never printed.
    let blob = run_json(&[
        "--browser",
        &browser,
        "--json",
        "network",
        "--live",
        "6",
        "--body",
        "--filter",
        "blob.bin",
    ]);
    let bin = entry_for(&blob, "/blob.bin");
    assert!(bin["body"].is_null(), "binary body was printed: {bin}");
    let omitted = bin["bodyOmitted"].as_str().unwrap_or("");
    assert!(
        omitted.contains("256 bytes") && omitted.contains("download"),
        "omission does not say what or how: {bin}"
    );

    // Unfiltered: the allowlist still guards --body, so the yaml body stays out.
    let unfiltered = run_json(&[
        "--browser",
        &browser,
        "--json",
        "network",
        "--live",
        "4",
        "--body",
    ]);
    let yaml = entry_for(&unfiltered, "/config.yaml");
    assert!(
        yaml["body"].is_null(),
        "unfiltered --body fetched a non-allowlisted type: {yaml}"
    );
}

#[test]
fn reaching_the_entry_limit_does_not_report_a_requested_body_as_absent() {
    if !common::browser_ready() {
        return;
    }
    let port = spawn_server();
    let guard = TestBrowser::new("network-body-limit");
    let browser = guard.name().to_string();
    let probe = format!(
        "{}?slow#http://127.0.0.1:{port}",
        common::fixture_url("network_body_probe.html")
    );

    assert_eq!(
        run_json(&["--browser", &browser, "--json", "goto", &probe])["ok"],
        true
    );

    let captured = run_json(&[
        "--browser",
        &browser,
        "--json",
        "network",
        "--live",
        "4",
        "--body",
        "--filter",
        "slow.json",
        "--limit",
        "1",
    ]);
    let slow = entry_for(&captured, "/slow.json");
    assert_eq!(
        slow["body"],
        serde_json::json!(r#"{"complete":true}"#),
        "ok:true omitted the requested body without an explanation: {slow}"
    );
}
