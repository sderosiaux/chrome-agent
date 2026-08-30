//! Reading the session store off disk, keeping "absent", "unreadable" and "unparsable" apart.
//!
//! `save_to` re-reads under the exclusive lock and merges, so an empty re-read makes it
//! publish this process's browsers over every other agent's entries. An UNREADABLE file
//! (`EIO`, `EMFILE`, a permission change under a shared home) must therefore refuse, not
//! default: the file is intact and every other agent can still read it. An UNPARSABLE one
//! may default and be rewritten — those bytes are a session store to nobody, so nothing is
//! lost, and no other command repairs the wedge. An absent file is not a failure at all.

use std::path::Path;

use crate::session::{SessionError, SessionStore};

/// Why a store on disk could not be turned into a [`SessionStore`]. Private: callers want
/// one of the two policies below, not the choice between them.
enum LoadFailure {
    /// The bytes never arrived. The file may be perfectly valid.
    Unreadable(SessionError),
    /// The bytes arrived and are not a session store. Nobody can read this file.
    Unparsable(SessionError),
}

impl LoadFailure {
    /// Flatten back to the single error type every caller outside this module reports.
    fn into_error(self) -> SessionError {
        match self {
            Self::Unreadable(e) | Self::Unparsable(e) => e,
        }
    }
}

/// Read and parse, keeping the two failures apart. An absent file is an empty store, not a
/// failure: that is the ordinary state of a fresh machine.
fn read_and_parse(path: &Path) -> Result<SessionStore, LoadFailure> {
    if !path.exists() {
        return Ok(SessionStore::default());
    }

    let contents = std::fs::read_to_string(path).map_err(|e| {
        LoadFailure::Unreadable(SessionError(format!(
            "Failed to read {}: {e}",
            path.display()
        )))
    })?;

    let mut store: SessionStore = serde_json::from_str(&contents).map_err(|e| {
        LoadFailure::Unparsable(SessionError(format!(
            "Failed to parse {}: {e}",
            path.display()
        )))
    })?;
    store.take_baseline();
    Ok(store)
}

/// Read a session store from an explicit path (empty store if absent), recording the loaded
/// names and bytes as the delete baseline. Both failures are errors: a store a command
/// could not read is a command it cannot run.
pub fn load_from(path: &Path) -> Result<SessionStore, SessionError> {
    read_and_parse(path).map_err(LoadFailure::into_error)
}

/// The same read, taken inside the save path's exclusive lock.
///
/// - Absent: empty store. The first save on a fresh machine goes through here.
/// - Unparsable: empty store, and the merge rewrites the file. Deliberate policy.
/// - Unreadable: `Err`, returned by `save_to`. The lock is held and nothing has been
///   written, so the file keeps what it had.
pub fn reread_for_merge(path: &Path) -> Result<SessionStore, SessionError> {
    match read_and_parse(path) {
        Ok(store) => Ok(store),
        Err(LoadFailure::Unparsable(_)) => Ok(SessionStore::default()),
        Err(LoadFailure::Unreadable(e)) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique per test AND per process: the harness runs tests on parallel threads, and two
    /// concurrent `cargo test` runs are normal here.
    fn temp_dir(label: &str) -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "chrome-agent-session-load-{label}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the test's own directory");
        dir
    }

    /// A file that is present and will not open is not an empty store. Simulated with a
    /// directory in the file's place: `read_to_string` fails with `EISDIR` while `exists()`
    /// says yes, needing neither root nor unsafe. The branch matters, not the errno.
    #[test]
    fn a_present_but_unreadable_store_is_refused_rather_than_emptied() {
        let dir = temp_dir("unreadable");
        let path = dir.join("sessions.json");
        std::fs::create_dir(&path).expect("stand in for a file that will not read");

        let err = reread_for_merge(&path).expect_err("a merge may not start from a guess");
        assert!(err.0.contains("Failed to read"), "{}", err.0);
        assert!(
            err.0.contains("sessions.json"),
            "the file that failed is the fact: {}",
            err.0
        );
        assert!(
            load_from(&path).is_err(),
            "the command-level read refuses it too"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A corrupt file surfaces a parse error to a command: no panic, no silent default.
    #[test]
    fn bug_session_corrupt_json() {
        let dir = temp_dir("corrupt");
        let path = dir.join("sessions.json");
        std::fs::write(&path, "NOT VALID JSON {{{").unwrap();
        let err = load_from(&path)
            .expect_err("corrupt JSON should error")
            .to_string();
        assert!(err.contains("Failed to parse"), "unexpected error: {err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// An empty (externally truncated) file is not valid JSON, so a command sees a parse
    /// error. The absent-file case hits the `!path.exists()` guard instead, contrasted here.
    #[test]
    fn bug_session_empty_file() {
        let dir = temp_dir("empty");
        let path = dir.join("sessions.json");
        std::fs::write(&path, "").unwrap();
        let err = load_from(&path)
            .expect_err("empty file should error")
            .to_string();
        assert!(err.contains("Failed to parse"), "unexpected error: {err}");

        std::fs::remove_file(&path).unwrap();
        let default = load_from(&path).expect("absent file should default");
        assert!(default.browsers.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The two directions that must NOT be refused: a fresh machine and a corrupted file
    /// both have to become usable again.
    #[test]
    fn an_absent_store_is_empty_and_an_unparsable_one_is_replaced() {
        let dir = temp_dir("absent-and-corrupt");
        let path = dir.join("sessions.json");

        let fresh = reread_for_merge(&path).expect("no file yet is the ordinary state");
        assert!(fresh.browsers.is_empty());

        // Garbage is unreadable to every agent, so rewriting it takes nothing from anyone.
        std::fs::write(&path, "{ not json").expect("write the corrupted file");
        let recovered = reread_for_merge(&path).expect("a corrupt store must not wedge the tool");
        assert!(recovered.browsers.is_empty());
        // The command-level read still reports it, naming which failure it was.
        let err = load_from(&path).expect_err("a corrupt store is an error to a command");
        assert!(err.0.contains("Failed to parse"), "{}", err.0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A store this process cannot READ is not an empty store, and the merge may not start
    /// from one: it would publish this process's browsers over every other agent's entries.
    /// Simulated with mode 0, a real cause. The last assertion carries the point — the bytes
    /// on disk are untouched, which is what refusing before any write buys.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_store_fails_the_save_instead_of_replacing_it() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("unreadable");
        let path = dir.join("sessions.json");

        // Another agent's store, valid and complete.
        let mut theirs = SessionStore::default();
        crate::session::ensure_browser(
            &mut theirs,
            "someone-else",
            "ws://theirs",
            None,
            true,
            None,
            Vec::new(),
        );
        crate::session::save_to(&path, &mut theirs).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(before.contains("someone-else"));

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

        // Root ignores the mode, so the fixture cannot hold there. Skip rather than assert.
        if std::fs::read_to_string(&path).is_ok() {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            let _ = std::fs::remove_dir_all(&dir);
            eprintln!("SKIP: this user can read a mode-0 file, so the read cannot be made to fail");
            return;
        }

        let mut mine = SessionStore::default();
        crate::session::ensure_browser(
            &mut mine,
            "mine",
            "ws://mine",
            None,
            true,
            None,
            Vec::new(),
        );
        let err = crate::session::save_to(&path, &mut mine)
            .expect_err("a save that cannot read the store must not publish over it")
            .0;
        assert!(err.contains("Failed to read"), "{err}");

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "the other agent's entries were replaced by the view of a process that read nothing"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Why the rule is not simply "refuse on any error": an absent file is not a failure,
    /// and the first save on a fresh machine goes through the same code path.
    #[test]
    fn an_absent_store_still_saves_from_empty() {
        let dir = temp_dir("fresh");
        let path = dir.join("sessions.json");
        assert!(!path.exists());

        let mut store = SessionStore::default();
        crate::session::ensure_browser(
            &mut store,
            "first",
            "ws://first",
            None,
            true,
            None,
            Vec::new(),
        );
        crate::session::save_to(&path, &mut store)
            .expect("no file yet is the ordinary state, not a failure");

        let disk = load_from(&path).unwrap();
        assert!(disk.browsers.contains_key("first"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
