use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

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
    /// Browser names present when this store was loaded. Used at save time to
    /// distinguish entries this process deliberately removed (delete from disk)
    /// from entries other processes added after our load (leave alone).
    #[serde(skip)]
    loaded_names: HashSet<String>,
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
    load_from(&session_path()?)
}

/// Save the session store to disk, merging with the current on-disk state so
/// parallel agents don't clobber each other's entries.
pub fn save_session(store: &mut SessionStore) -> Result<(), SessionError> {
    save_to(&session_path()?, store)
}

/// Read a session store from an explicit path (empty store if the file is
/// absent). Records the loaded browser names as the delete baseline.
fn load_from(path: &Path) -> Result<SessionStore, SessionError> {
    if !path.exists() {
        return Ok(SessionStore::default());
    }

    let mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());

    let contents = std::fs::read_to_string(path)
        .map_err(|e| SessionError(format!("Failed to read {}: {e}", path.display())))?;

    let mut store: SessionStore = serde_json::from_str(&contents)
        .map_err(|e| SessionError(format!("Failed to parse {}: {e}", path.display())))?;
    store.loaded_mtime = mtime;
    store.loaded_names = store.browsers.keys().cloned().collect();
    Ok(store)
}

/// Persist `store` to `path` under an exclusive lock, merging with whatever is
/// currently on disk. This is the concurrency-safe core:
///
/// 1. Take an exclusive advisory lock so no two writers interleave.
/// 2. Re-read the on-disk store (another agent may have written since we loaded).
/// 3. Delete only the browsers this process held at load but no longer holds
///    (e.g. `close`), leaving entries other agents added after our load intact.
/// 4. Upsert this process's browsers, then atomically replace the file.
fn save_to(path: &Path, store: &mut SessionStore) -> Result<(), SessionError> {
    let parent = path
        .parent()
        .ok_or_else(|| SessionError("session path has no parent directory".into()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| SessionError(format!("Failed to create dir: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }

    // Serialize concurrent writers for the read-merge-write critical section.
    let _lock = FileLock::acquire(&parent.join("sessions.lock"))?;

    // Merge our changes onto the freshest on-disk state.
    let mut merged = load_from(path).unwrap_or_default();
    for name in &store.loaded_names {
        if !store.browsers.contains_key(name) {
            merged.browsers.remove(name);
        }
    }
    for (name, entry) in &store.browsers {
        merged.browsers.insert(name.clone(), entry.clone());
    }

    let json = serde_json::to_string_pretty(&merged)
        .map_err(|e| SessionError(format!("Failed to serialize session: {e}")))?;

    // Atomic replace via a per-process temp file (unique name avoids clashing
    // with a crashed process's leftover temp; the lock covers same-process races).
    let tmp_path = path.with_extension(format!("json.tmp.{}", std::process::id()));
    std::fs::write(&tmp_path, &json)
        .map_err(|e| SessionError(format!("Failed to write {}: {e}", tmp_path.display())))?;
    // Restrict permissions before publishing: the file holds WebSocket URLs that
    // grant full browser control. Only the owning user should read it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp_path, path)
        .map_err(|e| SessionError(format!("Failed to rename session file: {e}")))?;

    // Our view is now the baseline for subsequent saves in this process.
    store.loaded_names = store.browsers.keys().cloned().collect();
    store.loaded_mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());

    Ok(())
}

/// Exclusive advisory file lock, released on drop. Best-effort no-op on
/// non-Unix platforms (single-user desktop usage).
#[cfg(unix)]
struct FileLock(std::fs::File);

#[cfg(unix)]
impl FileLock {
    fn acquire(path: &Path) -> Result<Self, SessionError> {
        use std::os::unix::io::AsRawFd;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|e| SessionError(format!("Failed to open lock {}: {e}", path.display())))?;
        // SAFETY: flock on a valid fd only takes an advisory lock; no memory unsafety.
        #[allow(unsafe_code)]
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if rc != 0 {
            return Err(SessionError(format!(
                "Failed to lock session store: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(Self(file))
    }
}

#[cfg(unix)]
impl Drop for FileLock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        // SAFETY: unlocking a valid fd we hold; no memory unsafety.
        #[allow(unsafe_code)]
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[cfg(not(unix))]
struct FileLock;

#[cfg(not(unix))]
impl FileLock {
    fn acquire(_path: &Path) -> Result<Self, SessionError> {
        Ok(Self)
    }
}

/// Remove stale browser sessions where the process is no longer running
/// or the WebSocket endpoint is unreachable.
pub fn cleanup_stale(store: &mut SessionStore) {
    store.browsers.retain(|_name, session| {
        if let Some(pid) = session.pid {
            is_process_alive(pid)
        } else {
            // External connection (--connect) — probe HTTP endpoint
            is_ws_reachable(&session.ws_endpoint)
        }
    });
}

/// Quick check if a WebSocket endpoint's Chrome is still alive
/// by probing the HTTP /json/version endpoint (same host:port).
fn is_ws_reachable(ws_url: &str) -> bool {
    let http_url = crate::browser::extract_http_from_ws(ws_url);
    let version_url = format!("{http_url}/json/version");
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_millis(500)))
        .build()
        .new_agent();
    agent.get(&version_url).call().is_ok()
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
    daemon_socket_path().is_ok_and(|p| p.exists())
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
        .map(|h| h.join(".chrome-agent"))
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
        let dir = std::env::temp_dir().join("chrome-agent_test_corrupt");
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

    // --- Concurrent session store (issue: parallel agents clobber sessions.json) ---

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("chrome-agent_sess_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn browser(ws: &str) -> BrowserSession {
        BrowserSession {
            ws_endpoint: ws.to_string(),
            pid: Some(1),
            headless: true,
            daemon_pid: None,
            pages: HashMap::new(),
        }
    }

    #[test]
    fn save_merges_concurrent_additions_from_another_process() {
        let dir = tmp_dir("merge");
        let path = dir.join(SESSION_FILE);

        // This process loads an empty store and stages its own browser "a".
        let mut mine = load_from(&path).unwrap();
        mine.browsers.insert("a".into(), browser("ws://a"));

        // Meanwhile another process persists browser "b".
        let mut theirs = load_from(&path).unwrap();
        theirs.browsers.insert("b".into(), browser("ws://b"));
        save_to(&path, &mut theirs).unwrap();

        // Our save must NOT clobber "b".
        save_to(&path, &mut mine).unwrap();

        let disk = load_from(&path).unwrap();
        assert!(disk.browsers.contains_key("a"), "own entry lost: {:?}", disk.browsers.keys());
        assert!(disk.browsers.contains_key("b"), "concurrent entry clobbered: {:?}", disk.browsers.keys());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_deletes_only_entries_this_process_removed() {
        let dir = tmp_dir("delete");
        let path = dir.join(SESSION_FILE);

        // Seed disk with two browsers.
        let mut seed = SessionStore::default();
        seed.browsers.insert("a".into(), browser("ws://a"));
        seed.browsers.insert("b".into(), browser("ws://b"));
        save_to(&path, &mut seed).unwrap();

        // Load, drop "a" (like `close --browser a`), save.
        let mut store = load_from(&path).unwrap();
        store.browsers.remove("a");
        save_to(&path, &mut store).unwrap();

        let disk = load_from(&path).unwrap();
        assert!(!disk.browsers.contains_key("a"), "removed entry should be gone");
        assert!(disk.browsers.contains_key("b"), "untouched entry should remain");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_does_not_delete_entries_added_by_others_after_load() {
        let dir = tmp_dir("nodelete");
        let path = dir.join(SESSION_FILE);

        // We load empty, stage "a".
        let mut mine = load_from(&path).unwrap();
        mine.browsers.insert("a".into(), browser("ws://a"));

        // Another process adds "c" after our load.
        let mut other = load_from(&path).unwrap();
        other.browsers.insert("c".into(), browser("ws://c"));
        save_to(&path, &mut other).unwrap();

        // Our save adds "a" and must leave "c" alone (we never knew about it).
        save_to(&path, &mut mine).unwrap();

        let disk = load_from(&path).unwrap();
        assert!(disk.browsers.contains_key("a"));
        assert!(disk.browsers.contains_key("c"), "must not delete an entry we never loaded");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn concurrent_saves_under_lock_lose_no_updates() {
        let dir = tmp_dir("threads");
        let path = dir.join(SESSION_FILE);
        let n = 24;

        let handles: Vec<_> = (0..n)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let mut store = load_from(&path).unwrap_or_default();
                    store.browsers.insert(format!("b{i}"), browser(&format!("ws://{i}")));
                    save_to(&path, &mut store).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let disk = load_from(&path).unwrap();
        for i in 0..n {
            assert!(
                disk.browsers.contains_key(&format!("b{i}")),
                "lost update for b{i}; have {:?}",
                disk.browsers.keys()
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
