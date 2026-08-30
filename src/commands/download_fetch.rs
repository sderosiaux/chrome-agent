//! The download a URL produces: the in-page `fetch`, the base64 that carries it over CDP, the
//! caps that ceiling is derived from, and the 0600 write.
//!
//! Also the naming both halves of `download` share, because every candidate name here is
//! server-supplied and has to pass the same path-traversal guard before it becomes a file.

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

/// Default in-page download limit: 64 MiB. Also `cli.rs`'s `--max-bytes` default.
pub const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;

/// Room left for everything in the CDP reply that is not the base64 payload: the envelope,
/// `mime` and `cd` (both server-supplied header values, hence generous rather than exact).
const ENVELOPE_RESERVE: usize = 64 * 1024;

/// The most the URL path can fetch, derived from what the wire will carry.
///
/// `run` returns the file base64-encoded inside one `Runtime.evaluate` reply, so a download is
/// 4/3 of its size on the wire. Advertising a cap the transport then refuses turns a bounded
/// "too big" into a dead connection, so the two numbers are one expression rather than two
/// choices: `MAX_MESSAGE_BYTES` is what the socket reads, this is what fits in it.
///
/// 96 MiB minus the reserve, times 3/4 → **75,448,320 bytes (71.95 MiB)**.
pub const MAX_FETCH_BYTES: usize =
    (crate::cdp::transport::MAX_MESSAGE_BYTES - ENVELOPE_RESERVE) / 4 * 3;

/// The tie, enforced by the compiler: the default this tool advertises has to fit through the
/// socket it will arrive on. Moving either constant without the other fails the build.
const _: () = assert!(DEFAULT_MAX_BYTES <= MAX_FETCH_BYTES);

/// Fetch `url` inside the page so the request inherits its cookies, then decode the base64 and
/// write it at 0600. Click-triggered downloads go through `download::dispatch` instead.
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
    // Refused here rather than discovered on the wire: past this the reply cannot be read, and
    // the connection dies without saying what size would have worked. The click path streams
    // to disk through Chrome and is not bound by this — only the in-page fetch is.
    if max_bytes > MAX_FETCH_BYTES {
        return Err(format!(
            "download: --max-bytes {max_bytes} is above the {MAX_FETCH_BYTES}-byte ceiling the \
             URL path can carry. It returns the file base64-encoded over CDP, which is 4/3 of \
             its size, and the connection reads at most {} bytes per message. Click the link \
             instead (download --uid or --selector), which streams to disk through Chrome and \
             is not bound by this.",
            crate::cdp::transport::MAX_MESSAGE_BYTES
        )
        .into());
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
        Ok(Err(error)) => return Err(format!("download failed fetching {url}: {error}").into()),
        Err(_elapsed) => {
            return Err(format!("download timed out after {timeout_secs}s fetching {url}").into());
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
    let data = obj
        .get("data")
        .and_then(|v| v.as_str())
        .ok_or("download: missing data")?;
    let mime = obj
        .get("mime")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let cd = obj.get("cd").and_then(|v| v.as_str()).unwrap_or("");
    let reported_bytes = obj
        .get("bytes")
        .and_then(serde_json::Value::as_u64)
        .ok_or("download: missing byte count")?;
    let reported_bytes = usize::try_from(reported_bytes)
        .map_err(|_| "download: byte count exceeds platform limits")?;

    if reported_bytes > max_bytes {
        return Err(
            format!("download exceeded {max_bytes} bytes; raise --max-bytes to allow it").into(),
        );
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
        return Err(
            format!("download exceeded {max_bytes} bytes; raise --max-bytes to allow it").into(),
        );
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
fn resolve_out_path(
    out: Option<&str>,
    content_disposition: &str,
    url: &str,
) -> Result<PathBuf, crate::BoxError> {
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
pub fn resolve_named_path(out: Option<&str>, suggested: &str) -> Result<PathBuf, crate::BoxError> {
    if let Some(o) = out {
        return Ok(PathBuf::from(o));
    }
    let cleaned = sanitize_name(suggested);
    let name = if cleaned.is_empty() {
        "download".to_string()
    } else {
        cleaned
    };
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    Ok(home.join(".chrome-agent").join("tmp").join(name))
}

/// Filename from a URL's last path segment (query/fragment stripped). Falls back to
/// `"download"` for a host-only URL; the host is never used as a filename.
#[must_use]
pub fn filename_from_url(url: &str) -> String {
    let no_query = url.split(['?', '#']).next().unwrap_or(url);
    // Drop the scheme so the host isn't mistaken for a path segment.
    let after_scheme = no_query
        .split_once("://")
        .map_or(no_query, |(_, rest)| rest);
    let path = after_scheme.split_once('/').map_or("", |(_, p)| p);
    let last = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim();
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
        let value = raw
            .split(';')
            .next()
            .unwrap_or(raw)
            .trim()
            .trim_matches('"');
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

    /// The cap the URL path advertises and the cap the socket enforces used to be two numbers
    /// chosen apart: 64 MiB of file is 85.3 MiB base64, which the transport refused — as a
    /// dead connection, not as a size. They are one expression now, and this pins the arithmetic
    /// rather than the constant, so raising the wire limit moves the ceiling with it.
    #[test]
    fn the_fetch_ceiling_is_what_the_wire_will_actually_carry() {
        // 4/3 of the ceiling, plus the reserve, still fits one message.
        let on_the_wire = MAX_FETCH_BYTES.div_ceil(3) * 4;
        assert!(
            on_the_wire + ENVELOPE_RESERVE <= crate::cdp::transport::MAX_MESSAGE_BYTES,
            "{MAX_FETCH_BYTES} bytes base64-encode to {on_the_wire}, past the wire limit"
        );
        // That the advertised default fits inside the ceiling is the `const` assertion beside
        // the constants, which fails the build rather than a test run.
        assert_eq!(
            MAX_FETCH_BYTES, 75_448_320,
            "the stated cap moved without its docs"
        );
    }

    #[test]
    fn url_filename_basic() {
        assert_eq!(
            filename_from_url("https://x.com/files/report.pdf"),
            "report.pdf"
        );
    }

    #[test]
    fn url_filename_strips_query_and_fragment() {
        assert_eq!(
            filename_from_url("https://x.com/a/b/data.csv?v=2&x=1"),
            "data.csv"
        );
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
            filename_from_content_disposition(
                "attachment; filename=\"fallback.bin\"; filename*=UTF-8''real.pdf"
            ),
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
        let n =
            filename_from_content_disposition("attachment; filename*=UTF-8''a%2fb.pdf").unwrap();
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
        let p = resolve_out_path(
            None,
            "attachment; filename=from-cd.pdf",
            "https://x/from-url.pdf",
        )
        .unwrap();
        assert!(p.ends_with("from-cd.pdf"));
    }

    #[test]
    fn resolve_out_falls_back_to_url() {
        let p = resolve_out_path(None, "inline", "https://x/from-url.pdf").unwrap();
        assert!(p.ends_with("from-url.pdf"));
    }
}
