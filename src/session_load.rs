//! Reading the session store off disk, and telling apart the two ways that can fail.
//!
//! Split from `session.rs` for the 1000-line cap. It is a small module with one idea in it:
//! "the file is not there", "the file would not read" and "the file is not a session store"
//! are three different facts, and the save path needs all three separated. It used to have
//! none of them — `load_from(path).unwrap_or_default()` (`session.rs:151`) turned every one
//! of them into an empty store, under the exclusive lock, one line before a merge whose
//! whole contract is that a writer publishes its own entries and leaves everyone else's
//! alone. An empty re-read makes that merge publish this process's view as the whole store.
//!
//! For a PARSE failure that is defensible: the bytes on disk are not a session store for
//! anyone, no reader can recover an entry from them, and rewriting the file is the only way
//! the tool ever becomes usable again. Truncation is a debatable self-repair, not a data
//! loss, because there was nothing left to lose.
//!
//! For a READ failure it is neither. `EIO` on the volume, `EMFILE` on this process,
//! permissions changed under a shared home: the file is intact, every other agent can still
//! read it, and this process is about to replace it with the one browser it happens to know
//! about. That is a silent, total loss of another agent's live entries — the exact invariant
//! `save_to` spends its compare-and-delete and untouched-entry logic defending, given away
//! one line before either of them runs.
//!
//! How reachable is it? Narrow, and worth saying so rather than letting the next reader
//! assume it is the common case. Every production load goes up through `?`
//! (`run_helpers.rs:525/728/755`, `pipe.rs:26/188/280`, `connect_cli.rs:25`,
//! `orphans.rs:111`), so a file that is already unreadable at startup fails the command
//! before `save_to` is ever called. What is left is the file that becomes unreadable
//! BETWEEN the load and the save — a long-running `pipe` session is exactly that shape —
//! plus every caller that reaches `save_to` without having loaded through those paths.

use std::path::Path;

use crate::session::{SessionError, SessionStore};

/// Why a store on disk could not be turned into a [`SessionStore`].
///
/// Two variants rather than one `SessionError`, because the two are not equally grave and
/// the save path acts differently on them. Kept private: everything outside this module
/// wants one of the two policies below, not the choice between them.
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

/// Read and parse, keeping the two failures apart. An absent file is not a failure at all:
/// no store yet is the ordinary state of a fresh machine and the empty store is the truth.
fn read_and_parse(path: &Path) -> Result<SessionStore, LoadFailure> {
    if !path.exists() {
        return Ok(SessionStore::default());
    }

    let contents = std::fs::read_to_string(path).map_err(|e| {
        LoadFailure::Unreadable(SessionError(format!("Failed to read {}: {e}", path.display())))
    })?;

    let mut store: SessionStore = serde_json::from_str(&contents).map_err(|e| {
        LoadFailure::Unparsable(SessionError(format!("Failed to parse {}: {e}", path.display())))
    })?;
    store.take_baseline();
    Ok(store)
}

/// Read a session store from an explicit path (empty store if the file is absent). Records
/// the loaded browser names and bytes as the delete baseline.
///
/// Both failures are errors here, which is what every command-level caller wants: a store it
/// could not read is a command it cannot run.
pub fn load_from(path: &Path) -> Result<SessionStore, SessionError> {
    read_and_parse(path).map_err(LoadFailure::into_error)
}

/// The same read, taken inside the save path's exclusive lock, where the two failures stop
/// being interchangeable.
///
/// - Absent: an empty store, unchanged. The first save on a fresh machine goes through here.
/// - Unparsable: an empty store, and the merge then rewrites the file. Stated as a policy
///   rather than an accident — see the module docs. The bytes are unusable to every reader,
///   so nothing is being taken from anyone, and refusing here would leave the tool wedged
///   with no command that repairs it.
/// - Unreadable: `Err`, and `save_to` returns it. The lock is held and NOTHING has been
///   written yet, so there is nothing to undo — the file keeps whatever it had, and the
///   command fails saying which file and which OS error. Losing every other agent's entries
///   because this process could not open a descriptor is not a failure mode worth having.
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
    /// `cargo test` runs on one machine is the normal regime here.
    fn temp_dir(label: &str) -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("chrome-agent-session-load-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the test's own directory");
        dir
    }

    /// A file that is present and will not open is not an empty store.
    ///
    /// Simulated with a directory where the file should be: `read_to_string` on it fails
    /// with `EISDIR` while `exists()` says yes, which is the shape of every real cause
    /// (`EIO`, `EMFILE`, a permission change under a shared home) and needs no root and no
    /// unsafe. What matters is the branch, not which errno reached it.
    #[test]
    fn a_present_but_unreadable_store_is_refused_rather_than_emptied() {
        let dir = temp_dir("unreadable");
        let path = dir.join("sessions.json");
        std::fs::create_dir(&path).expect("stand in for a file that will not read");

        let err = reread_for_merge(&path).expect_err("a merge may not start from a guess");
        assert!(err.0.contains("Failed to read"), "{}", err.0);
        assert!(err.0.contains("sessions.json"), "the file that failed is the fact: {}", err.0);
        assert!(load_from(&path).is_err(), "the command-level read refuses it too");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A corrupt file must surface a parse error to a command — not panic, and not silently
    /// default the on-disk state away. Moved here from `session.rs` with the function it
    /// exercises.
    #[test]
    fn bug_session_corrupt_json() {
        let dir = temp_dir("corrupt");
        let path = dir.join("sessions.json");
        std::fs::write(&path, "NOT VALID JSON {{{").unwrap();
        let err = load_from(&path).expect_err("corrupt JSON should error").to_string();
        assert!(err.contains("Failed to parse"), "unexpected error: {err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// An empty (e.g. externally truncated) file is not valid JSON, and the parse error is
    /// what a command sees — the absent-file case is the `!path.exists()` guard, contrasted
    /// here. Moved here from `session.rs` with the function it exercises.
    #[test]
    fn bug_session_empty_file() {
        let dir = temp_dir("empty");
        let path = dir.join("sessions.json");
        std::fs::write(&path, "").unwrap();
        let err = load_from(&path).expect_err("empty file should error").to_string();
        assert!(err.contains("Failed to parse"), "unexpected error: {err}");

        std::fs::remove_file(&path).unwrap();
        let default = load_from(&path).expect("absent file should default");
        assert!(default.browsers.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The other two directions, which must NOT be refused: they are how a fresh machine and
    /// a corrupted file both become usable again.
    #[test]
    fn an_absent_store_is_empty_and_an_unparsable_one_is_replaced() {
        let dir = temp_dir("absent-and-corrupt");
        let path = dir.join("sessions.json");

        let fresh = reread_for_merge(&path).expect("no file yet is the ordinary state");
        assert!(fresh.browsers.is_empty());

        // Truncation by an interrupted writer, or any other garbage: unreadable to every
        // agent, so rewriting it takes nothing from anyone.
        std::fs::write(&path, "{ not json").expect("write the corrupted file");
        let recovered = reread_for_merge(&path).expect("a corrupt store must not wedge the tool");
        assert!(recovered.browsers.is_empty());
        // The command-level read still reports it, and says which of the two it was.
        let err = load_from(&path).expect_err("a corrupt store is an error to a command");
        assert!(err.0.contains("Failed to parse"), "{}", err.0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A store this process cannot READ is not an empty store, and the merge may not start
    /// from one.
    ///
    /// `save_to` re-reads the file under the exclusive lock and used to `unwrap_or_default()`
    /// that read. The merge below it then treats every name it holds as "mine to publish"
    /// and every name absent from the (empty) disk view as gone, so one process's single
    /// browser was written over every other agent's entries. The cause needs no corruption:
    /// `EIO`, `EMFILE`, a permission change under a shared home — the file stays perfectly
    /// valid and every other agent keeps reading it.
    ///
    /// Simulated with mode 0, which is a real cause rather than a stand-in for one. The
    /// assertion that carries the point is the last: the bytes on disk are untouched, which
    /// is what taking the lock and refusing BEFORE any write buys.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_store_fails_the_save_instead_of_replacing_it() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("unreadable");
        let path = dir.join("sessions.json");

        // Another agent's store, valid and complete.
        let mut theirs = SessionStore::default();
        crate::session::ensure_browser(&mut theirs, "someone-else", "ws://theirs", None, true, None, Vec::new());
        crate::session::save_to(&path, &mut theirs).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(before.contains("someone-else"));

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

        // Root ignores the mode, so the fixture cannot hold there. Skip rather than assert
        // something the machine did not do.
        if std::fs::read_to_string(&path).is_ok() {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            let _ = std::fs::remove_dir_all(&dir);
            eprintln!("SKIP: this user can read a mode-0 file, so the read cannot be made to fail");
            return;
        }

        let mut mine = SessionStore::default();
        crate::session::ensure_browser(&mut mine, "mine", "ws://mine", None, true, None, Vec::new());
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

    /// The other half of the same rule, and the reason it is not simply "refuse on any
    /// error": an absent file is not a failure, and the first save on a fresh machine has to
    /// go straight through the same code path.
    #[test]
    fn an_absent_store_still_saves_from_empty() {
        let dir = temp_dir("fresh");
        let path = dir.join("sessions.json");
        assert!(!path.exists());

        let mut store = SessionStore::default();
        crate::session::ensure_browser(&mut store, "first", "ws://first", None, true, None, Vec::new());
        crate::session::save_to(&path, &mut store).expect("no file yet is the ordinary state, not a failure");

        let disk = load_from(&path).unwrap();
        assert!(disk.browsers.contains_key("first"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
