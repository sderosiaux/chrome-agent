//! What the default (text) output says about an action it has already measured: the fields the
//! JSON carries (`value`, `values_lost`, `intercepted_by`), plus a gloss on the verdict word.
//!
//! Two rules the shapes here follow:
//!
//! 1. A keyword first, so a line greps: `value: NOT KEPT — …`, `values lost: …`, `next: …`.
//! 2. Silence for the clean case. A fill the page kept prints no `value:` line — a tool that
//!    narrates its successes teaches the reader to skip its output.
//!
//! Colour only when stdout is a terminal (`std::io::IsTerminal`, no dependency). Piped output is
//! byte-identical to the plain text, pinned by `tests/text_output_tests.rs`.

use std::borrow::Cow;
use std::io::IsTerminal;

use serde_json::Value;

use crate::verdict::{Assessment, Verdict};

/// How long a value may be before it is cut, on a line meant to be read. The JSON keeps the whole
/// string. Same budget as `assert`'s own renderer.
const LINE_BUDGET: usize = 120;

/// Below this, a wait is not news. See [`waited_line`].
const WAIT_WORTH_SAYING_MS: u64 = 1000;

/// Whether this stream gets ANSI, decided once. A struct rather than a global: the tests render
/// both ways in one process, and a `OnceLock` would make the second call answer for the first.
#[derive(Clone, Copy)]
pub struct Paint {
    enabled: bool,
}

impl Paint {
    /// Colour if and only if stdout is a terminal.
    #[must_use]
    pub fn for_stdout() -> Self {
        Self { enabled: std::io::stdout().is_terminal() }
    }

    /// Never colour. What every pipe and file gets, and what the tests render with.
    #[cfg(test)]
    #[must_use]
    pub const fn off() -> Self {
        Self { enabled: false }
    }

    fn paint(self, code: &str, text: &str) -> Cow<'static, str> {
        if self.enabled {
            Cow::Owned(format!("\x1b[{code}m{text}\x1b[0m"))
        } else {
            Cow::Owned(text.to_string())
        }
    }

    /// A claim that something did not land: NOT KEPT, INTERCEPTED, a lost value.
    fn bad(self, text: &str) -> Cow<'static, str> {
        self.paint("1;31", text)
    }

    /// A claim that the page moved.
    fn good(self, text: &str) -> Cow<'static, str> {
        self.paint("1;32", text)
    }

    /// Anything the caller must not read as success: ignorance, or an observation that saw
    /// nothing (`unchanged`, `no_effect`).
    fn wary(self, text: &str) -> Cow<'static, str> {
        self.paint("1;33", text)
    }

    fn bold(self, text: &str) -> Cow<'static, str> {
        self.paint("1", text)
    }

    /// The colour a verdict word is allowed to be. Three classes, not eight: the reader is told
    /// whether they may act on this, not which rung fired.
    fn verdict_word(self, verdict: Verdict) -> Cow<'static, str> {
        let word = verdict.as_str();
        match verdict {
            Verdict::NotKept | Verdict::Intercepted => self.bad(word),
            Verdict::Changed | Verdict::Navigated => self.good(word),
            Verdict::Unchanged | Verdict::NoEffect | Verdict::Unknown | Verdict::NotChecked => {
                self.wary(word)
            }
        }
    }
}

/// A string as it should appear on a line: quoted, and cut if it is longer than a line.
#[must_use]
pub fn quote(text: &str) -> String {
    format!("{:?}", crate::truncate::truncate_str(text, LINE_BUDGET, "…"))
}

/// A JSON scalar rendered for a human-readable line. Shared with `assert`.
#[must_use]
pub fn compact(value: &Value) -> String {
    match value {
        Value::String(s) => quote(s),
        Value::Null => "null".into(),
        other => other.to_string(),
    }
}

/// Every line an action owes the reader after its own message and the delta. Returned as a list
/// so a test can assert on it without a terminal.
#[must_use]
pub fn action_lines(obj: &Value, assessment: Assessment, paint: Paint) -> Vec<String> {
    let mut lines = Vec::new();
    let said_value = value_lines(obj, paint, &mut lines);
    observation_line(obj, said_value, &mut lines);
    waited_line(obj, &mut lines);
    lost_value_lines(obj, paint, &mut lines);
    receiver_line(obj, paint, &mut lines);
    lines.push(format!(
        "verdict: {} ({}) — {}",
        paint.verdict_word(assessment.verdict),
        assessment.reason,
        crate::verdict::gloss(assessment)
    ));
    next_lines(assessment, paint, &mut lines);
    lines
}

/// What the page kept, for a single fill and for each field of a bulk one. Prints nothing when
/// the page held what was asked for (rule 2). A `caveat` is printed either way: a value over
/// `maxlength` is verbatim AND about to be rejected by the form.
///
/// Returns whether it said anything — a line it printed carries its own window, and
/// `observation_line` must not repeat it.
fn value_lines(obj: &Value, paint: Paint, lines: &mut Vec<String>) -> bool {
    let before = lines.len();
    if let Some(value) = obj.get("value") {
        push_value_line(value, None, paint, lines);
    }
    for field in obj.get("values").and_then(Value::as_array).into_iter().flatten() {
        let label = field
            .get("uid")
            .or_else(|| field.get("selector"))
            .and_then(Value::as_str);
        if let Some(value) = field.get("value") {
            push_value_line(value, label, paint, lines);
        }
    }
    lines.len() > before
}

fn push_value_line(value: &Value, label: Option<&str>, paint: Paint, lines: &mut Vec<String>) {
    let named = label.map_or_else(String::new, |l| format!("{l}: "));
    if value.get("verbatim").and_then(Value::as_bool) == Some(false) {
        let (state, wrote, holds) = if value.get("redacted").and_then(Value::as_bool) == Some(true) {
            // A secret reports two lengths and no strings: this line reaches stdout, the agent
            // transcript and any `--record` file.
            (
                lengths_state(value),
                format!("{} chars", length_of(value, "requested_length")),
                format!("{} chars", length_of(value, "actual_length")),
            )
        } else {
            let actual = value.get("actual").unwrap_or(&Value::Null);
            let state = if actual.as_str().is_none_or(str::is_empty) {
                "NOT KEPT"
            } else {
                "REWRITTEN"
            };
            (
                state,
                compact(value.get("requested").unwrap_or(&Value::Null)),
                compact(actual),
            )
        };
        lines.push(format!(
            "value: {}{} — wrote {wrote}, page holds {holds}{}",
            named,
            paint.bad(state),
            window(value).map_or_else(String::new, |ms| format!(" (read {ms} ms later)")),
        ));
    }
    if let Some(caveat) = value.get("caveat").and_then(Value::as_str) {
        lines.push(format!("caveat: {named}{caveat}"));
    }
}

/// A redacted field's state from the lengths alone — the same classification
/// `pipe_report::field_postcondition` makes, without reading the value.
fn lengths_state(value: &Value) -> &'static str {
    if length_of(value, "actual_length") == 0 { "NOT KEPT" } else { "REWRITTEN" }
}

fn length_of(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn window(value: &Value) -> Option<u64> {
    value.get("observed_after_ms").and_then(Value::as_u64)
}

/// The window a check or a select read its state back through. Those two report
/// `observed_after_ms` at the top level, not inside `value`, because the window covers the whole
/// action. Worded `observed` because the same field carries a `no_effect` verdict's window.
///
/// Skipped only when a `value:` line was actually PRINTED. Guarding on "the response has a
/// `value` field" instead would delete this line for every successful check and select, whose
/// `value:` line prints nothing (rule 2).
fn observation_line(obj: &Value, said_value: bool, lines: &mut Vec<String>) {
    if said_value {
        return;
    }
    if let Some(ms) = obj.get("observed_after_ms").and_then(Value::as_u64) {
        lines.push(format!("observed: {ms} ms after the action"));
    }
}

/// How long the action spent waiting for a page load, when that was long enough to notice.
/// Thresholded at one second, where a person starts wondering whether the tool is stuck.
/// `--json` carries the number whatever its size.
fn waited_line(obj: &Value, lines: &mut Vec<String>) {
    let Some(ms) = obj.get("waited_ms").and_then(Value::as_u64) else {
        return;
    };
    if ms < WAIT_WORTH_SAYING_MS {
        return;
    }
    // Integer arithmetic: one decimal is enough for any wait long enough to print.
    lines.push(format!(
        "waited: {}.{}s for the page to finish loading after this action",
        ms / 1000,
        (ms % 1000) / 100
    ));
}

/// Fields that held a value before this action and hold none after it. Without which field and
/// what it held, a form that submitted-and-cleared reads like one that threw the input away.
/// Redacted entries name the field and never the value.
fn lost_value_lines(obj: &Value, paint: Paint, lines: &mut Vec<String>) {
    let Some(lost) = obj.get("values_lost").and_then(Value::as_array) else {
        return;
    };
    if lost.is_empty() {
        return;
    }
    let shown = lost.len() as u64;
    // The list is capped at ten entries; the count never is.
    let total = obj.get("values_lost_total").and_then(Value::as_u64).unwrap_or(shown);
    lines.push(format!(
        "{}: {total} field{} held a value before this action and hold{} none now",
        paint.bad("values lost"),
        if total == 1 { "" } else { "s" },
        if total == 1 { "s" } else { "" },
    ));
    for entry in lost {
        let uid = entry.get("uid").and_then(Value::as_str).unwrap_or("?");
        let role = entry.get("role").and_then(Value::as_str).unwrap_or("");
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .map_or_else(String::new, |n| format!(" {}", quote(n)));
        let held = if entry.get("redacted").and_then(Value::as_bool) == Some(true) {
            "held a secret (not shown)".to_string()
        } else {
            format!("held {}", compact(entry.get("was").unwrap_or(&Value::Null)))
        };
        lines.push(format!("  {uid} {role}{name} {held}"));
    }
}

/// Who got the event, when it was not the target. `intercepted` without the receiver is a verdict
/// a person cannot act on.
fn receiver_line(obj: &Value, paint: Paint, lines: &mut Vec<String>) {
    let Some(receiver) = obj.get("intercepted_by") else {
        return;
    };
    let tag = receiver
        .get("tag")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_lowercase();
    let id = receiver
        .get("id")
        .and_then(Value::as_str)
        .map_or_else(String::new, |id| format!("#{id}"));
    let uid = receiver
        .get("uid")
        .and_then(Value::as_str)
        .map_or_else(String::new, |uid| format!(" ({uid})"));
    lines.push(format!("{}: {tag}{id}{uid}", paint.bad("received by")));
}

/// What to do: the token the JSON carries, then the words written for a person.
///
/// Uses `verdict_words::short_hint`, not the response's `verdict_hint`, which is written for an
/// agent reading JSON and wraps to seven terminal lines for `not_kept`. What that loses is the
/// action's own more specific wording; text mode prints that on its `received by:` line instead.
fn next_lines(assessment: Assessment, paint: Paint, lines: &mut Vec<String>) {
    let token = crate::verdict::next_for(assessment);
    lines.push(format!("{}: {}", paint.bold("next"), token.as_str()));
    if let Some(hint) = crate::verdict::short_hint(assessment) {
        lines.push(format!("hint: {hint}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verdict::PageSight;
    use serde_json::json;

    fn changed() -> Assessment {
        Assessment { verdict: Verdict::Changed, reason: "tree_delta", page: PageSight::Readable }
    }

    fn not_kept() -> Assessment {
        Assessment { verdict: Verdict::NotKept, reason: "value_reverted", page: PageSight::Readable }
    }

    fn rendered(obj: &Value, assessment: Assessment) -> String {
        action_lines(obj, assessment, Paint::off()).join("\n")
    }

    /// A value the page emptied reaches the terminal, not just the JSON.
    #[test]
    fn a_value_the_page_did_not_keep_is_on_the_line() {
        let obj = json!({
            "ok": true,
            "value": {"requested": "SAVE20", "actual": "", "verbatim": false, "observed_after_ms": 60},
        });
        let out = rendered(&obj, not_kept());
        assert!(out.contains("value: NOT KEPT"), "{out}");
        assert!(out.contains("wrote \"SAVE20\""), "the written value: {out}");
        assert!(out.contains("page holds \"\""), "and the held one: {out}");
        assert!(out.contains("read 60 ms later"), "with the window: {out}");
    }

    /// A mask is not a failure and must not read as one: the write landed, in the page's shape.
    #[test]
    fn a_rewritten_value_is_not_spelled_like_a_lost_one() {
        let obj = json!({
            "ok": true,
            "value": {"requested": "5551234567", "actual": "(555) 123-4567", "verbatim": false, "observed_after_ms": 60},
        });
        let out = rendered(&obj, Assessment { verdict: Verdict::NotKept, reason: "value_rewritten", page: PageSight::Readable });
        assert!(out.contains("value: REWRITTEN"), "{out}");
        assert!(!out.contains("NOT KEPT"), "a mask is not an empty field: {out}");
        assert!(out.contains("page holds \"(555) 123-4567\""), "{out}");
    }

    /// Rule 2: the clean case says nothing.
    #[test]
    fn a_value_the_page_kept_adds_no_line() {
        let obj = json!({
            "ok": true,
            "value": {"requested": "ada@example.com", "actual": "ada@example.com", "verbatim": true, "observed_after_ms": 60},
        });
        let out = rendered(&obj, changed());
        assert!(!out.contains("value:"), "a kept value is not news: {out}");
        assert_eq!(out.lines().count(), 2, "verdict and next, nothing else: {out}");
    }

    /// A value over `maxlength` is verbatim and the form will still reject it: the one clean
    /// shape that owes the reader a line.
    #[test]
    fn a_caveat_is_printed_even_when_the_value_was_kept() {
        let obj = json!({
            "ok": true,
            "value": {
                "requested": "1234567890", "actual": "1234567890", "verbatim": true,
                "observed_after_ms": 60, "caveat": "exceeds maxlength=5",
            },
        });
        let out = rendered(&obj, changed());
        assert!(out.contains("caveat: exceeds maxlength=5"), "{out}");
    }

    /// The redaction is not weaker on the surface a person reads.
    #[test]
    fn a_secret_the_page_dropped_reports_lengths_and_no_value() {
        let obj = json!({
            "ok": true,
            "value": {"redacted": true, "requested_length": 12, "actual_length": 0, "verbatim": false, "observed_after_ms": 60},
        });
        let out = rendered(&obj, not_kept());
        assert!(out.contains("value: NOT KEPT"), "{out}");
        assert!(out.contains("wrote 12 chars"), "{out}");
        assert!(out.contains("page holds 0 chars"), "{out}");
    }

    /// A bulk fill reports per field, naming only the fields that did not hold.
    #[test]
    fn a_bulk_fill_names_the_field_that_did_not_hold() {
        let obj = json!({"ok": true, "values": [
            {"uid": "n11", "value": {"requested": "a", "actual": "a", "verbatim": true, "observed_after_ms": 60}},
            {"uid": "n12", "value": {"requested": "SAVE20", "actual": "", "verbatim": false, "observed_after_ms": 60}},
        ]});
        let out = rendered(&obj, not_kept());
        assert!(out.contains("value: n12: NOT KEPT"), "{out}");
        assert!(!out.contains("n11"), "the field that held needs no line: {out}");
    }

    /// The submit worked AND cleared the field. Both facts, one report.
    #[test]
    fn a_lost_value_names_the_field_and_what_it_held() {
        let obj = json!({
            "ok": true,
            "values_lost": [{"uid": "n12", "role": "textbox", "name": "Coupon", "was": "SAVE20"}],
        });
        let out = rendered(&obj, Assessment { verdict: Verdict::Changed, reason: "values_lost", page: PageSight::Readable });
        assert!(out.contains("values lost: 1 field"), "{out}");
        assert!(out.contains("n12 textbox \"Coupon\" held \"SAVE20\""), "{out}");
        // Not a plain success: the caller must establish which of two things happened.
        assert!(out.contains("next: confirm"), "{out}");
    }

    #[test]
    fn a_lost_secret_names_the_field_and_never_the_value() {
        let obj = json!({
            "ok": true,
            "values_lost": [{"uid": "n9", "role": "textbox", "redacted": true}],
        });
        let out = rendered(&obj, Assessment { verdict: Verdict::Changed, reason: "values_lost", page: PageSight::Readable });
        assert!(out.contains("n9 textbox held a secret (not shown)"), "{out}");
    }

    /// The cap is on the list, not on the count.
    #[test]
    fn a_truncated_lost_value_list_still_reports_the_total() {
        let obj = json!({
            "ok": true,
            "values_lost": [{"uid": "n1", "role": "textbox", "was": "x"}],
            "values_lost_total": 40,
        });
        let out = rendered(&obj, Assessment { verdict: Verdict::Changed, reason: "values_lost", page: PageSight::Readable });
        assert!(out.contains("values lost: 40 fields"), "{out}");
    }

    /// `intercepted` without the receiver is a verdict nobody can act on.
    #[test]
    fn an_intercepted_click_names_who_got_it() {
        let obj = json!({
            "ok": true,
            "delivery": "intercepted",
            "intercepted_by": {"tag": "DIV", "id": "scrim", "uid": "n11"},
            "verdict_hint": "Dismiss div#scrim first.",
        });
        let out = rendered(
            &obj,
            Assessment { verdict: Verdict::Intercepted, reason: "hit_test_receiver", page: PageSight::Readable },
        );
        assert!(out.contains("received by: div#scrim (n11)"), "{out}");
        assert!(out.contains("next: dismiss"), "{out}");
        // The short hint points at the `received by:` line rather than repeating the receiver.
        assert!(out.contains("hint: Deal with the element named above first"), "{out}");
        assert!(out.lines().all(|l| l.len() < 200), "no line is a paragraph: {out}");
    }

    /// A wait a person noticed gets a line; a wait nobody noticed does not.
    #[test]
    fn only_a_wait_worth_noticing_reaches_the_terminal() {
        let mut lines = Vec::new();
        waited_line(&json!({"waited_ms": 10_100}), &mut lines);
        assert_eq!(lines, vec!["waited: 10.1s for the page to finish loading after this action"]);

        let mut quiet = Vec::new();
        waited_line(&json!({"waited_ms": 90}), &mut quiet);
        waited_line(&json!({"ok": true}), &mut quiet);
        assert!(quiet.is_empty(), "{quiet:?}");
    }

    /// Without the gloss, `unchanged (identical_tree)` reads as "nothing happened".
    #[test]
    fn the_verdict_line_carries_its_gloss() {
        let out = rendered(
            &json!({"ok": true}),
            Assessment { verdict: Verdict::Unchanged, reason: "identical_tree", page: PageSight::Readable },
        );
        let line = out.lines().find(|l| l.starts_with("verdict:")).expect("a verdict line");
        assert!(line.starts_with("verdict: unchanged (identical_tree) — "), "{line}");
        assert!(line.contains("identical"), "the gloss states the observation: {line}");
    }

    /// A check states what the box holds; only this line states when that was true.
    #[test]
    fn an_observation_window_is_reported_when_there_is_no_value_line() {
        let obj = json!({"ok": true, "delivery": "target_hit", "observed_after_ms": 60});
        let out = rendered(&obj, changed());
        assert!(out.contains("observed: 60 ms after the action"), "{out}");
        // A fill's window already rides on its own line.
        let with_value = json!({
            "ok": true, "observed_after_ms": 60,
            "value": {"requested": "a", "actual": "", "verbatim": false, "observed_after_ms": 60},
        });
        let out = rendered(&with_value, not_kept());
        assert!(!out.contains("observed:"), "{out}");
    }

    /// Colour is opt-in on a tty and nothing else, which keeps CI captures and greps stable.
    #[test]
    fn nothing_ansi_leaks_out_when_colour_is_off() {
        let obj = json!({
            "ok": true,
            "value": {"requested": "SAVE20", "actual": "", "verbatim": false, "observed_after_ms": 60},
            "intercepted_by": {"tag": "DIV", "id": "scrim"},
            "values_lost": [{"uid": "n1", "role": "textbox", "was": "x"}],
        });
        let plain = rendered(&obj, not_kept());
        assert!(!plain.contains('\x1b'), "{plain}");
        // The same content is there when it is on, wrapped.
        let coloured = action_lines(&obj, not_kept(), Paint { enabled: true }).join("\n");
        assert!(coloured.contains("\x1b[1;31mNOT KEPT\x1b[0m"), "{coloured}");
        // Rule 1: every top-level line starts with its keyword. Indented continuations under
        // `values lost:` belong to the line above them.
        for line in coloured.lines().filter(|l| !l.starts_with("  ")) {
            assert!(line.contains(": "), "a top-level line with no keyword: {line}");
        }
    }

    #[test]
    fn a_long_value_is_cut_to_a_line() {
        let long = "x".repeat(400);
        let obj = json!({
            "ok": true,
            "value": {"requested": long, "actual": "", "verbatim": false, "observed_after_ms": 60},
        });
        let out = rendered(&obj, not_kept());
        assert!(out.contains('…'), "{out}");
        let value_line = out.lines().find(|l| l.starts_with("value:")).expect("a value line");
        assert!(
            value_line.len() < 200,
            "the whole 400-char string stays in --json: {}",
            value_line.len()
        );
    }
}
