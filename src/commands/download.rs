use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::cdp::client::CdpClient;
use crate::cdp::types::EvaluateResult;
use crate::element_ref::ElementRef;

pub struct DownloadResult {
    pub path: String,
    pub bytes: usize,
    pub mime: String,
}

/// Default in-page download limit: 64 MiB.
pub const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;

/// What the caller pointed `download` at. Both mechanisms produce the same contract: a file at
/// a named path, 0600, with its size and the server's proposed name.
///
/// Exactly one of the three, refused rather than ranked — picking for a caller who gave two
/// would silently ignore half the invocation.
pub enum Target<'a> {
    /// Fetch this URL inside the page. Auth-preserving, and needs no click.
    Url(&'a str),
    /// Click this uid and capture whatever download it produces.
    Uid(&'a str),
    /// Click the element this selector resolves to, and capture the download.
    Selector(&'a str),
}

impl<'a> Target<'a> {
    pub fn parse(
        url: Option<&'a str>,
        uid: Option<&'a str>,
        selector: Option<&'a str>,
    ) -> Result<Self, crate::BoxError> {
        match (url, uid, selector) {
            (Some(url), None, None) => Ok(Self::Url(url)),
            (None, Some(uid), None) => Ok(Self::Uid(uid)),
            (None, None, Some(selector)) => Ok(Self::Selector(selector)),
            (None, None, None) => Err("download: provide a URL, --uid, or --selector".into()),
            _ => Err("download: provide only one of a URL, --uid, or --selector".into()),
        }
    }

    const fn is_click(&self) -> bool {
        matches!(self, Self::Uid(_) | Self::Selector(_))
    }
}

/// What one `download` did, in the shape all three modes render.
///
/// Branch on `downloaded`, not `ok`: a delivered click is never an error, because the only
/// recovery an error invites is a second click the page cannot distinguish from a deliberate one.
pub struct Outcome {
    /// `"fetch"` or `"click"`, so a caller reading a failure knows which mechanism it is about.
    pub via: &'static str,
    pub downloaded: bool,
    pub path: Option<String>,
    pub bytes: Option<u64>,
    pub mime: Option<String>,
    /// What `Browser.downloadWillBegin` proposed, before sanitising. Absent on the fetch path.
    pub suggested_filename: Option<String>,
    /// The address Chrome actually pulled from — often a `blob:` URL absent from the DOM.
    pub source_url: Option<String>,
    /// Where bytes this command could not account for were left, rather than deleted. Set only
    /// on the evidence-lost path; it is not a file this command claims to have downloaded.
    pub kept: Option<String>,
    /// The click's own report: `delivery`, `uid`, `role`, `name`, `aim`, `intercepted_by`.
    pub click: Option<Value>,
    pub message: String,
    pub waited_ms: Option<u64>,
    pub hint: Option<String>,
}

impl Outcome {
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut obj = json!({"ok": true, "via": self.via, "downloaded": self.downloaded});
        let map = obj.as_object_mut().expect("literal object");
        if let Some(path) = &self.path {
            map.insert("path".into(), json!(path));
        }
        if let Some(bytes) = self.bytes {
            map.insert("bytes".into(), json!(bytes));
        }
        if let Some(mime) = &self.mime {
            map.insert("mime".into(), json!(mime));
        }
        if let Some(name) = &self.suggested_filename {
            map.insert("suggested_filename".into(), json!(name));
        }
        if let Some(url) = &self.source_url {
            map.insert("source_url".into(), json!(url));
        }
        if let Some(kept) = &self.kept {
            map.insert("kept".into(), json!(kept));
        }
        if let Some(ms) = self.waited_ms {
            map.insert("observed_after_ms".into(), json!(ms));
        }
        map.insert("message".into(), json!(self.message));
        if let Some(fields) = self.click.as_ref().and_then(Value::as_object) {
            for (key, value) in fields {
                // `download` carries no verdict, so `verdict_hint` would name a vocabulary this
                // response does not have. What it had to say is folded into `hint` below.
                if key != "verdict_hint" {
                    map.insert(key.clone(), value.clone());
                }
            }
        }
        if let Some(hint) = &self.hint {
            map.insert("hint".into(), json!(hint));
        }
        obj
    }

    /// Non-`--json` output: the path on its own line when there is one, so a shell pipeline
    /// keeps working; otherwise the message and its hint, on stderr.
    pub fn print_text(&self) {
        if self.downloaded {
            let bytes = self.bytes.unwrap_or(0);
            match &self.mime {
                Some(mime) => println!("{} ({bytes} bytes, {mime})", self.path.as_deref().unwrap_or("")),
                None => println!("{} ({bytes} bytes)", self.path.as_deref().unwrap_or("")),
            }
            return;
        }
        eprintln!("{}", self.message);
        if let Some(receiver) = self
            .click
            .as_ref()
            .and_then(|c| c.get("intercepted_by"))
            .and_then(|r| r.get("uid"))
            .and_then(Value::as_str)
        {
            eprintln!("received by: {receiver}");
        }
        if let Some(hint) = &self.hint {
            eprintln!("hint: {hint}");
        }
    }
}

/// Run one `download`, whichever way it was aimed. The single entry point for CLI, pipe and
/// batch, so the two mechanisms cannot drift into two response shapes.
pub async fn dispatch(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    target: &Target<'_>,
    out: Option<&str>,
    timeout_secs: u64,
    max_bytes: usize,
    on_intercept: crate::hit_test::OnIntercept,
    browser: &str,
) -> Result<Outcome, crate::BoxError> {
    if !target.is_click() {
        let Target::Url(url) = target else { unreachable!("guarded by is_click") };
        let result = run(client, url, out, timeout_secs, max_bytes).await?;
        return Ok(Outcome {
            via: "fetch",
            downloaded: true,
            message: format!("{} ({} bytes, {})", result.path, result.bytes, result.mime),
            path: Some(result.path),
            bytes: Some(result.bytes as u64),
            mime: Some(result.mime),
            suggested_filename: None,
            source_url: Some((*url).to_string()),
            kept: None,
            click: None,
            waited_ms: None,
            hint: None,
        });
    }
    click_download(client, uid_map, target, out, timeout_secs, max_bytes, on_intercept, browser)
        .await
}

/// The click half: arm, click, wait, place. The click is `element::click`'s — same hit test,
/// same `--on-intercept`, same refusal messages.
async fn click_download(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    target: &Target<'_>,
    out: Option<&str>,
    timeout_secs: u64,
    max_bytes: usize,
    on_intercept: crate::hit_test::OnIntercept,
    browser: &str,
) -> Result<Outcome, crate::BoxError> {
    // Before the click, and may fail the command: clicking unarmed forces a second click.
    let mut armed = super::download_click::arm(client).await?;

    let (dispatched, described) = match target {
        Target::Uid(uid) => (
            crate::element::click(client, uid_map, uid, on_intercept).await,
            format!("uid={uid}"),
        ),
        Target::Selector(selector) => (
            crate::element::click_selector(client, selector, on_intercept).await,
            format!("selector '{selector}'"),
        ),
        Target::Url(_) => unreachable!("guarded by is_click"),
    };
    let dispatched = match dispatched {
        Ok(dispatched) => dispatched,
        Err(error) => {
            let message = error.to_string();
            super::download_click::disarm(client).await;
            super::download_click::clean_up(&armed).await;
            return Err(message.into());
        }
    };

    // Nothing was sent (aim never settled, or `--on-intercept refuse`). The only branch where a
    // retry is safe: the page never saw an event.
    if !dispatched.sent {
        super::download_click::disarm(client).await;
        super::download_click::clean_up(&armed).await;
        let message = dispatched
            .refusal_message("click", &described)
            .unwrap_or_else(|| format!("Did not click {described}."));
        return Ok(Outcome {
            via: "click",
            downloaded: false,
            path: None,
            bytes: None,
            mime: None,
            suggested_filename: None,
            source_url: None,
            kept: None,
            hint: Some(crate::hints::undispatched_download_hint(browser)),
            click: Some(dispatched.report()),
            message,
            waited_ms: None,
        });
    }

    let transfer = super::download_click::collect(
        client,
        &mut armed,
        Duration::from_secs(timeout_secs),
        max_bytes as u64,
    )
    .await;
    super::download_click::disarm(client).await;
    // Move BEFORE the sweep, which deletes the directory the file is still in. The error is
    // reduced to a `String`: `BoxError` is not `Send` and would poison the await below.
    let placed = match &transfer {
        super::download_click::Transfer::Completed { began, temp_path, .. } => Some(
            super::download_click::place(temp_path, &began.suggested_filename, out)
                .map_err(|error| error.to_string()),
        ),
        _ => None,
    };
    super::download_click::clean_up(&armed).await;
    let placed = placed.transpose().map_err(|error| -> crate::BoxError { error.into() })?;
    Ok(settle(transfer, placed, &described, timeout_secs, max_bytes, browser, &dispatched))
}

/// Turn what Chrome reported, and where the file ended up, into the response.
fn settle(
    transfer: super::download_click::Transfer,
    placed: Option<(String, u64)>,
    described: &str,
    timeout_secs: u64,
    max_bytes: usize,
    browser: &str,
    dispatched: &crate::hit_test::Dispatched,
) -> Outcome {
    use super::download_click::{Cancelled, Transfer};

    let base = |downloaded: bool, message: String, hint: Option<String>| Outcome {
        via: "click",
        downloaded,
        path: None,
        bytes: None,
        mime: None,
        suggested_filename: None,
        source_url: None,
        kept: None,
        click: Some(dispatched.report()),
        message,
        waited_ms: None,
        hint,
    };

    match transfer {
        Transfer::Completed { began, bytes, .. } => {
            let (path, on_disk) = placed.expect("a completed transfer is placed by the caller");
            let message = format!("{path} ({on_disk} bytes, from clicking {described})");
            Outcome {
                downloaded: true,
                path: Some(path),
                bytes: Some(if on_disk > 0 { on_disk } else { bytes }),
                suggested_filename: Some(began.suggested_filename),
                source_url: Some(began.url),
                ..base(true, message, None)
            }
        }
        Transfer::NeverBegan { waited_ms } => Outcome {
            waited_ms: Some(waited_ms),
            ..base(
                false,
                format!(
                    "Clicked {described} and no download began in the {timeout_secs}s that \
                     followed, so nothing was written."
                ),
                Some(crate::hints::no_download_hint(browser, timeout_secs, dispatched)),
            )
        },
        Transfer::EvidenceLost { began, dropped, kept, waited_ms } => {
            // Never `downloaded: true` — no `completed` was seen, and a file on disk is not a
            // finished one. Never "no download began" either: that is the claim the drops made
            // unavailable. The response says what was lost and where the bytes are.
            let subject = began.as_ref().map_or_else(
                || "whether a download began cannot be told".to_string(),
                |began| {
                    format!(
                        "what became of the download it started ({}) cannot be told",
                        began.suggested_filename
                    )
                },
            );
            let where_ = kept.as_ref().map_or_else(
                || " Nothing had been written into the transfer directory.".to_string(),
                |path| format!(" What Chrome had written was kept at {}.", path.display()),
            );
            let message = format!(
                "Clicked {described} and {dropped} CDP event(s) were dropped before this command \
                 could read them, so {subject}.{where_}"
            );
            Outcome {
                waited_ms: Some(waited_ms),
                suggested_filename: began.as_ref().map(|b| b.suggested_filename.clone()),
                source_url: began.map(|b| b.url),
                kept: kept.as_ref().map(|path| path.display().to_string()),
                ..base(false, message, Some(evidence_lost_hint(kept.as_ref())))
            }
        }
        Transfer::Canceled { began, why } => {
            let message = match why {
                Cancelled::ExceededCap => format!(
                    "Clicked {described} and the download it started ({}) exceeded {max_bytes} \
                     bytes, so it was cancelled and nothing was written.",
                    began.suggested_filename
                ),
                Cancelled::ByBrowser => format!(
                    "Clicked {described} and Chrome cancelled the download it started ({}), so \
                     nothing was written.",
                    began.suggested_filename
                ),
            };
            let hint = match why {
                Cancelled::ExceededCap => Some(crate::hints::download_cap_hint(browser, max_bytes)),
                Cancelled::ByBrowser => Some(crate::hints::download_cancelled_hint(browser)),
            };
            Outcome {
                suggested_filename: Some(began.suggested_filename),
                source_url: Some(began.url),
                ..base(false, message, hint)
            }
        }
        Transfer::Unfinished { began, received, total, waited_ms } => {
            let message = format!(
                "Clicked {described} and the download it started ({}) had {received} of {total} \
                 bytes after {timeout_secs}s, so it is incomplete and nothing was written.",
                began.suggested_filename
            );
            Outcome {
                waited_ms: Some(waited_ms),
                suggested_filename: Some(began.suggested_filename),
                source_url: Some(began.url),
                ..base(false, message, Some(crate::hints::download_unfinished_hint(browser)))
            }
        }
    }
}

/// The recovery when the events that would have answered were dropped.
///
/// Built here rather than in `hints`, because the one resolved thing it can name is a path this
/// invocation just created. It holds to the same three rules: one fact, one thing to do, and an
/// explicit refusal of the retry — the click already reached the page.
fn evidence_lost_hint(kept: Option<&PathBuf>) -> String {
    let what = kept.map_or_else(
        || {
            "Nothing reached the transfer directory, which is what an unstarted download looks \
             like, though the dropped events are why that cannot be stated as a fact."
                .to_string()
        },
        |path| {
            format!(
                "Read {} — Chrome wrote it, and chrome-agent cannot say whether it is the whole \
                 file, so check it against the source before using it.",
                path.display()
            )
        },
    );
    format!(
        "{what} Do not click again: the first click reached the page, and the page has no way to \
         tell a retry from a second deliberate action."
    )
}

/// Fetch `url` inside the page so the request inherits its cookies, then decode the base64 and
/// write it at 0600. Click-triggered downloads go through `click_download` instead.
pub async fn run(
    client: &CdpClient,
    url: &str,
    out: Option<&str>,
    timeout_secs: u64,
    max_bytes: usize,
) -> Result<DownloadResult, crate::BoxError> {
    if max_bytes == 0 {
        return Err("download: max_bytes must be greater than zero".into());
    }
    let url_lit = serde_json::to_string(url)?;
    let js = format!(
        r"(async () => {{
            const res = await fetch({url_lit}, {{ credentials: 'include' }});
            if (!res.ok) throw new Error('HTTP ' + res.status + ' fetching ' + {url_lit});
            const maxBytes = {max_bytes};
            const lengthHeader = res.headers.get('content-length');
            if (lengthHeader !== null) {{
                const announced = Number(lengthHeader);
                if (Number.isFinite(announced) && announced > maxBytes) {{
                    if (res.body) await res.body.cancel();
                    throw new Error('download exceeded ' + maxBytes + ' bytes; raise --max-bytes to allow it');
                }}
            }}

            const chunks = [];
            let total = 0;
            if (res.body) {{
                const reader = res.body.getReader();
                while (true) {{
                    const {{ done, value }} = await reader.read();
                    if (done) break;
                    total += value.byteLength;
                    if (total > maxBytes) {{
                        await reader.cancel();
                        throw new Error('download exceeded ' + maxBytes + ' bytes; raise --max-bytes to allow it');
                    }}
                    chunks.push(value);
                }}
            }} else {{
                // No readable stream (some cached/opaque responses): bounded buffered read,
                // so content is not silently dropped as an empty download.
                const fallback = new Uint8Array(await res.arrayBuffer());
                if (fallback.byteLength > maxBytes) {{
                    throw new Error('download exceeded ' + maxBytes + ' bytes; raise --max-bytes to allow it');
                }}
                total = fallback.byteLength;
                chunks.push(fallback);
            }}

            const buf = new Uint8Array(total);
            let offset = 0;
            for (const chunk of chunks) {{
                buf.set(chunk, offset);
                offset += chunk.byteLength;
            }}
            let bin = '';
            const CHUNK = 0x8000;
            for (let i = 0; i < buf.length; i += CHUNK) {{
                bin += String.fromCharCode.apply(null, buf.subarray(i, i + CHUNK));
            }}
            return {{
                data: btoa(bin),
                mime: res.headers.get('content-type') || '',
                cd: res.headers.get('content-disposition') || '',
                bytes: total,
            }};
        }})()"
    );

    // Two failures, two sentences. Only the outer one is a timeout, and only a timeout invites
    // raising `--timeout`; a CDP error (an unreadable answer, a closed transport) is reported as
    // itself, named with the URL it was fetching.
    let eval: EvaluateResult = match tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        client.call(
            "Runtime.evaluate",
            json!({ "expression": js, "returnByValue": true, "awaitPromise": true }),
        ),
    )
    .await
    {
        Ok(Ok(eval)) => eval,
        Ok(Err(error)) => {
            return Err(format!("download failed fetching {url}: {error}").into())
        }
        Err(_elapsed) => {
            return Err(format!("download timed out after {timeout_secs}s fetching {url}").into())
        }
    };

    if let Some(exc) = eval.exception_details {
        let detail = exc
            .exception
            .as_ref()
            .and_then(|exception| exception.description.as_deref())
            .unwrap_or(&exc.text);
        return Err(format!("download failed: {detail}").into());
    }

    let obj = eval.result.value.ok_or("download: page returned no data")?;
    let data = obj.get("data").and_then(|v| v.as_str()).ok_or("download: missing data")?;
    let mime = obj.get("mime").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let cd = obj.get("cd").and_then(|v| v.as_str()).unwrap_or("");
    let reported_bytes = obj
        .get("bytes")
        .and_then(serde_json::Value::as_u64)
        .ok_or("download: missing byte count")?;
    let reported_bytes = usize::try_from(reported_bytes)
        .map_err(|_| "download: byte count exceeds platform limits")?;

    if reported_bytes > max_bytes {
        return Err(format!("download exceeded {max_bytes} bytes; raise --max-bytes to allow it").into());
    }

    let bytes = crate::base64::decode(data)?;
    if bytes.len() != reported_bytes {
        return Err(format!(
            "download: decoded byte count mismatch (reported {reported_bytes}, decoded {})",
            bytes.len()
        )
        .into());
    }
    if bytes.len() > max_bytes {
        return Err(format!("download exceeded {max_bytes} bytes; raise --max-bytes to allow it").into());
    }

    let path = resolve_out_path(out, cd, url)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, &bytes)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(DownloadResult {
        path: path.display().to_string(),
        bytes: bytes.len(),
        mime,
    })
}

/// `--out` verbatim; otherwise Content-Disposition, then the URL, then `"download"`, placed
/// under `~/.chrome-agent/tmp`.
fn resolve_out_path(out: Option<&str>, content_disposition: &str, url: &str) -> Result<PathBuf, crate::BoxError> {
    if let Some(o) = out {
        return Ok(PathBuf::from(o));
    }
    let name = filename_from_content_disposition(content_disposition)
        .unwrap_or_else(|| filename_from_url(url));
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    Ok(home.join(".chrome-agent").join("tmp").join(name))
}

/// Where a click-triggered download lands: `--out` verbatim, otherwise Chrome's suggested name
/// under `~/.chrome-agent/tmp`. The suggestion is server-supplied, so it is sanitised — a
/// proposed `../../.ssh/authorized_keys` must not escape the download directory.
pub fn resolve_named_path(
    out: Option<&str>,
    suggested: &str,
) -> Result<PathBuf, crate::BoxError> {
    if let Some(o) = out {
        return Ok(PathBuf::from(o));
    }
    let cleaned = sanitize_name(suggested);
    let name = if cleaned.is_empty() { "download".to_string() } else { cleaned };
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    Ok(home.join(".chrome-agent").join("tmp").join(name))
}

/// Filename from a URL's last path segment (query/fragment stripped). Falls back to
/// `"download"` for a host-only URL; the host is never used as a filename.
#[must_use]
pub fn filename_from_url(url: &str) -> String {
    let no_query = url.split(['?', '#']).next().unwrap_or(url);
    // Drop the scheme so the host isn't mistaken for a path segment.
    let after_scheme = no_query.split_once("://").map_or(no_query, |(_, rest)| rest);
    let path = after_scheme.split_once('/').map_or("", |(_, p)| p);
    let last = path.trim_end_matches('/').rsplit('/').next().unwrap_or("").trim();
    if last.is_empty() {
        "download".to_string()
    } else {
        sanitize_name(last)
    }
}

/// Filename from a `Content-Disposition` value: `filename="x"`, `filename=x`, or RFC 5987
/// `filename*=UTF-8''x`. Percent escapes are kept literal — decoding `%2f` would break the
/// path-traversal guarantee.
#[must_use]
pub fn filename_from_content_disposition(header: &str) -> Option<String> {
    let lower = header.to_ascii_lowercase();
    // Prefer the extended form when present.
    if let Some(pos) = lower.find("filename*=") {
        let raw = &header[pos + "filename*=".len()..];
        let value = raw.split(';').next().unwrap_or(raw).trim();
        // `filename*=UTF-8''name.pdf` → the part after the last "''".
        let name = value.rsplit("''").next().unwrap_or(value).trim_matches('"');
        let cleaned = sanitize_name(name);
        if !cleaned.is_empty() {
            return Some(cleaned);
        }
    }
    if let Some(pos) = lower.find("filename=") {
        let raw = &header[pos + "filename=".len()..];
        let value = raw.split(';').next().unwrap_or(raw).trim().trim_matches('"');
        let cleaned = sanitize_name(value);
        if !cleaned.is_empty() {
            return Some(cleaned);
        }
    }
    None
}

/// Strip any directory component so a server-supplied name can't traverse paths.
fn sanitize_name(name: &str) -> String {
    Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::download_click::{Began, Transfer};

    fn lost(began: Option<Began>, kept: Option<PathBuf>) -> Outcome {
        settle(
            Transfer::EvidenceLost { began, dropped: 300, kept, waited_ms: 1200 },
            None,
            "selector '#export'",
            5,
            64,
            "b1",
            &crate::hit_test::Dispatched::js(),
        )
    }

    /// The whole point of the outcome: a download whose evidence was dropped must not be
    /// reported with the sentence a download that never started gets. The old loop discarded
    /// `Lagged`, `began` stayed `None`, and a completed file was announced as "no download
    /// began … nothing was written" — one word before the cleanup deleted it.
    #[test]
    fn a_download_whose_events_were_dropped_is_never_reported_as_one_that_never_began() {
        let outcome = lost(None, Some(PathBuf::from("/tmp/kept-1-2")));

        assert!(!outcome.downloaded, "no `completed` was seen, so nothing may be claimed");
        assert!(
            !outcome.message.contains("no download began"),
            "the claim the drops made unavailable: {}",
            outcome.message
        );
        assert!(outcome.message.contains("300 CDP event(s) were dropped"), "{}", outcome.message);
        assert!(outcome.message.contains("kept at /tmp/kept-1-2"), "{}", outcome.message);
        assert!(outcome.path.is_none(), "no file is claimed either: a success is not invented");

        let json = outcome.to_json();
        assert_eq!(json["ok"], true, "the click was delivered, so this is not an error");
        assert_eq!(json["downloaded"], false, "{json}");
        assert_eq!(json["kept"], "/tmp/kept-1-2", "the bytes are addressable: {json}");
        assert_eq!(json["observed_after_ms"], 1200, "{json}");

        let hint = outcome.hint.expect("a hint");
        assert!(hint.contains("/tmp/kept-1-2"), "the hint names the one thing to read: {hint}");
        assert!(hint.contains("Do not click again"), "rule 3, in words: {hint}");
    }

    /// With the directory empty, the honest reading is still "unproven", not "nothing began":
    /// the events that would have said so are the ones that were lost.
    #[test]
    fn an_empty_transfer_directory_after_a_drop_is_unproven_and_not_a_denial() {
        let outcome = lost(None, None);
        assert!(!outcome.message.contains("no download began"), "{}", outcome.message);
        assert!(outcome.message.contains("cannot be told"), "{}", outcome.message);
        assert!(outcome.kept.is_none());
        assert!(outcome.to_json().get("kept").is_none(), "no path is invented");
        assert!(
            outcome.hint.expect("a hint").contains("cannot be stated as a fact"),
            "the absence is described as unproven, not asserted"
        );
    }

    /// What was known before the drops still travels: the file Chrome named and where it came
    /// from are facts, and only the ending is missing.
    #[test]
    fn a_drop_after_the_download_began_keeps_what_was_already_known() {
        let began = Began {
            guid: "g-1".into(),
            suggested_filename: "report.csv".into(),
            url: "blob:null/x".into(),
        };
        let outcome = lost(Some(began), None);
        assert_eq!(outcome.suggested_filename.as_deref(), Some("report.csv"));
        assert_eq!(outcome.source_url.as_deref(), Some("blob:null/x"));
        assert!(outcome.message.contains("report.csv"), "{}", outcome.message);
        assert!(
            !outcome.message.contains("incomplete"),
            "a lost `completed` is not proof of an unfinished transfer: {}",
            outcome.message
        );
    }

    /// The control: with nothing dropped, the four outcomes still say what they said.
    #[test]
    fn a_wait_that_dropped_nothing_still_answers_plainly() {
        let outcome = settle(
            Transfer::NeverBegan { waited_ms: 2000 },
            None,
            "selector '#inert'",
            5,
            64,
            "b1",
            &crate::hit_test::Dispatched::js(),
        );
        assert!(!outcome.downloaded);
        assert!(outcome.message.contains("no download began"), "{}", outcome.message);
        assert!(outcome.kept.is_none());
    }

    #[test]
    fn url_filename_basic() {
        assert_eq!(filename_from_url("https://x.com/files/report.pdf"), "report.pdf");
    }

    #[test]
    fn url_filename_strips_query_and_fragment() {
        assert_eq!(filename_from_url("https://x.com/a/b/data.csv?v=2&x=1"), "data.csv");
        assert_eq!(filename_from_url("https://x.com/a/img.png#frag"), "img.png");
    }

    #[test]
    fn url_filename_trailing_slash_falls_back() {
        assert_eq!(filename_from_url("https://x.com/"), "download");
        assert_eq!(filename_from_url("https://x.com/dir/"), "dir");
    }

    #[test]
    fn url_filename_cannot_traverse() {
        // A crafted path segment must not escape the download dir.
        let n = filename_from_url("https://x.com/%2e%2e/etc/passwd");
        assert!(!n.contains('/'));
        assert_eq!(n, "passwd");
    }

    #[test]
    fn cd_quoted_filename() {
        assert_eq!(
            filename_from_content_disposition("attachment; filename=\"invoice 2024.pdf\""),
            Some("invoice 2024.pdf".to_string())
        );
    }

    #[test]
    fn cd_unquoted_filename() {
        assert_eq!(
            filename_from_content_disposition("attachment; filename=report.csv"),
            Some("report.csv".to_string())
        );
    }

    #[test]
    fn cd_extended_filename_preferred() {
        assert_eq!(
            filename_from_content_disposition("attachment; filename=\"fallback.bin\"; filename*=UTF-8''real.pdf"),
            Some("real.pdf".to_string())
        );
    }

    #[test]
    fn cd_filename_strips_path() {
        assert_eq!(
            filename_from_content_disposition("attachment; filename=\"../../etc/passwd\""),
            Some("passwd".to_string())
        );
    }

    #[test]
    fn cd_no_filename_returns_none() {
        assert_eq!(filename_from_content_disposition("inline"), None);
        assert_eq!(filename_from_content_disposition(""), None);
    }

    #[test]
    fn cd_key_is_case_insensitive() {
        assert_eq!(
            filename_from_content_disposition("attachment; FileName=report.csv"),
            Some("report.csv".to_string())
        );
        assert_eq!(
            filename_from_content_disposition("attachment; FILENAME*=UTF-8''real.pdf"),
            Some("real.pdf".to_string())
        );
    }

    #[test]
    fn cd_empty_extended_falls_through_to_plain() {
        // `filename*` present but empty → falls back to plain `filename=`.
        assert_eq!(
            filename_from_content_disposition("attachment; filename*=UTF-8''; filename=plain.bin"),
            Some("plain.bin".to_string())
        );
    }

    #[test]
    fn cd_preserves_percent_escapes_literally() {
        // No percent-decoding: `%2f` must not become '/'.
        let n = filename_from_content_disposition("attachment; filename*=UTF-8''a%2fb.pdf").unwrap();
        assert_eq!(n, "a%2fb.pdf");
        assert!(!n.contains('/'));
    }

    #[test]
    fn url_filename_host_only_no_slash() {
        assert_eq!(filename_from_url("https://x.com"), "download");
    }

    #[test]
    fn resolve_out_honours_explicit_path() {
        let p = resolve_out_path(Some("/tmp/mine.bin"), "", "https://x/y.pdf").unwrap();
        assert_eq!(p, PathBuf::from("/tmp/mine.bin"));
    }

    #[test]
    fn resolve_out_prefers_cd_over_url() {
        let p = resolve_out_path(None, "attachment; filename=from-cd.pdf", "https://x/from-url.pdf").unwrap();
        assert!(p.ends_with("from-cd.pdf"));
    }

    #[test]
    fn resolve_out_falls_back_to_url() {
        let p = resolve_out_path(None, "inline", "https://x/from-url.pdf").unwrap();
        assert!(p.ends_with("from-url.pdf"));
    }
}
