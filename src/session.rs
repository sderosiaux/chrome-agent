use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::element_ref::ElementRef;

const SESSION_FILE: &str = "sessions.json";

/// Top-level session state persisted to disk.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SessionStore {
    #[serde(default)]
    pub browsers: HashMap<String, BrowserSession>,
}

/// Per-browser session state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSession {
    pub ws_endpoint: String,
    pub pid: Option<u32>,
    #[serde(default)]
    pub headless: bool,
    #[serde(default)]
    pub daemon_pid: Option<u32>,
    #[serde(default)]
    pub pages: HashMap<String, PageSession>,
}

/// Per-page session state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageSession {
    pub target_id: String,
    #[serde(default)]
    pub uid_map: HashMap<String, ElementRef>,
}

/// Load the session store from disk. Returns empty store if file doesn't exist.
pub fn load_session() -> Result<SessionStore, SessionError> {
    let path = session_path()?;
    if !path.exists() {
        return Ok(SessionStore::default());
    }

    let contents = std::fs::read_to_string(&path)
        .map_err(|e| SessionError(format!("Failed to read {}: {e}", path.display())))?;

    serde_json::from_str(&contents)
        .map_err(|e| SessionError(format!("Failed to parse {}: {e}", path.display())))
}

/// Save the session store to disk.
pub fn save_session(store: &SessionStore) -> Result<(), SessionError> {
    let path = session_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| SessionError(format!("Failed to create dir: {e}")))?;
    }

    let json = serde_json::to_string_pretty(store)
        .map_err(|e| SessionError(format!("Failed to serialize session: {e}")))?;

    // Write atomically: write to temp file then rename
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, &json)
        .map_err(|e| SessionError(format!("Failed to write {}: {e}", tmp_path.display())))?;
    std::fs::rename(&tmp_path, &path)
        .map_err(|e| SessionError(format!("Failed to rename session file: {e}")))?;

    Ok(())
}

/// Remove stale browser sessions where the process is no longer running.
pub fn cleanup_stale(store: &mut SessionStore) {
    store.browsers.retain(|_name, session| {
        if let Some(pid) = session.pid {
            is_process_alive(pid)
        } else {
            // External connection (--connect) — keep, we'll verify on reconnect
            true
        }
    });
}

/// Ensure a browser session entry exists, returning a mutable ref.
pub fn ensure_browser<'a>(
    store: &'a mut SessionStore,
    name: &str,
    ws_endpoint: &str,
    pid: Option<u32>,
    headless: bool,
) -> &'a mut BrowserSession {
    store
        .browsers
        .entry(name.to_string())
        .or_insert_with(|| BrowserSession {
            ws_endpoint: ws_endpoint.to_string(),
            pid,
            headless,
            daemon_pid: None,
            pages: HashMap::new(),
        })
}

/// Ensure a page session entry exists, returning a mutable ref.
pub fn ensure_page<'a>(
    browser: &'a mut BrowserSession,
    page_name: &str,
    target_id: &str,
) -> &'a mut PageSession {
    browser
        .pages
        .entry(page_name.to_string())
        .or_insert_with(|| PageSession {
            target_id: target_id.to_string(),
            uid_map: HashMap::new(),
        })
}

/// Check if the daemon socket exists.
pub fn daemon_socket_exists() -> bool {
    daemon_socket_path()
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// Path to the daemon socket.
pub fn daemon_socket_path() -> Result<PathBuf, SessionError> {
    Ok(dev_browser_dir()?.join("daemon.sock"))
}

/// Path to the daemon PID file.
pub fn daemon_pid_path() -> Result<PathBuf, SessionError> {
    Ok(dev_browser_dir()?.join("daemon.pid"))
}

fn session_path() -> Result<PathBuf, SessionError> {
    Ok(dev_browser_dir()?.join(SESSION_FILE))
}

fn dev_browser_dir() -> Result<PathBuf, SessionError> {
    dirs::home_dir()
        .map(|h| h.join(".aibrowsr"))
        .ok_or_else(|| SessionError("Could not determine home directory".into()))
}

fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // kill(pid, 0) checks if process exists without sending a signal
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        // On Windows, assume alive — will fail on reconnect if dead
        let _ = pid;
        true
    }
}

#[derive(Debug)]
pub struct SessionError(pub String);

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for SessionError {}
