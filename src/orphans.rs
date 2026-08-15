//! Managed Chromes that no session entry claims.
//!
//! `sessions.json` is the only list this tool consults, and a browser leaves that list
//! for reasons that have nothing to do with whether it is running: `close` removes the
//! entry whether or not the kill landed, the relaunch path removes it before spawning a
//! replacement, `prune`/`cleanup_stale` drop entries whose pid reads as dead, and the
//! file itself can be rewritten by a newer version. Every one of those leaves a Chrome
//! holding a profile directory with nothing left pointing at it — invisible to `status`,
//! unreachable by `close`, and alive until the machine reboots. Two were measured at 19
//! days on a developer machine, next to five the registry still knew about.
//!
//! The profile sweep behind `close --purge-orphans` (`profiles.rs`) is the disk half of
//! the same idea and deliberately stays that way: it deletes directories, never signals
//! a process. This module is the process half, and it recognises its own browsers the
//! only way that survives the registry being wrong — by the `--user-data-dir` they were
//! launched with, which points inside this tool's own `browsers/` directory.

use std::collections::HashSet;
use std::path::Path;

/// A running Chrome under this tool's profile root that no session entry claims.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Orphan {
    pub pid: u32,
    /// The `--browser` name it was launched under, read back from its profile path.
    pub name: String,
}

/// The session name a command line was launched under, if it is one of ours.
///
/// Chrome passes `--user-data-dir` to its own helper processes too, so a match on the
/// flag alone reports a renderer per tab as a separate browser — 39 processes for the 5
/// browsers actually running, in the case that motivated this. Helpers are exactly the
/// processes carrying `--type=`; the browser process is the one without it.
#[must_use]
pub fn session_of(command: &str, browsers_dir: &Path) -> Option<String> {
    if command.contains("--type=") {
        return None;
    }
    let prefix = format!("--user-data-dir={}/", browsers_dir.display());
    let rest = command.split(&prefix).nth(1)?;
    // `<browsers_dir>/<name>/chromium-profile`. The name cannot contain a separator, and
    // a path that stops at the directory itself names no session.
    let name = rest.split('/').next()?;
    // The flag may be followed by more arguments; a value ending at whitespace means the
    // path stopped at `browsers_dir` and there is no session component to read.
    if name.is_empty() || name.split_whitespace().count() != 1 || name.contains(char::is_whitespace)
    {
        return None;
    }
    Some(name.to_string())
}

/// Split a `ps -eo pid=,command=` line into its pid and the command line.
#[must_use]
fn parse_ps_line(line: &str) -> Option<(u32, &str)> {
    let line = line.trim_start();
    let (pid, rest) = line.split_once(char::is_whitespace)?;
    Some((pid.parse().ok()?, rest.trim_start()))
}

/// Every managed Chrome in `ps` output whose pid is not one the registry holds.
///
/// Matched by pid rather than by name: a session that was relaunched keeps its name in
/// the registry while the previous process — a different pid under the same name — is
/// exactly the leak this looks for.
#[must_use]
pub fn from_ps(ps_output: &str, browsers_dir: &Path, claimed_pids: &HashSet<u32>) -> Vec<Orphan> {
    let mut found: Vec<Orphan> = ps_output
        .lines()
        .filter_map(parse_ps_line)
        .filter(|(pid, _)| !claimed_pids.contains(pid))
        .filter_map(|(pid, command)| {
            session_of(command, browsers_dir).map(|name| Orphan { pid, name })
        })
        .collect();
    found.sort_by_key(|o| o.pid);
    found
}

/// Read the process table. `None` when it could not be read at all, which is not the
/// same answer as "no orphans" and must not be printed as one.
#[cfg(unix)]
fn process_table() -> Option<String> {
    let out = std::process::Command::new("ps").args(["-eo", "pid=,command="]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(not(unix))]
fn process_table() -> Option<String> {
    None
}

/// Managed Chromes running right now that `store` does not account for.
pub fn scan(store: &crate::session::SessionStore) -> Option<Vec<Orphan>> {
    let browsers_dir = crate::session::browsers_dir().ok()?;
    let claimed: HashSet<u32> = store.browsers.values().filter_map(|b| b.pid).collect();
    Some(from_ps(&process_table()?, &browsers_dir, &claimed))
}

/// Close every managed Chrome no session entry claims.
///
/// Signals through `kill_pid`, so the pid-reuse guard applies here too even though the
/// pid came from the process table microseconds earlier — the window is small, not zero,
/// and this command exists precisely because pids outlive what claimed them.
///
/// Leaves the profile directories alone: `close --purge-orphans` is the disk sweep, and
/// deleting a profile out from under a Chrome that is still shutting down is what
/// `purge_profile` had to grow eight retries to survive.
pub fn cmd_close_orphans(json_mode: bool) -> Result<(), crate::BoxError> {
    let store = crate::session::load_session()?;
    let Some(orphans) = scan(&store) else {
        // Refusing beats guessing: an unreadable process table means every browser looks
        // like no browser, and reporting "0 closed" would read as "nothing was left".
        return Err("Could not read the process table, so no orphan could be identified.".into());
    };

    let (closed, skipped): (Vec<&Orphan>, Vec<&Orphan>) = orphans
        .iter()
        .partition(|o| crate::run_helpers::kill_pid(o.pid) == crate::run_helpers::KillOutcome::Signalled);

    let message = format!("Closed {} orphaned browser(s)", closed.len());
    if json_mode {
        let listed = |group: &[&Orphan]| {
            group
                .iter()
                .map(|o| serde_json::json!({"name": o.name, "pid": o.pid}))
                .collect::<Vec<_>>()
        };
        crate::run_helpers::json_output(&serde_json::json!({
            "ok": true,
            "message": message,
            "closed": listed(&closed),
            "skipped": listed(&skipped),
        }));
    } else {
        for orphan in &closed {
            println!("Closed orphan={}  pid={}", orphan.name, orphan.pid);
        }
        for orphan in &skipped {
            eprintln!(
                "warning: orphan={} pid={} stopped being a browser before it could be signalled",
                orphan.name, orphan.pid
            );
        }
        println!("{message}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dir() -> PathBuf {
        PathBuf::from("/Users/x/.chrome-agent/browsers")
    }

    #[test]
    fn the_browser_process_names_its_session() {
        let cmd = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome --user-data-dir=/Users/x/.chrome-agent/browsers/test-tabs/chromium-profile --remote-debugging-port=0";
        assert_eq!(session_of(cmd, &dir()).as_deref(), Some("test-tabs"));
    }

    #[test]
    fn helper_processes_are_not_separate_browsers() {
        // Chrome hands --user-data-dir to every helper. Counting them reported 39
        // browsers where 5 were running, and would have killed a live browser's
        // renderers one at a time.
        let cmd = "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Helper (Renderer).app/Contents/MacOS/Google Chrome Helper (Renderer) --type=renderer --user-data-dir=/Users/x/.chrome-agent/browsers/test-tabs/chromium-profile";
        assert_eq!(session_of(cmd, &dir()), None);
    }

    #[test]
    fn a_chrome_outside_our_profile_root_is_not_ours() {
        // The user's own Chrome, and any other tool's headless one. Neither is this
        // tool's to close, and both run the same executable.
        for cmd in [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome --user-data-dir=/Users/x/Library/Application Support/Google/Chrome",
            "/Users/x/.agent-browser/browsers/chrome-147/Google Chrome for Testing --remote-debugging-port=0",
        ] {
            assert_eq!(session_of(cmd, &dir()), None, "{cmd}");
        }
    }

    #[test]
    fn a_path_that_stops_at_the_profile_root_names_no_session() {
        let cmd = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome --user-data-dir=/Users/x/.chrome-agent/browsers/ --headless";
        assert_eq!(session_of(cmd, &dir()), None);
    }

    #[test]
    fn a_pid_the_registry_holds_is_not_an_orphan() {
        let ps = "\
 16504 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --user-data-dir=/Users/x/.chrome-agent/browsers/test-tabs/chromium-profile
 42289 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --user-data-dir=/Users/x/.chrome-agent/browsers/hcvar/chromium-profile
 88717 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome
";
        let claimed = HashSet::from([42289]);
        assert_eq!(
            from_ps(ps, &dir(), &claimed),
            vec![Orphan { pid: 16504, name: "test-tabs".into() }]
        );
    }

    #[test]
    fn a_relaunch_leaves_the_previous_pid_orphaned_under_a_claimed_name() {
        // The registry still knows the name — it is the pid that stopped being claimed,
        // which is why the match is on pid and not on name.
        let ps = " 16504 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --user-data-dir=/Users/x/.chrome-agent/browsers/hcvar/chromium-profile\n";
        let claimed = HashSet::from([42289]);
        assert_eq!(
            from_ps(ps, &dir(), &claimed),
            vec![Orphan { pid: 16504, name: "hcvar".into() }]
        );
    }

    #[test]
    fn a_ps_line_without_a_numeric_pid_is_skipped() {
        assert_eq!(from_ps("  PID COMMAND\n", &dir(), &HashSet::new()), vec![]);
    }
}
