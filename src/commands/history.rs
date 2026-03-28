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

fn history_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    dirs::home_dir()
        .map(|h| h.join(".aibrowsr").join("history.jsonl"))
        .ok_or_else(|| "Could not determine home directory".into())
}

/// Append a navigation entry to `~/.aibrowsr/history.jsonl`.
pub fn append(url: &str, title: &str, page: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = history_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let entry = json!({
        "ts": ts,
        "url": url,
        "title": title,
        "page": page,
    });

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(file, "{}", serde_json::to_string(&entry)?)?;
    Ok(())
}

/// Read history, optionally filter by URL pattern, return last `limit` entries.
pub fn run(filter: Option<&str>, limit: usize) -> Result<Vec<HistoryEntry>, Box<dyn std::error::Error>> {
    let path = history_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }

    let contents = std::fs::read_to_string(&path)?;
    let mut entries: Vec<HistoryEntry> = contents
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    if let Some(pattern) = filter {
        let pattern_lower = pattern.to_lowercase();
        entries.retain(|e| e.url.to_lowercase().contains(&pattern_lower));
    }

    // Return last N entries
    let start = entries.len().saturating_sub(limit);
    Ok(entries.split_off(start))
}

/// Format history entries as human-readable text.
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

fn format_timestamp(ts: u64) -> String {
    // Convert unix timestamp to YYYY-MM-DD HH:MM without external crate
    let secs = ts;
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;

    // Calculate date from days since epoch (1970-01-01)
    let (year, month, day) = days_to_date(days);
    format!("{year:04}-{month:02}-{day:02} {hours:02}:{minutes:02}")
}

const fn days_to_date(days: u64) -> (u64, u64, u64) {
    // Civil date from day count algorithm
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
