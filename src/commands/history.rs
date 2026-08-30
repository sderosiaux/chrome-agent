use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub ts: u64,
    pub url: String,
    pub title: String,
    pub page: String,
}

/// Entries kept when the file is rotated. At ~150 bytes an entry that is ~300 KB of history,
/// which `run` can still read whole.
const KEEP_ENTRIES: usize = 2000;

/// Size at which a rotation happens. Checked with one `metadata` call per navigation, so the
/// ordinary append pays a stat and nothing else; the rewrite happens once per ~2000 entries.
const ROTATE_BYTES: u64 = 512 * 1024;

fn history_path() -> Result<PathBuf, crate::BoxError> {
    dirs::home_dir()
        .map(|h| h.join(".chrome-agent").join("history.jsonl"))
        .ok_or_else(|| "Could not determine home directory".into())
}

/// chmod 0600, like `record::restrict` and every other file this tool writes. Applied on every
/// append, since the file may already exist wider from a run that predates this.
/// The store directory, 0700 at creation. `session::save_to` sets the same mode, but a first run
/// whose command fails before the save never reaches it and left the directory at 0755.
fn ensure_dir(parent: &std::path::Path) -> Result<(), crate::BoxError> {
    crate::secure_fs::create_private_dir_all(parent)?;
    Ok(())
}

/// What of a URL is written down: everything up to the first `?` or `#`.
///
/// A query string is where the single-use credentials live — an OAuth `?code=`, a password-reset
/// token, a pre-signed S3 signature — and this file is permanent, unlike the session store, which
/// prunes. The path is what makes an entry recognisable (`/oauth/callback`, `/reset`), so history
/// stays useful for "where did this agent go"; it stops being usable for replaying a navigation,
/// which is what `macro`/`replay` are for. A stripped URL keeps a trailing marker so an entry
/// reads as truncated rather than as a bare path that never had one.
#[must_use]
pub fn without_credentials(url: &str) -> String {
    match url.find(['?', '#']) {
        Some(cut) => format!("{}\u{2026}", &url[..cut]),
        None => url.to_string(),
    }
}

/// Append a navigation entry to `~/.chrome-agent/history.jsonl`, 0600, query string stripped.
///
/// Rotation and append happen under one exclusive lock, the same `session::FileLock` the store
/// takes. This file is shared by every browser on the machine, and rotation replaces it by
/// rename: without the lock an append made during another process's rewrite lands in a file that
/// is then renamed over, and the navigation is simply missing. Reproduced by the test suite
/// itself once `history.jsonl` grew past [`ROTATE_BYTES`] — one navigation in a parallel run
/// vanished. Best effort, like every other branch here: a lock we cannot take is not a reason to
/// drop the entry, so the append still happens.
pub fn append(url: &str, title: &str, page: &str) -> Result<(), crate::BoxError> {
    let path = history_path()?;
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    let _lock = crate::session::FileLock::acquire(&path.with_extension("lock")).ok();
    rotate_if_large(&path);

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());

    let entry = json!({
        "ts": ts,
        "url": without_credentials(url),
        "title": title,
        "page": page,
    });

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    crate::secure_fs::restrict_file(&path)?;
    writeln!(file, "{}", serde_json::to_string(&entry)?)?;
    Ok(())
}

/// Keep the newest [`KEEP_ENTRIES`] lines once the file passes [`ROTATE_BYTES`]. Best effort in
/// every branch: an unbounded history is the defect, and failing the navigation that noticed it
/// would be worse than one oversized file.
///
/// Written to a per-pid temp file and renamed, the same shape `session::save_to` uses, so a
/// reader never sees a half-written file. Called under [`append`]'s lock, which is what stops a
/// concurrent append from landing in the copy this then renames over.
fn rotate_if_large(path: &std::path::Path) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return; // no file yet
    };
    if metadata.len() <= ROTATE_BYTES {
        return;
    }
    let Ok(contents) = std::fs::read_to_string(path) else {
        return;
    };
    let lines: Vec<&str> = contents.lines().filter(|l| !l.trim().is_empty()).collect();
    let kept = lines.split_at(lines.len().saturating_sub(KEEP_ENTRIES)).1;
    let temp = path.with_extension(format!("jsonl.{}", std::process::id()));
    let mut body = kept.join("\n");
    body.push('\n');
    if std::fs::write(&temp, body).is_err() {
        return;
    }
    if crate::secure_fs::restrict_file(&temp).is_err() {
        let _ = std::fs::remove_file(&temp);
        return;
    }
    if std::fs::rename(&temp, path).is_err() {
        let _ = std::fs::remove_file(&temp);
    }
}

/// Read history, optionally filter by URL pattern, return last `limit` entries.
pub fn run(filter: Option<&str>, limit: usize) -> Result<Vec<HistoryEntry>, crate::BoxError> {
    let path = history_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }

    // Bounded by `rotate_if_large`, so "read it all" stays a fixed cost.
    let contents = std::fs::read_to_string(&path)?;
    let mut entries: Vec<HistoryEntry> = contents
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<HistoryEntry>(l).ok())
        // Also on the way out, not only on the way in: a file written before this rule still
        // holds whole query strings, and `history` is what would print them.
        .map(|e| HistoryEntry {
            url: without_credentials(&e.url),
            ..e
        })
        .collect();

    if let Some(pattern) = filter {
        let pattern_lower = pattern.to_lowercase();
        entries.retain(|e| e.url.to_lowercase().contains(&pattern_lower));
    }

    let start = entries.len().saturating_sub(limit);
    Ok(entries.split_off(start))
}

/// Format history entries as human-readable text.
/// The response every mode carries.
#[must_use]
pub fn to_json(entries: &[HistoryEntry]) -> serde_json::Value {
    let entries: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| json!({"ts": e.ts, "url": e.url, "title": e.title, "page": e.page}))
        .collect();
    json!({"ok": true, "entries": entries})
}

pub fn format_text(entries: &[HistoryEntry]) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for entry in entries {
        let dt = format_timestamp(entry.ts);
        let _ = writeln!(
            out,
            "[{dt}] {} \u{2014} {} (page: {})",
            entry.url, entry.title, entry.page
        );
    }
    out.trim_end().to_string()
}

/// Unix timestamp → `YYYY-MM-DD HH:MM`, without a date crate.
fn format_timestamp(ts: u64) -> String {
    let secs = ts;
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;

    let (year, month, day) = days_to_date(days);
    format!("{year:04}-{month:02}-{day:02} {hours:02}:{minutes:02}")
}

/// Days since the epoch → civil date (Howard Hinnant's algorithm).
const fn days_to_date(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// A scratch file no concurrent test thread or process shares.
#[cfg(test)]
fn scratch(tag: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "chrome-agent-history-{tag}-{}-{n}.jsonl",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The credentials live after the `?`, and a `#` carries them too — the implicit OAuth flow
    /// returns `#access_token=`.
    #[test]
    fn a_stored_url_keeps_the_path_and_never_the_query_or_fragment() {
        assert_eq!(
            without_credentials("https://app.example.com/oauth/callback?code=abc123&state=xyz"),
            "https://app.example.com/oauth/callback\u{2026}"
        );
        assert_eq!(
            without_credentials("https://example.com/#access_token=abc123"),
            "https://example.com/\u{2026}"
        );
        assert_eq!(
            without_credentials("https://example.com/docs/page"),
            "https://example.com/docs/page",
            "a URL that carried nothing is written down whole, marker included"
        );
    }

    /// Applied on read as well as on write, so an entry written before the rule still cannot
    /// print its token.
    #[test]
    fn stripping_is_idempotent() {
        let once = without_credentials("https://example.com/r?token=t");
        assert_eq!(without_credentials(&once), once);
    }

    /// The cap is what makes "read the whole file" a bounded cost, and the entries kept are the
    /// NEWEST — a history that dropped the last hour would answer nothing useful.
    #[test]
    fn rotation_keeps_the_newest_entries_and_drops_the_rest() {
        let path = scratch("rotate");
        let line = |n: usize| {
            format!(r#"{{"ts":{n},"url":"https://example.com/{n}","title":"t","page":"default"}}"#)
        };
        let big: String = (0..12_000).map(|n| line(n) + "\n").collect();
        assert!(
            big.len() as u64 > ROTATE_BYTES,
            "the fixture must exceed the cap"
        );
        std::fs::write(&path, &big).expect("seed");

        rotate_if_large(&path);

        let after = std::fs::read_to_string(&path).expect("rotated file");
        let lines: Vec<&str> = after.lines().collect();
        assert_eq!(lines.len(), KEEP_ENTRIES, "the cap is not applied");
        assert!(
            lines[0].contains("/10000"),
            "the newest window is what survived: {}",
            lines[0]
        );
        assert!(
            lines[KEEP_ENTRIES - 1].contains("/11999"),
            "the last entry was dropped"
        );
        assert!(!after.contains("\"ts\":0,"), "the oldest entry is gone");
        let _ = std::fs::remove_file(&path);
    }

    /// A file under the cap is left byte-for-byte alone: rotation is not a rewrite on every
    /// navigation.
    #[test]
    fn a_small_history_is_not_rewritten() {
        let path = scratch("small");
        let body =
            "{\"ts\":1,\"url\":\"https://example.com/\",\"title\":\"t\",\"page\":\"default\"}\n";
        std::fs::write(&path, body).expect("seed");
        rotate_if_large(&path);
        assert_eq!(std::fs::read_to_string(&path).expect("file"), body);
        let _ = std::fs::remove_file(&path);
    }

    /// The rewrite must not widen what the append narrowed.
    #[cfg(unix)]
    #[test]
    fn a_rotated_history_is_still_private() {
        use std::os::unix::fs::PermissionsExt;
        let path = scratch("rotate-perms");
        let line = r#"{"ts":1,"url":"https://example.com/x","title":"t","page":"default"}"#;
        let big = format!("{line}\n").repeat(12_000);
        std::fs::write(&path, &big).expect("seed");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("widen");

        rotate_if_large(&path);

        let mode = std::fs::metadata(&path).expect("file").permissions().mode() & 0o777;
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            mode, 0o600,
            "the rotated file holds the same URLs; got {mode:o}"
        );
    }
}
