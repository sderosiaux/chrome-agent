//! Turning a CLI invocation or a pipe command object into an [`Assertion`].
//!
//! Nothing here touches Chrome: an assertion that cannot be built is rejected before a
//! browser is asked anything.

use serde_json::Value;

use super::assert::{Assertion, Comparator, Kind, Want};


/// Build the assertion from the parsed CLI subcommand. clap's arg groups already enforce
/// "exactly one comparator" and "exactly one state"; the rest is checked here.
pub fn from_cli(what: &crate::cli::AssertWhat) -> Result<Assertion, crate::BoxError> {
    use crate::cli::AssertWhat as W;
    let assertion = match what {
        W::Value { selector, uid, equals, contains, matches } => Assertion {
            kind: Kind::Value(comparator(equals.as_deref(), contains.as_deref(), matches.as_deref(), "value")?),
            selector: selector.clone(),
            uid: uid.clone(),
        },
        W::Text { selector, uid, contains, matches } => Assertion {
            kind: Kind::Text(comparator(None, contains.as_deref(), matches.as_deref(), "text")?),
            selector: selector.clone(),
            uid: uid.clone(),
        },
        W::Url { equals, matches } => Assertion {
            kind: Kind::Url(comparator(equals.as_deref(), None, matches.as_deref(), "url")?),
            selector: None,
            uid: None,
        },
        W::State { selector, uid, checked, unchecked, selected, enabled, disabled, visible } => {
            let want = if *checked {
                Want::Checked
            } else if *unchecked {
                Want::Unchecked
            } else if let Some(option) = selected {
                Want::Selected(option.clone())
            } else if *enabled {
                Want::Enabled
            } else if *disabled {
                Want::Disabled
            } else if *visible {
                Want::Visible
            } else {
                return Err("assert state: name a state: --checked, --unchecked, --selected <option>, --enabled, --disabled or --visible.".into());
            };
            Assertion { kind: Kind::State(want), selector: selector.clone(), uid: uid.clone() }
        }
        W::Exists { selector, count, min } => Assertion {
            kind: Kind::Exists { count: *count, min: *min },
            selector: Some(selector.clone()),
            uid: None,
        },
    };
    Ok(assertion)
}

/// Build the assertion from a pipe/batch command object, shaped
/// `{"cmd":"assert","what":"value","selector":"#email","equals":"a@b.c"}`.
pub fn from_json(cmd: &Value) -> Result<Assertion, crate::BoxError> {
    let what = cmd
        .get("what")
        .and_then(Value::as_str)
        .ok_or("assert: missing \"what\" (one of value, text, url, state, exists)")?;
    let field = |name: &str| cmd.get(name).and_then(Value::as_str).map(str::to_string);
    let usize_field = |name: &str| -> Result<Option<usize>, crate::BoxError> {
        match cmd.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_u64()
                .map(|n| Some(n as usize))
                .ok_or_else(|| format!("assert exists: \"{name}\" must be a non-negative integer").into()),
        }
    };
    let flag = |name: &str| cmd.get(name).and_then(Value::as_bool).unwrap_or(false);

    let kind = match what {
        "value" => Kind::Value(comparator(
            field("equals").as_deref(),
            field("contains").as_deref(),
            field("matches").as_deref(),
            "value",
        )?),
        "text" => Kind::Text(comparator(
            field("equals").as_deref(),
            field("contains").as_deref(),
            field("matches").as_deref(),
            "text",
        )?),
        "url" => Kind::Url(comparator(
            field("equals").as_deref(),
            field("contains").as_deref(),
            field("matches").as_deref(),
            "url",
        )?),
        "state" => {
            let want = if flag("checked") {
                Want::Checked
            } else if flag("unchecked") {
                Want::Unchecked
            } else if let Some(option) = field("selected") {
                Want::Selected(option)
            } else if flag("enabled") {
                Want::Enabled
            } else if flag("disabled") {
                Want::Disabled
            } else if flag("visible") {
                Want::Visible
            } else {
                return Err("assert state: name a state: \"checked\", \"unchecked\", \"selected\", \"enabled\", \"disabled\" or \"visible\".".into());
            };
            Kind::State(want)
        }
        "exists" => Kind::Exists { count: usize_field("count")?, min: usize_field("min")? },
        other => {
            return Err(format!(
                "assert: unknown \"what\": {other} (expected value, text, url, state or exists)"
            )
            .into());
        }
    };
    Ok(Assertion { kind, selector: field("selector"), uid: field("uid") })
}

/// Pick the one comparator given, and refuse the combinations that mean nothing.
///
/// `text --equals` is refused: equality against `innerText` breaks on a cosmetic whitespace
/// edit. `url --contains` too: a URL is either exact or a pattern.
fn comparator(
    equals: Option<&str>,
    contains: Option<&str>,
    matches: Option<&str>,
    kind: &str,
) -> Result<Comparator, crate::BoxError> {
    let given = u8::from(equals.is_some()) + u8::from(contains.is_some()) + u8::from(matches.is_some());
    if given > 1 {
        return Err(format!("assert {kind}: give exactly one of --equals, --contains or --matches.").into());
    }
    if kind == "text" && equals.is_some() {
        return Err("assert text: --equals is not available on text (whitespace makes whole-text equality break on cosmetic edits). Use --contains, --matches, or `assert value --equals` for a form control.".into());
    }
    if kind == "url" && contains.is_some() {
        return Err("assert url: use --equals for the exact URL or --matches for a pattern.".into());
    }
    match (equals, contains, matches) {
        (Some(s), _, _) => Ok(Comparator::Equals(s.to_string())),
        (_, Some(s), _) => Ok(Comparator::Contains(s.to_string())),
        (_, _, Some(s)) => Ok(Comparator::Matches(s.to_string())),
        (None, None, None) => {
            Err(format!("assert {kind}: give one of --equals, --contains or --matches.").into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_refuses_equals_and_says_what_to_use_instead() {
        let err = comparator(Some("x"), None, None, "text").unwrap_err().to_string();
        assert!(err.contains("--contains"), "{err}");
        assert!(err.contains("assert value --equals"), "the alternative must be named: {err}");
    }

    #[test]
    fn a_comparator_is_required_and_exclusive() {
        assert!(comparator(None, None, None, "value").is_err());
        assert!(comparator(Some("a"), Some("b"), None, "value").is_err());
        assert_eq!(comparator(Some("a"), None, None, "value").unwrap(), Comparator::Equals("a".into()));
        assert_eq!(comparator(None, Some("a"), None, "text").unwrap(), Comparator::Contains("a".into()));
        assert_eq!(comparator(None, None, Some("a"), "url").unwrap(), Comparator::Matches("a".into()));
        assert!(comparator(None, Some("a"), None, "url").is_err());
    }

    #[test]
    fn from_json_maps_every_kind() {
        let cases = [
            (json!({"what": "value", "selector": "#a", "equals": "x"}), "value"),
            (json!({"what": "text", "contains": "x"}), "text"),
            (json!({"what": "url", "matches": "^https"}), "url"),
            (json!({"what": "state", "uid": "n1", "checked": true}), "state"),
            (json!({"what": "exists", "selector": ".row", "count": 3}), "exists"),
        ];
        for (cmd, expected) in cases {
            let a = from_json(&cmd).unwrap_or_else(|e| panic!("{cmd}: {e}"));
            assert_eq!(a.kind.name(), expected);
        }
        assert!(from_json(&json!({"what": "frobnicate"})).is_err());
        assert!(from_json(&json!({"selector": "#a"})).is_err(), "\"what\" is required");
        assert!(
            from_json(&json!({"what": "state", "uid": "n1"})).is_err(),
            "a state assertion with no state is not a claim"
        );
        assert!(
            from_json(&json!({"what": "exists", "selector": ".row", "count": -1})).is_err(),
            "a negative count is not a cardinality"
        );
    }
}
