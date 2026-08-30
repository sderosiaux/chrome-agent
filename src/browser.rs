use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Options for launching or connecting to a browser.
#[derive(Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct BrowserOptions {
    pub name: String,
    pub headless: bool,
    pub ignore_https_errors: bool,
    pub stealth: bool,
    pub connect: Option<String>,
    pub proxy_server: Option<String>,
    pub copy_cookies: bool,
    /// Extra flags for the Chrome command line (`--chrome-arg`), only for a browser this tool
    /// launches. See [`normalized_chrome_args_option`] for the forbidden-flag rules.
    pub chrome_args: Vec<String>,
}

impl Default for BrowserOptions {
    fn default() -> Self {
        Self {
            name: "default".into(),
            headless: false,
            ignore_https_errors: false,
            stealth: false,
            connect: None,
            proxy_server: None,
            copy_cookies: false,
            chrome_args: Vec::new(),
        }
    }
}

/// Result of resolving a browser connection.
pub struct BrowserConnection {
    /// Browser-level WebSocket endpoint (`Target.*` commands).
    pub ws_endpoint: String,
    /// HTTP base URL for /json/list queries.
    pub http_endpoint: Option<String>,
    pub pid: Option<u32>,
}

/// Fetch a target's page WebSocket URL from /json/list on the browser's HTTP endpoint.
pub async fn get_page_ws_url(
    http_endpoint: &str,
    target_id: &str,
) -> Result<String, BrowserError> {
    let url = format!("{}/json/list", http_endpoint.trim_end_matches('/'));

    // Retry a few times — Chrome may not be fully ready yet
    let mut last_err = BrowserError::NotFound("No attempts made".into());
    for _ in 0..5 {
        match http_get_json(&url, Duration::from_secs(2)).await {
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

/// Validate and normalize a managed-browser proxy without ever echoing a submitted value.
pub fn validate_proxy_server(value: &str) -> Result<String, BrowserError> {
    let invalid = || {
        BrowserError::Launch(
            "Invalid proxy server <redacted-proxy>: expected http(s)://host:port or socks4/5://host:port"
                .into(),
        )
    };
    if value.trim() != value || value.chars().any(char::is_whitespace) {
        return Err(invalid());
    }
    let (scheme, remainder) = value.split_once("://").ok_or_else(invalid)?;
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https" | "socks4" | "socks5") {
        return Err(invalid());
    }
    if remainder.contains(['?', '#', '@']) {
        return Err(invalid());
    }
    let authority = remainder.strip_suffix('/').unwrap_or(remainder);
    if authority.contains('/') || authority.is_empty() {
        return Err(invalid());
    }
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, suffix) = bracketed.split_once(']').ok_or_else(invalid)?;
        // Brackets are reserved for IPv6 literals; reject a bracketed hostname.
        if host.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(invalid());
        }
        let port = suffix.strip_prefix(':').ok_or_else(invalid)?;
        (format!("[{}]", host.to_ascii_lowercase()), port)
    } else {
        let (host, port) = authority.rsplit_once(':').ok_or_else(invalid)?;
        if host.contains(':') {
            return Err(invalid());
        }
        (host.to_ascii_lowercase(), port)
    };
    if host.is_empty() || port.parse::<u16>().ok().is_none_or(|port| port == 0) {
        return Err(invalid());
    }
    Ok(format!("{scheme}://{host}:{port}"))
}

/// Resolve the launch-only proxy contract shared by CLI, pipe, and replay modes.
pub fn normalized_proxy_option(
    connect: Option<&str>,
    proxy_server: Option<&str>,
) -> Result<Option<String>, BrowserError> {
    if connect.is_some() && proxy_server.is_some() {
        return Err(BrowserError::Launch(
            "--proxy-server applies only when chrome-agent launches Chrome; configure the attached browser's proxy before using --connect"
                .into(),
        ));
    }
    proxy_server.map(validate_proxy_server).transpose()
}

/// `--chrome-arg` validation lives in `chrome_args.rs`, re-exported here.
pub use crate::chrome_args::normalized_chrome_args_option;

/// Resolve a browser connection: either connect to an existing Chrome or launch one.
pub async fn resolve_browser(opts: &BrowserOptions) -> Result<BrowserConnection, BrowserError> {
    validate_browser_name(&opts.name)?;
    let mut resolved = opts.clone();
    resolved.proxy_server = normalized_proxy_option(
        resolved.connect.as_deref(),
        resolved.proxy_server.as_deref(),
    )?;
    resolved.chrome_args =
        normalized_chrome_args_option(resolved.connect.as_deref(), &resolved.chrome_args)?;
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
    launch_browser(&resolved).await
}

/// Launch a Chromium instance with remote debugging.
async fn launch_browser(opts: &BrowserOptions) -> Result<BrowserConnection, BrowserError> {
    let profile_dir = browser_profile_dir(&opts.name)?;
    std::fs::create_dir_all(&profile_dir).map_err(|e| {
        BrowserError::Launch(format!("Failed to create profile dir: {e}"))
    })?;
    // 0700: the profile can hold cookies and the Local State decryption key copied from
    // the user's real Chrome profile (--copy-cookies).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&profile_dir, std::fs::Permissions::from_mode(0o700));
    }

    // A DevToolsActivePort pointing at a live Chrome means reconnect, not a second launch.
    let port_file = profile_dir.join("DevToolsActivePort");
    let existing = try_reconnect_existing(&port_file).await;

    // Only when spawning a *fresh* browser: copying into a live managed Chrome overwrites
    // its in-use SQLite Cookies DB and would never be loaded anyway.
    if should_copy_cookies(opts.copy_cookies, existing.is_some()) {
        copy_chrome_cookies(&profile_dir)?;
    }

    if let Some(conn) = existing {
        return Ok(conn);
    }

    let chromium_path = find_chromium()?;

    let mut cmd = Command::new(&chromium_path);
    cmd.args(managed_launch_args(&profile_dir, opts));

    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());

    let mut child = cmd.spawn().map_err(|e| {
        BrowserError::Launch(format!("Failed to launch {}: {e}", chromium_path.display()))
    })?;

    let pid = child.id();
    // Until `save_session` writes it, this pid lives only in memory. Arming it lets the
    // interrupt and error paths reap it instead of leaking a browser. See `kill::UNPERSISTED`.
    crate::kill::arm(pid);

    // If DevToolsActivePort never appears, kill the child before propagating: `Child`'s drop
    // does not kill it and no pid is persisted yet, so nothing else could reap it.
    let port_file = profile_dir.join("DevToolsActivePort");
    let ws_endpoint = match wait_for_devtools_port(&port_file, Duration::from_secs(10)).await {
        Ok(ws) => ws,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            crate::kill::disarm(pid);
            return Err(e);
        }
    };

    // Extract http endpoint from ws URL: ws://127.0.0.1:PORT/... → http://127.0.0.1:PORT
    let http_endpoint = extract_http_endpoint(&ws_endpoint);

    Ok(BrowserConnection {
        ws_endpoint,
        http_endpoint: Some(http_endpoint),
        pid: Some(pid),
    })
}

fn managed_launch_args(profile_dir: &Path, opts: &BrowserOptions) -> Vec<String> {
    let mut args = vec![
        format!("--user-data-dir={}", profile_dir.display()),
        "--remote-debugging-port=0".into(),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--disable-background-timer-throttling".into(),
        "--disable-backgrounding-occluded-windows".into(),
        "--disable-renderer-backgrounding".into(),
    ];
    if let Some(proxy_server) = &opts.proxy_server {
        args.push(format!("--proxy-server={proxy_server}"));
    }
    if opts.headless {
        args.push("--headless=new".into());
    }
    if opts.ignore_https_errors {
        args.push("--ignore-certificate-errors".into());
    }
    if opts.stealth {
        args.push("--disable-infobars".into());
        args.push("--disable-component-extensions-with-background-pages".into());
    }
    // Last, so a caller's `--chrome-arg` overrides anything above that is not forbidden:
    // Chromium keeps the last value it parses for a repeated switch.
    args.extend(opts.chrome_args.iter().cloned());
    args
}

/// Whether `--copy-cookies` should run for this launch. Only on a fresh spawn: copying into a
/// running managed Chrome overwrites its live `SQLite` Cookies DB and is never loaded.
const fn should_copy_cookies(copy_requested: bool, reconnecting_to_live: bool) -> bool {
    copy_requested && !reconnecting_to_live
}

/// Reconnect to the Chrome a `DevToolsActivePort` file describes, if it is still reachable.
/// `None` when there is no port file or it is stale; a stale one is removed.
async fn try_reconnect_existing(port_file: &Path) -> Option<BrowserConnection> {
    if !port_file.exists() {
        return None;
    }
    if let Some(ws) = read_devtools_active_port(port_file) {
        let http = extract_http_endpoint(&ws);
        if http_get_json(&format!("{http}/json/version"), Duration::from_secs(1))
            .await
            .is_ok()
        {
            return Some(BrowserConnection {
                ws_endpoint: ws,
                http_endpoint: Some(http),
                pid: None,
            });
        }
    }
    // Port file exists but Chrome is dead: drop it and launch fresh.
    let _ = std::fs::remove_file(port_file);
    None
}

/// Kill the browser behind `pid` — on the way to relaunching `browser_name`, or on `close` —
/// and wait for the kernel to agree it is gone. The one copy of that sequence: `cmd_close`
/// kept its own, which removed `browsers_dir()/<name>/DevToolsActivePort`, one directory above
/// the file [`browser_profile_dir`] writes, so it had never removed anything.
///
/// A signal is not an exit. Chrome keeps answering `/json/version` while it tears down and its
/// `DevToolsActivePort` file outlives it, so a [`resolve_browser`] inside that window is met by
/// [`try_reconnect_existing`] handing back the corpse: the "fresh" browser is the one just
/// rejected, in the mode just rejected, recorded with `pid: None` so nothing can reach it
/// again. Waiting for the pid and removing the port file is what makes a relaunch a relaunch.
///
/// Killing goes through [`crate::kill::kill_pid`] — the guard that asks whether the pid is a
/// browser before signalling it, so a recycled pid is left alone. Its three refusals pass
/// through as `Ok`: nothing was signalled, so nothing is mid-teardown.
///
/// `Err` only when the signal landed and the process is still there after the wait. The caller
/// must not relaunch then: everything it would read about that profile is about to change.
/// The guard's own reading rides back on `Ok`, for `close`, which REPORTS the kill rather than
/// relaunching over it: `Err` is the one case it cannot infer (signalled, still there).
pub fn kill_and_await_exit(
    browser_name: &str,
    pid: u32,
) -> Result<crate::kill::KillOutcome, BrowserError> {
    const EXIT_WAIT: Duration = Duration::from_secs(5);

    let outcome = crate::kill::kill_pid(pid);
    if outcome != crate::kill::KillOutcome::Signalled {
        return Ok(outcome);
    }
    if !crate::kill::wait_until_gone(pid, EXIT_WAIT) {
        return Err(BrowserError::Launch(format!(
            "Browser '{browser_name}' (pid={pid}) was signalled to make room for a relaunch, \
             and is still running {}s later. Nothing was relaunched: a Chrome that is shutting \
             down still answers on its DevTools endpoint, so the replacement would have been \
             the same browser. Retry, or run `chrome-agent --browser {browser_name} close`.",
            EXIT_WAIT.as_secs()
        )));
    }
    // Only once the process is gone: a live Chrome owns this file, and deleting it under a
    // browser that survived would strand every later reconnect.
    if let Ok(dir) = browser_profile_dir(browser_name) {
        let _ = std::fs::remove_file(dir.join("DevToolsActivePort"));
    }
    Ok(outcome)
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
    // The cause travels: `fetch_ws_endpoint` already separates connection refused, timeout
    // and malformed JSON, and dropping it left every `--connect` failure looking alike —
    // including to `hints::error_hint`, which branches on "Connection refused".
    let ws = fetch_ws_endpoint(endpoint).await.map_err(|cause| {
        BrowserError::NotFound(format!(
            "Could not resolve CDP WebSocket from {endpoint}: {cause}. \
             If Chrome uses built-in remote debugging, run `chrome-agent --connect` \
             without a URL for auto-discovery."
        ))
    })?;

    Ok(BrowserConnection {
        http_endpoint: Some(endpoint.trim_end_matches('/').to_string()),
        ws_endpoint: ws,
        pid: None,
    })
}

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

    let response = http_get_json(&url, Duration::from_secs(2)).await?;

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
            .join(".chrome-agent")
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
        } else if cfg!(target_os = "windows") {
            let cft64 = managed.join("chrome-win64/chrome.exe");
            if cft64.exists() {
                return Ok(cft64);
            }
            let cft32 = managed.join("chrome-win32/chrome.exe");
            if cft32.exists() {
                return Ok(cft32);
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
        // For Windows: check if it's on PATH
        if cfg!(target_os = "windows")
            && let Ok(output) = Command::new("where").arg(candidate).output()
                && output.status.success() {
                    let found = String::from_utf8_lossy(&output.stdout)
                        .lines().next().unwrap_or("").trim().to_string();
                    if !found.is_empty() {
                        return Ok(PathBuf::from(found));
                    }
                }
    }

    // Windows: standard install locations (not necessarily on PATH)
    if cfg!(target_os = "windows") {
        for var in ["ProgramFiles", "LOCALAPPDATA"] {
            if let Some(dir) = std::env::var_os(var) {
                let candidate = PathBuf::from(dir).join("Google/Chrome/Application/chrome.exe");
                if candidate.exists() {
                    return Ok(candidate);
                }
            }
        }
    }

    Err(BrowserError::NotFound(
        "Could not find Chrome or Chromium. Install Chrome and ensure it's on your PATH."
            .into(),
    ))
}

/// Copy cookies and the `Local State` decryption key from the user's real Chrome profile, so
/// the launched Chrome has their logged-in sessions.
fn copy_chrome_cookies(profile_dir: &Path) -> Result<(), BrowserError> {
    let chrome_default = chrome_default_profile_dir()?;
    let cookies_src = chrome_default.join("Cookies");
    if !cookies_src.exists() {
        return Err(BrowserError::Launch(
            "Chrome cookies file not found. Is Chrome installed?".into(),
        ));
    }

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

    // Local State holds the cookie key on Windows/Linux; macOS keeps it in the Keychain, so a
    // failure here is a warning rather than an error.
    let local_state_src = chrome_default.parent().map(|p| p.join("Local State"));
    let local_state = match local_state_src {
        Some(src) if src.exists() => match std::fs::copy(&src, profile_dir.join("Local State")) {
            Ok(_) => LocalState::Copied,
            Err(e) => LocalState::Failed(e.to_string()),
        },
        _ => LocalState::Absent,
    };

    eprintln!("{}", cookie_copy_message(&local_state));
    Ok(())
}

/// What happened to the `Local State` file, which carries the cookie decryption key.
#[derive(Debug)]
enum LocalState {
    Copied,
    /// Not present in the source profile — nothing to copy, nothing to warn about.
    Absent,
    Failed(String),
}

/// What `--copy-cookies` may claim about itself. A copy without `Local State` must not read as
/// success: on Windows and Linux the cookies cannot be decrypted and the session looks logged out.
fn cookie_copy_message(local_state: &LocalState) -> String {
    match local_state {
        LocalState::Copied => "Copied cookies and decryption key from Chrome profile".into(),
        LocalState::Absent => "Copied cookies from Chrome profile (no Local State to copy)".into(),
        LocalState::Failed(e) => format!(
            "warning: copied cookies but NOT the decryption key (Local State: {e}). \
             On Windows and Linux the cookies will not decrypt — the session will look logged out. \
             Use --connect to a real Chrome instead."
        ),
    }
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
    Ok(home.join(".chrome-agent").join("browsers").join(name).join("chromium-profile"))
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
        let dir = std::env::temp_dir().join(format!("chrome-agent_test_devtools-{}", std::process::id()));
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
        let dir = std::env::temp_dir().join(format!("chrome-agent_test_devtools_bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("DevToolsActivePort");
        std::fs::write(&path, "not_a_number\n").unwrap();
        assert!(read_devtools_active_port(&path).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn should_copy_cookies_only_on_fresh_spawn() {
        assert!(should_copy_cookies(true, false));
        // Reconnecting to a live managed Chrome: never copy, even if requested.
        assert!(!should_copy_cookies(true, true));
        assert!(!should_copy_cookies(false, false));
        assert!(!should_copy_cookies(false, true));
    }

    #[test]
    fn validates_and_normalizes_supported_proxy_urls() {
        assert_eq!(
            validate_proxy_server("HTTP://Proxy.Example:8080/").unwrap(),
            "http://proxy.example:8080"
        );
        assert_eq!(
            validate_proxy_server("socks5://127.0.0.1:1080").unwrap(),
            "socks5://127.0.0.1:1080"
        );
        assert_eq!(
            validate_proxy_server("http://[2001:DB8::1]:3128").unwrap(),
            "http://[2001:db8::1]:3128"
        );
    }

    #[test]
    fn rejects_unsafe_proxy_urls_without_echoing_credentials() {
        for value in [
            "http://user:secret@proxy.example:8080",
            "ftp://proxy.example:21",
            "http://proxy.example",
            "http://proxy.example:8080/path",
            "http://proxy.example:8080?token=secret",
            "http://proxy.example:8080#secret",
            "http://[]:8080",
            "http://[proxy.example]:8080",
        ] {
            let error = validate_proxy_server(value).unwrap_err().to_string();
            assert!(error.contains("<redacted-proxy>"));
            assert!(!error.contains("secret"));
        }
    }

    #[test]
    fn managed_launch_args_include_one_proxy_flag() {
        let opts = BrowserOptions {
            proxy_server: Some("http://127.0.0.1:8080".into()),
            ..BrowserOptions::default()
        };
        let args = managed_launch_args(Path::new("/tmp/chrome-profile"), &opts);
        assert_eq!(
            args.iter()
                .filter(|arg| arg.starts_with("--proxy-server="))
                .collect::<Vec<_>>(),
            vec![&"--proxy-server=http://127.0.0.1:8080".to_string()]
        );
    }

    #[test]
    fn attached_browser_rejects_launch_proxy() {
        let error = normalized_proxy_option(
            Some("http://127.0.0.1:9222"),
            Some("http://127.0.0.1:8080"),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("applies only when chrome-agent launches Chrome"));
    }

    #[test]
    fn managed_launch_args_include_chrome_args_in_order_and_last() {
        let opts = BrowserOptions {
            chrome_args: vec![
                "--enable-features=WebMCP,WebMCPTesting".into(),
                "--auto-open-devtools-for-tabs".into(),
            ],
            ..BrowserOptions::default()
        };
        let args = managed_launch_args(Path::new("/tmp/chrome-profile"), &opts);
        assert_eq!(
            &args[args.len() - 2..],
            &[
                "--enable-features=WebMCP,WebMCPTesting".to_string(),
                "--auto-open-devtools-for-tabs".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn try_reconnect_existing_none_when_absent() {
        let dir = std::env::temp_dir().join(format!("chrome-agent_test_reconnect_absent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("DevToolsActivePort");
        std::fs::remove_file(&path).ok();
        assert!(try_reconnect_existing(&path).await.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn try_reconnect_existing_removes_stale_file() {
        let dir = std::env::temp_dir().join(format!("chrome-agent_test_reconnect_stale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("DevToolsActivePort");
        // Valid format, but the port has no listening server → stale.
        std::fs::write(&path, "59321\n/devtools/browser/dead-target\n").unwrap();
        let result = try_reconnect_existing(&path).await;
        assert!(result.is_none(), "unreachable port must not reconnect");
        assert!(!path.exists(), "stale port file should be removed");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The composed message used to discard the `BrowserError` it was built from, so a
    /// refused connection, a timeout and a malformed `/json/version` all read alike — and
    /// `hints::error_hint`, which branches on "Connection refused", could never see one.
    #[tokio::test]
    async fn a_connect_failure_carries_the_reason_it_failed() {
        // A port nothing listens on: bound then released, so it is free at this instant.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a free port");
        let port = listener.local_addr().expect("read the port").port();
        drop(listener);
        let endpoint = format!("http://127.0.0.1:{port}");

        let cause = fetch_ws_endpoint(&endpoint)
            .await
            .expect_err("nothing listens there")
            .to_string();
        let Err(composed) = resolve_http_endpoint(&endpoint).await else {
            panic!("nothing listens on {endpoint}, so this cannot resolve");
        };
        let composed = composed.to_string();

        assert!(
            composed.contains(&cause),
            "the cause must survive into the message a user reads:\n  cause: {cause}\n  said:  {composed}"
        );
        assert!(composed.contains(&endpoint), "the endpoint is still named: {composed}");
        assert!(
            composed.contains("auto-discovery"),
            "the existing suggestion must not be lost: {composed}"
        );
    }

    #[test]
    fn a_missing_decryption_key_is_not_reported_as_a_clean_copy() {
        // Cookies land, Local State does not: the caller must not read that as success.
        let failed = cookie_copy_message(&LocalState::Failed("Permission denied".into()));
        assert!(failed.contains("NOT the decryption key"), "{failed}");
        assert!(failed.contains("Permission denied"), "the OS error must survive: {failed}");
        assert!(failed.starts_with("warning:"), "a partial copy is not a success line: {failed}");

        let copied = cookie_copy_message(&LocalState::Copied);
        assert!(copied.contains("decryption key"), "{copied}");
        assert!(!copied.contains("warning"), "{copied}");

        // Nothing to copy is not a failure: say so rather than implying a key arrived.
        let absent = cookie_copy_message(&LocalState::Absent);
        assert!(!absent.contains("warning"), "{absent}");
        assert!(absent.contains("no Local State"), "{absent}");
    }
}
