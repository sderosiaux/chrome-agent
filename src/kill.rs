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
    /// The pid could not be classified, because the reading that classifies it did not
    /// happen. Nothing was signalled and — this is the whole point of the word — nothing
    /// is claimed about the browser: it may be running, and this invocation cannot say.
    ///
    /// It exists because the three words above are all ASSERTIONS about a process, and the
    /// only reading behind two of them was a `ps` whose failure was silently rounded to
    /// `Gone`. See [`process_name`] for the two doors that reach it.
    Unverified,
}

impl KillOutcome {
    /// Whether `close` may forget the session entry after this outcome.
    ///
    /// Three of the four settle what the entry names: the browser was signalled, the pid
    /// holds no process at all, or the pid holds a live process that is somebody else's —
    /// in each case the entry no longer names a running browser of ours, and dropping it
    /// loses nothing. `Unverified` settles none of that, and the entry is the ONLY handle
    /// anything has on that Chrome: `sessions.json` is what `status` lists and what `close`
    /// looks a pid up in. Dropping it there is precisely how a browser becomes an orphan
    /// that neither command can reach — the leak `orphans.rs` exists to clean up after, now
    /// manufactured by the command whose job was to prevent it.
    ///
    /// So the entry stays and `close` says why. The cost is stated rather than hidden: on a
    /// machine whose process table never answers (a `ps`-less image, or any non-Unix
    /// target, where nothing is ever signalled either) `close` stops removing entries at
    /// all. That is the honest shape of a command that cannot close anything there, and an
    /// entry whose process really did exit is dropped by `prune_dead` on the next save
    /// anyway — `liveness` answers that question without `ps`.
    #[must_use]
    pub const fn entry_may_be_dropped(self) -> bool {
        !matches!(self, Self::Unverified)
    }
}

/// The executable behind `pid` per the process table, or `None` when that table would not
/// answer — which is not the same fact and must not be reported as one.
///
/// Two doors reach the `None`, and only the first was ever checked. `ps` may not exist: a
/// distroless or scratch image is exactly the audience a fully static musl binary is built
/// for, and there `output()` fails to spawn. And a `ps` that exists may refuse the question:
/// busybox's applet does not implement `-p <pid> -o comm=`, so it exits non-zero with an
/// empty stdout, which `output()` reports as `Ok`. `orphans::process_table` already gates on
/// `status.success()` for exactly this reason (`orphans.rs:86`); this is the same rule,
/// applied to the other reader of the same table.
///
/// An empty name on a successful read is `None` too: [`kill_pid`] only asks after
/// `liveness` has said the pid is not dead, so a table that answers about a process it holds
/// and names nothing has told us nothing to classify.
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

/// What the two readings imply, as one pure function: `Some(outcome)` is a refusal to
/// signal, `None` is the single path that goes on to signal.
///
/// Pure because the alternative is untestable. The reading it stands in for is a `ps`
/// resolved through `PATH`, and a test cannot take `ps` away from itself — `set_var` is unsafe
/// in edition 2024, and `unsafe` here is `deny` (`Cargo.toml`, `[lints.rust]`) with each
/// exception spelled `#[allow(unsafe_code)]` on the statement itself under a `// SAFETY:` line.
/// There are six: `flock` acquire and release in `session::FileLock`, `kill(pid, 0)` in
/// `session::liveness`, `gethostname` and the `CStr::from_ptr` that reads its buffer in
/// `profiles::this_host`, and `utimensat` in one `#[cfg(test)]` helper. All six are a libc call
/// with no safe equivalent in std, which is the only thing they have in common and the only
/// reason any of them is here. So the door that mattered (a table that will not answer) was
/// never exercised, which is how it stayed wired to `Gone`.
#[cfg(any(unix, test))]
fn classify(existence: crate::session::Liveness, comm: Option<&str>) -> Option<KillOutcome> {
    if existence == crate::session::Liveness::Dead {
        return Some(KillOutcome::Gone);
    }
    match comm {
        // `Alive` or `Unknown`, and no name for it. Both readings failed to produce a fact,
        // and the pid is the only thing this invocation knows.
        None => Some(KillOutcome::Unverified),
        Some(comm) if !is_browser_process(comm) => Some(KillOutcome::NotABrowser),
        Some(_) => None,
    }
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
/// Two questions, asked in that order rather than merged into one reading. "Does this pid
/// exist" is [`crate::session::liveness`]: `kill(pid, 0)`, in-process, no subprocess, and
/// already tri-state, so a pid the OS declines to classify is not rounded to gone. Only
/// once that answer is not `Dead` is "is it a browser" worth asking, and only that second
/// question needs the process table. Merging them is what made a `ps` this tool could not
/// run report `Gone` — a statement about the process — for a pid it had never looked at.
///
/// Returns which of those four happened, so a caller can say so. Callers that kill
/// on their way to relaunching (`run.rs`) or on interrupt (`main.rs`) discard it:
/// they act the same either way.
///
/// One race is accepted and named: a process that exits between the `liveness` probe and
/// the `ps` read makes `ps` exit non-zero, and this answers `Unverified` where `Gone` would
/// have been exact. The window is one fork+exec, and "I did not establish it" is a weaker
/// claim than the truth rather than a contradiction of it — the direction this whole module
/// errs in.
pub fn kill_pid(pid: u32) -> KillOutcome {
    #[cfg(unix)]
    {
        let existence = crate::session::liveness(pid);
        // The process table is read only when the first question did not already settle it,
        // so a dead pid costs no fork and, more to the point, cannot be misclassified by a
        // `ps` that never ran.
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
        // No portable kill is wired here and no portable probe either (`liveness` answers
        // `Unknown` on every pid), so this platform signals nothing and checks nothing.
        // `Gone` and `NotABrowser` are both claims about a process it never looked at —
        // the second one being the lie it used to tell, since `close_message` then said the
        // pid "now belongs to another process". `Unverified` is what actually happened.
        // Consequence, deliberate: `close` here removes no session entry (see
        // `KillOutcome::entry_may_be_dropped`), which is the honest shape of a command that
        // cannot close anything — it used to drop the entry and leave the Chrome running.
        let _ = pid;
        KillOutcome::Unverified
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

/// What `close` says, given what the kill actually did. Only one of the four closed
/// anything; the other three name a pid this tool deliberately did not signal, and the
/// sentence has to be the same statement as the act. Pure, so the wording is testable
/// without spawning a browser.
///
/// Three of them also say `Removed session`, because the entry really does leave the store
/// there — a pid that is gone or reused describes a browser that is already not running.
/// `Unverified` is the one that does not, and its wording carries both halves of why:
/// nothing was signalled, and nothing was established, so the browser may still be running
/// and the entry is kept as the handle on it (`KillOutcome::entry_may_be_dropped`).
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

        for refused in [KillOutcome::NotABrowser, KillOutcome::Gone, KillOutcome::Unverified] {
            let message = close_message("s9", 80548, refused);
            assert!(
                !message.contains("Closed"),
                "a kill that never happened must not be worded as one: {message}"
            );
            assert!(message.contains("80548"), "the pid left alone is the fact: {message}");
        }
    }

    /// The reading that classifies a pid can fail, and its failure used to be spelled
    /// `Gone` — "pid was no longer running", a statement about the machine made by a
    /// process that had just failed to look at it.
    ///
    /// Two doors, both reachable on the platform this binary is built to run everywhere on:
    /// a distroless image has no `ps` at all (spawn fails), and a busybox `ps` refuses
    /// `-p <pid> -o comm=` (non-zero exit, empty stdout — an `Ok` from `output()`, which is
    /// why only the first door was ever checked). `process_name` collapses both to `None`;
    /// what this pins is that `None` no longer means dead.
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

        // The word has to stay true all the way to what a person reads and to what the
        // store does. Neither may claim the browser is gone.
        let message = close_message("s9", 80548, KillOutcome::Unverified);
        assert!(!message.contains("no longer running"), "{message}");
        assert!(!message.contains("another process"), "{message}");
        assert!(message.contains("may still be running"), "{message}");
        assert!(
            !KillOutcome::Unverified.entry_may_be_dropped(),
            "dropping the entry is how the browser it names becomes unreachable"
        );

        // And the other three are unchanged: each of them did establish something.
        assert_eq!(classify(Liveness::Dead, None), Some(KillOutcome::Gone));
        assert_eq!(classify(Liveness::Alive, Some("sleep")), Some(KillOutcome::NotABrowser));
        assert_eq!(classify(Liveness::Alive, Some("Google Chrome")), None, "this one signals");
        for settled in [KillOutcome::Signalled, KillOutcome::Gone, KillOutcome::NotABrowser] {
            assert!(settled.entry_may_be_dropped(), "{settled:?} settles what the entry names");
        }
    }

    /// `liveness` precedes `ps` rather than replacing it: a dead pid is answered without
    /// spawning anything, so the classification of a gone browser no longer depends on a
    /// process table being present. `Gone` stays reachable, and now for a measured reason.
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
