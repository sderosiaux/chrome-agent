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
/// The buffer used to be read as `window.__chrome_agent_console || []`, so a page
/// where the interceptor had never been installed answered with the same empty
/// list as a page that simply had not logged. "No console messages captured" is a
/// statement about the page, and an agent uses it to conclude the page is healthy;
/// on a page with no interceptor it is a statement about nothing. The asymmetry
/// showed it was not a choice — the same read already checked `exceptionDetails`
/// for its OWN evaluation and checked nothing for the injection.
///
/// **The scope of `installed: false` is exactly one thing: the bootstrap did not
/// take on this page.** It is the same gesture as `verdict: unknown / read_failed`
/// — a report that the measurement did not happen, not a claim about what would
/// have been measured. In particular it does NOT cover the other half of the
/// silence: anything the page logged BEFORE a chrome-agent connection existed is
/// gone whatever this field says, because the interceptor captures forward only.
/// `installed: true` therefore means "something was listening from the moment it
/// was installed", never "nothing was missed".
///
/// Derefs to the entries so a caller that only wants the list is unaffected.
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

/// The shape the read script returns: the probe and the buffer in one round trip.
#[derive(Debug, Deserialize)]
struct RawReading {
    installed: bool,
    entries: Vec<ConsoleEntry>,
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
///
/// Both errors stay discarded here, and the check lives in [`run`] instead. That
/// is deliberate, not an omission: propagating them would catch the two ways CDP
/// can refuse the call and miss the way the bootstrap actually fails most quietly
/// — a JS exception raised while the snippet runs, which `send` never sees
/// because `Runtime.evaluate` answers `Ok` and reports it in `exceptionDetails`.
/// A probe at read time catches all three at once, and catches the fourth thing
/// no injection-time check can: a page that dropped the buffer afterwards.
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
///
/// The probe rides on the expression that already reads the buffer — same call,
/// same round trip, no added cost — and its answer is reported beside the list
/// rather than raised: this is a read command, and a missing interceptor does not
/// stop it reading. See [`ConsoleReading`] for what `installed: false` does and
/// does not claim.
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

    let RawReading { installed, entries } = serde_json::from_str(raw)
        .map_err(|e| format!("Failed to parse console buffer: {e}"))?;

    if !installed {
        // stderr, never stdout, so --json stays clean — the channel the dialog
        // handler already uses for a fact the response has no field for.
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

    Ok(ConsoleReading { installed, entries: limited })
}

/// Empty the page's buffer, and say on stderr when that did not happen.
///
/// The result used to be discarded, which made every read that asked for a clear
/// report the entries as consumed: on a failure they are still in the page and
/// the next read returns them again, which reads as the page having logged them
/// twice. Three things can go wrong and all three are answered here — the call
/// can fail, the expression can throw, and the buffer can be absent.
///
/// The guard is load-bearing, not defensive: the old expression ASSIGNED an empty
/// array, so clearing a page with no interceptor CREATED the buffer and the next
/// read would have probed `installed: true` on a page where nothing is listening.
/// Emptying in place also holds for any reference the interceptor captured.
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
        Ok(r) if r.exception_details.is_none() => {
            r.result.value.as_ref().and_then(serde_json::Value::as_bool).unwrap_or(false)
        }
        _ => false,
    };

    if !emptied {
        eprintln!(
            "warning: console --clear did not empty the page's buffer; the messages just \
             reported are still there and the next read will return them again."
        );
    }
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

/// Format a reading for text output.
///
/// The two silences are told apart here, because they are two different facts and
/// only one of them is about the page. Takes the whole reading rather than the
/// slice for exactly that reason.
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

    #[test]
    fn text_output_tells_the_two_silences_apart() {
        let quiet = ConsoleReading { installed: true, entries: vec![] };
        let blind = ConsoleReading { installed: false, entries: vec![] };

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
        // The measurement is "nothing was listening". "The page logged things you missed" is
        // a different claim, and this tool has no way to make it.
        let text = format_text(&ConsoleReading { installed: false, entries: vec![] }).to_lowercase();
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
        assert!(format_text(&reading).contains("LOG: boom"), "{}", format_text(&reading));
        // And the reading is still usable as the slice every caller wants.
        assert_eq!(reading.len(), 1);
    }
}
