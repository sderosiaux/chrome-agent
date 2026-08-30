//! The download a CLICK produces, for files with no fetchable URL (blob, POST-backed endpoint).
//!
//! `Browser.setDownloadBehavior` and the `Browser.download*` events work on the page websocket,
//! but the override dies with the CDP session — so arming must happen on the connection that
//! clicks, which is why this is a flag on `download` and not a separate verb.
//!
//! Subscribe BEFORE the CDP call: `CdpClient::events()` only delivers messages that arrive
//! after the subscribe, and a blob download begins ~100 ms after the click.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::cdp::client::CdpClient;
use crate::cdp::types::CdpEvent;
use crate::session::{Liveness, liveness};

/// What Chrome said about the download this action armed for.
pub enum Transfer {
    /// Nothing began in the window. The click still happened, so this is not an error.
    NeverBegan { waited_ms: u64 },
    /// Events were dropped before this command could read them, and no terminal state was seen.
    ///
    /// Not an outcome of the download — an outcome of the OBSERVATION. `downloadWillBegin` fires
    /// once, so a drop that eats it leaves every later progress event unmatched and the tail
    /// would otherwise read as "nothing began" over a file Chrome is writing. `kept` names the
    /// transfer directory when it holds anything, taken out of the sweep's reach.
    EvidenceLost {
        began: Option<Began>,
        dropped: u64,
        kept: Option<PathBuf>,
        waited_ms: u64,
    },
    /// Chrome reported `completed` and named the file it wrote.
    Completed {
        began: Began,
        bytes: u64,
        temp_path: PathBuf,
    },
    /// Chrome reported `canceled`. `why` separates our own size cap (the caller can raise it)
    /// from the browser's or the page's (they cannot).
    Canceled { began: Began, why: Cancelled },
    /// Began and unfinished when the window closed. The bytes on disk are a prefix, so nothing
    /// is moved into place and no path is claimed.
    Unfinished {
        began: Began,
        received: u64,
        total: u64,
        waited_ms: u64,
    },
}

/// Who ended a download that started.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cancelled {
    /// `--max-bytes`: this tool asked Chrome to stop.
    ExceededCap,
    /// Chrome or the page stopped it.
    ByBrowser,
}

/// What `Browser.downloadWillBegin` said. Present for every outcome except `NeverBegan`.
pub struct Began {
    pub guid: String,
    /// Proposed by the server or the `download` attribute. Never used as a path without
    /// `download::sanitize_name` first.
    pub suggested_filename: String,
    pub url: String,
}

/// A subscription and a private directory, held across the click.
pub struct Armed {
    events: broadcast::Receiver<CdpEvent>,
    dir: PathBuf,
    /// Set by [`Armed::preserve`]: the directory may hold a completed file this action lost the
    /// evidence for, so no sweep may take it.
    preserved: bool,
}

impl Armed {
    /// Take the transfer directory out of the sweep's namespace when it holds anything, and say
    /// where it went. `None` when the directory is empty — there is nothing to keep.
    ///
    /// A rename, not a flag alone: [`collect_abandoned`] recognises a directory by its
    /// `.incoming-` prefix and by a pid, and this process's pid dies long before anyone can look
    /// at the file. A flag only survives until this invocation returns.
    fn preserve(&mut self) -> Option<PathBuf> {
        if !self.holds_anything() {
            return None;
        }
        // Whatever the rename does, `clean_up` must not remove this.
        self.preserved = true;
        let kept =
            self.dir
                .with_file_name(format!("{KEPT_PREFIX}{}-{}", std::process::id(), nanos()));
        if std::fs::rename(&self.dir, &kept).is_ok() {
            self.dir.clone_from(&kept);
            return Some(kept);
        }
        // The rename is the durable half; without it the file is still there for now, and the
        // response points at where it actually is rather than where it was meant to go.
        Some(self.dir.clone())
    }

    fn holds_anything(&self) -> bool {
        std::fs::read_dir(&self.dir).is_ok_and(|mut entries| entries.next().is_some())
    }
}

/// Sweep budget: 5 × 30 ms, only paid while Chrome is still writing. The fast path, not the
/// guarantee — see [`collect_abandoned`].
const SWEEP_ATTEMPTS: u32 = 5;
const SWEEP_GAP_MS: u64 = 30;

/// Transfer-directory prefix. The filter is load-bearing: `~/.chrome-agent/tmp` also holds
/// unnamed `screenshot`/`pdf`/`download <url>` output, which is not ours to remove.
const INCOMING_PREFIX: &str = ".incoming-";

/// Where a transfer directory goes when the evidence about it was lost. Deliberately outside
/// [`INCOMING_PREFIX`], so neither `clean_up` nor a later `collect_abandoned` can delete a file
/// that may be complete, and deliberately visible, since the response names the path and nothing
/// else will ever remove it.
const KEPT_PREFIX: &str = "kept-";

/// How many transfer directories one arming examines. Bounds the readdir only; removal is
/// uncapped, since a transfer directory holds one file.
const COLLECT_CAP: usize = 64;

/// Point Chrome's downloads at a directory this invocation owns, and start listening.
///
/// Fails the command rather than clicking unarmed: a delivered click cannot be taken back, and
/// the caller would have to click twice to get the file.
pub async fn arm(client: &CdpClient) -> Result<Armed, crate::BoxError> {
    let tmp = tmp_root()?;
    // Before creating our own directory, so the collector never sees a half-created one.
    let _ = collect_abandoned(&tmp, COLLECT_CAP);
    let dir = incoming_dir(&tmp);
    crate::secure_fs::create_private_dir_all(&dir)?;
    // Before the CDP call: the subscription must predate anything that can produce an event.
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
    Ok(Armed {
        events,
        dir,
        preserved: false,
    })
}

/// Give downloads back to Chrome's own setting. Best effort, unchecked: the override dies with
/// the CDP session anyway, so this only matters for the rest of a `pipe` session.
pub async fn disarm(client: &CdpClient) {
    let _ = client
        .call::<_, Value>(
            "Browser.setDownloadBehavior",
            json!({"behavior": "default"}),
        )
        .await;
}

/// Wait for the download the click was supposed to produce, bounded by `timeout`.
///
/// One bound covers both "did anything start" and "did it finish": a blob begins in ~100 ms, an
/// attachment begins when the server's headers arrive, so a fixed short start window would
/// report "nothing began" for every slow server.
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
    let mut dropped = 0_u64;

    while let Some(left) = timeout.checked_sub(started.elapsed()) {
        let event = match tokio::time::timeout(left, armed.events.recv()).await {
            // A dropped event is not an absent one. This used to `continue` on the grounds that
            // the states we care about repeat on every progress tick — true of
            // `downloadProgress`, false of `Browser.downloadWillBegin`, which fires ONCE. The
            // count is carried to the tail, where it forbids a confident answer.
            Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                dropped = dropped.saturating_add(n);
                continue;
            }
            // Window closed or connection gone; the tail below turns what we know into an outcome.
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
                let Some(current) = began.as_ref() else {
                    continue;
                };
                if string_field(&event.params, "guid") != current.guid {
                    continue;
                }
                last_received = number_field(&event.params, "receivedBytes");
                last_total = number_field(&event.params, "totalBytes");
                let state = string_field(&event.params, "state");
                if !cancelled_by_us && last_received.max(last_total) > max_bytes {
                    cancelled_by_us = true;
                    let _ = client
                        .call::<_, Value>("Browser.cancelDownload", json!({"guid": current.guid}))
                        .await;
                    // Do not return yet: Chrome answers with a `canceled` progress event and
                    // deletes the partial file on that transition.
                    continue;
                }
                match state.as_str() {
                    "completed" => {
                        let began = began.take().expect("guarded above");
                        // `allowAndName` names the file after the guid; prefer Chrome's own
                        // `filePath` when present.
                        let temp_path = event
                            .params
                            .get("filePath")
                            .and_then(Value::as_str)
                            .map_or_else(|| armed.dir.join(&began.guid), PathBuf::from);
                        if cancelled_by_us {
                            // Finished before the cancel landed, and over the cap: remove it.
                            let _ = std::fs::remove_file(&temp_path);
                            return Transfer::Canceled {
                                began,
                                why: Cancelled::ExceededCap,
                            };
                        }
                        return Transfer::Completed {
                            began,
                            bytes: last_received,
                            temp_path,
                        };
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

    // Only the tail is unproven: `completed` and `canceled` return above, carrying the fact
    // itself. Anything Chrome wrote is taken out of the sweep's reach BEFORE the outcome is
    // built, so the two cannot disagree.
    let kept = if dropped > 0 { armed.preserve() } else { None };
    conclude(
        dropped,
        began,
        kept,
        last_received,
        last_total,
        elapsed_ms(started),
    )
}

/// What the end of the wait means. Pure, so the rule "a dropped event never produces a confident
/// answer" is testable without a browser.
fn conclude(
    dropped: u64,
    began: Option<Began>,
    kept: Option<PathBuf>,
    received: u64,
    total: u64,
    waited_ms: u64,
) -> Transfer {
    if dropped > 0 {
        return Transfer::EvidenceLost {
            began,
            dropped,
            kept,
            waited_ms,
        };
    }
    match began {
        None => Transfer::NeverBegan { waited_ms },
        Some(began) => Transfer::Unfinished {
            began,
            received,
            total,
            waited_ms,
        },
    }
}

/// Move a completed download to where the caller asked for it, at 0600 like every other file
/// this tool writes (Chrome writes it with whatever the umask allowed).
pub fn place(
    completed_path: &std::path::Path,
    suggested: &str,
    out: Option<&str>,
) -> Result<(String, u64), crate::BoxError> {
    let destination = super::download::resolve_named_path(out, suggested)?;
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // `rename` works when both sides share a filesystem; an `--out` on another volume copies.
    if std::fs::rename(completed_path, &destination).is_err() {
        copy_without_following(completed_path, &destination)?;
        let _ = std::fs::remove_file(completed_path);
    }
    crate::secure_fs::restrict_file(&destination)?;
    let bytes = std::fs::metadata(&destination)?.len();
    Ok((destination.display().to_string(), bytes))
}

/// The cross-device half of [`place`], with `rename`'s semantics rather than `copy`'s.
///
/// `fs::rename` REPLACES a symlink at the destination; `fs::copy` FOLLOWS it and writes through
/// to wherever it points. The destination name is server-supplied (`sanitize_name` keeps it
/// inside the directory, and says nothing about what is already there), so the two halves of one
/// verb disagreed about whether a planted `~/.chrome-agent/tmp/report.csv -> ~/.ssh/authorized_keys`
/// gets replaced or written through.
///
/// Unlink first, then `create_new`: the unlink removes the LINK, and `create_new` refuses
/// anything that appears in between rather than opening it.
fn copy_without_following(
    from: &std::path::Path,
    to: &std::path::Path,
) -> Result<(), crate::BoxError> {
    let _ = std::fs::remove_file(to);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut target = options.open(to)?;
    let mut source = std::fs::File::open(from)?;
    std::io::copy(&mut source, &mut target)?;
    Ok(())
}

/// Drop the private directory, retrying while Chrome is still writing into it.
///
/// The retry matters on the `--max-bytes` path: Chrome answers `canceled`, we return, and it
/// then finalises, recreating the directory and a zero-byte stub after the removal. This is the
/// fast path only — a still-running transfer outlives the process; [`collect_abandoned`] is what
/// converges.
pub async fn clean_up(armed: &Armed) {
    // What `preserve` kept may be the caller's file. Deleting it is the one thing this must not
    // do; the response names the path instead.
    if armed.preserved {
        return;
    }
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
/// The pid in `.incoming-<pid>-<nanos>` is the whole predicate: nothing but Chrome acting for
/// that invocation writes there, and the override dies with its CDP session. Every unresolved
/// case keeps the directory — [`Liveness::Unknown`] (another uid, and every non-Unix platform,
/// where this is a no-op), an unparseable pid, a directory that is not one of ours, and a
/// recycled pid. Known gap: a `HOME` shared across pid namespaces.
///
/// Runs at arming: 227 µs with nothing to collect, 6.1 ms to drain a full 64-entry window.
pub fn collect_abandoned(tmp: &Path, cap: usize) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(tmp) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with(INCOMING_PREFIX))
        .collect();
    // Sorted so a backlog behind the cap drains deterministically instead of following readdir
    // order. Lexicographic, not age order — the predicate is the owner, not the clock.
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
    pid.parse::<u32>()
        .is_ok_and(|pid| liveness(pid) == Liveness::Dead)
}

/// Where every file this tool writes without being told a path goes.
fn tmp_root() -> Result<PathBuf, crate::BoxError> {
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    Ok(home.join(".chrome-agent").join("tmp"))
}

/// A directory only this invocation writes to, so `allowAndName`'s guid-named files cannot
/// collide with a concurrent agent's.
fn incoming_dir(tmp: &Path) -> PathBuf {
    tmp.join(format!(
        "{INCOMING_PREFIX}{}-{}",
        std::process::id(),
        nanos()
    ))
}

/// Wall clock since the epoch, in nanoseconds: what separates two directories one process opens.
fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

fn string_field(params: &Value, key: &str) -> String {
    params
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Byte counters are CDP `number`s. `as_u64` alone reports 0 for a float; casting the float
/// directly turns Chrome's `-1` ("size unknown") into 18 exabytes, which `--max-bytes` would
/// then cancel over.
fn number_field(params: &Value, key: &str) -> u64 {
    let Some(value) = params.get(key) else {
        return 0;
    };
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

    /// Two invocations must not share a transfer directory: one's sweep would take the other's
    /// file.
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

    /// An `Armed` over a directory, with a subscription nothing sends on: `preserve` and
    /// `clean_up` never read the events.
    fn armed_over(dir: PathBuf) -> Armed {
        let (_tx, events) = broadcast::channel(1);
        Armed {
            events,
            dir,
            preserved: false,
        }
    }

    /// The bug this guards: a lost `Browser.downloadWillBegin` (it fires once, and the channel
    /// holds 256 messages) left `began` at `None`, which read as `NeverBegan` — "no download
    /// began, nothing was written" — over a file Chrome had finished writing.
    #[test]
    fn a_dropped_event_never_produces_a_confident_answer() {
        let began = || Began {
            guid: "g".into(),
            suggested_filename: "report.csv".into(),
            url: "blob:null/x".into(),
        };

        assert!(
            matches!(
                conclude(0, None, None, 0, 0, 12),
                Transfer::NeverBegan { .. }
            ),
            "with nothing dropped, an empty wait is still an answer"
        );
        assert!(
            matches!(
                conclude(0, Some(began()), None, 3, 9, 12),
                Transfer::Unfinished { .. }
            ),
            "with nothing dropped, a started-and-unfinished transfer is still an answer"
        );

        let kept = PathBuf::from("/tmp/kept-1-2");
        let lost = conclude(1, None, Some(kept.clone()), 0, 0, 12);
        let Transfer::EvidenceLost {
            began: none,
            dropped,
            kept: where_,
            waited_ms,
        } = lost
        else {
            panic!("a drop with no terminal state cannot claim that nothing began");
        };
        assert!(none.is_none());
        assert_eq!(dropped, 1);
        assert_eq!(where_, Some(kept));
        assert_eq!(waited_ms, 12);

        assert!(
            matches!(
                conclude(7, Some(began()), None, 3, 9, 12),
                Transfer::EvidenceLost { .. }
            ),
            "a drop after the download began may have eaten `completed`, so `incomplete` is a \
             claim this cannot make either"
        );
    }

    /// What a lost-evidence directory holds may be the caller's whole file, so neither sweep may
    /// take it: `clean_up` skips it and `collect_abandoned` cannot even see it.
    #[test]
    fn a_directory_whose_evidence_was_lost_survives_both_sweeps() {
        let tmp = scratch("kept-dir");
        let dir = transfer_dir(
            &tmp,
            &format!(
                "{INCOMING_PREFIX}{}-1788086042802162000",
                std::process::id()
            ),
        );
        let mut armed = armed_over(dir.clone());

        let kept = armed
            .preserve()
            .expect("a directory holding a file is kept");
        assert!(
            !dir.exists(),
            "the transfer directory was left where a sweep can find it"
        );
        assert!(kept.exists(), "the file was not moved anywhere");
        assert_eq!(
            std::fs::read(kept.join("6f1c1f0e-guid")).unwrap(),
            b"partial"
        );
        let name = kept.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(INCOMING_PREFIX),
            "still in the collector's namespace, so the next arming takes it: {name}"
        );

        assert!(collect_abandoned(&tmp, COLLECT_CAP).is_empty());
        block_on(clean_up(&armed));
        assert!(
            kept.exists(),
            "clean_up deleted a directory that may hold a completed file"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// An empty directory holds no file to lose, so it stays collectible: the exception exists
    /// for bytes on disk, not for every lag.
    #[test]
    fn an_empty_transfer_directory_is_not_kept() {
        let tmp = scratch("kept-empty");
        let dir = tmp.join(format!(
            "{INCOMING_PREFIX}{}-1788086042802162001",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut armed = armed_over(dir.clone());

        assert!(armed.preserve().is_none(), "there was nothing to keep");
        assert!(!armed.preserved);
        block_on(clean_up(&armed));
        assert!(!dir.exists(), "an empty directory is still swept");

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// The destination name comes from the server. `fs::copy` follows a symlink sitting there
    /// and writes through it; the file this verb produces must land where it says it did.
    #[cfg(unix)]
    #[test]
    fn the_cross_device_copy_replaces_a_symlink_instead_of_writing_through_it() {
        let tmp = scratch("copy-symlink");
        let source = tmp.join("completed");
        std::fs::write(&source, b"the download").unwrap();
        let elsewhere = tmp.join("private-key");
        std::fs::write(&elsewhere, b"untouched").unwrap();
        let destination = tmp.join("report.csv");
        std::os::unix::fs::symlink(&elsewhere, &destination).unwrap();

        copy_without_following(&source, &destination).expect("the copy still lands");

        assert_eq!(
            std::fs::read(&elsewhere).unwrap(),
            b"untouched",
            "written through the link"
        );
        assert_eq!(std::fs::read(&destination).unwrap(), b"the download");
        assert!(
            !std::fs::symlink_metadata(&destination)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link was replaced, not followed"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 0600 at creation, not after: a `--out` on another volume is the same file the rename
    /// path narrows, and a window where it is world-readable is a window.
    #[cfg(unix)]
    #[test]
    fn the_cross_device_copy_creates_the_file_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = scratch("copy-perms");
        let source = tmp.join("completed");
        std::fs::write(&source, b"bytes").unwrap();
        let destination = tmp.join("out.bin");

        copy_without_following(&source, &destination).expect("copy");

        let mode = std::fs::metadata(&destination)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        std::fs::remove_dir_all(&tmp).ok();
        assert_eq!(mode, 0o600, "got {mode:o}");
    }

    /// Run one future to completion on a current-thread runtime: `clean_up` is `async` only for
    /// its retry sleep, and these tests plant the state rather than race it.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("a test runtime")
            .block_on(future)
    }

    /// A scratch directory no concurrent test thread or process can collide with.
    fn scratch(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir =
            std::env::temp_dir().join(format!("chrome-agent-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn transfer_dir(tmp: &Path, name: &str) -> PathBuf {
        let dir = tmp.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        // Non-empty, since `allowAndName` leaves a guid-named file: an empty directory would let
        // a `remove_dir` that cannot handle contents pass by accident.
        std::fs::write(dir.join("6f1c1f0e-guid"), b"partial").unwrap();
        dir
    }

    /// A pid nothing holds any more: spawned, waited on, and therefore reaped.
    #[cfg(unix)]
    fn a_reaped_pid() -> u32 {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn");
        let pid = child.id();
        child.wait().expect("wait");
        pid
    }

    /// A directory whose owner has exited is collected by the next arming; one whose owner is
    /// alive is not. The race state is planted rather than reproduced.
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

        let abandoned = transfer_dir(
            &tmp,
            &format!("{INCOMING_PREFIX}{dead}-1788086042802162000"),
        );
        let live = transfer_dir(
            &tmp,
            &format!(
                "{INCOMING_PREFIX}{}-1788086042802162001",
                std::process::id()
            ),
        );

        let removed = collect_abandoned(&tmp, COLLECT_CAP);

        assert!(
            !abandoned.exists(),
            "a directory nothing can write to any more was kept"
        );
        assert_eq!(removed.len(), 1, "{removed:?}");
        assert!(
            live.exists(),
            "a running process's transfer directory was taken, which on a concurrent agent is \
             its download"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// Everything the predicate cannot resolve keeps the directory, and other commands' output
    /// in `~/.chrome-agent/tmp` is never touched.
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

    /// The cap bounds one arming; the rest drains over the invocations that follow.
    #[cfg(unix)]
    #[test]
    fn the_cap_bounds_one_arming_and_the_backlog_still_converges() {
        let tmp = scratch("collect-cap");
        let dead = a_reaped_pid();
        assert_eq!(liveness(dead), Liveness::Dead, "the pid was recycled");
        for n in 0..5 {
            transfer_dir(
                &tmp,
                &format!("{INCOMING_PREFIX}{dead}-178808604280216200{n}"),
            );
        }

        assert_eq!(
            collect_abandoned(&tmp, 2).len(),
            2,
            "the cap is not applied"
        );
        assert_eq!(collect_abandoned(&tmp, 2).len(), 2);
        assert_eq!(collect_abandoned(&tmp, 2).len(), 1);
        assert!(
            collect_abandoned(&tmp, 2).is_empty(),
            "the backlog did not drain"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }
}
