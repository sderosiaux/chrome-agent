//! Turning a session that worked into a macro: what becomes a guard, and what is refused.
//!
//! An observation becomes an expectation only if it would still be true tomorrow, on the same
//! task, succeeding the same way. Everything else is dropped, never kept as context.
//!
//! KEPT: `delivery: target_hit` and only that reading, since the others describe a step that did
//! not do what it was asked; the verdict WORD, never the reason, which may move for the same
//! success; `verbatim: true`, which guards a secret field while its value stays out of the file;
//! `url_matches`, from the PATH only, because session ids and tracking parameters move the rest.
//!
//! REFUSED, written nowhere: the `changed` counters (a node count is not an intention, and it is
//! the most tempting field on the response), uids (numbered per document), any duration (a
//! slower machine is not a failure), `verdict_reason`, and the `delta` prose.
//!
//! `text_contains` and `exists` are in the format and deliberately not derived: which string
//! means success rather than a date or an order id is a judgement only the agent can make.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::macros::{Guards, Macro, Param, Step};

/// One command and the slim projection of its response the distiller reads. Slim because a pipe
/// session holds these for its whole life, and because retaining only what the whitelist needs
/// stops a page's text leaking into a later `macro record`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    pub cmd: Value,
    pub ok: bool,
    pub verdict: Option<String>,
    pub delivery: Option<String>,
    pub dispatched: Option<bool>,
    pub verbatim: Option<bool>,
    pub secret_value: bool,
    pub uid: Option<String>,
    pub role: Option<String>,
    pub name: Option<String>,
    pub landed_url: Option<String>,
}

impl Observed {
    /// Read one off a command and its response.
    #[must_use]
    pub fn read(cmd: &Value, response: &Value) -> Self {
        let value = response.get("value");
        Self {
            cmd: cmd.clone(),
            ok: response.get("ok").and_then(Value::as_bool).unwrap_or(false),
            verdict: string_at(response, "verdict"),
            delivery: string_at(response, "delivery"),
            dispatched: response.get("dispatched").and_then(Value::as_bool),
            verbatim: value.and_then(|v| v.get("verbatim")).and_then(Value::as_bool),
            // The fill report's own redaction flag marks the parameter, so the recorder and
            // `element::SECRET_FIELD` cannot disagree about what a secret is.
            secret_value: value
                .and_then(|v| v.get("redacted"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            uid: string_at(response, "uid"),
            role: string_at(response, "role"),
            name: string_at(response, "name"),
            landed_url: response
                .get("landed")
                .and_then(|l| l.get("final"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| string_at(response, "url")),
        }
    }
}

impl Observed {
    /// The same reading, with the snapshot that was current when the command ran. The uid path
    /// of an action reports no role or name, so without this only selector-aimed steps are
    /// recordable.
    #[must_use]
    pub fn read_with_snapshot(cmd: &Value, response: &Value, snapshot: Option<&str>) -> Self {
        let mut observed = Self::read(cmd, response);
        if observed.role.is_none()
            && let (Some(uid), Some(snapshot)) = (observed.uid.as_deref(), snapshot)
            && let Some((role, name)) = role_and_name(snapshot, uid)
        {
            observed.role = Some(role);
            observed.name = Some(name);
        }
        observed
    }
}

/// The role and accessible name a snapshot gives one uid, read from the line shape `diff` also
/// parses. A node with no name yields nothing: a locator matching every unnamed button of that
/// role is worse than no locator.
#[must_use]
pub fn role_and_name(snapshot: &str, uid: &str) -> Option<(String, String)> {
    snapshot.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix("uid=")?;
        let mut tokens = rest.split_whitespace();
        if tokens.next()? != uid {
            return None;
        }
        let role = tokens.next()?.to_string();
        let name = rest.split_once('"')?.1.split_once('"')?.0;
        (!name.is_empty()).then(|| (role, name.to_string()))
    })
}

fn string_at(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

/// What the distiller refused, and why — one line per dropped step, in the caller's hands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub index: usize,
    pub reason: String,
}

/// The result of distilling: a macro, and everything that did not make it in.
pub struct Distilled {
    pub macro_file: Macro,
    /// Steps dropped as exploration or as failures. Not errors: the fumbling is meant to go.
    pub dropped: Vec<Refusal>,
    /// Steps that acted on the page and COULD NOT be recorded, so the macro is short of the task.
    pub refused: Vec<Refusal>,
}

/// Commands that change the page; anything else is exploration and never becomes a step. Built
/// from `pipe_report::mutates_page` so the lists cannot drift, plus `goto`, excluded there only
/// because it needs no change report.
fn is_step(cmd_name: &str) -> bool {
    cmd_name == "goto"
        || cmd_name == "navigate"
        || cmd_name == "open"
        || cmd_name == "go"
        || crate::pipe_report::mutates_page(cmd_name)
}

fn cmd_name(cmd: &Value) -> &str {
    cmd.get("cmd").and_then(Value::as_str).unwrap_or_default()
}

/// Where the task begins, by default: the last successful `goto` in the session. Retrospective,
/// since the agent only finds out afterwards that the task worked. `--from N` overrides it and
/// the response states which entry it used.
#[must_use]
pub fn default_start(history: &[Observed]) -> usize {
    history
        .iter()
        .enumerate()
        .rfind(|(_, o)| o.ok && matches!(cmd_name(&o.cmd), "goto" | "navigate" | "open" | "go"))
        .map_or(0, |(index, _)| index)
}

/// Distil a session's history into a macro.
pub fn distil(name: &str, history: &[Observed], from: usize) -> Result<Distilled, crate::BoxError> {
    let mut steps = Vec::new();
    let mut dropped = Vec::new();
    let mut refused = Vec::new();
    let mut params: BTreeMap<String, Param> = BTreeMap::new();
    let mut site = None;

    for (index, observed) in history.iter().enumerate().skip(from) {
        let verb = cmd_name(&observed.cmd);
        if !is_step(verb) {
            dropped.push(Refusal { index, reason: format!("`{verb}` reads the page; a macro keeps what changes it") });
            continue;
        }
        if !observed.ok {
            dropped.push(Refusal { index, reason: format!("`{verb}` failed, and a macro is the path that worked") });
            continue;
        }
        if observed.dispatched == Some(false) {
            dropped.push(Refusal {
                index,
                reason: format!("`{verb}` dispatched nothing, so the page never saw it"),
            });
            continue;
        }

        let action = match locate(observed) {
            Ok(action) => action,
            Err(reason) => {
                refused.push(Refusal { index, reason });
                continue;
            }
        };
        if site.is_none() {
            site = observed.landed_url.as_deref().and_then(host_of);
        }
        let (action, declared) = parameterise(action, observed);
        for (key, param) in declared {
            params.insert(key, param);
        }
        let expect = guards_for(observed);
        let unguarded = expect.is_empty().then(|| unguarded_reason(observed));
        steps.push(Step { action, expect, unguarded });
    }

    if steps.is_empty() {
        return Err(format!(
            "Nothing to record from step {from} on: {} command(s) were exploration or failures{}. \
             `macro record --from N` names the first step of the task.",
            dropped.len(),
            if refused.is_empty() {
                String::new()
            } else {
                format!(", and {} could not be given a durable locator", refused.len())
            }
        )
        .into());
    }

    Ok(Distilled {
        macro_file: Macro {
            name: name.to_string(),
            site,
            recorded_at: Some(now()),
            params,
            steps,
        },
        dropped,
        refused,
    })
}

/// The command as it will be written: the same verb, aimed at something that survives. In order:
/// the CSS selector the agent used, the accessible role and name, then a refusal. A uid is never
/// written (numbered per document), and `--xy` names no element.
fn locate(observed: &Observed) -> Result<Value, String> {
    let mut action = observed.cmd.clone();
    // `_record` is a session's business, not a step's.
    if let Some(map) = action.as_object_mut() {
        map.remove("_record");
        map.remove("inspect");
    }
    let verb = cmd_name(&action);
    if action.get("xy").is_some() {
        return Err(format!(
            "`{verb} --xy` names no element: a coordinate is not a locator, and the next \
             layout moves it"
        ));
    }
    if action.get("selector").is_some() || action.get("uid").is_none() {
        // Either it already carries a durable locator, or it targets nothing (goto, press).
        return Ok(action);
    }
    // Aimed by uid: role and accessible name survive a navigation where the uid does not.
    let (Some(role), Some(name)) = (observed.role.as_deref(), observed.name.as_deref()) else {
        return Err(format!(
            "`{verb}` was aimed by uid and the response carried no role and name to replace it \
             with. A uid is numbered per document and means nothing tomorrow, so the step is \
             refused rather than recorded as one that works once. Re-run the step with \
             --selector, or inspect the page so the action can report what it hit."
        ));
    };
    if let Some(map) = action.as_object_mut() {
        map.remove("uid");
        map.insert("role".into(), json!(role));
        map.insert("name".into(), json!(name));
    }
    Ok(action)
}

/// Replace a recorded secret with a declared parameter, and leave the rest alone. The value is
/// not in the response to begin with, so the file gets `{{param}}` and a declaration.
fn parameterise(mut action: Value, observed: &Observed) -> (Value, Vec<(String, Param)>) {
    if !observed.secret_value {
        return (action, Vec::new());
    }
    let key = secret_param_name(&action);
    if let Some(map) = action.as_object_mut() {
        map.insert("value".into(), json!(format!("{{{{{key}}}}}")));
    }
    (action, vec![(key, Param { required: true, secret: true })])
}

/// A name for the secret, taken from what the step aimed at rather than invented.
fn secret_param_name(action: &Value) -> String {
    let from_target = action
        .get("selector")
        .or_else(|| action.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("secret");
    let cleaned: String = from_target
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() { "secret".to_string() } else { trimmed.to_string() }
}

/// The whitelist, applied to one observation.
fn guards_for(observed: &Observed) -> Guards {
    let mut guards = Guards::default();
    if observed.delivery.as_deref() == Some("target_hit") {
        guards.delivery = Some("target_hit".into());
    }
    // `not_checked` means the caller declined to look and `unknown` that the tool could not
    // tell; promising either would demand that tomorrow's run be equally blind.
    if let Some(word) = observed.verdict.as_deref()
        && !matches!(word, "not_checked" | "unknown")
    {
        guards.verdict = Some(word.to_string());
    }
    if observed.verbatim == Some(true) {
        guards.verbatim = Some(true);
    }
    if let Some(pattern) = observed.landed_url.as_deref().and_then(url_pattern) {
        guards.url_matches = Some(pattern);
    }
    guards
}

/// Why a step ended up with nothing to promise, in words a reader can act on.
fn unguarded_reason(observed: &Observed) -> String {
    match observed.verdict.as_deref() {
        Some("not_checked") => {
            "the session ran with --verdict off, so nothing was observed to promise".into()
        }
        Some("unknown") => {
            "the tool could not tell what this action did, and an expectation of not knowing \
             is not an expectation"
                .into()
        }
        _ => "this action reported no delivery, no verdict and no read-back".into(),
    }
}

/// A pattern for where a navigation landed: the path, escaped, and nothing else. A path of `/`
/// matches every site, so it yields nothing rather than a guard that cannot fail.
fn url_pattern(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = after_scheme.find('/').map(|i| &after_scheme[i..])?;
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let path = path.trim_end_matches('/');
    if path.is_empty() || path == "/" {
        return None;
    }
    Some(escape(path))
}

/// Escape `regex-lite` metacharacters: a path may carry `.` or `+`.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn host_of(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = after_scheme.split(['/', '?', '#']).next()?;
    (!host.is_empty()).then(|| host.to_string())
}

/// Seconds since the epoch, the stamp this crate already uses elsewhere.
fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!("{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed(cmd: &Value, response: &Value) -> Observed {
        Observed::read(cmd, response)
    }

    /// The response carries a dozen fields and only the whitelisted ones survive.
    #[test]
    fn a_click_that_landed_keeps_its_delivery_and_its_verdict_word_and_nothing_else() {
        let step = observed(&json!({"cmd": "click", "selector": "[data-test=billing]"}),
            &json!({
                "ok": true, "delivery": "target_hit", "verdict": "changed",
                "verdict_reason": "tree_delta", "next": "proceed",
                "uid": "n12", "waited_ms": 91, "observed_after_ms": 60,
                "changed": {"added": 450, "removed": 2},
                "delta": "+ uid=n200 heading \"Success\""
            }),
        );
        let guards = guards_for(&step);
        assert_eq!(guards.delivery.as_deref(), Some("target_hit"));
        assert_eq!(guards.verdict.as_deref(), Some("changed"));
        assert_eq!(guards.verbatim, None);
        assert_eq!(guards.url_matches, None);
        // The blacklist, on a shape carrying all of it at once.
        let written = serde_json::to_string(&guards).unwrap();
        for forbidden in ["450", "tree_delta", "n12", "91", "60", "delta", "added"] {
            assert!(!written.contains(forbidden), "{forbidden} survived into {written}");
        }
    }

    /// The two words that must never become an expectation.
    #[test]
    fn a_verdict_that_says_nothing_is_not_promised() {
        for word in ["not_checked", "unknown"] {
            let step = observed(&json!({"cmd": "click", "selector": "#x"}),
                &json!({"ok": true, "verdict": word, "verdict_reason": "reporting_disabled"}),
            );
            let guards = guards_for(&step);
            assert_eq!(guards.verdict, None, "{word} became a guard");
            assert!(guards.is_empty());
            assert!(unguarded_reason(&step).contains(if word == "not_checked" { "--verdict off" } else { "could not tell" }));
        }
    }

    #[test]
    fn a_navigation_keeps_the_path_and_not_the_query() {
        assert_eq!(url_pattern("https://example.com/account?sid=9f2&utm=x"), Some("/account".into()));
        assert_eq!(url_pattern("https://example.com/orders/42/"), Some("/orders/42".into()));
        // A bare slash matches everything, which is not a guard.
        assert_eq!(url_pattern("https://example.com/"), None);
        assert_eq!(url_pattern("https://example.com"), None);
        // Escaped, because a path may carry regex metacharacters.
        assert_eq!(url_pattern("https://e.com/v1.0/list+all"), Some(r"/v1\.0/list\+all".into()));
    }

    /// The locator rules, all three branches.
    #[test]
    fn a_uid_becomes_a_role_and_a_name_or_the_step_is_refused() {
        let by_selector = observed(&json!({"cmd": "click", "selector": "#confirm"}), &json!({"ok": true}));
        assert_eq!(locate(&by_selector).unwrap()["selector"], "#confirm");

        let by_uid = observed(&json!({"cmd": "click", "uid": "n12"}),
            &json!({"ok": true, "uid": "n12", "role": "button", "name": "Manage billing"}),
        );
        let action = locate(&by_uid).expect("role and name are enough");
        assert!(action.get("uid").is_none(), "the uid must not survive: {action}");
        assert_eq!(action["role"], "button");
        assert_eq!(action["name"], "Manage billing");

        let anonymous = observed(&json!({"cmd": "click", "uid": "n12"}), &json!({"ok": true, "uid": "n12"}));
        let error = locate(&anonymous).expect_err("nothing durable to write");
        assert!(error.contains("numbered per document"), "{error}");
        assert!(error.contains("--selector"), "the way out is named: {error}");

        let coordinates = observed(&json!({"cmd": "click", "xy": [10, 20]}), &json!({"ok": true}));
        assert!(locate(&coordinates).unwrap_err().contains("not a locator"));
    }

    /// A uid's role and name come from the accessibility tree, since the uid path of an action
    /// reports neither.
    #[test]
    fn a_uid_takes_its_role_and_name_from_the_snapshot_that_was_current() {
        let snapshot = "uid=n1 RootWebArea \"Account\"\n  uid=n12 button \"Manage billing\"\n  uid=n13 generic\n";
        assert_eq!(
            role_and_name(snapshot, "n12"),
            Some(("button".to_string(), "Manage billing".to_string()))
        );
        // A node with no name yields nothing rather than a locator matching every generic.
        assert_eq!(role_and_name(snapshot, "n13"), None);
        assert_eq!(role_and_name(snapshot, "n99"), None);

        let observed = Observed::read_with_snapshot(
            &json!({"cmd": "click", "uid": "n12"}),
            &json!({"ok": true, "uid": "n12", "verdict": "changed"}),
            Some(snapshot),
        );
        let action = locate(&observed).expect("the snapshot supplied the locator");
        assert_eq!(action["role"], "button");
        assert_eq!(action["name"], "Manage billing");
        assert!(action.get("uid").is_none());
    }

    /// A secret never reaches the file; what is written is a declaration.
    #[test]
    fn a_secret_field_becomes_a_declared_parameter_and_no_value() {
        let fill = observed(&json!({"cmd": "fill", "selector": "#password", "value": "hunter2"}),
            &json!({"ok": true, "verdict": "changed",
                   "value": {"redacted": true, "verbatim": true, "actual_length": 7}}),
        );
        let (action, declared) = parameterise(locate(&fill).unwrap(), &fill);
        assert_eq!(action["value"], "{{password}}");
        assert_eq!(declared.len(), 1);
        assert_eq!(declared[0].0, "password");
        assert!(declared[0].1.secret);
        assert!(!serde_json::to_string(&action).unwrap().contains("hunter2"));
        // The guard is still the read-back: a secret is verified, never printed.
        assert_eq!(guards_for(&fill).verbatim, Some(true));
    }

    /// The distiller end to end: a goto, some exploration, a dead end, and actions that worked.
    #[test]
    fn distilling_keeps_the_path_and_drops_the_fumbling() {
        let history = vec![
            observed(&json!({"cmd": "goto", "url": "https://example.com/account"}),
                     &json!({"ok": true, "landed": {"final": "https://example.com/account"}})),
            observed(&json!({"cmd": "inspect"}), &json!({"ok": true, "snapshot": "…"})),
            observed(&json!({"cmd": "click", "selector": "#nope"}),
                     &json!({"ok": false, "error": "No element matches selector: #nope"})),
            observed(&json!({"cmd": "click", "selector": "[data-test=billing]"}),
                     &json!({"ok": true, "delivery": "target_hit", "verdict": "changed"})),
            observed(&json!({"cmd": "eval", "expression": "1+1"}), &json!({"ok": true, "result": 2})),
            observed(&json!({"cmd": "fill", "selector": "#email", "value": "ada@example.com"}),
                     &json!({"ok": true, "verdict": "changed", "value": {"verbatim": true}})),
        ];
        let distilled = distil("cancel", &history, 0).expect("a macro");
        let steps = &distilled.macro_file.steps;
        assert_eq!(steps.len(), 3, "goto, click, fill — and nothing else: {steps:?}");
        assert_eq!(steps[0].action["cmd"], "goto");
        assert_eq!(steps[0].expect.url_matches.as_deref(), Some("/account"));
        assert_eq!(steps[1].expect.delivery.as_deref(), Some("target_hit"));
        assert_eq!(steps[2].expect.verbatim, Some(true));
        assert_eq!(distilled.macro_file.site.as_deref(), Some("example.com"));
        assert_eq!(distilled.dropped.len(), 3, "{:?}", distilled.dropped);
        assert!(distilled.refused.is_empty());
        // Every dropped step says why: a macro shorter than the task must be visible.
        assert!(distilled.dropped.iter().any(|r| r.reason.contains("reads the page")));
        assert!(distilled.dropped.iter().any(|r| r.reason.contains("failed")));
    }

    /// A step that acted and could not be written is a refusal, not a silent drop.
    #[test]
    fn an_unwritable_step_is_refused_and_named() {
        let history = vec![
            observed(&json!({"cmd": "goto", "url": "https://example.com/a"}), &json!({"ok": true, "landed": {"final": "https://example.com/a"}})),
            observed(&json!({"cmd": "click", "uid": "n7"}), &json!({"ok": true, "uid": "n7", "verdict": "changed"})),
        ];
        let distilled = distil("x", &history, 0).expect("the goto still records");
        assert_eq!(distilled.macro_file.steps.len(), 1);
        assert_eq!(distilled.refused.len(), 1);
        assert_eq!(distilled.refused[0].index, 1);
        assert!(distilled.refused[0].reason.contains("aimed by uid"));
    }

    /// The default start is the last navigation, announced rather than guessed in silence.
    #[test]
    fn the_default_start_is_the_last_navigation() {
        let history = vec![
            observed(&json!({"cmd": "goto", "url": "https://a.example/1"}), &json!({"ok": true})),
            observed(&json!({"cmd": "click", "selector": "#x"}), &json!({"ok": true})),
            observed(&json!({"cmd": "goto", "url": "https://a.example/2"}), &json!({"ok": true})),
            observed(&json!({"cmd": "click", "selector": "#y"}), &json!({"ok": true})),
        ];
        assert_eq!(default_start(&history), 2);
        assert_eq!(default_start(&[]), 0);
    }

    /// A step whose observation produced nothing says so IN THE FILE, or it is trusted like a
    /// guarded one.
    #[test]
    fn a_step_with_no_guard_carries_its_reason() {
        let history = vec![observed(&json!({"cmd": "press", "key": "Enter"}),
            &json!({"ok": true, "verdict": "not_checked", "verdict_reason": "reporting_disabled"}),
        )];
        let distilled = distil("x", &history, 0).expect("a macro");
        let step = &distilled.macro_file.steps[0];
        assert!(step.expect.is_empty());
        assert!(step.unguarded.as_deref().unwrap().contains("--verdict off"));
        // It survives the round trip, so a reader of the file sees it too.
        let text = serde_json::to_string(&distilled.macro_file).unwrap();
        assert!(text.contains("unguarded"), "{text}");
    }
}
