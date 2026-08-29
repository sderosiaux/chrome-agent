//! Signalling a pid, and saying truthfully what that did.
//!
//! Split from `run_helpers.rs` for the 1000-line cap, re-exported via `pub use`.
//!
//! One rule holds this module together: the guard that decides whether to signal and
//! the sentence a user reads must be the same statement about the same event. They were
//! not — `kill_pid` returned `()`, so `close` printed `Closed browser=…` whether the
//! signal went out, the pid was gone, or the pid had been reused by an unrelated process
//! and was deliberately left alone.

/// Browsers this invocation spawned and nothing has persisted yet.
///
/// Between `cmd.spawn()` in `browser.rs` and the `save_session` in `run.rs` there is a
/// live Chrome whose pid exists only in this process's memory: `Child`'s drop does not
/// kill it, `sessions.json` does not name it, and the Ctrl+C handler — which reads the
/// store — finds nothing to stop. Everything in that window leaks a browser that no
/// later command can reach: the two `?` after the launch (`CdpClient::connect`,
/// `resolve_page_target`) and any signal. Reproduced by interrupting a `goto` within
/// ~0.3 s of a cold start: no session file at all, Chrome still running. It is where
/// the 19-day-old `test-tabs` and `test-integration` came from — two test-shaped names,
/// which is what an interrupted test run leaves.
///
/// A pid is armed at spawn and disarmed by `session::save_session`, so what disarms it
/// is the write that makes it reachable — not a call some future path could forget.
static UNPERSISTED: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

/// Record a browser this invocation just spawned. Call immediately after the spawn:
/// the gap this closes is measured in milliseconds and starts there.
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

/// Kill every browser this invocation spawned and never managed to persist.
///
/// Called on the two exits that would otherwise leak: the interrupt handler and the
/// error path out of `run`. A browser already in the store is NOT reaped here — it is
/// reachable, and a failed `goto` leaving a usable browser behind is the existing
/// contract, not a leak.
pub fn reap_unpersisted() {
    let armed: Vec<u32> = UNPERSISTED.lock().map(|mut a| std::mem::take(&mut *a)).unwrap_or_default();
    for pid in armed {
        let _ = kill_pid(pid);
    }
}

/// Whether `comm` (the executable per `ps -o comm=`) is a browser this tool could have
/// launched. The kill below is gated on it — see `kill_pid`.
///
/// A plain substring match on "chrome" is not enough: this tool's own binary is named
/// `chrome-agent`, and `chromedriver` exists too. Under the exact PID-reuse race the
/// guard is for, a reused pid landing on a sibling chrome-agent process would have been
/// classified as a browser and killed — the scenario the guard claims to prevent.
#[cfg(any(unix, test))]
fn is_browser_process(comm: &str) -> bool {
    let base = comm.rsplit('/').next().unwrap_or(comm).to_ascii_lowercase();
    if base.contains("chrome-agent") || base.contains("chromedriver") {
        return false;
    }
    base.contains("chrome") || base.contains("chromium") || base.contains("headless_shell")
}

/// What [`kill_pid`] did. The guard below declines to signal more often than it
/// signals, and a caller that reports "closed" either way describes an outcome it
/// never had: the pid-reuse refusal reached a user as `Closed browser=s9 (pid=80548)`
/// over a pid that by then belonged to `git fsmonitor--daemon` and was — correctly —
/// left alone. The message and the act were two different statements about one event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KillOutcome {
    /// The pid named a browser and the signal was sent.
    Signalled,
    /// The pid holds a live process that is not a browser: a reused number, left alone.
    NotABrowser,
    /// The pid holds no process. Nothing to signal, nothing lost.
    Gone,
}

/// Kill a managed-browser process (best-effort, unix only). Killing the
/// main Chrome process is enough — its helper processes exit with it.
///
/// Guarded against PID reuse: a stored pid may have died and been reassigned by the
/// OS to an unrelated process, and signalling whatever holds the number now is data
/// loss, not cleanup. The executable is checked first; a pid that is gone, or that
/// no longer names a browser, is left alone. The check-then-kill window is
/// milliseconds — not zero, but no longer unbounded.
///
/// Returns which of those three happened, so a caller can say so. Callers that kill
/// on their way to relaunching (`run.rs`) or on interrupt (`main.rs`) discard it:
/// they act the same either way.
pub fn kill_pid(pid: u32) -> KillOutcome {
    #[cfg(unix)]
    {
        let comm = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        let Some(comm) = comm.filter(|c| !c.is_empty()) else {
            return KillOutcome::Gone;
        };
        if !is_browser_process(&comm) {
            return KillOutcome::NotABrowser;
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
        // No portable kill wired here, so nothing is ever signalled — and saying
        // `Gone` would be a claim about the process this platform never checked.
        let _ = pid;
        KillOutcome::NotABrowser
    }
}

/// Wait until `pid` no longer names a process, bounded.
///
/// A SIGTERM returns before Chrome exits, and the gap is not theoretical: a relaunch
/// inside it finds the old `DevToolsActivePort`, whose HTTP endpoint still answers
/// mid-teardown, and the WebSocket handshake then fails with a transport error —
/// `close` immediately followed by `goto` on the same name failed reliably on this
/// machine and passed with a 2 s sleep between them. Waiting here makes "Closed"
/// mean closed. `false` after the deadline is reported, not papered over.
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

/// What `close` says, given what the kill actually did. The entry leaves the store in
/// all three cases — a pid that is gone or reused describes a browser that is already
/// not running — but only one of them closed anything, and the other two name a pid
/// this tool deliberately did not signal. Pure, so the wording is testable without
/// spawning a browser.
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
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unpersisted_spawn_is_reaped_and_a_persisted_one_is_left_alone() {
        // The window: armed at spawn, disarmed by the save. Whatever is still armed when
        // this invocation gives up is a browser nothing else can name.
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

        // Re-armed and reaped: the process is a `sleep`, so `kill_pid`'s guard declines
        // it — what this asserts is that the list is drained, not that a bystander dies.
        arm(leaked.id());
        reap_unpersisted();
        assert!(UNPERSISTED.lock().unwrap().is_empty(), "reap must drain what it took");
        let _ = leaked.kill();
        let _ = leaked.wait();
    }

    #[test]
    fn a_refused_kill_is_not_reported_as_a_close() {
        // The refusal below is correct and was invisible: `close` printed
        // `Closed browser=s9 (pid=80548)` over a pid that by then belonged to
        // `git fsmonitor--daemon`, which it had — correctly — left alone. A user
        // reading that line has no way to tell a closed browser from a forgotten one.
        let signalled = close_message("s9", 80548, KillOutcome::Signalled);
        assert!(signalled.starts_with("Closed browser=s9"), "{signalled}");

        for refused in [KillOutcome::NotABrowser, KillOutcome::Gone] {
            let message = close_message("s9", 80548, refused);
            assert!(
                !message.contains("Closed"),
                "a kill that never happened must not be worded as one: {message}"
            );
            assert!(message.contains("80548"), "the pid left alone is the fact: {message}");
        }
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
        // A stored pid can be reaped and reassigned by the OS to an unrelated
        // process. Killing whatever holds the number now is data loss, not cleanup.
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
            // The guard's own binary contains "chrome": under the PID-reuse race it
            // protects against, a sibling chrome-agent must not be classified as prey.
            "chrome-agent",
            "/tmp/chrome-agent",
            "chromedriver",
        ] {
            assert!(!is_browser_process(bystander), "must not kill {bystander}");
        }
    }
}
