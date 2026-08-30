//! Removal of browser profile directories that nothing owns any more.
//!
//! A profile is created by `launch_browser` and deleted only by `close --purge`, so an
//! agent that omits the flag or crashes leaves ~14 MB behind. Measured: 1204 directories,
//! 24.98 GB, against 3 entries in the store. The predicate has three conditions and every
//! one fails towards keeping — an abandoned profile costs bytes, a deleted live one
//! destroys whatever it was logged into.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::session::{Liveness, liveness};

/// How long a profile must sit untouched before it may be removed. The window closes the
/// create-then-write race without new coordination: a profile legitimately has no store
/// entry for as long as the launch takes (up to the 10 s `DevToolsActivePort` wait) plus
/// the command. A day is ~8600x that. It sacrifices a browser someone logged into by hand
/// and expects to still be logged in next week.
const GRACE: Duration = Duration::from_hours(24);

/// Profiles examined, and profiles removed, per invocation. The sweep runs on the save path
/// of *every* command, so it must be bounded however many orphans exist. Removal is capped
/// harder: one 14 MB profile is thousands of unlinks, one examination is a readdir.
const EXAMINE_CAP: usize = 32;
const REMOVE_CAP: usize = 1;

/// The browser every invocation targets when no `--browser` is given. Exempt from the
/// automatic sweep; `close --purge default` still removes it on request.
const IMPLICIT_BROWSER: &str = "default";

/// The subdirectory `browser::browser_profile_dir` puts Chrome's user data in. A directory
/// under `browsers/` without one was not created by a launch, so it is not ours to delete.
const PROFILE_SUBDIR: &str = "chromium-profile";

/// Caps and window for one sweep. Separate from the constants so tests can drive a window
/// in milliseconds instead of waiting out a day.
pub struct Limits {
    pub grace: Duration,
    pub examine: usize,
    pub remove: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            grace: GRACE,
            examine: EXAMINE_CAP,
            remove: REMOVE_CAP,
        }
    }
}

/// Whether a profile is held by a browser that is still running.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Hold {
    /// Every artefact that could name a holder says there is none.
    Free,
    Held,
    /// An artefact exists but resolves to no verdict: a lock from another host, a pid the OS
    /// will not classify, a socket left by a dying Chrome. Never rounded to `Free`.
    Unknown,
}

/// Remove profile directories passing the predicate, bounded by `limits`, returning the
/// names removed. `referenced` must be the store as it is about to be written, read under
/// the same exclusive lock; otherwise another process may be halfway through updating it.
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

    // Rotate the window: a fixed first `examine` names never reaches an orphan sitting
    // behind 32 profiles that keep failing the predicate.
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

/// A per-invocation starting point, derived from the clock and the pid so nothing extra has
/// to be persisted or locked.
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

/// Whether anything still holds this profile. `SingletonLock` is load-bearing: a symlink to
/// `hostname-pid`, the only place a profile states its owner. `DevToolsActivePort` names a
/// port and no pid, so it is answered by asking whether anything is listening.
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
    // unlinking them. Whether it is still going down cannot be told from here.
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
    // A home directory can be shared, and another machine's lock says nothing about pids
    // here. Treating its pid as ours risks deleting a profile live somewhere else.
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

/// Ask whether anything is listening on the port a `DevToolsActivePort` file names. A
/// refused connection is the only answer that frees the profile; anything that answers
/// holds it, even if the port was recycled by an unrelated process.
fn devtools_port_holder(path: &Path) -> Hold {
    let Ok(contents) = std::fs::read_to_string(path) else {
        // Only absence means "no browser announced itself here"; unreadable stays `Unknown`.
        return if path.symlink_metadata().is_ok() {
            Hold::Unknown
        } else {
            Hold::Free
        };
    };
    let Some(port) = contents
        .lines()
        .next()
        .and_then(|l| l.trim().parse::<u16>().ok())
    else {
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

/// Most recent mtime reachable without walking the profile, or `None` if the scan errored.
/// Shallow on purpose — a full walk of thousands of files on every save is what this module
/// avoids. Terms: the two directories, every direct child of `chromium-profile`, and every
/// direct child of `Default`. It can read older than the truth, so it is a lower bound on
/// activity: hence a day-long grace window and [`holder`] being consulted first.
fn last_touched(root: &Path, profile: &Path) -> Option<SystemTime> {
    let mut newest = mtime(root)?.max(mtime(profile)?);
    for dir in [profile.to_path_buf(), profile.join("Default")] {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            // A profile that never launched has no `Default`.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && dir != profile => continue,
            Err(_) => return None,
        };
        for entry in entries {
            // A directory we cannot enumerate is one whose age we do not know.
            let entry = entry.ok()?;
            let meta = entry
                .metadata()
                .ok()
                .or_else(|| entry.path().symlink_metadata().ok())?;
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
        // One byte short of the buffer: POSIX does not promise NUL termination on truncation.
        let mut buf = vec![0 as libc::c_char; 256];
        let len = buf.len() - 1;
        // SAFETY: gethostname writes at most `len` bytes into a buffer of len + 1 we own.
        #[allow(unsafe_code)]
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr(), len) };
        if rc != 0 {
            return None;
        }
        // SAFETY: the buffer is zero-initialised and one byte longer than gethostname was
        // allowed to write, so it is NUL-terminated within bounds.
        #[allow(unsafe_code)]
        let host = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
        host.to_str().ok().map(str::to_owned)
    }
    #[cfg(not(unix))]
    {
        // Without a hostname every lock reads as another machine's, so nothing is swept.
        None
    }
}

/// Every profile the predicate judges removable, uncapped. Backs `close --purge-orphans`,
/// since the automatic sweep removes only one profile per command.
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
            removable(browsers_dir, &name, referenced, now, grace).then(|| browsers_dir.join(name))
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

    /// Set the mtime of every term `last_touched` reads, via `utimensat` (no `filetime`
    /// crate in the dependency graph).
    fn backdate(root: &Path, idle: Duration) {
        let when = SystemTime::now() - idle;
        let secs = when
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
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
        let ts = libc::timespec {
            tv_sec: secs as libc::time_t,
            tv_nsec: 0,
        };
        let times = [ts, ts];
        // SAFETY: both pointers are valid for the duration of the call.
        #[allow(unsafe_code)]
        unsafe {
            libc::utimensat(
                libc::AT_FDCWD,
                c_path.as_ptr(),
                times.as_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            );
        }
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "chrome-agent_profiles_{}_{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn week() -> Duration {
        Duration::from_hours(24 * 7)
    }

    /// A live process a `SingletonLock` fixture can name. Reaped on drop.
    struct LivePid(std::process::Child);

    impl LivePid {
        fn spawn() -> Self {
            Self(
                std::process::Command::new("sleep")
                    .arg("30")
                    .spawn()
                    .unwrap(),
            )
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

        // Referenced by the store, and old enough that only the reference saves it.
        profile(&browsers, "in-store", week());
        // Orphaned and idle: the one removable case.
        profile(&browsers, "orphan-old", week());
        // Orphaned but inside the grace window.
        profile(&browsers, "orphan-fresh", Duration::from_secs(0));
        // Orphaned and idle, but its SingletonLock names a running process.
        let live = profile(&browsers, "orphan-locked", week());
        std::os::unix::fs::symlink(
            format!("{}-{}", this_host().unwrap(), held.0.id()),
            live.join(PROFILE_SUBDIR).join("SingletonLock"),
        )
        .unwrap();

        let referenced: HashSet<String> = std::iter::once("in-store".to_string()).collect();
        let limits = Limits {
            grace: Duration::from_mins(1),
            examine: 64,
            remove: 64,
        };
        let removed = sweep_orphans(&browsers, &referenced, &limits);

        assert_eq!(removed, vec!["orphan-old".to_string()], "wrong set removed");
        for kept in ["in-store", "orphan-fresh", "orphan-locked"] {
            assert!(browsers.join(kept).is_dir(), "{kept} was removed");
        }
        std::fs::remove_dir_all(&browsers).ok();
    }

    /// Why the grace window is not optional: two agents launch at once, neither has written
    /// its entry, and each sweeps against a store naming only the other.
    #[test]
    fn a_just_created_profile_survives_a_concurrent_agents_sweep() {
        let browsers = tmp_dir("race");
        profile(&browsers, "agent-a", Duration::from_secs(0));
        profile(&browsers, "agent-b", Duration::from_secs(0));

        let limits = || Limits {
            grace: GRACE,
            examine: 64,
            remove: 64,
        };
        let a_sees: HashSet<String> = std::iter::once("agent-b".to_string()).collect();
        let b_sees: HashSet<String> = std::iter::once("agent-a".to_string()).collect();
        assert!(sweep_orphans(&browsers, &a_sees, &limits()).is_empty());
        assert!(sweep_orphans(&browsers, &b_sees, &limits()).is_empty());

        assert!(
            browsers.join("agent-a").is_dir(),
            "agent-a's fresh profile was deleted"
        );
        assert!(
            browsers.join("agent-b").is_dir(),
            "agent-b's fresh profile was deleted"
        );
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

        let limits = Limits {
            grace: Duration::from_mins(1),
            examine: 32,
            remove: 3,
        };
        let removed = sweep_orphans(&browsers, &referenced, &limits);
        assert_eq!(removed.len(), 3, "removal cap ignored: {removed:?}");
        assert_eq!(
            std::fs::read_dir(&browsers).unwrap().count(),
            9,
            "removed a different number than reported"
        );
        std::fs::remove_dir_all(&browsers).ok();
    }

    /// Examination is capped and rotates, so repeated sweeps reach every orphan.
    #[test]
    fn repeated_sweeps_reach_every_orphan() {
        let browsers = tmp_dir("rotate");
        for i in 0..8 {
            profile(&browsers, &format!("orphan-{i}"), week());
        }
        let referenced = HashSet::new();
        for _ in 0..40 {
            let limits = Limits {
                grace: Duration::from_mins(1),
                examine: 2,
                remove: 1,
            };
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
        // No `chromium-profile` inside: no launch created it.
        std::fs::create_dir_all(browsers.join("notes")).unwrap();
        std::fs::write(browsers.join("notes").join("keep.txt"), "mine").unwrap();
        // A name a launch could not have produced.
        std::fs::create_dir_all(browsers.join("has space").join(PROFILE_SUBDIR)).unwrap();

        let removed = sweep_orphans(
            &browsers,
            &HashSet::new(),
            &Limits {
                grace: Duration::from_secs(0),
                examine: 64,
                remove: 64,
            },
        );
        assert!(removed.is_empty(), "removed a non-profile: {removed:?}");
        assert!(browsers.join("notes").join("keep.txt").exists());
        std::fs::remove_dir_all(&browsers).ok();
    }

    /// Every flagless invocation lands on `default`, so it is never swept automatically.
    #[test]
    fn the_implicit_browser_is_exempt() {
        let browsers = tmp_dir("implicit");
        profile(&browsers, IMPLICIT_BROWSER, week());
        let removed = sweep_orphans(
            &browsers,
            &HashSet::new(),
            &Limits {
                grace: Duration::from_mins(1),
                examine: 64,
                remove: 64,
            },
        );
        assert!(
            removed.is_empty(),
            "the default profile was swept: {removed:?}"
        );
        std::fs::remove_dir_all(&browsers).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_lock_from_another_host_is_never_a_verdict() {
        // Same pid shape, different machine: the number means nothing locally.
        assert_eq!(
            singleton_lock_holder("some-other-host-1"),
            Hold::Unknown,
            "another host's lock was read as a local pid"
        );
        assert_eq!(singleton_lock_holder("no-separator"), Hold::Unknown);
        let host = this_host().unwrap();
        assert_eq!(
            singleton_lock_holder(&format!("{host}-notanumber")),
            Hold::Unknown
        );
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
