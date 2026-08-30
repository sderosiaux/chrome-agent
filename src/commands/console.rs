use serde::Deserialize;
use serde_json::json;

use crate::cdp::client::CdpClient;

#[derive(Debug, Deserialize)]
pub struct ConsoleEntry {
    pub level: String,
    pub message: String,
    pub timestamp: u64,
}

/// What a `console` read found, and whether anything was listening when it ran.
///
/// `installed: false` means only that the bootstrap did not take on this page, so an empty
/// list is a missing listener rather than a quiet page. The interceptor captures forward
/// only, so `installed: true` never means "nothing was missed". Derefs to the entries.
#[derive(Debug)]
pub struct ConsoleReading {
    /// Whether `window.__chrome_agent_console` existed in the page at read time.
    pub installed: bool,
    pub entries: Vec<ConsoleEntry>,
}

impl std::ops::Deref for ConsoleReading {
    type Target = [ConsoleEntry];

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

/// The response every mode carries. `installed` rides beside the messages because an empty list
/// means two different things without it.
#[must_use]
pub fn to_json(reading: &ConsoleReading) -> serde_json::Value {
    let messages: Vec<serde_json::Value> = reading
        .entries
        .iter()
        .map(|e| json!({"level": e.level, "message": e.message, "timestamp": e.timestamp}))
        .collect();
    json!({"ok": true, "installed": reading.installed, "messages": messages})
}

/// The shape the read script returns: the probe and the buffer in one round trip.
#[derive(Debug, Deserialize)]
struct RawReading {
    installed: bool,
    entries: Vec<ConsoleEntry>,
}

/// Entries the in-page buffer keeps. Oldest are shifted out, so a read always answers the most
/// recent messages, which are the ones the agent's own action produced.
const BUFFER_ENTRIES: usize = 200;

/// Characters one captured message may carry. A stack trace, a serialised DOM node or a base64
/// data URL passed to `console.log` is a single entry that can be megabytes on its own.
const MESSAGE_CHARS: usize = 2000;

/// Monkey-patches console.log/warn/error/info and captures unhandled errors and promise
/// rejections into `window.__chrome_agent_console`.
///
/// All three producers go through ONE `__push`: the `console[level]` wrapper carried the cap and
/// the two `addEventListener` producers below it did not, so a page throwing in a loop grew the
/// array without bound — and `run` pulls the whole thing back in one `JSON.stringify`. The clamp
/// is on the same helper for the same reason: the cap bounds the COUNT and says nothing about
/// the size of one entry.
///
/// `__push` is a block-scoped `const` captured by the three closures, not a property on
/// `window`: the interceptor already adds two globals a page can see, and a third buys nothing.
fn interceptor_js() -> String {
    format!(
        r"
    if (!window.__chrome_agent_console_installed) {{
    window.__chrome_agent_console_installed = true;
    window.__chrome_agent_console = window.__chrome_agent_console || [];
    const __clamp = (s) => {{
        const text = String(s);
        return text.length <= {MESSAGE_CHARS}
            ? text
            : text.slice(0, {MESSAGE_CHARS}) + '… (+' + (text.length - {MESSAGE_CHARS}) + ' chars)';
    }};
    const __push = (level, message) => {{
        const buffer = window.__chrome_agent_console;
        if (!buffer) return;
        buffer.push({{ level, message: __clamp(message), timestamp: Date.now() }});
        while (buffer.length > {BUFFER_ENTRIES}) buffer.shift();
    }};
    const __origConsole = {{
        log: console.log.bind(console),
        warn: console.warn.bind(console),
        error: console.error.bind(console),
        info: console.info.bind(console),
    }};
    ['log','warn','error','info'].forEach(level => {{
        console[level] = (...args) => {{
            __push(level, args.map(a => {{
                try {{ return typeof a === 'object' && a !== null ? JSON.stringify(a) : String(a); }}
                catch (e) {{ return String(a); }}
            }}).join(' '));
            __origConsole[level](...args);
        }};
    }});
    window.addEventListener('error', (e) => {{
        __push('exception', e.message + (e.filename ? ' at ' + e.filename + ':' + e.lineno : ''));
    }});
    window.addEventListener('unhandledrejection', (e) => {{
        __push('exception', 'Unhandled rejection: ' + String(e.reason));
    }});
    }} // end guard: __chrome_agent_console_installed
"
    )
}

/// Inject the console interceptor: `addScriptToEvaluateOnNewDocument` for future navigations,
/// plus a guarded `Runtime.evaluate` for the current page. No `Runtime.enable`, so
/// stealth-safe. Errors are discarded on purpose; [`run`] probes at read time instead, which
/// also catches a JS exception in the snippet and a page that dropped the buffer later.
pub async fn inject(client: &CdpClient) {
    let source = interceptor_js();
    let _ = client
        .send(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": source }),
        )
        .await;

    // Bootstrap on the current page; the guard prevents double-init.
    let guarded = format!("if (!window.__chrome_agent_console) {{ {source} }}");
    let _ = client
        .send("Runtime.evaluate", json!({ "expression": guarded }))
        .await;
}

/// Read captured console messages, optionally filtered by level and cleared after.
///
/// The `installed` probe rides on the same round trip as the buffer read, and is reported
/// beside the list rather than raised: a missing interceptor does not stop a read.
pub async fn run(
    client: &CdpClient,
    level_filter: Option<&str>,
    clear: bool,
    limit: usize,
) -> Result<ConsoleReading, crate::BoxError> {
    let result: crate::cdp::types::EvaluateResult = client
        .call(
            "Runtime.evaluate",
            json!({
                "expression": "JSON.stringify({ \
                    installed: typeof window.__chrome_agent_console !== 'undefined', \
                    entries: window.__chrome_agent_console || [] \
                })",
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
        .ok_or("Failed to read console buffer: the page returned no value")?;

    let RawReading { installed, entries } =
        serde_json::from_str(raw).map_err(|e| format!("Failed to parse console buffer: {e}"))?;

    if !installed {
        // stderr, never stdout, so `--json` stays clean.
        eprintln!(
            "warning: console interceptor not installed on this page \
             (window.__chrome_agent_console is undefined), so nothing was capturing \
             console output. An empty result here is a missing listener, not a quiet page."
        );
    }

    let filtered: Vec<ConsoleEntry> = if let Some(level) = level_filter {
        entries.into_iter().filter(|e| e.level == level).collect()
    } else {
        entries
    };

    let limited = keep_recent(filtered, limit);
    if clear {
        clear_buffer(client, installed).await;
    }

    Ok(ConsoleReading {
        installed,
        entries: limited,
    })
}

/// Empty the page's buffer, saying on stderr when that did not happen — a failed clear
/// otherwise reports the entries as consumed and the next read returns them again.
///
/// The `installed` guard is load-bearing: assigning an empty array would CREATE the buffer on
/// a page with no interceptor. Emptying in place also holds for the interceptor's reference.
async fn clear_buffer(client: &CdpClient, installed: bool) {
    if !installed {
        eprintln!(
            "warning: console --clear had nothing to clear; no interceptor buffer exists \
             on this page."
        );
        return;
    }
    let result: Result<crate::cdp::types::EvaluateResult, _> = client
        .call(
            "Runtime.evaluate",
            json!({
                "expression": "!!window.__chrome_agent_console && \
                    (window.__chrome_agent_console.length = 0, true)",
                "returnByValue": true,
            }),
        )
        .await;

    let emptied = match result {
        Ok(r) if r.exception_details.is_none() => r
            .result
            .value
            .as_ref()
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        _ => false,
    };

    if !emptied {
        eprintln!(
            "warning: console --clear did not empty the page's buffer; the messages just \
             reported are still there and the next read will return them again."
        );
    }
}

/// Keep the most recent `limit` entries of an oldest→newest buffer, preserving order, so the
/// agent sees the messages it just triggered.
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

/// Format a reading for text output. Takes the whole reading, not the slice, so the two
/// silences (quiet page, absent listener) stay distinguishable.
#[must_use]
pub fn format_text(reading: &ConsoleReading) -> String {
    if !reading.installed {
        return "Console interceptor not installed on this page \
                (window.__chrome_agent_console is undefined) — nothing was capturing console \
                output, so this is not a report that the page logged nothing."
            .to_string();
    }
    if reading.entries.is_empty() {
        return "No console messages captured.".to_string();
    }
    reading
        .entries
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

    /// One helper carries the cap and the clamp, and all three producers reach it. The three
    /// used to be three pushes, two of them unbounded.
    #[test]
    fn every_producer_pushes_through_the_one_capped_helper() {
        let js = interceptor_js();
        assert_eq!(
            js.matches("buffer.push(").count(),
            1,
            "the buffer is written in exactly one place, or the cap is optional again: {js}"
        );
        assert_eq!(
            js.matches("const __push =").count(),
            1,
            "one definition: {js}"
        );
        assert_eq!(
            js.matches("__push(").count(),
            3,
            "three producers, all of them: {js}"
        );
        assert!(
            js.contains(&format!("buffer.length > {BUFFER_ENTRIES}")),
            "{js}"
        );
        assert!(
            js.contains(&format!("text.slice(0, {MESSAGE_CHARS})")),
            "{js}"
        );
    }

    #[test]
    fn keep_recent_returns_newest_n_in_order() {
        // Oldest ("m0") → newest ("m4").
        let entries: Vec<ConsoleEntry> = (0..5).map(|i| entry(&format!("m{i}"), i)).collect();

        let limited = keep_recent(entries, 3);

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

    #[test]
    fn text_output_tells_the_two_silences_apart() {
        let quiet = ConsoleReading {
            installed: true,
            entries: vec![],
        };
        let blind = ConsoleReading {
            installed: false,
            entries: vec![],
        };

        assert_eq!(format_text(&quiet), "No console messages captured.");
        assert_ne!(
            format_text(&blind),
            format_text(&quiet),
            "an empty page and an absent listener are two different facts"
        );
        assert!(
            format_text(&blind).contains("not installed"),
            "{}",
            format_text(&blind)
        );
    }

    #[test]
    fn the_blind_report_claims_nothing_about_what_the_page_logged() {
        // The measurement is "nothing was listening", not "you missed messages".
        let text = format_text(&ConsoleReading {
            installed: false,
            entries: vec![],
        })
        .to_lowercase();
        for forbidden in ["missed", "lost", "were logged"] {
            assert!(!text.contains(forbidden), "over-claims: {text}");
        }
    }

    #[test]
    fn an_installed_interceptor_reports_its_entries_unchanged() {
        let reading = ConsoleReading {
            installed: true,
            entries: vec![entry("boom", 0)],
        };
        assert!(
            format_text(&reading).contains("LOG: boom"),
            "{}",
            format_text(&reading)
        );
        // Still usable as a slice.
        assert_eq!(reading.len(), 1);
    }
}
