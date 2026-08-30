//! `network --live --body`: a URL filter is an explicit selection, so it overrides the
//! MIME allowlist (issue #27 — an `application/yaml` response matched `--filter` and its
//! body was silently omitted), and a binary body is counted rather than printed.
//!
//! The server is real: a `TcpListener` on a loopback port serving one YAML and one
//! binary response, fetched on an interval by a fixture page so the requests happen
//! while the capture window is open.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::process::Command;

use serde_json::Value;

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

fn run_json(args: &[&str]) -> Value {
    let output = Command::new(binary())
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

/// Serve `/config.yaml` (textual, non-allowlisted MIME) and `/blob.bin` (256 bytes that
/// force `base64Encoded: true`) until the process exits. `Access-Control-Allow-Origin: *`
/// because the fixture fetches from a `file://` origin.
fn spawn_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = [0u8; 2048];
            let Ok(n) = stream.read(&mut request) else { continue };
            let request = String::from_utf8_lossy(&request[..n]);
            let (mime, body): (&str, Vec<u8>) = if request.starts_with("GET /config.yaml") {
                (
                    "application/yaml",
                    b"retries: 3\nname: chrome-agent-e2e\n".to_vec(),
                )
            } else {
                ("application/octet-stream", (0u8..=255).collect())
            };
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
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

    // Filtered: application/yaml is not on the allowlist, and the filter overrides it.
    let filtered = run_json(&[
        "--browser", &browser, "--json",
        "network", "--live", "6", "--body", "--filter", "config.yaml",
    ]);
    let yaml = entry_for(&filtered, "/config.yaml");
    assert_eq!(yaml["contentType"], "application/yaml");
    assert!(
        yaml["body"].as_str().unwrap_or("").contains("retries: 3"),
        "filtered yaml body was not captured: {yaml}"
    );

    // Filtered binary: selected, fetched, counted — never printed.
    let blob = run_json(&[
        "--browser", &browser, "--json",
        "network", "--live", "6", "--body", "--filter", "blob.bin",
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
        "--browser", &browser, "--json",
        "network", "--live", "4", "--body",
    ]);
    let yaml = entry_for(&unfiltered, "/config.yaml");
    assert!(
        yaml["body"].is_null(),
        "unfiltered --body fetched a non-allowlisted type: {yaml}"
    );

}
