use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Options for launching or connecting to a browser.
#[allow(clippy::struct_excessive_bools)]
pub struct BrowserOptions {
    pub name: String,
    pub headless: bool,
    pub ignore_https_errors: bool,
    pub stealth: bool,
    pub connect: Option<String>,
    pub copy_cookies: bool,
}

impl Default for BrowserOptions {
    fn default() -> Self {
        Self {
            name: "default".into(),
            headless: false,
            ignore_https_errors: false,
            stealth: false,
            connect: None,
            copy_cookies: false,
        }
    }
}

/// Result of resolving a browser connection.
pub struct BrowserConnection {
    /// WebSocket endpoint for the browser (Target.* commands).
    pub ws_endpoint: String,
    /// HTTP base URL for /json/list queries.
    pub http_endpoint: Option<String>,
    pub pid: Option<u32>,
}

/// Fetch the page-specific WebSocket URL for a given target ID.
/// Queries /json/list on the browser's HTTP endpoint.
pub async fn get_page_ws_url(
    http_endpoint: &str,
    target_id: &str,
) -> Result<String, BrowserError> {
    let url = format!("{}/json/list", http_endpoint.trim_end_matches('/'));

    // Retry a few times — Chrome may not be fully ready yet
    let mut last_err = BrowserError::NotFound("No attempts made".into());
    for _ in 0..5 {
        match http_get_json(&url, Duration::from_millis(2000)).await {
            Ok(list) => {
                if let Some(pages) = list.as_array() {
                    for page in pages {
                        let id = page.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        if id == target_id
                            && let Some(ws) = page.get("webSocketDebuggerUrl").and_then(|v| v.as_str()) {
                                return Ok(ws.to_string());
                            }
                    }
                    // Target not found in list — might not be created yet
                    last_err = BrowserError::NotFound(format!(
                        "Target {target_id} not found in /json/list"
                    ));
                }
            }
            Err(e) => {
                last_err = e;
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    Err(last_err)
}

/// Validate a browser profile name. Prevents path traversal via `--browser "../../etc"`.
pub fn validate_browser_name(name: &str) -> Result<(), BrowserError> {
    if name.is_empty() {
        return Err(BrowserError::Launch("Browser name cannot be empty".into()));
    }
    if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err(BrowserError::Launch(
            "Browser name must contain only alphanumeric characters, hyphens, and underscores".into(),
        ));
    }
    Ok(())
}

/// Resolve a browser connection: either connect to an existing Chrome or launch one.
pub async fn resolve_browser(opts: &BrowserOptions) -> Result<BrowserConnection, BrowserError> {
    validate_browser_name(&opts.name)?;
    if let Some(endpoint) = &opts.connect {
        if endpoint == "auto" {
            return auto_discover().await;
        }
        if endpoint.starts_with("ws://") || endpoint.starts_with("wss://") {
            return Ok(BrowserConnection {
                ws_endpoint: endpoint.clone(),
                http_endpoint: Some(extract_http_endpoint(endpoint)),
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
/// Uses a lock file to prevent concurrent launches from racing.
async fn launch_browser(opts: &BrowserOptions) -> Result<BrowserConnection, BrowserError> {
    let profile_dir = browser_profile_dir(&opts.name)?;
    std::fs::create_dir_all(&profile_dir).map_err(|e| {
        BrowserError::Launch(format!("Failed to create profile dir: {e}"))
    })?;

    // Copy cookies from the user's real Chrome profile if requested
    if opts.copy_cookies {
        copy_chrome_cookies(&profile_dir)?;
    }

    // Prevent concurrent launches: if DevToolsActivePort already exists, wait for it
    // Check for existing DevToolsActivePort — another process may have launched Chrome
    let port_file = profile_dir.join("DevToolsActivePort");
    if port_file.exists() {
        if let Some(ws) = read_devtools_active_port(&port_file) {
            // Verify the WebSocket is actually reachable (not stale)
            let http = extract_http_endpoint(&ws);
            if http_get_json(
                &format!("{http}/json/version"),
                Duration::from_millis(1000),
            )
            .await
            .is_ok()
            {
                return Ok(BrowserConnection {
                    ws_endpoint: ws,
                    http_endpoint: Some(http),
                    pid: None,
                });
            }
        }
        // Port file exists but Chrome is dead — remove stale file and launch fresh
        let _ = std::fs::remove_file(&port_file);
    }

    let chromium_path = find_chromium()?;

    let mut cmd = Command::new(&chromium_path);
    cmd.arg(format!("--user-data-dir={}", profile_dir.display()));
    cmd.arg("--remote-debugging-port=0"); // auto-assign port
    cmd.arg("--no-first-run");
    cmd.arg("--no-default-browser-check");
    // Prevent Chrome from merging into an existing instance on macOS
    cmd.arg("--new-window");
    cmd.arg("--disable-background-timer-throttling");
    cmd.arg("--disable-backgrounding-occluded-windows");
    cmd.arg("--disable-renderer-backgrounding");
    // Keep Chrome alive even when all tabs are closed (critical for headed mode)
    cmd.arg("--keep-alive-for-test");
    cmd.arg("--disable-session-crashed-bubble");

    if opts.headless {
        cmd.arg("--headless=new");
    } else {
        // Headed mode: open about:blank to keep Chrome alive between commands.
        // Without this, Chrome exits when the navigated page tab is the only one.
        cmd.arg("about:blank");
    }

    if opts.ignore_https_errors {
        cmd.arg("--ignore-certificate-errors");
    }

    if opts.stealth {
        // Suppress the "Chrome is being controlled by automated test software" infobar
        cmd.arg("--disable-infobars");
        // Exclude automation-related Chrome switches from navigator.userAgent
        cmd.arg("--disable-component-extensions-with-background-pages");
    }

    cmd.stdin(Stdio::null());
    if opts.headless {
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
    } else {
        // In headed mode, let Chrome write to inherited stderr.
        // Suppressing it causes issues with DevTools port binding on macOS.
        cmd.stdout(Stdio::null());
    }

    let child = cmd.spawn().map_err(|e| {
        BrowserError::Launch(format!("Failed to launch {}: {e}", chromium_path.display()))
    })?;

    let pid = child.id();

    // Wait for DevToolsActivePort to appear
    let port_file = profile_dir.join("DevToolsActivePort");
    let ws_endpoint = wait_for_devtools_port(&port_file, Duration::from_secs(10)).await?;

    // Extract http endpoint from ws URL: ws://127.0.0.1:PORT/... → http://127.0.0.1:PORT
    let http_endpoint = extract_http_endpoint(&ws_endpoint);

    Ok(BrowserConnection {
        ws_endpoint,
        http_endpoint: Some(http_endpoint),
        pid: Some(pid),
    })
}

/// Auto-discover a running Chrome instance with remote debugging enabled.
async fn auto_discover() -> Result<BrowserConnection, BrowserError> {
    // 1. Check DevToolsActivePort files from known Chrome profile paths
    for candidate in devtools_active_port_candidates() {
        if let Some(ws) = read_devtools_active_port(&candidate)
            && probe_ws_endpoint(&ws).await {
                return Ok(BrowserConnection {
                    http_endpoint: Some(extract_http_endpoint(&ws)),
                    ws_endpoint: ws,
                    pid: None,
                });
            }
    }

    // 2. Probe common debugging ports
    for port in DISCOVERY_PORTS {
        if let Ok(ws) = fetch_ws_endpoint(&format!("http://127.0.0.1:{port}")).await {
            return Ok(BrowserConnection {
                http_endpoint: Some(format!("http://127.0.0.1:{port}")),
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
             If Chrome uses built-in remote debugging, run `aibrowsr --connect` \
             without a URL for auto-discovery."
        ))
    })?;

    Ok(BrowserConnection {
        http_endpoint: Some(endpoint.trim_end_matches('/').to_string()),
        ws_endpoint: ws,
        pid: None,
    })
}

/// Extract an HTTP endpoint from a WebSocket URL.
/// `ws://127.0.0.1:9222/devtools/browser/...` → `http://127.0.0.1:9222`
pub fn extract_http_from_ws(ws_url: &str) -> String {
    extract_http_endpoint(ws_url)
}

fn extract_http_endpoint(ws_url: &str) -> String {
    let without_scheme = ws_url
        .strip_prefix("ws://")
        .or_else(|| ws_url.strip_prefix("wss://"))
        .unwrap_or(ws_url);
    let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);
    format!("http://{host_port}")
}

/// Fetch the webSocketDebuggerUrl from a /json/version endpoint.
async fn fetch_ws_endpoint(base_url: &str) -> Result<String, BrowserError> {
    let url = format!(
        "{}/json/version",
        base_url.trim_end_matches('/')
    );

    let response = http_get_json(&url, Duration::from_millis(2000)).await?;

    let ws_url = response
        .get("webSocketDebuggerUrl")
        .and_then(|v| v.as_str())
        .ok_or_else(|| BrowserError::NotFound("No webSocketDebuggerUrl in /json/version".into()))?;

    Ok(ws_url.to_string())
}

/// HTTP GET that returns JSON. Uses ureq (blocking, run on tokio `spawn_blocking`).
async fn http_get_json(
    url: &str,
    timeout: Duration,
) -> Result<serde_json::Value, BrowserError> {
    let url = url.to_string();
    

    tokio::task::spawn_blocking(move || {
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(timeout))
            .timeout_recv_body(Some(timeout))
            .build()
            .new_agent();

        let body = agent
            .get(&url)
            .header("Accept", "application/json")
            .call()
            .map_err(|e| BrowserError::NotFound(format!("HTTP request failed: {e}")))?
            .body_mut()
            .read_to_string()
            .map_err(|e| BrowserError::NotFound(format!("Failed to read body: {e}")))?;

        serde_json::from_str(&body)
            .map_err(|e| BrowserError::NotFound(format!("Invalid JSON: {e}")))
    })
    .await
    .map_err(|e| BrowserError::NotFound(format!("Task failed: {e}")))?
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

/// Wait for `DevToolsActivePort` file to appear and parse it.
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

/// Parse a `DevToolsActivePort` file: line 1 = port, line 2 = ws path.
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

/// `DevToolsActivePort` file candidates per platform.
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
        if cfg!(target_os = "linux")
            && let Ok(output) = Command::new("which").arg(candidate).output()
                && output.status.success() {
                    let found = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !found.is_empty() {
                        return Ok(PathBuf::from(found));
                    }
                }
    }

    Err(BrowserError::NotFound(
        "Could not find Chrome or Chromium. Install Chrome and ensure it's on your PATH."
            .into(),
    ))
}

/// Copy cookies (and Local State for decryption key) from the user's real Chrome profile.
/// This gives the launched headless Chrome access to the user's logged-in sessions.
fn copy_chrome_cookies(profile_dir: &Path) -> Result<(), BrowserError> {
    let chrome_default = chrome_default_profile_dir()?;
    let cookies_src = chrome_default.join("Cookies");
    if !cookies_src.exists() {
        return Err(BrowserError::Launch(
            "Chrome cookies file not found. Is Chrome installed?".into(),
        ));
    }

    // Copy Cookies database
    let cookies_dst = profile_dir.join("Default");
    std::fs::create_dir_all(&cookies_dst).map_err(|e| {
        BrowserError::Launch(format!("Failed to create Default dir: {e}"))
    })?;
    std::fs::copy(&cookies_src, cookies_dst.join("Cookies")).map_err(|e| {
        BrowserError::Launch(format!("Failed to copy Cookies: {e}"))
    })?;
    // Also copy WAL/SHM if they exist (SQLite journal files)
    for ext in ["Cookies-journal", "Cookies-wal", "Cookies-shm"] {
        let src = chrome_default.join(ext);
        if src.exists() {
            let _ = std::fs::copy(&src, cookies_dst.join(ext));
        }
    }

    // Copy Local State (contains the encryption key for cookies on macOS/Windows)
    let local_state_src = chrome_default.parent().map(|p| p.join("Local State"));
    if let Some(src) = local_state_src
        && src.exists() {
            let dst = profile_dir.join("Local State");
            let _ = std::fs::copy(&src, dst);
        }

    eprintln!("Copied cookies from Chrome profile");
    Ok(())
}

/// Locate the user's default Chrome profile directory.
fn chrome_default_profile_dir() -> Result<PathBuf, BrowserError> {
    let base = if cfg!(target_os = "macos") {
        dirs::home_dir().map(|h| h.join("Library/Application Support/Google/Chrome/Default"))
    } else if cfg!(target_os = "windows") {
        dirs::data_local_dir().map(|d| d.join("Google/Chrome/User Data/Default"))
    } else {
        dirs::config_dir().map(|c| c.join("google-chrome/Default"))
    };
    base.ok_or_else(|| BrowserError::Launch("Could not locate Chrome profile directory".into()))
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

#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    #[error("{0}")]
    Launch(String),
    #[error("{0}")]
    NotFound(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_browser_name_accepts_valid() {
        assert!(validate_browser_name("default").is_ok());
        assert!(validate_browser_name("my-browser").is_ok());
        assert!(validate_browser_name("test_123").is_ok());
    }

    #[test]
    fn validate_browser_name_rejects_traversal() {
        assert!(validate_browser_name("../../etc").is_err());
        assert!(validate_browser_name("").is_err());
        assert!(validate_browser_name("foo bar").is_err());
        assert!(validate_browser_name("foo/bar").is_err());
    }

    #[test]
    fn extract_http_from_ws_works() {
        assert_eq!(
            extract_http_from_ws("ws://127.0.0.1:9222/devtools/browser/abc"),
            "http://127.0.0.1:9222"
        );
        assert_eq!(
            extract_http_from_ws("wss://host:443/path"),
            "http://host:443"
        );
    }

    #[test]
    fn read_devtools_active_port_parses_correctly() {
        let dir = std::env::temp_dir().join("aibrowsr_test_devtools");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("DevToolsActivePort");
        std::fs::write(&path, "9222\n/devtools/browser/abc-123\n").unwrap();
        let result = read_devtools_active_port(&path);
        assert_eq!(
            result,
            Some("ws://127.0.0.1:9222/devtools/browser/abc-123".into())
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_devtools_active_port_rejects_invalid() {
        let dir = std::env::temp_dir().join("aibrowsr_test_devtools_bad");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("DevToolsActivePort");
        std::fs::write(&path, "not_a_number\n").unwrap();
        assert!(read_devtools_active_port(&path).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
