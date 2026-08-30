//! What an action read back off the element it acted on, as response fields: `fill`, the bulk
//! fills, `select`, `check`/`uncheck`. Re-exported from `run_helpers`.
//!
//! All four write ONE key, `value`, as `{requested, actual, verbatim}`, because
//! `pipe_report::postcondition_from_response` reads exactly that key to decide whether an action
//! has evidence about its own target (rung 11 of the `verdict` ladder).

use serde_json::json;

/// What a fill put in and what the page kept, so a reformatted or truncated value is visible.
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
    // "The field holds X" is only true of a moment: one measured page reverted at 400 ms.
    v["observed_after_ms"] = json!(outcome.observed_after_ms);
    if let Some(caveat) = &outcome.caveat {
        v["caveat"] = json!(caveat);
    }
    v
}

/// Per-field report for a bulk fill. `key` is "uid" or "selector"; secrets go through the same
/// redaction as a single fill.
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

/// What a select asked the element to hold, and what it still held, in `fill`'s vocabulary.
/// `verbatim` is the read-back's `kept`, which compares the option INDEX — two options sharing a
/// label are not each other.
#[must_use]
pub fn select_report(outcome: &crate::element::SelectOutcome) -> serde_json::Value {
    let value = if outcome.secret {
        // Lengths still classify it, so a secret is never the silent case.
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
    // Top level, not inside `value`: the window covers setting the option AND looking again.
    json!({"observed_after_ms": outcome.observed_after_ms, "value": value})
}

/// Split a check/uncheck outcome into the message and the fields that go with it. Both
/// `observed_after_ms` and `value` are absent when the element already held the state: nothing
/// was dispatched, so there is no window and no write of ours.
///
/// `value` holds the checked state, not the `value` attribute a checkbox submits — one key for
/// "what the element holds now". Spelled `checked`/`unchecked`, not the probe's `true`/`false`.
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

    /// Same evidence, same key, so `pipe_report` reaches the same rung for both verbs.
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

    /// A secret `<select>` reports lengths and no option text — still classifiable.
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

    /// The message is the other place the text would reach stdout.
    #[test]
    fn a_secret_selection_keeps_its_option_out_of_the_message() {
        assert_eq!(selected("Sesame", Some("Sesame"), true).label(), "(redacted)");
        assert_eq!(selected("Beta", Some("Beta"), false).label(), "Beta");
    }

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

    /// An element that already held the state claims nothing; the alternative is `value_kept`
    /// about a click never made.
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
