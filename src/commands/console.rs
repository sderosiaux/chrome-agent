use serde::Deserialize;
use serde_json::json;

use crate::cdp::client::CdpClient;

#[derive(Debug, Deserialize)]
pub struct ConsoleEntry {
    pub level: String,
    pub message: String,
    pub timestamp: u64,
}

/// JS snippet that monkey-patches console.log/warn/error/info and captures
/// unhandled errors + promise rejections into `window.__chrome_agent_console`.
const INTERCEPTOR_JS: &str = r"
    if (!window.__chrome_agent_console_installed) {
    window.__chrome_agent_console_installed = true;
    window.__chrome_agent_console = window.__chrome_agent_console || [];
    const __origConsole = {
        log: console.log.bind(console),
        warn: console.warn.bind(console),
        error: console.error.bind(console),
        info: console.info.bind(console),
    };
    ['log','warn','error','info'].forEach(level => {
        console[level] = (...args) => {
            window.__chrome_agent_console.push({
                level,
                message: args.map(a => typeof a === 'object' ? JSON.stringify(a) : String(a)).join(' '),
                timestamp: Date.now(),
            });
            if (window.__chrome_agent_console.length > 200) window.__chrome_agent_console.shift();
            __origConsole[level](...args);
        };
    });
    window.addEventListener('error', (e) => {
        window.__chrome_agent_console.push({
            level: 'exception',
            message: e.message + (e.filename ? ' at ' + e.filename + ':' + e.lineno : ''),
            timestamp: Date.now(),
        });
    });
    window.addEventListener('unhandledrejection', (e) => {
        window.__chrome_agent_console.push({
            level: 'exception',
            message: 'Unhandled rejection: ' + String(e.reason),
            timestamp: Date.now(),
        });
    });
    } // end guard: __chrome_agent_console_installed
";

/// Inject the console interceptor into the page.
///
/// 1. `addScriptToEvaluateOnNewDocument` — survives future navigations.
/// 2. `Runtime.evaluate` with a guard — bootstraps on the current page immediately.
///
/// Does NOT require `Runtime.enable`, so it is stealth-safe.
pub async fn inject(client: &CdpClient) {
    // Runs on every future navigation automatically
    let _ = client
        .send(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": INTERCEPTOR_JS }),
        )
        .await;

    // Bootstrap on the current page (guard prevents double-init)
    let guarded = format!(
        "if (!window.__chrome_agent_console) {{ {INTERCEPTOR_JS} }}"
    );
    let _ = client
        .send(
            "Runtime.evaluate",
            json!({ "expression": guarded }),
        )
        .await;
}

/// Read captured console messages from the injected interceptor.
/// Optionally filter by level and clear after reading.
pub async fn run(
    client: &CdpClient,
    level_filter: Option<&str>,
    clear: bool,
    limit: usize,
) -> Result<Vec<ConsoleEntry>, crate::BoxError> {
    let result: crate::cdp::types::EvaluateResult = client
        .call(
            "Runtime.evaluate",
            json!({
                "expression": "JSON.stringify(window.__chrome_agent_console || [])",
                "returnByValue": true,
            }),
        )
        .await?;

    if let Some(exception) = &result.exception_details {
        return Err(format!(
            "Failed to read console buffer: {}",
            exception
                .exception
                .as_ref()
                .and_then(|e| e.description.as_deref())
                .unwrap_or(&exception.text)
        )
        .into());
    }

    let raw = result
        .result
        .value
        .as_ref()
        .and_then(|v| v.as_str())
        .unwrap_or("[]");

    let entries: Vec<ConsoleEntry> = serde_json::from_str(raw)
        .map_err(|e| format!("Failed to parse console buffer: {e}"))?;

    let filtered: Vec<ConsoleEntry> = if let Some(level) = level_filter {
        entries.into_iter().filter(|e| e.level == level).collect()
    } else {
        entries
    };

    let limited = keep_recent(filtered, limit);

    if clear {
        let _ = client
            .send(
                "Runtime.evaluate",
                json!({
                    "expression": "window.__chrome_agent_console = []",
                    "returnByValue": true,
                }),
            )
            .await;
    }

    Ok(limited)
}

/// Keep the most-recent `limit` entries of a chronological (oldest→newest)
/// buffer, preserving order. Mirrors `history::run` so the agent sees the
/// newest messages it just triggered rather than the oldest.
fn keep_recent(mut entries: Vec<ConsoleEntry>, limit: usize) -> Vec<ConsoleEntry> {
    let start = entries.len().saturating_sub(limit);
    entries.split_off(start)
}

/// Format a timestamp (epoch ms) as HH:MM:SS.
fn format_time(ts: u64) -> String {
    let secs = ts / 1000;
    let h = (secs / 3600) % 24;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

/// Format entries for text output.
pub fn format_text(entries: &[ConsoleEntry]) -> String {
    if entries.is_empty() {
        return "No console messages captured.".to_string();
    }
    entries
        .iter()
        .map(|e| {
            format!(
                "[{}] {}: {}",
                format_time(e.timestamp),
                e.level.to_uppercase(),
                e.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(msg: &str, ts: u64) -> ConsoleEntry {
        ConsoleEntry {
            level: "log".to_string(),
            message: msg.to_string(),
            timestamp: ts,
        }
    }

    #[test]
    fn keep_recent_returns_newest_n_in_order() {
        // Chronological buffer: oldest ("m0") → newest ("m4").
        let entries: Vec<ConsoleEntry> = (0..5).map(|i| entry(&format!("m{i}"), i)).collect();

        let limited = keep_recent(entries, 3);

        // Must be the 3 most-recent, preserving oldest→newest order.
        let msgs: Vec<&str> = limited.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(msgs, vec!["m2", "m3", "m4"]);
    }

    #[test]
    fn keep_recent_shorter_than_limit_returns_all() {
        let entries = vec![entry("a", 0), entry("b", 1)];
        let limited = keep_recent(entries, 10);
        let msgs: Vec<&str> = limited.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(msgs, vec!["a", "b"]);
    }
}
