use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::cdp::types::EvaluateResult;

pub struct DownloadResult {
    pub path: String,
    pub bytes: usize,
    pub mime: String,
}

/// Default in-page download limit: 64 MiB.
pub const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;

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
