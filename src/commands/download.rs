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

/// What the caller pointed `download` at.
///
/// Two mechanisms, one verb, because the contract is the same in both: a file at a named path,
/// 0600, with its size and the name the server proposed. What differs is how the bytes are
/// reached — an address the caller already has, or an element only a click can persuade.
///
/// Exactly one of the three, refused rather than ranked: a `download` carrying both a URL and a
/// selector is a caller who has not decided, and picking for them would silently ignore half the
/// invocation.
pub enum Target<'a> {
    /// Fetch this URL inside the page. Auth-preserving, and the only path that needs no click.
    Url(&'a str),
    /// Click this uid and capture whatever download it produces.
    Uid(&'a str),
    /// Click the element this selector resolves to, and capture whatever download it produces.
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
/// `downloaded` is the field to branch on, and it is deliberately not `ok`. A click that was
/// delivered is not an error whatever it failed to produce: the only recovery an error invites is
/// a second click, and the page cannot tell that from a second deliberate action. This is the
/// same shape `fill` uses when the page throws the write away — `ok:true` beside a field that
/// denies the requested state — and for the same reason.
pub struct Outcome {
    /// `"fetch"` or `"click"`. On the response so a caller reading a failure knows which of the
    /// two mechanisms it is about.
    pub via: &'static str,
    pub downloaded: bool,
    pub path: Option<String>,
    pub bytes: Option<u64>,
    pub mime: Option<String>,
    /// What `Browser.downloadWillBegin` proposed, before sanitising. Absent on the fetch path,
    /// where the equivalent already went through `Content-Disposition`.
    pub suggested_filename: Option<String>,
    /// The address Chrome actually pulled from — often a `blob:` URL that exists nowhere in the
    /// DOM, which is exactly the case `download <url>` cannot reach.
    pub source_url: Option<String>,
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
        if let Some(ms) = self.waited_ms {
            map.insert("observed_after_ms".into(), json!(ms));
        }
        map.insert("message".into(), json!(self.message));
        if let Some(fields) = self.click.as_ref().and_then(Value::as_object) {
            for (key, value) in fields {
                // `verdict_hint` belongs to the verdict vocabulary, and `download` carries no
                // verdict (see the design note): it would name a word that is not on the
                // response. Whatever it had to say is folded into `hint` below.
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

    /// The default (non-`--json`) output: the path on its own line when there is one, so a shell
    /// pipeline keeps working, and the news plus its hint when there is not.
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

/// Run one `download`, whichever way it was aimed. The single entry point the CLI, pipe and batch
/// all go through, so the two mechanisms cannot drift into two response shapes.
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
            click: None,
            waited_ms: None,
            hint: None,
        });
    }
    click_download(client, uid_map, target, out, timeout_secs, max_bytes, on_intercept, browser)
        .await
}

/// The click half: arm, click, wait, place.
///
/// The click is the one that already exists — same hit test, same `--on-intercept`, same refusal
/// messages — so a covered button behaves here exactly as it does under `click`. What is added
/// around it is the arming and the bounded wait.
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
    // Before the click, and it may fail the command: dispatching a click whose product cannot be
    // captured is worse than not clicking, because the caller then has to click a second time.
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

    // Nothing was sent — the aim never settled, or `--on-intercept refuse` stopped it. There is
    // no download to wait for and, unlike every other branch here, a retry after clearing the
    // obstruction is safe, because this click never reached the page.
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
    // The move happens BEFORE the sweep, because the sweep deletes the directory the file is
    // still in — and its error is reduced to a `String` here rather than carried as a
    // `BoxError`, which is not `Send` and would make every dispatcher's future non-`Send` the
    // moment it were held across the await below.
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

/// Download `url` by fetching it inside the page, so the request inherits the
/// page's cookies/session (auth-preserving). The bytes are returned as base64,
/// decoded, and written to disk.
///
/// Note: click-triggered/browser-native downloads are not handled here — resolve
/// the target href (e.g. `inspect --urls`) and pass it as the URL.
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
                // No readable stream exposed (e.g. some cached/opaque responses):
                // fall back to a bounded buffered read so content is not silently
                // dropped as an empty download.
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

    let eval: EvaluateResult = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        client.call(
            "Runtime.evaluate",
            json!({ "expression": js, "returnByValue": true, "awaitPromise": true }),
        ),
    )
    .await
    .map_err(|_| format!("download timed out after {timeout_secs}s fetching {url}"))??;

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

/// Resolve the destination path. `--out` (if given) is honoured verbatim as a
/// user-chosen path; otherwise the name is derived from the Content-Disposition
/// header, then the URL, then a fallback, and placed under `~/.chrome-agent/tmp`.
fn resolve_out_path(out: Option<&str>, content_disposition: &str, url: &str) -> Result<PathBuf, crate::BoxError> {
    if let Some(o) = out {
        return Ok(PathBuf::from(o));
    }
    let name = filename_from_content_disposition(content_disposition)
        .unwrap_or_else(|| filename_from_url(url));
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    Ok(home.join(".chrome-agent").join("tmp").join(name))
}

/// Where a click-triggered download lands: `--out` verbatim, otherwise the name Chrome's
/// `downloadWillBegin` proposed, under `~/.chrome-agent/tmp`.
///
/// The suggested name is a server-supplied string like a `Content-Disposition` filename, so it
/// goes through the same `sanitize_name` — a `../../.ssh/authorized_keys` proposed by a page must
/// not escape the download directory.
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

/// Derive a filename from a URL's last path segment (query/fragment stripped).
///
/// Falls back to `"download"` when the URL has no path (host-only) or ends in a
/// slash — the host is never used as a filename.
#[must_use]
pub fn filename_from_url(url: &str) -> String {
    let no_query = url.split(['?', '#']).next().unwrap_or(url);
    // Drop the scheme so the host isn't mistaken for a path segment.
    let after_scheme = no_query.split_once("://").map_or(no_query, |(_, rest)| rest);
    // Everything after the first '/' is the path; host-only URLs have none.
    let path = after_scheme.split_once('/').map_or("", |(_, p)| p);
    let last = path.trim_end_matches('/').rsplit('/').next().unwrap_or("").trim();
    if last.is_empty() {
        "download".to_string()
    } else {
        sanitize_name(last)
    }
}

/// Extract a filename from a `Content-Disposition` header value.
///
/// Handles `filename="x"`, `filename=x`, and RFC 5987 `filename*=UTF-8''x`
/// (percent-decoding left to the caller's OS since names are typically ASCII).
#[must_use]
pub fn filename_from_content_disposition(header: &str) -> Option<String> {
    let lower = header.to_ascii_lowercase();
    // Prefer the extended form when present.
    if let Some(pos) = lower.find("filename*=") {
        let raw = &header[pos + "filename*=".len()..];
        let value = raw.split(';').next().unwrap_or(raw).trim();
        // filename*=UTF-8''actual%20name.pdf → take the part after the last "''".
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
        // Real-world headers vary in case; the key match must not be case-sensitive.
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
        // filename* present but empty → must fall back to the plain filename=.
        assert_eq!(
            filename_from_content_disposition("attachment; filename*=UTF-8''; filename=plain.bin"),
            Some("plain.bin".to_string())
        );
    }

    #[test]
    fn cd_preserves_percent_escapes_literally() {
        // Contract: no percent-decoding — %2f must NOT become '/', or the
        // path-traversal guarantee would break. It stays a literal segment.
        let n = filename_from_content_disposition("attachment; filename*=UTF-8''a%2fb.pdf").unwrap();
        assert_eq!(n, "a%2fb.pdf");
        assert!(!n.contains('/'));
    }

    #[test]
    fn url_filename_host_only_no_slash() {
        // Exercises the split_once('/')→None branch (distinct from trailing-slash).
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
