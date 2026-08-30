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

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::cdp::client::CdpClient;
use crate::cdp::types::CdpEvent;

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
/// download that completed.
const SWEEP_ATTEMPTS: u32 = 5;
const SWEEP_GAP_MS: u64 = 30;

/// Point Chrome's downloads at a directory this invocation owns, and start listening.
///
/// Fails the command rather than clicking unarmed: a click that has been delivered cannot be
/// taken back, and dispatching one whose product we cannot capture is the one outcome worse than
/// refusing — the caller would have to click a second time to get the file, and the page cannot
/// tell that from a second deliberate action.
pub async fn arm(client: &CdpClient) -> Result<Armed, crate::BoxError> {
    let dir = incoming_dir()?;
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
/// removal, so every cancelled download left one `.incoming-<pid>-<nanos>/` behind for good. It
/// is the same lesson `close --purge` learnt about a Chrome that has been told to stop and has
/// not finished stopping: a removal that reports success on its first `Ok` is claiming a
/// convergence that has not happened.
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

/// A directory only this invocation writes to, so `allowAndName`'s guid-named files cannot
/// collide with a concurrent agent's and the sweep cannot delete one.
fn incoming_dir() -> Result<PathBuf, crate::BoxError> {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    Ok(home
        .join(".chrome-agent")
        .join("tmp")
        .join(format!(".incoming-{}-{nanos}", std::process::id())))
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
        let first = incoming_dir().unwrap();
        std::thread::sleep(Duration::from_millis(2));
        let second = incoming_dir().unwrap();
        assert_ne!(first, second);
        assert!(first.to_string_lossy().contains(".incoming-"));
    }
}
