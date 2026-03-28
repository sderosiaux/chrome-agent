use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::element_ref::ElementRef;

const SESSION_FILE: &str = "sessions.json";

/// Top-level session state persisted to disk.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SessionStore {
    #[serde(default)]
    pub browsers: HashMap<String, BrowserSession>,
    /// Tracks the file modification time when loaded (not serialized).
    #[serde(skip)]
    pub loaded_mtime: Option<std::time::SystemTime>,
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
    #[serde(default)]
    pub last_snapshot: Option<String>,
}

/// Load the session store from disk. Returns empty store if file doesn't exist.
pub fn load_session() -> Result<SessionStore, SessionError> {
    let path = session_path()?;
    if !path.exists() {
        return Ok(SessionStore::default());
    }

    let mtime = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok());

    let contents = std::fs::read_to_string(&path)
        .map_err(|e| SessionError(format!("Failed to read {}: {e}", path.display())))?;

    let mut store: SessionStore = serde_json::from_str(&contents)
        .map_err(|e| SessionError(format!("Failed to parse {}: {e}", path.display())))?;
    store.loaded_mtime = mtime;
    Ok(store)
}

/// Save the session store to disk.
pub fn save_session(store: &mut SessionStore) -> Result<(), SessionError> {
    let path = session_path()?;

    // Detect concurrent modification (another aibrowsr process touched the file)
    if let Some(loaded_mtime) = store.loaded_mtime
        && let Ok(current_mtime) = std::fs::metadata(&path).and_then(|m| m.modified())
            && current_mtime != loaded_mtime {
                eprintln!("warning: session file was modified by another process. Use --browser <name> to isolate parallel agents.");
            }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| SessionError(format!("Failed to create dir: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }

    let json = serde_json::to_string_pretty(store)
        .map_err(|e| SessionError(format!("Failed to serialize session: {e}")))?;

    // Write atomically: write to temp file then rename
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, &json)
        .map_err(|e| SessionError(format!("Failed to write {}: {e}", tmp_path.display())))?;
    std::fs::rename(&tmp_path, &path)
        .map_err(|e| SessionError(format!("Failed to rename session file: {e}")))?;

    // Restrict permissions: session file contains WebSocket URLs that grant
    // full browser control. Only the owning user should read it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    // Update loaded_mtime so subsequent saves in the same process don't
    // false-positive the concurrent modification warning.
    store.loaded_mtime = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok());

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
            last_snapshot: None,
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
        // SAFETY: kill(pid, 0) only checks if the process exists. No signal sent.
        #[allow(unsafe_code)]
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        result == 0
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SessionError(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_roundtrip() {
        let mut store = SessionStore::default();
        let browser =
            ensure_browser(&mut store, "test", "ws://localhost:9222", Some(1234), true);
        ensure_page(browser, "main", "target-abc");

        let json = serde_json::to_string(&store).unwrap();
        let loaded: SessionStore = serde_json::from_str(&json).unwrap();

        assert!(loaded.browsers.contains_key("test"));
        let b = &loaded.browsers["test"];
        assert_eq!(b.ws_endpoint, "ws://localhost:9222");
        assert_eq!(b.pid, Some(1234));
        assert!(b.headless);
        assert!(b.pages.contains_key("main"));
        assert_eq!(b.pages["main"].target_id, "target-abc");
    }

    #[test]
    fn bug_session_corrupt_json() {
        let dir = std::env::temp_dir().join("aibrowsr_test_corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sessions.json");
        std::fs::write(&path, "NOT VALID JSON {{{").unwrap();
        let result: Result<SessionStore, _> = serde_json::from_str("NOT VALID JSON {{{");
        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bug_session_empty_file() {
        let result: Result<SessionStore, _> = serde_json::from_str("");
        assert!(result.is_err());
    }

    #[test]
    fn bug_element_ref_unknown_type() {
        let json = r#"{"type":"futureType","data":"unknown"}"#;
        let result: Result<crate::element_ref::ElementRef, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}
