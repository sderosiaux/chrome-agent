//! The download a CLICK produces, rather than one the caller already has a URL for.
//!
//! `download <url>` fetches inside the page, which is the right mechanism when the file has an
//! address: the request inherits the session's cookies and the bytes come back through the
//! evaluation. It cannot reach the other half of the web, where the file is built client-side
//! (`URL.createObjectURL`) or handed out by a POST-backed endpoint the anchor never names. There
//! the only way to the bytes is to click, and Chrome writes the file itself.
//!
//! # What was measured before this was written
//!
//! - `Browser.setDownloadBehavior` is accepted on the PAGE websocket this tool already holds, and
//!   `Browser.downloadWillBegin` / `Browser.downloadProgress` are delivered on it. No second
//!   browser-level connection is needed.
//! - The override does NOT outlive the CDP session that set it: a fresh connection to the same
//!   page, clicking the same link with nothing armed, produced no file at the armed path. That is
//!   the same rule `emulation.rs` documents for `Emulation.*`, and it decides the shape of this
//!   module — the arming has to happen on the connection that clicks, which is why this is a flag
//!   on `download` and not a separate `wait --download` verb. A separate verb would work in pipe
//!   mode, where one connection spans both commands, and silently capture nothing from the CLI,
//!   where each invocation opens its own.
//! - `Browser.cancelDownload` is implemented (a bogus guid answers `-32602 No download item found
//!   for the given GUID`), so `--max-bytes` can be enforced on this path rather than declared
//!   inapplicable to it.
//!
//! # Why the subscription is taken before anything else
//!
//! `CdpClient::events()` only delivers messages that arrive after the subscribe. A blob download
//! begins within ~100 ms of the click, so subscribing afterwards is a race that loses the
//! `downloadWillBegin` — and with it the server's suggested filename, which is the one piece of
//! the report that exists nowhere else.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::cdp::client::CdpClient;
use crate::cdp::types::CdpEvent;
use crate::session::{liveness, Liveness};

/// What Chrome said about the download this action armed for.
pub enum Transfer {
    /// Nothing began in the window. The click still happened — that is the whole reason this is
    /// not an error.
    NeverBegan { waited_ms: u64 },
    /// Chrome reported `completed` and named the file it wrote.
    Completed { began: Began, bytes: u64, temp_path: PathBuf },
    /// Chrome reported `canceled`. `why` names who cancelled it, because the two cases have
    /// opposite recoveries: our own size cap is a flag the caller can raise, the page's or the
    /// browser's is not.
    Canceled { began: Began, why: Cancelled },
    /// It began and had not finished when the window closed. The bytes on disk are a prefix of
    /// the file, so nothing is moved into place and no path is claimed.
    Unfinished { began: Began, received: u64, total: u64, waited_ms: u64 },
}

/// Who ended a download that started.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cancelled {
    /// `--max-bytes`: this tool asked Chrome to stop.
    ExceededCap,
    /// Chrome or the page stopped it. We only know that it stopped.
    ByBrowser,
}

/// What `Browser.downloadWillBegin` said. Present for every outcome except `NeverBegan`, so the
/// report can name the file even when the transfer did not finish.
pub struct Began {
    pub guid: String,
    /// The name the server (or the `download` attribute) proposed. Never used as a path without
    /// `download::sanitize_name` first.
    pub suggested_filename: String,
    pub url: String,
}

/// A subscription and a private directory, held across the click.
pub struct Armed {
    events: broadcast::Receiver<CdpEvent>,
    dir: PathBuf,
}

/// How hard the sweep tries before giving the directory up, and how long it waits between tries.
///
/// 5 × 30 ms, and only paid when Chrome is still writing: the first attempt succeeds on every
/// download that completed. It is the fast path, not the guarantee — see [`collect_abandoned`].
const SWEEP_ATTEMPTS: u32 = 5;
const SWEEP_GAP_MS: u64 = 30;

/// What names a transfer directory, and the reason the filter on it is load-bearing rather than
/// decorative: `~/.chrome-agent/tmp` is also where `screenshot`, `pdf` and `download <url>` put a
/// file the caller did not name, and none of those are ours to remove.
const INCOMING_PREFIX: &str = ".incoming-";

/// How many transfer directories one arming looks at.
///
/// Far looser than `profiles.rs`'s 32, and deliberately so: examining one profile there is a
/// recursive scan for holder artefacts and a modification time, while examining one of these is a
/// string split and a `kill(pid, 0)`. What the cap actually bounds is the readdir of a directory
/// somebody let grow. Removal is uncapped for the same asymmetry read the other way — a transfer
/// directory holds one file, so unlinking it is a handful of syscalls whatever that file weighs,
/// where removing one 14 MB profile is thousands of them.
const COLLECT_CAP: usize = 64;

/// Point Chrome's downloads at a directory this invocation owns, and start listening.
///
/// Fails the command rather than clicking unarmed: a click that has been delivered cannot be
/// taken back, and dispatching one whose product we cannot capture is the one outcome worse than
/// refusing — the caller would have to click a second time to get the file, and the page cannot
/// tell that from a second deliberate action.
pub async fn arm(client: &CdpClient) -> Result<Armed, crate::BoxError> {
    let tmp = tmp_root()?;
    // Before this invocation's own directory exists, so the collector never has to reason about a
    // half-created one. Ours would survive it either way — our pid is alive, and that is the whole
    // predicate — but a sweep that cannot see its own caller is one fewer thing to argue about.
    let _ = collect_abandoned(&tmp, COLLECT_CAP);
    let dir = incoming_dir(&tmp);
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    // Before the CDP call, not after: the subscription has to predate anything that could
    // produce an event, and `setDownloadBehavior` is the first thing that can.
    let events = client.events();
    let path = dir.display().to_string();
    if let Err(error) = client
        .call::<_, Value>(
            "Browser.setDownloadBehavior",
            json!({"behavior": "allowAndName", "downloadPath": path, "eventsEnabled": true}),
        )
        .await
    {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(format!(
            "download: Chrome refused to hand downloads to this session ({error}), so the click \
             was not dispatched"
        )
        .into());
    }
    Ok(Armed { events, dir })
}

/// Give downloads back to Chrome's own setting.
///
/// Best effort and deliberately not checked: the override dies with this CDP session anyway (see
/// the module docs), so the only window this closes is the rest of a `pipe` session, where a
/// later command's download would otherwise land in this invocation's private directory under a
/// guid instead of wherever the browser normally puts it.
pub async fn disarm(client: &CdpClient) {
    let _ = client
        .call::<_, Value>("Browser.setDownloadBehavior", json!({"behavior": "default"}))
        .await;
}

/// Wait for the download the click was supposed to produce, bounded by `timeout`.
///
/// One bound for both questions this asks — "did anything start" and "did it finish" — because
/// the natural scale of the first is not knowable in advance: a blob begins in a hundred
/// milliseconds, an attachment begins when the server's response headers arrive. A caller who
/// expects no download, or a fast one, passes a smaller `--timeout`; a fixed short window for
/// the beginning would report "nothing began" for every slow server.
pub async fn collect(
    client: &CdpClient,
    armed: &mut Armed,
    timeout: Duration,
    max_bytes: u64,
) -> Transfer {
    let started = Instant::now();
    let mut began: Option<Began> = None;
    let mut last_received = 0_u64;
    let mut last_total = 0_u64;
    let mut cancelled_by_us = false;

    while let Some(left) = timeout.checked_sub(started.elapsed()) {
        let event = match tokio::time::timeout(left, armed.events.recv()).await {
            // A dropped event on this channel is not recoverable by waiting harder, but the
            // states we care about are re-sent on every progress tick, so keep listening.
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            // The window closed, or the connection went away. Both leave the caller with
            // whatever was learnt so far, which the tail below turns into an outcome.
            Err(_) | Ok(Err(broadcast::error::RecvError::Closed)) => break,
            Ok(Ok(event)) => event,
        };
        match event.method.as_str() {
            "Browser.downloadWillBegin" if began.is_none() => {
                began = Some(Began {
                    guid: string_field(&event.params, "guid"),
                    suggested_filename: string_field(&event.params, "suggestedFilename"),
                    url: string_field(&event.params, "url"),
                });
            }
            "Browser.downloadProgress" => {
                // A second, concurrent download is not this action's answer. First guid wins.
                let Some(current) = began.as_ref() else { continue };
                if string_field(&event.params, "guid") != current.guid {
                    continue;
                }
                last_received = number_field(&event.params, "receivedBytes");
                last_total = number_field(&event.params, "totalBytes");
                let state = string_field(&event.params, "state");
                if !cancelled_by_us && last_received.max(last_total) > max_bytes {
                    cancelled_by_us = true;
                    let _ = client
                        .call::<_, Value>(
                            "Browser.cancelDownload",
                            json!({"guid": current.guid}),
                        )
                        .await;
                    // Do not return yet: Chrome answers with a `canceled` progress event, and
                    // the file it had already written is deleted on that transition.
                    continue;
                }
                match state.as_str() {
                    "completed" => {
                        let began = began.take().expect("guarded above");
                        // `allowAndName` names the file after the guid; `filePath` is what
                        // Chrome reported, and is preferred when present so a future naming
                        // change does not silently break the move.
                        let temp_path = event
                            .params
                            .get("filePath")
                            .and_then(Value::as_str)
                            .map_or_else(|| armed.dir.join(&began.guid), PathBuf::from);
                        if cancelled_by_us {
                            // It finished before the cancel landed. The bytes are over the cap
                            // the caller set, so they are removed rather than handed back.
                            let _ = std::fs::remove_file(&temp_path);
                            return Transfer::Canceled { began, why: Cancelled::ExceededCap };
                        }
                        return Transfer::Completed { began, bytes: last_received, temp_path };
                    }
                    "canceled" => {
                        let began = began.take().expect("guarded above");
                        let why = if cancelled_by_us {
                            Cancelled::ExceededCap
                        } else {
                            Cancelled::ByBrowser
                        };
                        return Transfer::Canceled { began, why };
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let waited_ms = elapsed_ms(started);
    match began {
        None => Transfer::NeverBegan { waited_ms },
        Some(began) => {
            Transfer::Unfinished { began, received: last_received, total: last_total, waited_ms }
        }
    }
}

/// Move a completed download to where the caller asked for it, at 0600.
///
/// The bytes reached disk through Chrome with whatever the umask allowed; every other file this
/// tool writes (screenshot, pdf, `download <url>`, the session store, a recording) is 0600, and a
/// downloaded file is at least as likely to be the thing worth protecting.
pub fn place(
    completed_path: &std::path::Path,
    suggested: &str,
    out: Option<&str>,
) -> Result<(String, u64), crate::BoxError> {
    let destination = super::download::resolve_named_path(out, suggested)?;
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // `rename` is the cheap path and works whenever the private directory and the destination
    // share a filesystem, which is the default (both under ~/.chrome-agent/tmp). An explicit
    // `--out` on another volume falls back to a copy.
    if std::fs::rename(completed_path, &destination).is_err() {
        std::fs::copy(completed_path, &destination)?;
        let _ = std::fs::remove_file(completed_path);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o600));
    }
    let bytes = std::fs::metadata(&destination)?.len();
    Ok((destination.display().to_string(), bytes))
}

/// Drop the private directory, and keep trying while Chrome is still writing into it.
///
/// Separate from `place` so every outcome pays it, including the ones that wrote nothing. The
/// retry is not decoration: measured on the `--max-bytes` path, Chrome answers `canceled`, we
/// return, and it then finalises — recreating the directory and a zero-byte stub AFTER the
/// removal. It is the same lesson `close --purge` learnt about a Chrome that has been told to
/// stop and has not finished stopping: a removal that reports success on its first `Ok` is
/// claiming a convergence that has not happened.
///
/// What it is NOT is the guarantee. This runs before the process ends, and on the paths where
/// Chrome is not finished with the directory it is racing something no budget can bound — see
/// [`collect_abandoned`] for the measurement and for what actually converges. Keeping it is still
/// worth it, and that is the whole of its case: a download that completed clears on the first
/// attempt and never reaches the collector, so nothing routinely accumulates for a later
/// invocation to find.
pub async fn clean_up(armed: &Armed) {
    for attempt in 0..SWEEP_ATTEMPTS {
        let _ = std::fs::remove_dir_all(&armed.dir);
        if !armed.dir.exists() {
            return;
        }
        if attempt + 1 < SWEEP_ATTEMPTS {
            tokio::time::sleep(Duration::from_millis(SWEEP_GAP_MS)).await;
        }
    }
}

/// Remove the transfer directories of processes that are provably gone. Returns their names.
///
/// # What no sweep budget can cover
///
/// [`clean_up`] runs before this process ends, and on the paths where Chrome is not finished with
/// the directory it is chasing something it cannot bound. Measured here with the shipped 5 × 30 ms
/// budget, over eight downloads whose transfer was still running when `--timeout` expired: three
/// directories were back on disk the moment the last invocation returned, and **all eight** were
/// there fifteen seconds later, each holding the zero-byte stub `allowAndName` names after the
/// guid. The transfer does not stop when chrome-agent does — Chrome keeps the download it was
/// handed — so the only window wide enough is the length of the download, which is exactly the
/// bound `--timeout` already declined to be. Widening it would move the failure onto a slower
/// runner, which is where it was found in the first place.
///
/// # Why the pid is the whole predicate
///
/// The name is `.incoming-<pid>-<nanos>` and the pid is the process that armed it. Only Chrome,
/// acting on that invocation's `setDownloadBehavior`, ever writes there, and the override dies
/// with the CDP session (module docs); so once the OS no longer knows that pid, the directory
/// cannot gain another byte from anyone and any later invocation may take it. That is
/// `profiles.rs`'s shape — a removal predicated on "no live holder" rather than on a delay — and
/// it converges without inventing a number.
///
/// This is also why a concurrent agent is safe rather than merely unlikely to be hit: its
/// directory carries ITS pid, [`liveness`] answers `Alive`, and it is kept. A recycled pid answers
/// `Alive` too, which keeps a directory that could have gone — the harmless direction.
///
/// # Why arming, and what it costs there
///
/// It is the only moment anyone has a reason to care about this directory, and it is already a
/// filesystem operation on a command that is about to spend a CDP round trip, a click and up to
/// `--timeout` seconds waiting. Measured: 227 µs with nothing to collect, which is the shape of
/// every invocation after the first; 241 µs with 500 unrelated files beside it, since the prefix
/// filter answers before any pid is probed; 6.1 ms to examine and remove a full window of 64,
/// which is a backlog being drained rather than a recurring cost. The session store's save path —
/// where `profiles.rs` sweeps — was the alternative and was rejected: it runs on every command
/// including read-only ones, and this directory is created by exactly one verb.
///
/// The price of that choice, stated: a caller who abandons a download and never runs another
/// leaves the crumbs there. `close --purge-orphans` does not cover them either. What bounds the
/// damage is that a transfer directory holds one partial file, and the next `download` takes 64.
///
/// Every case that does not resolve keeps the directory: [`Liveness::Unknown`] (a pid under
/// another uid, and every non-Unix platform, where no probe is wired and this is therefore a
/// no-op), a name whose pid does not parse, and anything here that is not a transfer directory at
/// all.
///
/// The known gap, stated rather than guarded: a `HOME` shared between machines or containers puts
/// two pid namespaces over one directory, and a pid dead here may be alive there. `profiles.rs`
/// guards that case with the hostname in Chrome's `SingletonLock`; this does not, because what is
/// at risk is a partial download of the current second rather than a profile something is logged
/// into, and the session store beside it already assumes one machine.
pub fn collect_abandoned(tmp: &Path, cap: usize) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(tmp) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with(INCOMING_PREFIX))
        .collect();
    // Sorted only so the window is the same set on two successive invocations rather than whatever
    // order the filesystem answered in — a backlog behind the cap drains deterministically instead
    // of depending on readdir. It is lexicographic over the whole name, so it is not any
    // meaningful age order, and nothing here needs one: the predicate is about the owner, not the
    // clock, which is the difference from `profiles.rs` and the reason no rotation is needed.
    names.sort_unstable();
    names.truncate(cap);

    let mut removed = Vec::new();
    for name in names {
        if !is_abandoned(&name) {
            continue;
        }
        if std::fs::remove_dir_all(tmp.join(&name)).is_ok() {
            removed.push(name);
        }
    }
    removed
}

/// `.incoming-<pid>-<nanos>` → whether the process named in it is provably gone.
fn is_abandoned(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(INCOMING_PREFIX) else {
        return false;
    };
    let Some((pid, _nanos)) = rest.split_once('-') else {
        return false;
    };
    pid.parse::<u32>().is_ok_and(|pid| liveness(pid) == Liveness::Dead)
}

/// Where every file this tool writes without being told a path goes.
fn tmp_root() -> Result<PathBuf, crate::BoxError> {
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    Ok(home.join(".chrome-agent").join("tmp"))
}

/// A directory only this invocation writes to, so `allowAndName`'s guid-named files cannot
/// collide with a concurrent agent's and the sweep cannot delete one.
fn incoming_dir(tmp: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    tmp.join(format!("{INCOMING_PREFIX}{}-{nanos}", std::process::id()))
}

fn string_field(params: &Value, key: &str) -> String {
    params.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// Byte counters arrive as JSON numbers and Chrome sends them as integers, but the protocol types
/// them as `number`. Reading only `as_u64` would silently report 0 for a float, and reading the
/// float straight into `u64` would turn Chrome's `-1` for "size not known yet" into 18 exabytes —
/// which the `--max-bytes` check would then cancel the download over.
fn number_field(params: &Value, key: &str) -> u64 {
    let Some(value) = params.get(key) else { return 0 };
    if let Some(exact) = value.as_u64() {
        return exact;
    }
    let truncated = value.as_f64().map_or(0_i64, |number| number.trunc() as i64);
    u64::try_from(truncated).unwrap_or(0)
}

fn elapsed_ms(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_counters_survive_an_integer_or_a_float() {
        assert_eq!(number_field(&json!({"totalBytes": 22}), "totalBytes"), 22);
        assert_eq!(number_field(&json!({"totalBytes": 22.0}), "totalBytes"), 22);
        assert_eq!(number_field(&json!({"totalBytes": -1}), "totalBytes"), 0);
        assert_eq!(number_field(&json!({}), "totalBytes"), 0);
    }

    #[test]
    fn a_missing_string_field_is_empty_not_a_panic() {
        assert_eq!(string_field(&json!({}), "guid"), "");
        assert_eq!(string_field(&json!({"guid": 7}), "guid"), "");
        assert_eq!(string_field(&json!({"guid": "abc"}), "guid"), "abc");
    }

    /// Two invocations must not share the directory Chrome writes guid-named files into: the
    /// sweep at the end of one would take the other's file with it.
    #[test]
    fn each_invocation_gets_its_own_incoming_directory() {
        let tmp = scratch("incoming-names");
        let first = incoming_dir(&tmp);
        std::thread::sleep(Duration::from_millis(2));
        let second = incoming_dir(&tmp);
        assert_ne!(first, second);
        assert!(first.to_string_lossy().contains(INCOMING_PREFIX));
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// A scratch directory no second process can guess, since these run on parallel threads and
    /// alongside other `cargo test` processes.
    fn scratch(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir()
            .join(format!("chrome-agent-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn transfer_dir(tmp: &Path, name: &str) -> PathBuf {
        let dir = tmp.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        // `allowAndName` leaves a guid-named file; an empty directory is not the shape found on
        // disk and would let a `remove_dir` that cannot handle contents pass by accident.
        std::fs::write(dir.join("6f1c1f0e-guid"), b"partial").unwrap();
        dir
    }

    /// A pid nothing holds any more: spawned, waited on, and therefore reaped.
    #[cfg(unix)]
    fn a_reaped_pid() -> u32 {
        let mut child =
            std::process::Command::new("/bin/sh").args(["-c", "exit 0"]).spawn().expect("spawn");
        let pid = child.id();
        child.wait().expect("wait");
        pid
    }

    /// The defect, without waiting for a real race: a directory whose owner has exited is
    /// collected by the next arming, and one whose owner is alive is not.
    ///
    /// Before the collector existed, `clean_up`'s 5 × 30 ms was the only thing that ever removed
    /// one of these, so the abandoned directory here survived for good — measured on the path
    /// where the transfer is still running at eight invocations out of eight.
    #[cfg(unix)]
    #[test]
    fn a_transfer_directory_is_collected_once_its_process_is_gone() {
        let tmp = scratch("collect-abandoned");
        let dead = a_reaped_pid();
        assert_eq!(
            liveness(dead),
            Liveness::Dead,
            "the pid was recycled between the wait and the probe, so this proves nothing"
        );

        let abandoned = transfer_dir(&tmp, &format!("{INCOMING_PREFIX}{dead}-1788086042802162000"));
        let live = transfer_dir(
            &tmp,
            &format!("{INCOMING_PREFIX}{}-1788086042802162001", std::process::id()),
        );

        let removed = collect_abandoned(&tmp, COLLECT_CAP);

        assert!(!abandoned.exists(), "a directory nothing can write to any more was kept");
        assert_eq!(removed.len(), 1, "{removed:?}");
        assert!(
            live.exists(),
            "a running process's transfer directory was taken, which on a concurrent agent is \
             its download"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// Everything the predicate cannot resolve keeps the directory, and everything else in
    /// `~/.chrome-agent/tmp` is not the predicate's business at all.
    #[test]
    fn an_unreadable_owner_or_another_command_s_file_is_left_alone() {
        let tmp = scratch("collect-keeps");
        let unparseable = transfer_dir(&tmp, &format!("{INCOMING_PREFIX}not-a-pid"));
        let no_separator = transfer_dir(&tmp, INCOMING_PREFIX.trim_end_matches('-'));
        // A `screenshot`/`pdf`/`download <url>` output sitting in the same directory.
        let neighbour = tmp.join("shot-1788086042.png");
        std::fs::write(&neighbour, b"png").unwrap();

        assert!(collect_abandoned(&tmp, COLLECT_CAP).is_empty());
        assert!(unparseable.exists());
        assert!(no_separator.exists());
        assert!(neighbour.exists());
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// The cap bounds the readdir of a directory somebody let grow, and the rest is drained by
    /// the invocations that follow rather than dropped.
    #[cfg(unix)]
    #[test]
    fn the_cap_bounds_one_arming_and_the_backlog_still_converges() {
        let tmp = scratch("collect-cap");
        let dead = a_reaped_pid();
        assert_eq!(liveness(dead), Liveness::Dead, "the pid was recycled");
        for n in 0..5 {
            transfer_dir(&tmp, &format!("{INCOMING_PREFIX}{dead}-178808604280216200{n}"));
        }

        assert_eq!(collect_abandoned(&tmp, 2).len(), 2, "the cap is not applied");
        assert_eq!(collect_abandoned(&tmp, 2).len(), 2);
        assert_eq!(collect_abandoned(&tmp, 2).len(), 1);
        assert!(collect_abandoned(&tmp, 2).is_empty(), "the backlog did not drain");
        std::fs::remove_dir_all(&tmp).ok();
    }
}
