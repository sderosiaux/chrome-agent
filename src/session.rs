use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::element_ref::ElementRef;

pub const SESSION_FILE: &str = "sessions.json";

/// Top-level session state persisted to disk.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SessionStore {
    #[serde(default)]
    pub browsers: HashMap<String, BrowserSession>,
    /// Browser names present at load time. A save deletes only names this process dropped.
    /// `pub(crate)` for `session_save::save_to`, the only reader.
    #[serde(skip)]
    pub(crate) loaded_names: HashSet<String>,
    /// Serialized value of each browser as loaded. An entry still matching it was never
    /// touched here, so the save keeps the on-disk copy; republishing it is a lost update
    /// between parallel `--browser <name>` agents.
    #[serde(skip)]
    pub(crate) loaded_entries: HashMap<String, String>,
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
    pub proxy_server: Option<String>,
    /// Extra `--chrome-arg` flags this browser was launched with, in the order given.
    /// Fixed at launch like `proxy_server`; a mismatch on reconnect is refused.
    #[serde(default)]
    pub chrome_args: Vec<String>,
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
    /// `(frameId, loaderId)` of the document `last_snapshot` was taken from. `backendNodeId`
    /// counters overlap between documents, so `diff` needs this to tell "the page changed"
    /// from "different document". The loader id moves exactly when the document is replaced.
    #[serde(default)]
    pub last_snapshot_frame: Option<String>,
    #[serde(default)]
    pub last_snapshot_loader: Option<String>,
    /// Requested device metrics, reapplied to this page's current target on each connection.
    /// A Chrome relaunch replaces the browser entry, so they expire with the process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_emulation: Option<crate::emulation::DeviceEmulation>,
}

impl PageSession {
    /// Take this reading as the page's baseline: the uid map, the text `diff` compares against,
    /// and the identity that says whether the two are even comparable.
    ///
    /// The four assignments were copy-pasted at ten sites across `run.rs`, `run_helpers.rs`,
    /// `pipe_dispatch.rs` and `pipe_report.rs` — the surface area of the seven-path baseline bug
    /// (`.claude/rules/snapshot-and-inspect.md`). A site that stores three of the four is the
    /// shape that bug took, and there is now no way to write one.
    pub fn store_snapshot(&mut self, snapshot: crate::snapshot::Snapshot) {
        self.uid_map = snapshot.uid_map;
        self.last_snapshot = Some(snapshot.text);
        let (frame, loader) = snapshot
            .identity
            .map_or((None, None), |(f, l)| (Some(f), Some(l)));
        self.last_snapshot_frame = frame;
        self.last_snapshot_loader = loader;
    }
}

/// The disk halves live beside this file: reading in `session_load.rs`, writing (the lock, the
/// read-merge-write, the dead-pid prune) in `session_save.rs`. Re-exported so call sites keep
/// spelling them `session::X`.
pub use crate::session_save::{FileLock, Liveness, cleanup_stale, liveness, save_to};

/// Load the session store from disk. Returns empty store if file doesn't exist.
pub fn load_session() -> Result<SessionStore, SessionError> {
    crate::session_load::load_from(&session_path()?)
}

/// Save the session store to disk, merging with the current on-disk state so
/// parallel agents don't clobber each other's entries.
pub fn save_session(store: &mut SessionStore) -> Result<(), SessionError> {
    let result = save_to(&session_path()?, store);
    if result.is_ok() {
        // A pid on disk is reachable by `close`/`status`/the interrupt handler, so it is no
        // longer this invocation's to reap. Disarmed here, not at call sites, so any future
        // save path inherits it. See `kill::UNPERSISTED`.
        for pid in store.browsers.values().filter_map(|b| b.pid) {
            crate::kill::disarm(pid);
        }
    }
    result
}

impl SessionStore {
    /// Baseline the next save merges against: names held, plus the exact bytes of each
    /// entry. Both fields must be set together; a name with no bytes reads as "changed"
    /// and gets republished.
    pub fn take_baseline(&mut self) {
        self.loaded_names = self.browsers.keys().cloned().collect();
        self.loaded_entries = self
            .browsers
            .iter()
            .filter_map(|(name, entry)| {
                serde_json::to_string(entry)
                    .ok()
                    .map(|json| (name.clone(), json))
            })
            .collect();
    }
}

/// Where `browser::browser_profile_dir` puts profiles. Exposed so `close --purge-orphans`
/// sweeps the same directory the save path does.
pub fn browsers_dir() -> Result<PathBuf, SessionError> {
    Ok(dev_browser_dir()?.join("browsers"))
}

/// Ensure a browser session entry exists, returning a mutable ref.
pub fn ensure_browser<'a>(
    store: &'a mut SessionStore,
    name: &str,
    ws_endpoint: &str,
    pid: Option<u32>,
    headless: bool,
    proxy_server: Option<String>,
    chrome_args: Vec<String>,
) -> &'a mut BrowserSession {
    store
        .browsers
        .entry(name.to_string())
        .or_insert_with(|| BrowserSession {
            ws_endpoint: ws_endpoint.to_string(),
            pid,
            headless,
            proxy_server,
            chrome_args,
            daemon_pid: None,
            pages: HashMap::new(),
        })
}

/// Guard proxy compatibility when reconnecting to a live named browser.
///
/// A managed browser's proxy is fixed at launch. No proxy requested inherits the existing
/// one silently; only an explicitly *different* proxy is refused.
pub fn ensure_proxy_compatible(
    browser: &BrowserSession,
    requested_proxy: Option<&str>,
) -> Result<(), SessionError> {
    let Some(requested) = requested_proxy else {
        return Ok(());
    };
    if browser.proxy_server.as_deref() == Some(requested) {
        return Ok(());
    }
    Err(SessionError(
        "named browser is already running with a different proxy; close or purge it (chrome-agent --browser <name> close --purge), or select another browser name"
            .into(),
    ))
}

/// `--chrome-arg` compatibility lives in `chrome_args.rs`, re-exported so call sites keep
/// spelling it `session::ensure_chrome_args_compatible`.
pub use crate::chrome_args::ensure_chrome_args_compatible;

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
            last_snapshot_frame: None,
            last_snapshot_loader: None,
            device_emulation: None,
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

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SessionError(pub String);

/// A minimal entry, shared with `session_save`'s tests so both halves build the same fixture.
#[cfg(test)]
pub fn browser_fixture(ws: &str) -> BrowserSession {
    BrowserSession {
        ws_endpoint: ws.to_string(),
        pid: Some(1),
        headless: true,
        proxy_server: None,
        chrome_args: Vec::new(),
        daemon_pid: None,
        pages: HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::browser_fixture as browser;
    use super::*;

    #[test]
    fn session_roundtrip() {
        let mut store = SessionStore::default();
        let browser = ensure_browser(
            &mut store,
            "test",
            "ws://localhost:9222",
            Some(1234),
            true,
            Some("http://127.0.0.1:8080".into()),
            vec!["--enable-features=WebMCP,WebMCPTesting".into()],
        );
        ensure_page(browser, "main", "target-abc");

        let json = serde_json::to_string(&store).unwrap();
        let loaded: SessionStore = serde_json::from_str(&json).unwrap();

        assert!(loaded.browsers.contains_key("test"));
        let b = &loaded.browsers["test"];
        assert_eq!(b.ws_endpoint, "ws://localhost:9222");
        assert_eq!(b.pid, Some(1234));
        assert!(b.headless);
        assert_eq!(b.proxy_server.as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!(
            b.chrome_args,
            vec!["--enable-features=WebMCP,WebMCPTesting".to_string()]
        );
        assert!(b.pages.contains_key("main"));
        assert_eq!(b.pages["main"].target_id, "target-abc");
    }

    #[test]
    fn named_browser_proxy_must_match_before_reuse() {
        let existing = browser("ws://localhost:9222");
        assert!(ensure_proxy_compatible(&existing, None).is_ok());
        assert!(
            ensure_proxy_compatible(&existing, Some("http://127.0.0.1:8080"))
                .unwrap_err()
                .to_string()
                .contains("different proxy")
        );
    }

    #[test]
    fn proxied_browser_inherits_proxy_when_flag_omitted() {
        let mut existing = browser("ws://localhost:9222");
        existing.proxy_server = Some("http://127.0.0.1:8080".into());
        // Omitted or identical inherits; a different explicit proxy is refused.
        assert!(ensure_proxy_compatible(&existing, None).is_ok());
        assert!(ensure_proxy_compatible(&existing, Some("http://127.0.0.1:8080")).is_ok());
        assert!(ensure_proxy_compatible(&existing, Some("http://127.0.0.1:9090")).is_err());
    }

    // `ensure_chrome_args_compatible` is tested in `chrome_args.rs`, where it lives.

    #[test]
    fn bug_element_ref_unknown_type() {
        let json = r#"{"type":"futureType","data":"unknown"}"#;
        let result: Result<crate::element_ref::ElementRef, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}
