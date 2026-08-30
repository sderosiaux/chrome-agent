//! Writing the session store to disk: the exclusive lock, the read-merge-write, and the
//! dead-pid prune that stops it growing forever.
//!
//! The mirror of `session_load.rs`. Both are disk mechanics over the types in `session.rs`,
//! which keeps the entry points (`load_session`, `save_session`) and re-exports these.

use std::collections::HashMap;
use std::path::Path;

use crate::session::{BrowserSession, SessionError, SessionStore};

/// Persist `store` to `path`, merging with what is on disk. Order matters:
///
/// 1. Exclusive advisory lock, so no two writers interleave.
/// 2. Re-read the on-disk store (`session_load::reread_for_merge`).
/// 3. Delete only browsers held at load and no longer held; leave entries others added.
/// 4. Upsert this process's browsers.
/// 5. `prune_dead` every entry whose browser process is provably gone.
/// 6. Atomically replace the file.
pub fn save_to(path: &Path, store: &mut SessionStore) -> Result<(), SessionError> {
    let parent = path
        .parent()
        .ok_or_else(|| SessionError("session path has no parent directory".into()))?;
    crate::secure_fs::create_private_dir_all(parent)
        .map_err(|e| SessionError(format!("Failed to create dir: {e}")))?;

    // Serialize concurrent writers for the read-merge-write critical section.
    let _lock = FileLock::acquire(&parent.join("sessions.lock"))?;

    // Never `unwrap_or_default()` this: an empty store from a failed READ would publish this
    // process's browsers as the whole file. `?` fires under the lock, before any write.
    let mut merged = crate::session_load::reread_for_merge(path)?;
    for name in &store.loaded_names {
        if !store.browsers.contains_key(name) {
            // Compare-and-delete: the drop was decided about the entry we loaded. If another
            // writer republished the name since, deleting it would orphan a live Chrome.
            let on_disk_is_what_we_loaded = merged
                .browsers
                .get(name)
                .and_then(|entry| serde_json::to_string(entry).ok())
                .is_none_or(|json| store.loaded_entries.get(name) == Some(&json));
            if on_disk_is_what_we_loaded {
                merged.browsers.remove(name);
            }
        }
    }
    for (name, entry) in &store.browsers {
        // Untouched since load: keep the on-disk value, which may be newer than ours.
        let untouched = serde_json::to_string(entry)
            .ok()
            .is_some_and(|json| store.loaded_entries.get(name) == Some(&json));
        if untouched && merged.browsers.contains_key(name) {
            continue;
        }
        merged.browsers.insert(name.clone(), entry.clone());
    }

    // After the upsert, so it also covers a browser that died mid-command.
    let pruned = prune_dead(&mut merged.browsers);
    // Drop them from `store` too, or the next save re-upserts them: its "untouched and
    // absent from disk" branch reads them as ours to publish.
    for name in &pruned {
        store.browsers.remove(name);
    }

    let json = serde_json::to_string_pretty(&merged)
        .map_err(|e| SessionError(format!("Failed to serialize session: {e}")))?;

    // Atomic replace via a per-process temp file, so a crashed process's leftover temp
    // cannot clash. The lock covers same-process races.
    let tmp_path = path.with_extension(format!("json.tmp.{}", std::process::id()));
    std::fs::write(&tmp_path, &json)
        .map_err(|e| SessionError(format!("Failed to write {}: {e}", tmp_path.display())))?;
    // 0600 before publishing: the file holds WebSocket URLs granting full browser control.
    crate::secure_fs::restrict_file(&tmp_path).map_err(|e| {
        SessionError(format!(
            "Failed to restrict {} to mode 0600: {e}",
            tmp_path.display()
        ))
    })?;
    std::fs::rename(&tmp_path, path).map_err(|e| {
        // The temp name is per-PID, so nothing else will ever reclaim it.
        let _ = std::fs::remove_file(&tmp_path);
        SessionError(format!("Failed to rename session file: {e}"))
    })?;

    // Judged against the store just published, still under the lock that fixes it. After
    // the rename: housekeeping must not delay or endanger the write it rides on.
    crate::profiles::sweep_orphans(
        &parent.join("browsers"),
        &merged.browsers.keys().cloned().collect(),
        &crate::profiles::Limits::default(),
    );

    // Our view is now the baseline for subsequent saves in this process.
    store.take_baseline();

    Ok(())
}

/// Exclusive advisory file lock, released on drop. Best-effort no-op on
/// non-Unix platforms (single-user desktop usage).
///
/// `pub` for `commands::history`, whose file is shared by every browser on the machine
/// and which rotates it by rename — one lock implementation, and one `unsafe` block, for both.
#[cfg(unix)]
pub struct FileLock(std::fs::File);

#[cfg(unix)]
impl FileLock {
    pub fn acquire(path: &Path) -> Result<Self, SessionError> {
        use std::os::unix::io::AsRawFd;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|e| SessionError(format!("Failed to open lock {}: {e}", path.display())))?;
        crate::secure_fs::restrict_file(path).map_err(|e| {
            SessionError(format!(
                "Failed to restrict lock {} to mode 0600: {e}",
                path.display()
            ))
        })?;
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
pub struct FileLock;

#[cfg(not(unix))]
impl FileLock {
    pub fn acquire(_path: &Path) -> Result<Self, SessionError> {
        Ok(Self)
    }
}

/// Drop every entry whose browser process is provably gone, returning the names dropped.
/// Runs on the merged map inside the exclusive lock, so it tests the pid on disk at that
/// instant. Without it the store grows forever. Measured: 5,212,694 bytes / 2131 entries
/// before, 7,827 bytes / 8 entries after one save.
fn prune_dead(browsers: &mut HashMap<String, BrowserSession>) -> Vec<String> {
    let dead: Vec<String> = browsers
        .iter()
        .filter(|(_, session)| is_provably_dead(session))
        .map(|(name, _)| name.clone())
        .collect();
    for name in &dead {
        browsers.remove(name);
    }
    dead
}

/// Whether an entry may be dropped. One-sided: a stale entry costs bytes, a wrongly deleted
/// one costs the caller its browser. `pid: None` is kept without a probe (`--connect` and a
/// managed reconnect both store no pid, and probing would add an HTTP round trip per entry
/// to every save); so is a pid the OS will not classify. See [`Liveness`].
fn is_provably_dead(session: &BrowserSession) -> bool {
    session
        .pid
        .is_some_and(|pid| liveness(pid) == Liveness::Dead)
}

/// What the OS will say about a pid. `Unknown` must never be rounded to `Dead`: `EPERM`
/// means the process exists under another uid. Both it and a recycled pid keep the entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Liveness {
    Alive,
    Dead,
    Unknown,
}

pub fn liveness(pid: u32) -> Liveness {
    #[cfg(unix)]
    {
        // kill() reads a non-positive pid as a process *group*: a different question.
        let Ok(raw) = libc::pid_t::try_from(pid) else {
            return Liveness::Unknown;
        };
        if raw <= 0 {
            return Liveness::Unknown;
        }
        // SAFETY: kill(pid, 0) only checks existence and permission. No signal sent.
        #[allow(unsafe_code)]
        let rc = unsafe { libc::kill(raw, 0) };
        if rc == 0 {
            return Liveness::Alive;
        }
        if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            Liveness::Dead
        } else {
            Liveness::Unknown
        }
    }
    #[cfg(not(unix))]
    {
        // No portable probe, so no entry is ever provably dead and the store never prunes.
        let _ = pid;
        Liveness::Unknown
    }
}

/// Drop every entry whose browser process is provably gone. The daemon heartbeat's copy of
/// the save path's sweep, so the store shrinks even while no command runs.
///
/// It applies [`is_provably_dead`] and nothing else — one predicate, one rule. It used to
/// keep its own, whose pidless branch deleted an entry when a 500 ms blocking
/// `/json/version` probe did not answer: the exact opposite of the one-sided rule the store
/// is built on, so a `--connect` browser busy for half a second lost its entry, its `uid_map`
/// and its `last_snapshot` — on every beat, and with N pidless entries blocking a tokio
/// worker for N × 500 ms. A probe that cannot distinguish "gone" from "busy" may not delete,
/// and once it may not delete there is nothing left for it to decide.
pub fn cleanup_stale(store: &mut SessionStore) {
    prune_dead(&mut store.browsers);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::session::{SESSION_FILE, browser_fixture as browser, ensure_browser, ensure_page};
    use crate::session_load::load_from;

    /// An agent that only *read* another's entry must not write its stale copy back over it.
    #[test]
    fn a_reader_does_not_clobber_another_agents_concurrent_write() {
        let dir =
            std::env::temp_dir().join(format!("chrome-agent-session-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sessions.json");
        let _ = std::fs::remove_file(&path);

        let mut a = SessionStore::default();
        ensure_browser(&mut a, "agent-a", "ws://a", None, true, None, Vec::new());
        save_to(&path, &mut a).unwrap();

        // B loads, so it holds a copy of agent-a it never touched.
        let mut b = load_from(&path).unwrap();
        ensure_browser(&mut b, "agent-b", "ws://b", None, true, None, Vec::new());

        // A records a snapshot while B is still working.
        let mut a2 = load_from(&path).unwrap();
        let browser = a2.browsers.get_mut("agent-a").unwrap();
        let page = ensure_page(browser, "default", "target-1");
        page.last_snapshot = Some("uid=n1 RootWebArea".into());
        save_to(&path, &mut a2).unwrap();

        // B saves last. Its own entry must land, and A's snapshot must survive.
        save_to(&path, &mut b).unwrap();

        let final_state = load_from(&path).unwrap();
        assert!(
            final_state.browsers.contains_key("agent-b"),
            "agent-b should be saved"
        );
        let snapshot = final_state.browsers["agent-a"]
            .pages
            .get("default")
            .and_then(|p| p.last_snapshot.as_deref());
        assert_eq!(
            snapshot,
            Some("uid=n1 RootWebArea"),
            "agent-a's snapshot was clobbered by an agent that only read it"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The heartbeat judges 'foo' dead outside the lock. If another agent relaunches 'foo'
    /// (same name, new pid) before it saves, the delete must not take the fresh entry down.
    /// Both pids are live on purpose: a gone pid would be swept by `prune_dead` first.
    #[test]
    fn a_stale_delete_does_not_clobber_a_concurrent_relaunch() {
        let dir = std::env::temp_dir().join(format!(
            "chrome-agent-session-relaunch-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sessions.json");
        let relaunched = LivePid::spawn();

        let mut original = SessionStore::default();
        ensure_browser(
            &mut original,
            "foo",
            "ws://old",
            Some(std::process::id()),
            true,
            None,
            Vec::new(),
        );
        save_to(&path, &mut original).unwrap();

        // Heartbeat tick: loads, judges the entry stale, drops it in memory only.
        let mut heartbeat = load_from(&path).unwrap();
        heartbeat.browsers.remove("foo");

        // Before the heartbeat saves, another agent relaunches 'foo'.
        let mut agent = load_from(&path).unwrap();
        agent.browsers.remove("foo");
        ensure_browser(
            &mut agent,
            "foo",
            "ws://fresh",
            Some(relaunched.id()),
            true,
            None,
            Vec::new(),
        );
        save_to(&path, &mut agent).unwrap();

        // The heartbeat's delete was decided about the old entry, not this one.
        save_to(&path, &mut heartbeat).unwrap();

        let final_state = load_from(&path).unwrap();
        let survivor = final_state.browsers.get("foo");
        assert_eq!(
            survivor.and_then(|b| b.pid),
            Some(relaunched.id()),
            "the freshly relaunched browser was deleted by a stale-cleanup decision made about its predecessor"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A live process standing in for a running Chrome, so the fixture entry survives the
    /// dead-pid prune. Reaped on drop.
    struct LivePid(std::process::Child);

    impl LivePid {
        fn spawn() -> Self {
            Self(
                std::process::Command::new("sleep")
                    .arg("30")
                    .spawn()
                    .expect("spawn a stand-in for a running browser"),
            )
        }
        fn id(&self) -> u32 {
            self.0.id()
        }
    }

    impl Drop for LivePid {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// A save that cannot publish leaves no temp file behind. The destination is a
    /// non-empty directory, so the save fails at the re-read and creates nothing. The
    /// rename-failure arm's `remove_file` needs `EXDEV` and is not covered here.
    #[test]
    fn a_save_that_cannot_publish_leaves_no_temp_file_behind() {
        let dir = std::env::temp_dir().join(format!(
            "chrome-agent-session-tmpleak-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sessions.json");
        std::fs::create_dir_all(path.join("occupied")).unwrap();

        let mut store = SessionStore::default();
        ensure_browser(&mut store, "leaky", "ws://x", None, true, None, Vec::new());
        let result = save_to(&path, &mut store);
        let err = result
            .expect_err("an unpublishable destination should fail the save")
            .0;
        assert!(
            err.contains("Failed to read"),
            "the refusal names what it could not do: {err}"
        );

        let tmp_path = path.with_extension(format!("json.tmp.{}", std::process::id()));
        assert!(
            !tmp_path.exists(),
            "failed save left {} behind",
            tmp_path.display()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pid the OS reports as gone. Searched, not hardcoded: any fixed number may be in
    /// use on the machine running the test.
    #[cfg(unix)]
    fn a_dead_pid() -> u32 {
        (60_000..99_990u32)
            .find(|&pid| liveness(pid) == Liveness::Dead)
            .expect("no unused pid in range")
    }

    /// A save drops entries whose Chrome has exited, and only those.
    #[cfg(unix)]
    #[test]
    fn save_drops_dead_browsers_and_keeps_live_and_pidless_ones() {
        let dir = tmp_dir("prune");
        let path = dir.join(SESSION_FILE);

        let mut seed = SessionStore::default();
        // Alive: this very test process.
        ensure_browser(
            &mut seed,
            "live",
            "ws://live",
            Some(std::process::id()),
            true,
            None,
            Vec::new(),
        );
        // Dead: exited Chrome, the case that accumulated.
        ensure_browser(
            &mut seed,
            "dead",
            "ws://dead",
            Some(a_dead_pid()),
            true,
            None,
            Vec::new(),
        );
        ensure_browser(
            &mut seed,
            "dead-2",
            "ws://dead2",
            Some(a_dead_pid()),
            true,
            None,
            Vec::new(),
        );
        // No pid: `--connect`, or a managed reconnect via DevToolsActivePort. Dropping this
        // is how an agent loses the user's real Chrome.
        ensure_browser(
            &mut seed,
            "external",
            "ws://127.0.0.1:9222/x",
            None,
            false,
            None,
            Vec::new(),
        );
        // Give the dead entries the bulk an accumulated store carries.
        for name in ["dead", "dead-2"] {
            let browser = seed.browsers.get_mut(name).unwrap();
            let page = ensure_page(browser, "default", "target-1");
            page.last_snapshot = Some("x".repeat(4096));
        }
        // Written without `save_to`: the file an older binary left behind.
        std::fs::write(&path, serde_json::to_string_pretty(&seed).unwrap()).unwrap();
        let size_with_dead = std::fs::metadata(&path).unwrap().len();

        // Any save prunes — including one from a process that only read the file.
        let mut reader = load_from(&path).unwrap();
        save_to(&path, &mut reader).unwrap();

        let disk = load_from(&path).unwrap();
        let mut survivors: Vec<&str> = disk.browsers.keys().map(String::as_str).collect();
        survivors.sort_unstable();
        assert_eq!(
            survivors,
            ["external", "live"],
            "expected only the dead entries to go"
        );
        let size_pruned = std::fs::metadata(&path).unwrap().len();
        assert!(
            size_pruned < size_with_dead,
            "file did not shrink: {size_with_dead} -> {size_pruned}"
        );

        // The saving process must stop carrying the dropped entries, or its next save
        // republishes them via the "untouched and absent from disk" upsert branch.
        assert!(
            !reader.browsers.contains_key("dead"),
            "the pruned entry is still staged in memory: {:?}",
            reader.browsers.keys()
        );
        save_to(&path, &mut reader).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            size_pruned,
            "a second save was not a no-op"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Pruning tests the pid on disk under the lock, so another agent's running browser
    /// survives: its pid answers `kill(pid, 0)`.
    #[cfg(unix)]
    #[test]
    fn pruning_leaves_a_concurrent_agents_live_browser_alone() {
        let dir = tmp_dir("prune-concurrent");
        let path = dir.join(SESSION_FILE);

        let mut a = SessionStore::default();
        ensure_browser(
            &mut a,
            "agent-a",
            "ws://a",
            Some(std::process::id()),
            true,
            None,
            Vec::new(),
        );
        let browser = a.browsers.get_mut("agent-a").unwrap();
        ensure_page(browser, "default", "target-a").last_snapshot =
            Some("uid=n1 RootWebArea".into());
        save_to(&path, &mut a).unwrap();

        // B loads that view, adds its own browser and a dead leftover, and saves.
        let mut b = load_from(&path).unwrap();
        ensure_browser(
            &mut b,
            "agent-b",
            "ws://b",
            Some(std::process::id()),
            true,
            None,
            Vec::new(),
        );
        ensure_browser(
            &mut b,
            "leftover",
            "ws://old",
            Some(a_dead_pid()),
            true,
            None,
            Vec::new(),
        );
        save_to(&path, &mut b).unwrap();

        // A saves again, holding its own stale copy of agent-b.
        save_to(&path, &mut a).unwrap();

        let disk = load_from(&path).unwrap();
        assert!(
            !disk.browsers.contains_key("leftover"),
            "dead entry survived"
        );
        assert_eq!(
            disk.browsers["agent-a"]
                .pages
                .get("default")
                .and_then(|p| p.last_snapshot.as_deref()),
            Some("uid=n1 RootWebArea"),
            "agent-a lost its snapshot"
        );
        assert!(
            disk.browsers.contains_key("agent-b"),
            "another agent's live browser was pruned: {:?}",
            disk.browsers.keys()
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The one-sided predicate, stated directly.
    #[test]
    fn only_a_pid_the_os_calls_gone_makes_an_entry_droppable() {
        let mut external = browser("ws://127.0.0.1:9222/x");
        external.pid = None;
        assert!(!is_provably_dead(&external), "--connect entry must be kept");

        let mut live = browser("ws://live");
        live.pid = Some(std::process::id());
        assert!(!is_provably_dead(&live));

        // Out of pid_t range: kill() would read it as a process group, so the entry is kept.
        let mut absurd = browser("ws://absurd");
        absurd.pid = Some(u32::MAX);
        assert!(!is_provably_dead(&absurd));
        let mut zero = browser("ws://zero");
        zero.pid = Some(0);
        assert!(!is_provably_dead(&zero));

        #[cfg(unix)]
        {
            let mut dead = browser("ws://dead");
            dead.pid = Some(a_dead_pid());
            assert!(is_provably_dead(&dead));
        }
    }

    /// The heartbeat sweep answers to the same one-sided rule as the save path. Its own
    /// predicate used to delete a pidless entry whose `/json/version` probe timed out, which
    /// is how a `--connect` browser busy for 500 ms lost its `uid_map` and `last_snapshot`.
    #[test]
    fn the_heartbeat_sweep_keeps_what_it_cannot_prove_dead() {
        let mut store = SessionStore::default();
        // Port 1 on loopback: nothing listens, so any probe-based rule would drop it.
        let mut external = browser("ws://127.0.0.1:1/devtools/browser/x");
        external.pid = None;
        store.browsers.insert("external".into(), external);
        let mut live = browser("ws://127.0.0.1:1/devtools/browser/y");
        live.pid = Some(std::process::id());
        store.browsers.insert("live".into(), live);

        cleanup_stale(&mut store);

        assert!(
            store.browsers.contains_key("external"),
            "a pidless entry carries no liveness information and must be kept"
        );
        assert!(
            store.browsers.contains_key("live"),
            "a live pid must be kept"
        );

        #[cfg(unix)]
        {
            let mut dead = browser("ws://dead");
            dead.pid = Some(a_dead_pid());
            store.browsers.insert("dead".into(), dead);
            cleanup_stale(&mut store);
            assert!(
                !store.browsers.contains_key("dead"),
                "a pid the OS calls gone is the one thing the sweep may drop"
            );
        }
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("chrome-agent_sess_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn save_merges_concurrent_additions_from_another_process() {
        let dir = tmp_dir("merge");
        let path = dir.join(SESSION_FILE);

        let mut mine = load_from(&path).unwrap();
        mine.browsers.insert("a".into(), browser("ws://a"));

        // Another process persists "b" while we hold our staged view.
        let mut theirs = load_from(&path).unwrap();
        theirs.browsers.insert("b".into(), browser("ws://b"));
        save_to(&path, &mut theirs).unwrap();

        // Our save must NOT clobber "b".
        save_to(&path, &mut mine).unwrap();

        let disk = load_from(&path).unwrap();
        assert!(
            disk.browsers.contains_key("a"),
            "own entry lost: {:?}",
            disk.browsers.keys()
        );
        assert!(
            disk.browsers.contains_key("b"),
            "concurrent entry clobbered: {:?}",
            disk.browsers.keys()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_deletes_only_entries_this_process_removed() {
        let dir = tmp_dir("delete");
        let path = dir.join(SESSION_FILE);

        let mut seed = SessionStore::default();
        seed.browsers.insert("a".into(), browser("ws://a"));
        seed.browsers.insert("b".into(), browser("ws://b"));
        save_to(&path, &mut seed).unwrap();

        // Drop "a", as `close --browser a` would.
        let mut store = load_from(&path).unwrap();
        store.browsers.remove("a");
        save_to(&path, &mut store).unwrap();

        let disk = load_from(&path).unwrap();
        assert!(
            !disk.browsers.contains_key("a"),
            "removed entry should be gone"
        );
        assert!(
            disk.browsers.contains_key("b"),
            "untouched entry should remain"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_does_not_delete_entries_added_by_others_after_load() {
        let dir = tmp_dir("nodelete");
        let path = dir.join(SESSION_FILE);

        let mut mine = load_from(&path).unwrap();
        mine.browsers.insert("a".into(), browser("ws://a"));

        // Another process adds "c" after our load.
        let mut other = load_from(&path).unwrap();
        other.browsers.insert("c".into(), browser("ws://c"));
        save_to(&path, &mut other).unwrap();

        // Our save adds "a" and must leave the never-loaded "c" alone.
        save_to(&path, &mut mine).unwrap();

        let disk = load_from(&path).unwrap();
        assert!(disk.browsers.contains_key("a"));
        assert!(
            disk.browsers.contains_key("c"),
            "must not delete an entry we never loaded"
        );
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
                    store
                        .browsers
                        .insert(format!("b{i}"), browser(&format!("ws://{i}")));
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
