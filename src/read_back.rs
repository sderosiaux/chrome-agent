//! What an action read back off the element it acted on, as response fields.
//!
//! Split out of `run_helpers.rs` for the repo's 1000-line file cap and re-exported from it, so
//! every call site stays `run_helpers::fill_value_report` / `check_report`. What lives here is
//! one idea: the four verbs that write a state and then look at the element again — `fill`, the
//! bulk fills, `select`, `check`/`uncheck` — and the fields they put on the response for it.
//!
//! All four write ONE key, `value`, in one vocabulary: `{requested, actual, verbatim}`. That is
//! not a stylistic preference. `pipe_report::postcondition_from_response` reads that single key
//! to decide whether an action has evidence about its own target (rung 11 of the ladder in
//! `verdict`), and a second key for the same idea would mean a second reader that can fall out
//! of step with the first — which is how `select` and `check` came to perform a read-back whose
//! answer the classifier never saw.

use serde_json::json;

/// What a fill put in, and what the page kept. Emitted on every fill so a value that was
/// reformatted, truncated or rejected is visible rather than hidden behind "Filled".
pub fn fill_value_report(outcome: &crate::element::FillOutcome) -> serde_json::Value {
    let mut v = if outcome.sensitive {
        json!({
            "redacted": true,
            "requested_length": outcome.requested.chars().count(),
            "actual_length": outcome.actual.as_ref().map(|a| a.chars().count()),
            "verbatim": outcome.verbatim(),
        })
    } else {
        json!({
            "requested": outcome.requested,
            "actual": outcome.actual,
            "verbatim": outcome.verbatim(),
        })
    };
    // "The field holds X" is only true as of a moment. Saying which moment is the only
    // honest form of the claim: a page can revert at any time, and one did at 400ms.
    v["observed_after_ms"] = json!(outcome.observed_after_ms);
    if let Some(caveat) = &outcome.caveat {
        v["caveat"] = json!(caveat);
    }
    v
}

/// Per-field report for a bulk fill: what each target was, and what it kept.
///
/// `key` is "uid" or "selector" depending on how the caller named the field. Secrets go
/// through the same redaction as a single fill — a bulk path that printed them would be a
/// way around it.
#[must_use]
pub fn bulk_fill_report(
    key: &str,
    outcomes: &[(String, crate::element::FillOutcome)],
) -> serde_json::Value {
    serde_json::Value::Array(
        outcomes
            .iter()
            .map(|(target, outcome)| json!({key: target, "value": fill_value_report(outcome)}))
            .collect(),
    )
}

/// What a select asked the element to hold, and what it still held when read back.
///
/// `select` already set the option, dispatched `change`, waited through the observation window
/// and re-read the selection — and then reported none of it, so a confirmed selection reached
/// the classifier as nothing at all and a fresh session answered `unknown / no_baseline` for an
/// action whose own target had been measured. `fill`'s vocabulary, because it is the same
/// measurement on a different kind of control.
///
/// `verbatim` is the read-back's own `kept`, which compares the option INDEX: two options
/// sharing a label are not each other, and a text comparison would call them equal.
#[must_use]
pub fn select_report(outcome: &crate::element::SelectOutcome) -> serde_json::Value {
    let value = if outcome.secret {
        // A `<select>` whose `autocomplete` names a password or a card number: the same
        // redaction a fill applies, for the same reason — this reaches stdout, the transcript
        // and any recording. Lengths still classify it, so a secret is never the silent case.
        json!({
            "redacted": true,
            "requested_length": outcome.text.chars().count(),
            "actual_length": outcome.actual.as_ref().map(|a| a.chars().count()),
            "verbatim": outcome.kept,
        })
    } else {
        json!({
            "requested": outcome.text,
            "actual": outcome.actual,
            "verbatim": outcome.kept,
        })
    };
    // At the top level, not inside `value`: the window covers the whole action — setting the
    // option AND looking again — and the same number in two fields on one response is two
    // fields that can disagree.
    json!({"observed_after_ms": outcome.observed_after_ms, "value": value})
}

/// Split a check/uncheck outcome into the message and the fields that go with it.
///
/// `observed_after_ms` and `value` are both absent when the element already held the desired
/// state: nothing was dispatched, so claiming an observation window afterwards would invent
/// one, and there is no write of ours to have been kept. An already-correct checkbox is not
/// evidence that THIS action changed anything, and `value_kept` there would be a claim about
/// a click that never happened.
///
/// `value` holds the checked state, not the `value` content attribute a checkbox submits —
/// nothing in this tool reports that, and one key for "what the element holds now" is what
/// keeps the postcondition reader single. `checked`/`unchecked` rather than `true`/`false`:
/// the words the message uses, and readable without knowing the probe's tokens.
#[must_use]
pub fn check_report(outcome: crate::element::CheckOutcome) -> (String, Option<serde_json::Value>) {
    let mut details = json!({"delivery": outcome.delivery.as_str()});
    if let Some(read) = outcome.read_back {
        details["observed_after_ms"] = json!(read.observed_after_ms);
        details["value"] = json!({
            "requested": read.requested,
            "actual": read.actual,
            "verbatim": read.actual == read.requested,
        });
    }
    (outcome.message, Some(details))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::element::{CheckOutcome, SelectOutcome};
    use crate::element_controls::CheckReadBack;
    use crate::verdict::{Delivery, Postcondition};

    fn selected(text: &str, actual: Option<&str>, secret: bool) -> SelectOutcome {
        SelectOutcome {
            text: text.to_string(),
            actual: actual.map(str::to_string),
            kept: actual == Some(text),
            secret,
            observed_after_ms: 60,
        }
    }

    /// The asymmetry this module closes: the same class of evidence, the same key, so the one
    /// reader in `pipe_report` reaches the same rung for both verbs.
    #[test]
    fn a_kept_selection_reads_as_the_same_postcondition_a_kept_fill_does() {
        let mut out = json!({"ok": true, "message": "Selected \"Beta\" on uid=n5"});
        let report = select_report(&selected("Beta", Some("Beta"), false));
        for (key, value) in report.as_object().expect("the report is an object") {
            out[key.as_str()] = value.clone();
        }
        assert_eq!(
            crate::pipe_report::postcondition_from_response(&out),
            Postcondition::Kept
        );
        assert_eq!(out["value"]["requested"], "Beta");
        assert_eq!(out["value"]["actual"], "Beta");
        assert_eq!(out["observed_after_ms"], 60, "the window stays where it was");
    }

    /// A secret `<select>` reports the lengths a fill would, and no option text. The state is
    /// still classifiable from them, which is the point of reporting lengths at all.
    #[test]
    fn a_secret_selection_reports_lengths_and_never_the_option() {
        let report = select_report(&selected("Sesame", Some("Sesame"), true));
        let value = &report["value"];
        assert_eq!(value["redacted"], true);
        assert_eq!(value["requested_length"], 6);
        assert_eq!(value["actual_length"], 6);
        assert_eq!(value["verbatim"], true);
        assert!(value.get("requested").is_none(), "no option text: {value}");
        assert!(value.get("actual").is_none(), "no option text: {value}");
        assert_eq!(
            crate::pipe_report::postcondition_from_response(&json!({"value": value.clone()})),
            Postcondition::Kept
        );
    }

    /// The message is the other place the text would reach stdout, so it is redacted too.
    #[test]
    fn a_secret_selection_keeps_its_option_out_of_the_message() {
        assert_eq!(selected("Sesame", Some("Sesame"), true).label(), "(redacted)");
        assert_eq!(selected("Beta", Some("Beta"), false).label(), "Beta");
    }

    /// A confirmed check reports the state it read back, in the words its message uses.
    #[test]
    fn a_confirmed_check_reads_as_kept() {
        let outcome = CheckOutcome {
            message: "Checked uid=n12".into(),
            read_back: Some(CheckReadBack {
                requested: "checked",
                actual: "checked".into(),
                observed_after_ms: 60,
            }),
            delivery: Delivery::TargetHit,
        };
        let (message, details) = check_report(outcome);
        assert_eq!(message, "Checked uid=n12");
        let details = details.expect("a check reports its delivery at least");
        assert_eq!(details["value"]["verbatim"], true);
        assert_eq!(details["observed_after_ms"], 60);
        assert_eq!(
            crate::pipe_report::postcondition_from_response(&details),
            Postcondition::Kept
        );
    }

    /// An element that already held the state claims nothing: nothing was dispatched, so
    /// there is no read-back of ours and no window to name. `no_baseline` on a fresh session
    /// is the honest answer there — the alternative is `value_kept` about a click that never
    /// happened.
    #[test]
    fn an_already_correct_check_reports_no_postcondition() {
        let (_, details) = check_report(CheckOutcome {
            message: "Already checked uid=n12".into(),
            read_back: None,
            delivery: Delivery::NotProbed,
        });
        let details = details.expect("the delivery field survives");
        assert!(details.get("value").is_none(), "{details}");
        assert!(details.get("observed_after_ms").is_none(), "{details}");
        assert_eq!(
            crate::pipe_report::postcondition_from_response(&details),
            Postcondition::NotRead
        );
    }
}
