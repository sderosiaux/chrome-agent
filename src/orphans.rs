//! Managed Chromes that no session entry claims.
//!
//! A browser leaves `sessions.json` for reasons unrelated to whether it is running (`close`
//! removes the entry whether or not the kill landed; the relaunch path removes it before
//! spawning a replacement; the dead-pid prune drops it), leaving a Chrome invisible to
//! `status` and unreachable by `close`. Recognition is therefore by the `--user-data-dir` a
//! process was launched with, which stays true when the registry is wrong. This is the
//! process half; `profiles.rs` is the disk half and never signals a process.

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
/// Chrome passes `--user-data-dir` to its helper processes too, so matching the flag alone
/// counts a renderer per tab as a browser (measured: 39 processes for 5 browsers). Helpers
/// are exactly the processes carrying `--type=`.
#[must_use]
pub fn session_of(command: &str, browsers_dir: &Path) -> Option<String> {
    if command.contains("--type=") {
        return None;
    }
    let prefix = format!("--user-data-dir={}/", browsers_dir.display());
    let rest = command.split(&prefix).nth(1)?;
    // `<browsers_dir>/<name>/chromium-profile`; the name contains no separator.
    let name = rest.split('/').next()?;
    // More arguments may follow the flag: whitespace here means the path stopped at
    // `browsers_dir` and names no session.
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

/// Every managed Chrome in `ps` output whose pid is not one the registry holds. Matched by
/// pid, not name: a relaunched session keeps its name while the previous process, a
/// different pid under that same name, is exactly the leak this looks for.
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

/// Read the process table. `None` means it could not be read, which is not "no orphans".
///
/// `ps` at an absolute path, never through `PATH` — same rule and same list as `kill.rs`, which
/// states why. A system with `ps` somewhere else reads as "could not be read", and
/// `cmd_close_orphans` already refuses rather than reporting "0 closed".
#[cfg(unix)]
fn process_table() -> Option<String> {
    let ps = crate::kill::first_existing(crate::kill::PS_PATHS)?;
    let out = std::process::Command::new(ps)
        .args(["-eo", "pid=,command="])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
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
/// Signals through `kill_pid`, so the pid-reuse guard still applies even though the pid came
/// from the process table microseconds earlier. Leaves profile directories alone; deleting
/// one out from under a Chrome that is still shutting down is what `purge_profile` needs
/// eight retries to survive. `close --purge-orphans` is the disk sweep.
pub fn cmd_close_orphans(json_mode: bool) -> Result<(), crate::BoxError> {
    let store = crate::session::load_session()?;
    let Some(orphans) = scan(&store) else {
        // An unreadable process table makes every browser look like no browser, and
        // "0 closed" would read as "nothing was left".
        return Err("Could not read the process table, so no orphan could be identified.".into());
    };

    // The outcome is kept rather than reduced to a boolean: the three ways of not signalling
    // are three different facts, and the warnings below name which.
    use crate::run_helpers::KillOutcome;
    let attempted: Vec<(&Orphan, KillOutcome)> = orphans
        .iter()
        .map(|o| (o, crate::run_helpers::kill_pid(o.pid)))
        .collect();
    let (closed, skipped): (Vec<_>, Vec<_>) = attempted
        .iter()
        .partition(|(_, outcome)| *outcome == KillOutcome::Signalled);

    let message = format!("Closed {} orphaned browser(s)", closed.len());
    if json_mode {
        let listed = |group: &[&(&Orphan, KillOutcome)]| {
            group
                .iter()
                .map(|(o, _)| serde_json::json!({"name": o.name, "pid": o.pid}))
                .collect::<Vec<_>>()
        };
        crate::run_helpers::json_output(&serde_json::json!({
            "ok": true,
            "message": message,
            "closed": listed(&closed),
            "skipped": listed(&skipped),
        }));
    } else {
        for (orphan, _) in &closed {
            out_line!("Closed orphan={}  pid={}", orphan.name, orphan.pid);
        }
        for (orphan, outcome) in &skipped {
            let reason = match outcome {
                KillOutcome::Gone => "it had already exited",
                KillOutcome::NotABrowser => "its pid now belongs to another process",
                KillOutcome::Unverified => {
                    "its pid could not be checked against the process table, or could not be signalled"
                }
                // Unreachable: this half is everything that is not `Signalled`.
                KillOutcome::Signalled => "it was signalled",
            };
            eprintln!(
                "warning: orphan={} pid={} was not signalled: {reason}",
                orphan.name, orphan.pid
            );
        }
        out_line!("{message}");
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
        // Chrome hands --user-data-dir to every helper: counting them reported 39 browsers
        // where 5 were running, and would have killed a live browser's renderers.
        let cmd = "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Helper (Renderer).app/Contents/MacOS/Google Chrome Helper (Renderer) --type=renderer --user-data-dir=/Users/x/.chrome-agent/browsers/test-tabs/chromium-profile";
        assert_eq!(session_of(cmd, &dir()), None);
    }

    #[test]
    fn a_chrome_outside_our_profile_root_is_not_ours() {
        // The user's own Chrome and another tool's headless one run the same executable.
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
            vec![Orphan {
                pid: 16504,
                name: "test-tabs".into()
            }]
        );
    }

    #[test]
    fn a_relaunch_leaves_the_previous_pid_orphaned_under_a_claimed_name() {
        // The registry still knows the name; only the pid stopped being claimed.
        let ps = " 16504 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --user-data-dir=/Users/x/.chrome-agent/browsers/hcvar/chromium-profile\n";
        let claimed = HashSet::from([42289]);
        assert_eq!(
            from_ps(ps, &dir(), &claimed),
            vec![Orphan {
                pid: 16504,
                name: "hcvar".into()
            }]
        );
    }

    #[test]
    fn a_ps_line_without_a_numeric_pid_is_skipped() {
        assert_eq!(from_ps("  PID COMMAND\n", &dir(), &HashSet::new()), vec![]);
    }
}
