//! Signalling a pid, and saying truthfully what that did.
//!
//! One rule: the guard that decides whether to signal and the sentence a user reads must be
//! the same statement about the same event. The guard declines more often than it signals,
//! so every outcome is carried, never reduced to a boolean.

/// Browsers this invocation spawned that nothing has persisted yet.
///
/// Between `cmd.spawn()` in `browser.rs` and `save_session` in `run.rs` a live Chrome's pid
/// exists only in memory: `Child`'s drop does not kill it and the Ctrl+C handler reads the
/// store, which does not name it. Any exit in that window (the `?` on `CdpClient::connect`
/// and `resolve_page_target`, or a signal) leaks a browser no later command can reach.
///
/// Armed at spawn, disarmed by `session::save_session`, so the write that makes a pid
/// reachable is what ends the window.
static UNPERSISTED: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

/// Record a browser this invocation just spawned. Call immediately after the spawn.
pub fn arm(pid: u32) {
    if let Ok(mut armed) = UNPERSISTED.lock() {
        armed.push(pid);
    }
}

/// Forget a pid: something durable now names it, so the store-based paths own it.
pub fn disarm(pid: u32) {
    if let Ok(mut armed) = UNPERSISTED.lock() {
        armed.retain(|&p| p != pid);
    }
}

/// Kill every browser this invocation spawned and never persisted. Called on the two exits
/// that would otherwise leak: the interrupt handler and the error path out of `run`. A
/// browser already in the store is NOT reaped — a failed `goto` leaving a usable browser
/// behind is the existing contract.
pub fn reap_unpersisted() {
    let armed: Vec<u32> = UNPERSISTED.lock().map(|mut a| std::mem::take(&mut *a)).unwrap_or_default();
    for pid in armed {
        let _ = kill_pid(pid);
    }
}

/// Whether `comm` (the executable per `ps -o comm=`) is a browser this tool could have
/// launched. Gates the kill in [`kill_pid`]. A plain substring match on "chrome" would
/// classify this tool's own `chrome-agent` binary, and `chromedriver`, as prey.
#[cfg(any(unix, test))]
fn is_browser_process(comm: &str) -> bool {
    let base = comm.rsplit('/').next().unwrap_or(comm).to_ascii_lowercase();
    if base.contains("chrome-agent") || base.contains("chromedriver") {
        return false;
    }
    base.contains("chrome") || base.contains("chromium") || base.contains("headless_shell")
}

/// What [`kill_pid`] did. Four distinct outcomes, because a caller that reports "closed"
/// either way describes an outcome it never had.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KillOutcome {
    /// The pid named a browser and the signal was sent.
    Signalled,
    /// The pid holds a live process that is not a browser: a reused number, left alone.
    NotABrowser,
    /// The pid holds no process. Nothing to signal, nothing lost.
    Gone,
    /// The reading that classifies the pid did not happen. Nothing was signalled and
    /// nothing is claimed: the browser may be running. The other three are assertions
    /// about a process; this one is the absence of one. See [`process_name`].
    Unverified,
}

impl KillOutcome {
    /// Whether `close` may forget the session entry after this outcome.
    ///
    /// Three of the four settle that the entry no longer names a running browser of ours, so
    /// dropping it loses nothing. `Unverified` settles nothing, and the entry is the only
    /// handle on that Chrome — dropping it is how a browser becomes an orphan.
    ///
    /// Cost, stated: where the process table never answers (a `ps`-less image, or any
    /// non-Unix target) `close` removes no entries at all. `prune_dead` still drops an entry
    /// whose process really exited, since `liveness` needs no `ps`.
    #[must_use]
    pub const fn entry_may_be_dropped(self) -> bool {
        !matches!(self, Self::Unverified)
    }
}

/// The executable behind `pid` per the process table, or `None` when the table would not
/// answer — a different fact from "the pid is gone".
///
/// Three doors reach `None`: `ps` may not exist (a distroless image, the audience a static
/// musl binary is built for); busybox's `ps` does not implement `-p <pid> -o comm=` and
/// exits non-zero with empty stdout, which `output()` still reports as `Ok`, hence the
/// `status.success()` gate; and an empty name on a successful read classifies nothing.
#[cfg(unix)]
fn process_name(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let comm = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!comm.is_empty()).then_some(comm)
}

/// What the two readings imply: `Some(outcome)` refuses to signal, `None` is the one path
/// that goes on to signal. Pure so it is testable — a test cannot take `ps` away from itself
/// (`set_var` is unsafe in edition 2024, and `unsafe_code` is `deny` here).
#[cfg(any(unix, test))]
fn classify(existence: crate::session::Liveness, comm: Option<&str>) -> Option<KillOutcome> {
    if existence == crate::session::Liveness::Dead {
        return Some(KillOutcome::Gone);
    }
    match comm {
        // `Alive` or `Unknown` with no name: neither reading produced a fact.
        None => Some(KillOutcome::Unverified),
        Some(comm) if !is_browser_process(comm) => Some(KillOutcome::NotABrowser),
        Some(_) => None,
    }
}

/// Kill a managed-browser process (best-effort, unix only). Killing the main Chrome process
/// is enough; its helpers exit with it.
///
/// Guarded against PID reuse: a stored pid may have been reassigned, and signalling whatever
/// holds the number now is data loss. Two questions, in this order and never merged. Does
/// the pid exist — [`crate::session::liveness`], in-process and tri-state. Only if that is
/// not `Dead`, is it a browser — the only question needing the process table. Merging them
/// made a `ps` this tool could not run report `Gone` for a pid it never looked at. The
/// check-then-kill window is milliseconds, not zero.
///
/// Callers that kill on their way to relaunching (`run.rs`) or on interrupt (`main.rs`)
/// discard the outcome; they act the same either way.
///
/// Accepted race: a process exiting between the `liveness` probe and the `ps` read answers
/// `Unverified` where `Gone` would have been exact. A weaker claim, not a wrong one.
pub fn kill_pid(pid: u32) -> KillOutcome {
    #[cfg(unix)]
    {
        let existence = crate::session::liveness(pid);
        // Read the table only if the first question did not settle it: a dead pid then costs
        // no fork and cannot be misclassified by a `ps` that never ran.
        let comm = (existence != crate::session::Liveness::Dead)
            .then(|| process_name(pid))
            .flatten();
        if let Some(refusal) = classify(existence, comm.as_deref()) {
            return refusal;
        }
        let _ = std::process::Command::new("kill")
            .arg(pid.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        KillOutcome::Signalled
    }
    #[cfg(not(unix))]
    {
        // No portable kill and no portable probe (`liveness` answers `Unknown` on every
        // pid), so this platform signals nothing and checks nothing. `Gone` and
        // `NotABrowser` would both be claims about a process never looked at. Deliberate
        // consequence: `close` removes no session entry here (`entry_may_be_dropped`).
        let _ = pid;
        KillOutcome::Unverified
    }
}

/// Wait until `pid` no longer names a process, bounded by `timeout`.
///
/// SIGTERM returns before Chrome exits, and a relaunch inside that gap finds the old
/// `DevToolsActivePort`, whose HTTP endpoint still answers mid-teardown, so the WebSocket
/// handshake fails. `false` after the deadline is reported, not papered over.
#[must_use]
pub fn wait_until_gone(pid: u32, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if crate::session::liveness(pid) == crate::session::Liveness::Dead {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

/// What `close` says, given what the kill actually did. Pure, so the wording is testable
/// without spawning a browser. Only one of the four closed anything; the other three name a
/// pid deliberately not signalled. Three say `Removed session` because the entry really does
/// leave the store; `Unverified` does not, since the entry is kept as the only handle on a
/// browser that may still be running (`KillOutcome::entry_may_be_dropped`).
#[must_use]
pub fn close_message(browser_name: &str, pid: u32, outcome: KillOutcome) -> String {
    match outcome {
        KillOutcome::Signalled => format!("Closed browser={browser_name} (pid={pid})"),
        KillOutcome::Gone => {
            format!("Removed session={browser_name} (pid={pid} was no longer running)")
        }
        KillOutcome::NotABrowser => format!(
            "Removed session={browser_name} (pid={pid} now belongs to another process and was left alone)"
        ),
        KillOutcome::Unverified => format!(
            "Kept session={browser_name} (pid={pid} could not be checked against the process table, \
             so nothing was signalled and the browser may still be running)"
        ),
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unpersisted_spawn_is_reaped_and_a_persisted_one_is_left_alone() {
        // Armed at spawn, disarmed by the save. Whatever is still armed when this
        // invocation gives up is a browser nothing else can name.
        let mut leaked = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("stand-in for a spawned browser");
        arm(leaked.id());
        assert_eq!(UNPERSISTED.lock().unwrap().as_slice(), &[leaked.id()]);

        disarm(leaked.id());
        assert!(
            UNPERSISTED.lock().unwrap().is_empty(),
            "a saved pid is the store's to reap, not this invocation's"
        );

        // The stand-in is a `sleep`, so `kill_pid`'s guard declines it. What is asserted is
        // that the list drains, not that a bystander dies.
        arm(leaked.id());
        reap_unpersisted();
        assert!(UNPERSISTED.lock().unwrap().is_empty(), "reap must drain what it took");
        let _ = leaked.kill();
        let _ = leaked.wait();
    }

    #[test]
    fn a_refused_kill_is_not_reported_as_a_close() {
        // A reader of `Closed browser=…` must be able to tell a closed browser from a
        // forgotten one; the three refusals below closed nothing.
        let signalled = close_message("s9", 80548, KillOutcome::Signalled);
        assert!(signalled.starts_with("Closed browser=s9"), "{signalled}");

        for refused in [KillOutcome::NotABrowser, KillOutcome::Gone, KillOutcome::Unverified] {
            let message = close_message("s9", 80548, refused);
            assert!(
                !message.contains("Closed"),
                "a kill that never happened must not be worded as one: {message}"
            );
            assert!(message.contains("80548"), "the pid left alone is the fact: {message}");
        }
    }

    /// The reading that classifies a pid can fail — a distroless image has no `ps`, and a
    /// busybox `ps` refuses `-p <pid> -o comm=` with a non-zero exit and empty stdout.
    /// `process_name` collapses both to `None`; this pins that `None` does not mean dead.
    #[test]
    fn a_process_table_that_will_not_answer_is_not_a_dead_process() {
        use crate::session::Liveness;

        for existence in [Liveness::Alive, Liveness::Unknown] {
            assert_eq!(
                classify(existence, None),
                Some(KillOutcome::Unverified),
                "a reading that did not happen is not a fact about the process ({existence:?})"
            );
        }

        // Neither the message nor the store may claim the browser is gone.
        let message = close_message("s9", 80548, KillOutcome::Unverified);
        assert!(!message.contains("no longer running"), "{message}");
        assert!(!message.contains("another process"), "{message}");
        assert!(message.contains("may still be running"), "{message}");
        assert!(
            !KillOutcome::Unverified.entry_may_be_dropped(),
            "dropping the entry is how the browser it names becomes unreachable"
        );

        // The other three each did establish something.
        assert_eq!(classify(Liveness::Dead, None), Some(KillOutcome::Gone));
        assert_eq!(classify(Liveness::Alive, Some("sleep")), Some(KillOutcome::NotABrowser));
        assert_eq!(classify(Liveness::Alive, Some("Google Chrome")), None, "this one signals");
        for settled in [KillOutcome::Signalled, KillOutcome::Gone, KillOutcome::NotABrowser] {
            assert!(settled.entry_may_be_dropped(), "{settled:?} settles what the entry names");
        }
    }

    /// `liveness` precedes `ps`: a dead pid is answered without spawning anything, so
    /// classifying a gone browser does not depend on a process table being present.
    #[test]
    fn a_pid_that_is_gone_is_answered_without_the_process_table() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn a pid we can then make dead");
        let pid = child.id();
        child.kill().expect("signal the stand-in");
        child.wait().expect("reap it, so the pid is genuinely free");

        assert_eq!(
            crate::session::liveness(pid),
            crate::session::Liveness::Dead,
            "the fixture did not produce a dead pid"
        );
        assert_eq!(kill_pid(pid), KillOutcome::Gone);
    }

    #[test]
    fn kill_pid_reports_the_pid_it_refused_instead_of_staying_silent() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn a stand-in for the reused pid");
        assert_eq!(kill_pid(child.id()), KillOutcome::NotABrowser);
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn kill_pid_refuses_a_pid_that_no_longer_belongs_to_a_browser() {
        // A stored pid can be reassigned to an unrelated process; killing whatever holds
        // the number now is data loss, not cleanup.
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn a stand-in for the reused pid");
        let _ = kill_pid(child.id());
        std::thread::sleep(std::time::Duration::from_millis(300));
        let status = child.try_wait().expect("poll the stand-in");
        let survived = status.is_none();
        let _ = child.kill();
        let _ = child.wait();
        assert!(survived, "kill_pid killed an unrelated process holding a reused pid");
    }

    #[test]
    fn browser_executables_are_recognised_and_bystanders_are_not() {
        for browser in [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "chrome",
            "chromium",
            "chromium-browser",
            "headless_shell",
            "Google Chrome for Testing",
        ] {
            assert!(is_browser_process(browser), "should recognise {browser}");
        }
        for bystander in [
            "sleep",
            "postgres",
            "/usr/bin/python3",
            "node",
            // This tool's own binary contains "chrome": a sibling chrome-agent must not be
            // classified as prey under the PID-reuse race the guard protects against.
            "chrome-agent",
            "/tmp/chrome-agent",
            "chromedriver",
        ] {
            assert!(!is_browser_process(bystander), "must not kill {bystander}");
        }
    }
}
