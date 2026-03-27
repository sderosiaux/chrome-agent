use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Options for launching or connecting to a browser.
pub struct BrowserOptions {
    pub name: String,
    pub headless: bool,
    pub ignore_https_errors: bool,
    pub connect: Option<String>,
}

impl Default for BrowserOptions {
    fn default() -> Self {
        Self {
            name: "default".into(),
            headless: false,
            ignore_https_errors: false,
            connect: None,
        }
    }
}

/// Result of resolving a browser connection.
pub struct BrowserConnection {
    pub ws_endpoint: String,
    pub pid: Option<u32>,
}

/// Resolve a browser connection: either connect to an existing Chrome or launch one.
pub async fn resolve_browser(opts: &BrowserOptions) -> Result<BrowserConnection, BrowserError> {
    if let Some(endpoint) = &opts.connect {
        if endpoint == "auto" {
            return auto_discover().await;
        }
        if endpoint.starts_with("ws://") || endpoint.starts_with("wss://") {
            return Ok(BrowserConnection {
                ws_endpoint: endpoint.clone(),
                pid: None,
            });
        }
        // HTTP endpoint — resolve to WebSocket via /json/version
        return resolve_http_endpoint(endpoint).await;
    }

    // No --connect: launch a managed browser
    launch_browser(opts).await
}

/// Launch a Chromium instance with remote debugging.
async fn launch_browser(opts: &BrowserOptions) -> Result<BrowserConnection, BrowserError> {
    let profile_dir = browser_profile_dir(&opts.name)?;
    std::fs::create_dir_all(&profile_dir).map_err(|e| {
        BrowserError::Launch(format!("Failed to create profile dir: {e}"))
    })?;

    let chromium_path = find_chromium()?;

    let mut cmd = Command::new(&chromium_path);
    cmd.arg(format!("--user-data-dir={}", profile_dir.display()));
    cmd.arg("--remote-debugging-port=0"); // auto-assign port
    cmd.arg("--no-first-run");
    cmd.arg("--no-default-browser-check");
    cmd.arg("--disable-background-timer-throttling");
    cmd.arg("--disable-backgrounding-occluded-windows");
    cmd.arg("--disable-renderer-backgrounding");

    if opts.headless {
        cmd.arg("--headless=new");
    }

    if opts.ignore_https_errors {
        cmd.arg("--ignore-certificate-errors");
    }

    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());

    let child = cmd.spawn().map_err(|e| {
        BrowserError::Launch(format!("Failed to launch {}: {e}", chromium_path.display()))
    })?;

    let pid = child.id();

    // Wait for DevToolsActivePort to appear
    let port_file = profile_dir.join("DevToolsActivePort");
    let ws_endpoint = wait_for_devtools_port(&port_file, Duration::from_secs(10)).await?;

    Ok(BrowserConnection {
        ws_endpoint,
        pid: Some(pid),
    })
}

/// Auto-discover a running Chrome instance with remote debugging enabled.
async fn auto_discover() -> Result<BrowserConnection, BrowserError> {
    // 1. Check DevToolsActivePort files from known Chrome profile paths
    for candidate in devtools_active_port_candidates() {
        if let Some(ws) = read_devtools_active_port(&candidate) {
            if probe_ws_endpoint(&ws).await {
                return Ok(BrowserConnection {
                    ws_endpoint: ws,
                    pid: None,
                });
            }
        }
    }

    // 2. Probe common debugging ports
    for port in DISCOVERY_PORTS {
        if let Ok(ws) = fetch_ws_endpoint(&format!("http://127.0.0.1:{port}")).await {
            return Ok(BrowserConnection {
                ws_endpoint: ws,
                pid: None,
            });
        }
    }

    Err(BrowserError::NotFound(auto_connect_error_message()))
}

/// Resolve an HTTP endpoint to a WebSocket URL via /json/version.
async fn resolve_http_endpoint(endpoint: &str) -> Result<BrowserConnection, BrowserError> {
    let ws = fetch_ws_endpoint(endpoint).await.map_err(|_| {
        BrowserError::NotFound(format!(
            "Could not resolve CDP WebSocket from {endpoint}. \
             If Chrome uses built-in remote debugging, run `dev-browser --connect` \
             without a URL for auto-discovery."
        ))
    })?;

    Ok(BrowserConnection {
        ws_endpoint: ws,
        pid: None,
    })
}

/// Fetch the webSocketDebuggerUrl from a /json/version endpoint.
async fn fetch_ws_endpoint(base_url: &str) -> Result<String, BrowserError> {
    let url = format!(
        "{}/json/version",
        base_url.trim_end_matches('/')
    );

    let response = reqwest_get_json(&url, Duration::from_millis(2000)).await?;

    let ws_url = response
        .get("webSocketDebuggerUrl")
        .and_then(|v| v.as_str())
        .ok_or_else(|| BrowserError::NotFound("No webSocketDebuggerUrl in /json/version".into()))?;

    Ok(ws_url.to_string())
}

/// Minimal HTTP GET that returns JSON. Uses tokio's TCP + manual HTTP/1.1.
/// We avoid pulling in reqwest/hyper to keep dependencies minimal.
async fn reqwest_get_json(
    url: &str,
    timeout: Duration,
) -> Result<serde_json::Value, BrowserError> {
    // Parse URL manually to extract host, port, path
    let url_str = url.strip_prefix("http://").unwrap_or(url);
    let (host_port, path) = match url_str.find('/') {
        Some(i) => (&url_str[..i], &url_str[i..]),
        None => (url_str, "/"),
    };

    let addr = format!("{host_port}");
    let stream = tokio::time::timeout(
        timeout,
        tokio::net::TcpStream::connect(&addr),
    )
    .await
    .map_err(|_| BrowserError::NotFound(format!("Timeout connecting to {addr}")))?
    .map_err(|e| BrowserError::NotFound(format!("Cannot connect to {addr}: {e}")))?;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut reader, mut writer) = stream.into_split();

    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host_port}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    writer.write_all(request.as_bytes()).await.map_err(|e| {
        BrowserError::NotFound(format!("Write failed: {e}"))
    })?;

    let mut buf = Vec::with_capacity(4096);
    tokio::time::timeout(timeout, async {
        reader.read_to_end(&mut buf).await
    })
    .await
    .map_err(|_| BrowserError::NotFound("Timeout reading response".into()))?
    .map_err(|e| BrowserError::NotFound(format!("Read failed: {e}")))?;

    let response = String::from_utf8_lossy(&buf);
    let body = response
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or("");

    serde_json::from_str(body)
        .map_err(|e| BrowserError::NotFound(format!("Invalid JSON from {url}: {e}")))
}

/// Check if a WebSocket endpoint is reachable.
async fn probe_ws_endpoint(ws_url: &str) -> bool {
    // Try connecting with a short timeout
    tokio::time::timeout(
        Duration::from_millis(500),
        tokio_tungstenite::connect_async(ws_url),
    )
    .await
    .is_ok_and(|r| r.is_ok())
}

/// Wait for DevToolsActivePort file to appear and parse it.
async fn wait_for_devtools_port(
    path: &Path,
    timeout: Duration,
) -> Result<String, BrowserError> {
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        if let Some(ws) = read_devtools_active_port(path) {
            return Ok(ws);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Err(BrowserError::Launch(format!(
        "DevToolsActivePort did not appear at {} within {}s",
        path.display(),
        timeout.as_secs()
    )))
}

/// Parse a DevToolsActivePort file: line 1 = port, line 2 = ws path.
fn read_devtools_active_port(path: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    let mut lines = contents.lines();
    let port: u16 = lines.next()?.trim().parse().ok()?;
    let ws_path = lines.next()?.trim();

    if port == 0 || !ws_path.starts_with("/devtools/browser/") {
        return None;
    }

    Some(format!("ws://127.0.0.1:{port}{ws_path}"))
}

/// DevToolsActivePort file candidates per platform.
fn devtools_active_port_candidates() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };

    if cfg!(target_os = "macos") {
        let base = home.join("Library").join("Application Support");
        vec![
            base.join("Google/Chrome/DevToolsActivePort"),
            base.join("Google/Chrome Canary/DevToolsActivePort"),
            base.join("Chromium/DevToolsActivePort"),
            base.join("BraveSoftware/Brave-Browser/DevToolsActivePort"),
        ]
    } else if cfg!(target_os = "linux") {
        let config = home.join(".config");
        vec![
            config.join("google-chrome/DevToolsActivePort"),
            config.join("chromium/DevToolsActivePort"),
            config.join("google-chrome-beta/DevToolsActivePort"),
            config.join("google-chrome-unstable/DevToolsActivePort"),
            config.join("BraveSoftware/Brave-Browser/DevToolsActivePort"),
        ]
    } else if cfg!(target_os = "windows") {
        let local = home.join("AppData").join("Local");
        vec![
            local.join("Google/Chrome/User Data/DevToolsActivePort"),
            local.join("Google/Chrome Beta/User Data/DevToolsActivePort"),
            local.join("Google/Chrome SxS/User Data/DevToolsActivePort"),
            local.join("Chromium/User Data/DevToolsActivePort"),
            local.join("BraveSoftware/Brave-Browser/User Data/DevToolsActivePort"),
        ]
    } else {
        vec![]
    }
}

const DISCOVERY_PORTS: &[u16] = &[9222, 9223, 9224, 9225, 9226, 9227, 9228, 9229];

/// Find the Chromium executable.
fn find_chromium() -> Result<PathBuf, BrowserError> {
    // 1. Check for managed Chromium
    if let Some(home) = dirs::home_dir() {
        let managed = home
            .join(".aibrowsr")
            .join("chromium");

        if cfg!(target_os = "macos") {
            let app = managed.join("Chromium.app/Contents/MacOS/Chromium");
            if app.exists() {
                return Ok(app);
            }
            // Chrome for Testing
            let cft = managed.join("chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing");
            if cft.exists() {
                return Ok(cft);
            }
            let cft_x64 = managed.join("chrome-mac-x64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing");
            if cft_x64.exists() {
                return Ok(cft_x64);
            }
        } else if cfg!(target_os = "linux") {
            let bin = managed.join("chrome");
            if bin.exists() {
                return Ok(bin);
            }
            let cft = managed.join("chrome-linux64/chrome");
            if cft.exists() {
                return Ok(cft);
            }
        }
    }

    // 2. Check system Chrome
    let system_candidates: &[&str] = if cfg!(target_os = "macos") {
        &[
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
        ]
    } else if cfg!(target_os = "linux") {
        &[
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
        ]
    } else if cfg!(target_os = "windows") {
        &[
            "chrome.exe",
        ]
    } else {
        &[]
    };

    for candidate in system_candidates {
        let path = PathBuf::from(candidate);
        if path.exists() {
            return Ok(path);
        }
        // For Linux: check if it's on PATH
        if cfg!(target_os = "linux") {
            if let Ok(output) = Command::new("which").arg(candidate).output() {
                if output.status.success() {
                    let found = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !found.is_empty() {
                        return Ok(PathBuf::from(found));
                    }
                }
            }
        }
    }

    Err(BrowserError::NotFound(
        "Could not find Chrome or Chromium. Run 'dev-browser install' to download one, \
         or install Chrome and ensure it's on your PATH."
            .into(),
    ))
}

/// Get the profile directory for a named browser instance.
fn browser_profile_dir(name: &str) -> Result<PathBuf, BrowserError> {
    let home = dirs::home_dir().ok_or_else(|| {
        BrowserError::Launch("Could not determine home directory".into())
    })?;
    Ok(home.join(".aibrowsr").join("browsers").join(name).join("chromium-profile"))
}

fn auto_connect_error_message() -> String {
    let launch_cmd = if cfg!(target_os = "macos") {
        "/Applications/Google\\ Chrome.app/Contents/MacOS/Google\\ Chrome --remote-debugging-port=9222"
    } else if cfg!(target_os = "windows") {
        "chrome.exe --remote-debugging-port=9222"
    } else {
        "google-chrome --remote-debugging-port=9222"
    };

    format!(
        "Could not auto-discover Chrome with remote debugging enabled.\n\
         Enable at chrome://inspect/#remote-debugging\n\
         or launch with: {launch_cmd}"
    )
}

#[derive(Debug)]
pub enum BrowserError {
    Launch(String),
    NotFound(String),
}

impl std::fmt::Display for BrowserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Launch(msg) => write!(f, "{msg}"),
            Self::NotFound(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for BrowserError {}
