//! Removal of browser profile directories that nothing owns any more.
//!
//! A profile is created by `launch_browser` and deleted only by `close --purge`. An
//! agent that omits the flag, or crashes, leaves ~14 MB behind for good — and the
//! session-store prune makes that worse, not better: it drops the entry as soon as the
//! browser's pid is gone, so the directory loses the only name anything knew it by.
//! Measured on a developer machine: `browsers/` held 1204 directories totalling 24.98 GB
//! against 3 entries in the store.
//!
//! The predicate is three-condition and every condition fails towards keeping, because
//! the two outcomes are not comparable: keeping an abandoned profile costs bytes, while
//! deleting a live one destroys whatever that browser is logged into.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::session::{liveness, Liveness};

/// How long a profile must sit untouched before it may be removed.
///
/// The window is what closes the create-then-write race without any new coordination:
/// `launch_browser` creates the directory and the store entry is only written when the
/// command saves, so a profile legitimately has no entry for as long as the launch takes
/// (up to the 10 s `DevToolsActivePort` wait) plus the command itself. A day is ~8600x
/// that, and it also covers the gap between two commands of one workflow, which is what
/// a wall-clock signal can actually distinguish an abandoned profile by.
///
/// What it deliberately sacrifices: a named browser that a person logged into by hand and
/// expects to still be logged in next week. That is not how the documented mechanisms
/// work (`--copy-cookies` re-imports on every fresh launch, `--connect` uses the real
/// Chrome), and trading a month of retention for 25 GB of certain growth is the wrong way
/// round. `close --purge-orphans` exists for a sweep on a schedule of the caller's choosing.
const GRACE: Duration = Duration::from_hours(24);

/// Profiles whose predicate is evaluated per invocation, and profiles removed per
/// invocation. This runs inside the session store's exclusive lock on the save path of
/// *every* command, including read-only ones, so the work has to be bounded by something
/// other than "however many orphans exist" — a first run against 1204 directories would
/// otherwise put a full recursive scan and 25 GB of unlinking on one `text --selector body`.
/// Removal is capped far harder than examination because removing one 14 MB profile is
/// thousands of unlinks, while examining one is a readdir and a few dozen stats.
const EXAMINE_CAP: usize = 32;
const REMOVE_CAP: usize = 1;

/// The browser every invocation targets when no `--browser` is given. It is exempt from
/// the automatic sweep: it is the profile a person is most likely to have shaped by hand
/// and least likely to have named for a throwaway task, and unlike every other name it
/// takes no typing to land on. `close --purge default` still removes it on request.
const IMPLICIT_BROWSER: &str = "default";

/// The subdirectory `browser::browser_profile_dir` puts Chrome's user data in. A directory
/// under `browsers/` without one was not created by a launch, so it is not ours to delete.
const PROFILE_SUBDIR: &str = "chromium-profile";

/// Caps and window for one sweep. Separated from the constants so the tests can drive a
/// window in milliseconds instead of waiting out a day.
pub struct Limits {
    pub grace: Duration,
    pub examine: usize,
    pub remove: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { grace: GRACE, examine: EXAMINE_CAP, remove: REMOVE_CAP }
    }
}

/// Whether a profile is held by a browser that is still running.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Hold {
    /// Every artefact that could name a holder says there is none.
    Free,
    Held,
    /// An artefact exists but does not resolve to a verdict. Not rounded to `Free`: the
    /// cases behind it are a lock from another host, a pid the OS will not classify, and a
    /// socket left by a Chrome that died mid-shutdown.
    Unknown,
}

/// Remove profile directories that pass the three-condition predicate, newest-first-safe
/// and bounded by `limits`. Returns the names removed.
///
/// `referenced` must be the store as it is about to be written, read under the same
/// exclusive lock: condition (a) is only meaningful against a store no other process can
/// be halfway through updating.
pub fn sweep_orphans(
    browsers_dir: &Path,
    referenced: &HashSet<String>,
    limits: &Limits,
) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(browsers_dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    names.sort_unstable();
    if names.is_empty() {
        return Vec::new();
    }

    // Rotate the window so successive invocations cover the whole directory. Reading the
    // first `examine` names every time would never reach an orphan sitting behind 32
    // profiles that keep failing the predicate.
    let rotation = rotation_offset(names.len());
    let now = SystemTime::now();
    let mut removed = Vec::new();
    for offset in 0..names.len().min(limits.examine) {
        let name = &names[(rotation + offset) % names.len()];
        if !removable(browsers_dir, name, referenced, now, limits.grace) {
            continue;
        }
        if std::fs::remove_dir_all(browsers_dir.join(name)).is_ok() {
            removed.push(name.clone());
        }
        if removed.len() >= limits.remove {
            break;
        }
    }
    removed
}

/// A per-invocation starting point. Derived from the clock and the pid rather than stored,
/// so nothing new has to be persisted or locked to make the sweep fair across the directory.
fn rotation_offset(len: usize) -> usize {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mixed = secs.wrapping_add(u64::from(std::process::id()));
    usize::try_from(mixed % len as u64).unwrap_or(0)
}

/// The three-condition predicate. Every branch that cannot reach a verdict returns false.
fn removable(
    browsers_dir: &Path,
    name: &str,
    referenced: &HashSet<String>,
    now: SystemTime,
    grace: Duration,
) -> bool {
    if name == IMPLICIT_BROWSER || referenced.contains(name) {
        return false;
    }
    // A name a launch could not have produced was not produced by one.
    if crate::browser::validate_browser_name(name).is_err() {
        return false;
    }
    let root = browsers_dir.join(name);
    let profile = root.join(PROFILE_SUBDIR);
    if !profile.is_dir() {
        return false;
    }
    if holder(&profile) != Hold::Free {
        return false;
    }
    let Some(touched) = last_touched(&root, &profile) else {
        return false;
    };
    now.duration_since(touched).is_ok_and(|idle| idle >= grace)
}

/// Whether anything still holds this profile, from the artefacts Chrome leaves in it.
///
/// `SingletonLock` is the load-bearing one: Chrome makes it a symlink whose target is
/// `hostname-pid`, which is the only place a profile states its owner. `DevToolsActivePort`
/// names a port but no pid, so it is answered by asking whether anything is listening.
fn holder(profile: &Path) -> Hold {
    match std::fs::read_link(profile.join("SingletonLock")) {
        Ok(target) => {
            let hold = singleton_lock_holder(&target.to_string_lossy());
            if hold != Hold::Free {
                return hold;
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        // Present but not a symlink, or unreadable: an artefact we cannot interpret.
        Err(_) => return Hold::Unknown,
    }
    // A socket or cookie with no lock beside it is a Chrome that went down without
    // unlinking them. Whether it is still going down is exactly what we cannot tell.
    for artefact in ["SingletonSocket", "SingletonCookie"] {
        if profile.join(artefact).symlink_metadata().is_ok() {
            return Hold::Unknown;
        }
    }
    devtools_port_holder(&profile.join("DevToolsActivePort"))
}

/// Read a `SingletonLock` target (`hostname-pid`) as a verdict about its pid.
fn singleton_lock_holder(target: &str) -> Hold {
    let Some((host, pid)) = target.rsplit_once('-') else {
        return Hold::Unknown;
    };
    // A lock written by another machine says nothing about pids on this one, and a home
    // directory can be shared. Treating its pid as ours risks deleting a profile that is
    // live somewhere else.
    if this_host().is_none_or(|ours| ours != host) {
        return Hold::Unknown;
    }
    let Ok(pid) = pid.parse::<u32>() else {
        return Hold::Unknown;
    };
    match liveness(pid) {
        Liveness::Alive => Hold::Held,
        Liveness::Dead => Hold::Free,
        Liveness::Unknown => Hold::Unknown,
    }
}

/// Ask whether anything is listening on the port a `DevToolsActivePort` file names.
///
/// A refused connection is the only answer that frees the profile. Anything that answers
/// holds it, even if what answered is not this profile's Chrome — the port may have been
/// recycled, and that ambiguity is not worth 14 MB.
fn devtools_port_holder(path: &Path) -> Hold {
    let Ok(contents) = std::fs::read_to_string(path) else {
        // Absent is the common case and the only one that means "no browser announced
        // itself here". An unreadable file is not distinguished: both land on the read
        // error, and `last_touched` would have failed on such a profile anyway.
        return if path.symlink_metadata().is_ok() { Hold::Unknown } else { Hold::Free };
    };
    let Some(port) = contents.lines().next().and_then(|l| l.trim().parse::<u16>().ok()) else {
        return Hold::Unknown;
    };
    if port == 0 {
        return Hold::Unknown;
    }
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    match std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(80)) {
        Ok(_) => Hold::Held,
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => Hold::Free,
        Err(_) => Hold::Unknown,
    }
}

/// Most recent mtime reachable without walking the profile, or `None` if the scan hit an
/// error anywhere.
///
/// Deliberately shallow: a profile is a Chromium user-data directory of thousands of files
/// across a hundred subdirectories, and `browsers/` held 24.98 GB of them — walking it on
/// the save path of every command is the cost this whole module is trying not to pay. The
/// terms are the two directories themselves, every direct child of `chromium-profile`, and
/// every direct child of `Default`, which is where the state a caller would miss lives.
///
/// This can read older than the truth: writing into an existing deep file moves neither
/// its parent's mtime nor any term here. It is a lower bound on activity, which is why the
/// grace window is a day rather than a minute, and why [`holder`] is consulted first.
fn last_touched(root: &Path, profile: &Path) -> Option<SystemTime> {
    let mut newest = mtime(root)?.max(mtime(profile)?);
    for dir in [profile.to_path_buf(), profile.join("Default")] {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            // `Default` need not exist; a profile that never launched has no such dir.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && dir != profile => continue,
            Err(_) => return None,
        };
        for entry in entries {
            // A directory we cannot enumerate is one whose age we do not know.
            let entry = entry.ok()?;
            let meta = entry.metadata().ok().or_else(|| entry.path().symlink_metadata().ok())?;
            newest = newest.max(meta.modified().ok()?);
        }
    }
    Some(newest)
}

fn mtime(path: &Path) -> Option<SystemTime> {
    path.symlink_metadata().ok()?.modified().ok()
}

/// This machine's hostname, as `SingletonLock` spells it.
fn this_host() -> Option<String> {
    #[cfg(unix)]
    {
        // One byte short of the buffer, so a truncated name still ends on the zero the
        // buffer was initialised with: POSIX does not promise termination on truncation.
        let mut buf = vec![0 as libc::c_char; 256];
        let len = buf.len() - 1;
        // SAFETY: gethostname writes at most `len` bytes into a buffer of len + 1 we own.
        #[allow(unsafe_code)]
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr(), len) };
        if rc != 0 {
            return None;
        }
        // SAFETY: the buffer is zero-initialised and one byte longer than what
        // gethostname was allowed to write, so it is NUL-terminated within bounds.
        #[allow(unsafe_code)]
        let host = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
        host.to_str().ok().map(str::to_owned)
    }
    #[cfg(not(unix))]
    {
        // Without a hostname every lock reads as another machine's, so no profile is ever
        // provably free. Growth is the previous behaviour, not a regression.
        None
    }
}

/// Every profile the predicate judges removable, uncapped. Backs `close --purge-orphans`,
/// which is the only way to reclaim a store that accumulated before any of this existed:
/// the automatic sweep removes one profile per command by design, so 1204 of them would
/// take 1204 commands.
pub fn all_removable(
    browsers_dir: &Path,
    referenced: &HashSet<String>,
    grace: Duration,
) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(browsers_dir) else {
        return Vec::new();
    };
    let now = SystemTime::now();
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            removable(browsers_dir, &name, referenced, now, grace)
                .then(|| browsers_dir.join(name))
        })
        .collect();
    found.sort_unstable();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A profile directory as `launch_browser` would leave it, aged by `idle`.
    fn profile(browsers: &Path, name: &str, idle: Duration) -> PathBuf {
        let root = browsers.join(name);
        let dir = root.join(PROFILE_SUBDIR);
        std::fs::create_dir_all(dir.join("Default")).unwrap();
        std::fs::write(dir.join("Local State"), "{}").unwrap();
        std::fs::write(dir.join("Default").join("Cookies"), "x").unwrap();
        backdate(&root, idle);
        root
    }

    /// Set the mtime of every term `last_touched` reads. No `filetime` crate in the graph,
    /// so this goes through `utimensat` on the paths directly.
    fn backdate(root: &Path, idle: Duration) {
        let when = SystemTime::now() - idle;
        let secs = when.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
        let profile = root.join(PROFILE_SUBDIR);
        let mut paths = vec![root.to_path_buf(), profile.clone()];
        for dir in [profile.clone(), profile.join("Default")] {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                paths.extend(entries.filter_map(|e| e.ok().map(|e| e.path())));
            }
        }
        // Children first: touching a directory's entries bumps the directory.
        paths.reverse();
        for path in paths {
            set_mtime(&path, secs);
        }
    }

    #[cfg(unix)]
    fn set_mtime(path: &Path, secs: u64) {
        use std::os::unix::ffi::OsStrExt;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        let ts = libc::timespec { tv_sec: secs as libc::time_t, tv_nsec: 0 };
        let times = [ts, ts];
        // SAFETY: both pointers are valid for the duration of the call.
        #[allow(unsafe_code)]
        unsafe {
            libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), libc::AT_SYMLINK_NOFOLLOW);
        }
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("chrome-agent_profiles_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn week() -> Duration {
        Duration::from_hours(24 * 7)
    }

    /// A live process standing in for a running Chrome, so a `SingletonLock` fixture names
    /// a pid the OS will answer for. Reaped on drop.
    struct LivePid(std::process::Child);

    impl LivePid {
        fn spawn() -> Self {
            Self(std::process::Command::new("sleep").arg("30").spawn().unwrap())
        }
    }

    impl Drop for LivePid {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// The whole predicate in one place: four profiles, one removable.
    #[cfg(unix)]
    #[test]
    fn only_an_unreferenced_unheld_and_idle_profile_is_removed() {
        let browsers = tmp_dir("predicate");
        let held = LivePid::spawn();

        // (i) referenced by the store, and old enough that only the reference saves it.
        profile(&browsers, "in-store", week());
        // (ii) orphaned and idle: the one case this module exists for.
        profile(&browsers, "orphan-old", week());
        // (iii) orphaned but touched just now — inside the grace window.
        profile(&browsers, "orphan-fresh", Duration::from_secs(0));
        // (iv) orphaned and idle, but its SingletonLock names a process that is running.
        let live = profile(&browsers, "orphan-locked", week());
        std::os::unix::fs::symlink(
            format!("{}-{}", this_host().unwrap(), held.0.id()),
            live.join(PROFILE_SUBDIR).join("SingletonLock"),
        )
        .unwrap();

        let referenced: HashSet<String> = std::iter::once("in-store".to_string()).collect();
        let limits = Limits { grace: Duration::from_mins(1), examine: 64, remove: 64 };
        let removed = sweep_orphans(&browsers, &referenced, &limits);

        assert_eq!(removed, vec!["orphan-old".to_string()], "wrong set removed");
        for kept in ["in-store", "orphan-fresh", "orphan-locked"] {
            assert!(browsers.join(kept).is_dir(), "{kept} was removed");
        }
        std::fs::remove_dir_all(&browsers).ok();
    }

    /// The reason the grace window is not optional. Two agents launch at the same instant;
    /// neither has written its store entry yet, and each sweeps against a store naming only
    /// the other. Without the window each would judge the other's profile an orphan.
    #[test]
    fn a_just_created_profile_survives_a_concurrent_agents_sweep() {
        let browsers = tmp_dir("race");
        profile(&browsers, "agent-a", Duration::from_secs(0));
        profile(&browsers, "agent-b", Duration::from_secs(0));

        let limits = || Limits { grace: GRACE, examine: 64, remove: 64 };
        let a_sees: HashSet<String> = std::iter::once("agent-b".to_string()).collect();
        let b_sees: HashSet<String> = std::iter::once("agent-a".to_string()).collect();
        assert!(sweep_orphans(&browsers, &a_sees, &limits()).is_empty());
        assert!(sweep_orphans(&browsers, &b_sees, &limits()).is_empty());

        assert!(browsers.join("agent-a").is_dir(), "agent-a's fresh profile was deleted");
        assert!(browsers.join("agent-b").is_dir(), "agent-b's fresh profile was deleted");
        std::fs::remove_dir_all(&browsers).ok();
    }

    /// The cap is what keeps a read-only command from paying for the whole backlog.
    #[test]
    fn removal_is_capped_per_invocation() {
        let browsers = tmp_dir("cap");
        for i in 0..12 {
            profile(&browsers, &format!("orphan-{i}"), week());
        }
        let referenced = HashSet::new();

        let limits = Limits { grace: Duration::from_mins(1), examine: 32, remove: 3 };
        let removed = sweep_orphans(&browsers, &referenced, &limits);
        assert_eq!(removed.len(), 3, "removal cap ignored: {removed:?}");
        assert_eq!(
            std::fs::read_dir(&browsers).unwrap().count(),
            9,
            "removed a different number than reported"
        );
        std::fs::remove_dir_all(&browsers).ok();
    }

    /// Examination is capped too, and rotates: repeated sweeps must eventually reach every
    /// orphan rather than re-reading the same window.
    #[test]
    fn repeated_sweeps_reach_every_orphan() {
        let browsers = tmp_dir("rotate");
        for i in 0..8 {
            profile(&browsers, &format!("orphan-{i}"), week());
        }
        let referenced = HashSet::new();
        for _ in 0..40 {
            let limits = Limits { grace: Duration::from_mins(1), examine: 2, remove: 1 };
            if sweep_orphans(&browsers, &referenced, &limits).is_empty()
                && std::fs::read_dir(&browsers).unwrap().count() == 0
            {
                break;
            }
        }
        assert_eq!(
            std::fs::read_dir(&browsers).unwrap().count(),
            0,
            "a capped sweep never reached some orphans"
        );
        std::fs::remove_dir_all(&browsers).ok();
    }

    /// Anything that is not a profile directory is not ours to delete, however old.
    #[test]
    fn a_directory_that_is_not_a_profile_is_left_alone() {
        let browsers = tmp_dir("foreign");
        // No `chromium-profile` inside: not something a launch created.
        std::fs::create_dir_all(browsers.join("notes")).unwrap();
        std::fs::write(browsers.join("notes").join("keep.txt"), "mine").unwrap();
        // A name a launch could not have produced.
        std::fs::create_dir_all(browsers.join("has space").join(PROFILE_SUBDIR)).unwrap();

        let removed = sweep_orphans(
            &browsers,
            &HashSet::new(),
            &Limits { grace: Duration::from_secs(0), examine: 64, remove: 64 },
        );
        assert!(removed.is_empty(), "removed a non-profile: {removed:?}");
        assert!(browsers.join("notes").join("keep.txt").exists());
        std::fs::remove_dir_all(&browsers).ok();
    }

    /// The default browser is the one every flagless invocation lands on, so it is never
    /// swept automatically however idle it looks.
    #[test]
    fn the_implicit_browser_is_exempt() {
        let browsers = tmp_dir("implicit");
        profile(&browsers, IMPLICIT_BROWSER, week());
        let removed = sweep_orphans(
            &browsers,
            &HashSet::new(),
            &Limits { grace: Duration::from_mins(1), examine: 64, remove: 64 },
        );
        assert!(removed.is_empty(), "the default profile was swept: {removed:?}");
        std::fs::remove_dir_all(&browsers).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_lock_from_another_host_is_never_a_verdict() {
        // Same pid shape, different machine: the number means nothing here.
        assert_eq!(
            singleton_lock_holder("some-other-host-1"),
            Hold::Unknown,
            "another host's lock was read as a local pid"
        );
        assert_eq!(singleton_lock_holder("no-separator"), Hold::Unknown);
        let host = this_host().unwrap();
        assert_eq!(singleton_lock_holder(&format!("{host}-notanumber")), Hold::Unknown);
        assert_eq!(
            singleton_lock_holder(&format!("{host}-{}", std::process::id())),
            Hold::Held
        );
    }

    /// An empty `browsers/` (and a missing one) must not error or delete anything.
    #[test]
    fn an_absent_or_empty_store_sweeps_to_nothing() {
        let dir = tmp_dir("absent");
        assert!(sweep_orphans(&dir.join("nope"), &HashSet::new(), &Limits::default()).is_empty());
        assert!(sweep_orphans(&dir, &HashSet::new(), &Limits::default()).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
