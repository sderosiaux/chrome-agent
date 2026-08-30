//! Running a macro: the same dispatcher as `pipe`, one guard check per step, and a full stop.
//!
//! No repair, no retry, no branch, no skip. A guard that does not hold ends the run there and
//! reports the step, the expectation, what was there instead, and the action's own `next`. Only
//! the guarding lives here; `pipe::dispatch_on` executes.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::cli::Cli;
use crate::commands::assert::{Assertion, Comparator, Kind};
use crate::macros::{Guards, Macro, Step};
use crate::pipe_emulation::EmulationRecovery;

/// A macro that stopped: it has already printed its own report, and `main` only needs the exit
/// code. Same shape as `commands::assert::NotHeld` — a generic handler printing a second line
/// after it is how `--json` stops being one object per command.
#[derive(Debug)]
pub struct Stopped {
    report: Value,
    json_mode: bool,
}

impl Stopped {
    #[must_use]
    pub const fn new(report: Value, json_mode: bool) -> Self {
        Self { report, json_mode }
    }

    /// Print on the stream the caller reads, and answer with the exit code.
    pub fn report(&self) -> i32 {
        if self.json_mode {
            crate::run_helpers::json_output(&self.report);
        } else {
            eprint!("{}", render_run(&self.report));
        }
        1
    }
}

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.report.get("error").and_then(Value::as_str).unwrap_or("the macro stopped"))
    }
}

impl std::error::Error for Stopped {}

/// What happened to one step.
struct StepOutcome {
    response: Value,
    /// The guard that did not hold, if one did not.
    failure: Option<GuardFailure>,
}

struct GuardFailure {
    guard: &'static str,
    expected: String,
    observed: String,
}

/// Run a macro end to end. Returns the report as JSON rather than printing it; the CLI prints,
/// and the shape is the same in both modes.
pub async fn run(
    cli: &Cli,
    name: &str,
    vars: &BTreeMap<String, String>,
) -> Result<Value, crate::BoxError> {
    let macro_file = Macro::load(name)?;
    // Before the browser: a run that cannot finish for want of a password must not first open
    // a page and act on it.
    macro_file.bind(vars)?;

    let mut session = crate::pipe::open_session(cli).await?;
    let mut recovery = EmulationRecovery::new(
        &session.client,
        &session.store,
        &cli.browser,
        &cli.page,
    )
    .await;

    // A verdict is a comparison and needs a baseline, which distillation drops with the
    // exploration that produced it. Without this, every run's first guarded step reports
    // `unknown / no_baseline`. One snapshot at the start and one after each navigation.
    let mut needs_baseline = true;
    let mut steps_done = Vec::new();
    for (index, step) in macro_file.steps.iter().enumerate() {
        let action = macro_file.resolve(step, vars)?;
        let action = match resolve_locator(&mut session, cli, &action, &mut recovery).await {
            Ok(action) => action,
            Err(e) => {
                let report = stopped(&macro_file, index, step, &json!({"ok": false, "error": e.to_string()}), None, &steps_done);
                crate::session::save_session(&mut session.store)?;
                return Ok(report);
            }
        };

        if needs_baseline && step.expect.verdict.is_some() {
            let _ = crate::pipe::dispatch_on(&mut session, cli, &json!({"cmd": "inspect"}), &mut recovery).await;
            needs_baseline = false;
        }
        let outcome = execute(&mut session, cli, step, &action, &mut recovery).await;
        // A navigation replaces the document, so the next step's baseline is the old page.
        if outcome.response.get("landed").is_some()
            || outcome.response.get("verdict").and_then(Value::as_str) == Some("navigated")
        {
            needs_baseline = true;
        }
        let ok = outcome.response.get("ok").and_then(Value::as_bool).unwrap_or(false);
        if !ok || outcome.failure.is_some() {
            let report = stopped(
                &macro_file,
                index,
                step,
                &outcome.response,
                outcome.failure.as_ref(),
                &steps_done,
            );
            crate::session::save_session(&mut session.store)?;
            return Ok(report);
        }
        steps_done.push(json!({
            "step": index,
            "cmd": action.get("cmd").cloned().unwrap_or_default(),
            "guards": guard_names(&step.expect),
            "unguarded": step.unguarded,
        }));
    }

    crate::session::save_session(&mut session.store)?;
    let unguarded = macro_file.steps.iter().filter(|s| s.expect.is_empty()).count();
    Ok(json!({
        "ok": true,
        "macro": macro_file.name,
        "steps_run": steps_done.len(),
        "steps": steps_done,
        // Stated on success too: five steps of which two promised nothing is not the same
        // evidence as five guarded ones.
        "unguarded_steps": unguarded,
    }))
}

/// Dispatch one step and check its guards.
async fn execute(
    session: &mut crate::pipe::Session,
    cli: &Cli,
    step: &Step,
    action: &Value,
    recovery: &mut EmulationRecovery,
) -> StepOutcome {
    let response = crate::pipe::dispatch_on(session, cli, action, recovery).await;
    if response.get("ok").and_then(Value::as_bool) != Some(true) {
        return StepOutcome { response, failure: None };
    }
    // Response-first: these cost nothing, and a step that already failed one does not pay for
    // a page read to fail a second.
    for (guard, expected) in step.expect.response_guards() {
        let observed = observed_for(guard, &response);
        if observed.as_deref() != Some(expected.as_str()) {
            return StepOutcome {
                failure: Some(GuardFailure {
                    guard,
                    expected,
                    observed: observed.unwrap_or_else(|| "absent from the response".into()),
                }),
                response,
            };
        }
    }
    match check_page_guards(session, &step.expect).await {
        Ok(Some(failure)) => StepOutcome { response, failure: Some(failure) },
        Ok(None) => StepOutcome { response, failure: None },
        Err(e) => StepOutcome {
            failure: Some(GuardFailure {
                guard: "page",
                expected: "a readable page".into(),
                observed: e.to_string(),
            }),
            response,
        },
    }
}

/// The value a response carries for a response-settled guard.
fn observed_for(guard: &str, response: &Value) -> Option<String> {
    match guard {
        "delivery" | "verdict" => {
            response.get(guard).and_then(Value::as_str).map(str::to_string)
        }
        "verbatim" => response
            .get("value")
            .and_then(|v| v.get("verbatim"))
            .and_then(Value::as_bool)
            .map(|b| b.to_string()),
        _ => None,
    }
}

/// The guards that need a look at the page, read through `assert`'s own readers rather than a
/// second implementation that could disagree with them.
async fn check_page_guards(
    session: &crate::pipe::Session,
    guards: &Guards,
) -> Result<Option<GuardFailure>, crate::BoxError> {
    // Empty on purpose: a macro carries no uid, so every assertion below targets a selector
    // or the URL.
    let uid_map = std::collections::HashMap::new();
    let mut checks: Vec<(&'static str, Assertion)> = Vec::new();
    if let Some(pattern) = &guards.url_matches {
        checks.push((
            "url_matches",
            Assertion { kind: Kind::Url(Comparator::Matches(pattern.clone())), selector: None, uid: None },
        ));
    }
    if let Some(text) = &guards.text_contains {
        checks.push((
            "text_contains",
            Assertion {
                kind: Kind::Text(Comparator::Contains(text.clone())),
                selector: Some("body".into()),
                uid: None,
            },
        ));
    }
    if let Some(exists) = &guards.exists {
        checks.push((
            "exists",
            Assertion {
                kind: Kind::Exists { count: None, min: Some(exists.min) },
                selector: Some(exists.selector.clone()),
                uid: None,
            },
        ));
    }
    for (guard, assertion) in checks {
        let outcome = crate::commands::assert::run(&session.client, &uid_map, &assertion).await?;
        if !outcome.held {
            let json = outcome.assertion_json();
            return Ok(Some(GuardFailure {
                guard,
                expected: compact(json.get("expected")),
                observed: compact(json.get("actual")),
            }));
        }
    }
    Ok(None)
}

fn compact(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => "unknown".into(),
    }
}

/// Turn a `role` + `name` locator into the uid the dispatcher takes, resolved against the page
/// it finds. One match is the target; none is a page that no longer has the control, and several
/// are an ambiguity a macro may not settle by picking one.
async fn resolve_locator(
    session: &mut crate::pipe::Session,
    cli: &Cli,
    action: &Value,
    recovery: &mut EmulationRecovery,
) -> Result<Value, crate::BoxError> {
    let (Some(role), Some(name)) = (
        action.get("role").and_then(Value::as_str),
        action.get("name").and_then(Value::as_str),
    ) else {
        return Ok(action.clone());
    };
    // Through the dispatcher, so the snapshot is the one the store keeps and the uid it hands
    // back resolves for the next command.
    let snapshot = crate::pipe::dispatch_on(session, cli, &json!({"cmd": "inspect"}), recovery).await;
    let text = snapshot.get("snapshot").and_then(Value::as_str).unwrap_or_default();
    let matches = find(text, role, name);
    let uid = match matches.as_slice() {
        [uid] => uid.clone(),
        [] => {
            return Err(format!(
                "No {role} named \"{name}\" on this page. The macro was recorded against one, so \
                 either the page changed or this is not the page the macro expects. Run `inspect` \
                 to see what is there; a macro does not guess a replacement."
            )
            .into());
        }
        several => {
            return Err(format!(
                "{} elements match {role} \"{name}\", and a macro may not pick one of them. Record \
                 the step again with a CSS selector that names exactly one.",
                several.len()
            )
            .into());
        }
    };
    let mut resolved = action.clone();
    if let Some(map) = resolved.as_object_mut() {
        map.remove("role");
        map.remove("name");
        map.insert("uid".into(), json!(uid));
    }
    Ok(resolved)
}

/// Every uid in a snapshot whose line is exactly this role with this accessible name. Matched on
/// the whole name, never a substring: "Save" and "Save draft" are different controls.
fn find(snapshot: &str, role: &str, name: &str) -> Vec<String> {
    snapshot
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            let rest = trimmed.strip_prefix("uid=")?;
            let mut tokens = rest.split_whitespace();
            let uid = tokens.next()?;
            if tokens.next()? != role {
                return None;
            }
            let quoted = rest.split_once('"')?.1;
            let found = quoted.split_once('"')?.0;
            (found == name).then(|| uid.to_string())
        })
        .collect()
}

/// The report of a run that stopped: everything needed to decide what to do, and nothing that
/// pretends to have decided it.
fn stopped(
    macro_file: &Macro,
    index: usize,
    step: &Step,
    response: &Value,
    failure: Option<&GuardFailure>,
    done: &[Value],
) -> Value {
    let mut report = json!({
        "ok": false,
        "macro": macro_file.name,
        "stopped_at": index,
        "steps_run": done.len(),
        "steps": done.to_vec(),
        "cmd": step.action.get("cmd").cloned().unwrap_or_default(),
    });
    if let Some(failure) = failure {
        report["guard"] = json!(failure.guard);
        report["expected"] = json!(failure.expected);
        report["observed"] = json!(failure.observed);
        report["error"] = json!(format!(
            "Step {index} of macro '{}' ran and its guard did not hold: expected {} {}, observed {}.",
            macro_file.name, failure.guard, failure.expected, failure.observed
        ));
    } else {
        report["error"] = json!(format!(
            "Step {index} of macro '{}' did not run: {}",
            macro_file.name,
            response.get("error").and_then(Value::as_str).unwrap_or("the command failed")
        ));
    }
    // The action's own words, carried rather than summarised: `next` is the token agents branch
    // on, and swallowing it would make the caller guess what the response already says.
    for key in ["next", "verdict", "verdict_reason", "verdict_hint", "hint", "delivery", "intercepted_by"] {
        if let Some(value) = response.get(key) {
            report[key] = value.clone();
        }
    }
    report["stop"] = json!(
        "The run stopped here. Nothing was repaired, retried or skipped: the steps after this \
         one did not happen."
    );
    report
}

fn guard_names(guards: &Guards) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = guards.response_guards().into_iter().map(|(g, _)| g).collect();
    if guards.url_matches.is_some() {
        names.push("url_matches");
    }
    if guards.text_contains.is_some() {
        names.push("text_contains");
    }
    if guards.exists.is_some() {
        names.push("exists");
    }
    names
}

/// One run, for a person: what ran, and where it stopped. The failing step first, then the
/// action's own `next`, then the sentence saying the rest did not happen.
#[must_use]
pub fn render_run(report: &Value) -> String {
    if report["ok"].as_bool().unwrap_or(false) {
        let mut out = format!("{} step(s) ran, every guard held\n", report["steps_run"]);
        if report["unguarded_steps"].as_u64().unwrap_or(0) > 0 {
            out.push_str(&format!(
                "{} of them promised nothing, so nothing about them was verified\n",
                report["unguarded_steps"]
            ));
        }
        return out;
    }
    let mut out = format!("stopped at step {}\n", report["stopped_at"]);
    if let Some(error) = report["error"].as_str() {
        out.push_str(&format!("{error}\n"));
    }
    if let Some(guard) = report["guard"].as_str() {
        out.push_str(&format!(
            "guard: {guard} expected {}, observed {}\n",
            report["expected"].as_str().unwrap_or_default(),
            report["observed"].as_str().unwrap_or_default()
        ));
    }
    if let Some(next) = report["next"].as_str() {
        out.push_str(&format!("next: {next}\n"));
    }
    out.push_str("the steps after this one did not run\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNAPSHOT: &str = "uid=n1 RootWebArea \"Account\"\n  uid=n12 button \"Manage billing\"\n  uid=n13 button \"Save\"\n  uid=n14 button \"Save draft\"\n  uid=n15 link \"Manage billing\"\n";

    /// The resolver matches the role and the WHOLE name: "Save" and "Save draft" are two
    /// controls.
    #[test]
    fn a_locator_matches_one_role_and_the_whole_name() {
        assert_eq!(find(SNAPSHOT, "button", "Manage billing"), vec!["n12"]);
        assert_eq!(find(SNAPSHOT, "link", "Manage billing"), vec!["n15"]);
        assert_eq!(find(SNAPSHOT, "button", "Save"), vec!["n13"]);
        assert!(find(SNAPSHOT, "button", "Cancel").is_empty());
        assert!(find(SNAPSHOT, "checkbox", "Save").is_empty());
    }

    #[test]
    fn two_matches_are_an_ambiguity_and_not_a_choice() {
        let two = "uid=n1 button \"Delete\"\nuid=n2 button \"Delete\"\n";
        assert_eq!(find(two, "button", "Delete").len(), 2);
    }

    /// The stop report carries the action's own words rather than a summary.
    #[test]
    fn a_stopped_run_names_the_step_the_guard_and_what_the_action_said() {
        let macro_file = Macro::parse(
            r##"{"name":"m","steps":[{"do":{"cmd":"click","selector":"#a"},"expect":{"verdict":"changed"}}]}"##,
        )
        .unwrap();
        let response = json!({
            "ok": true, "verdict": "unchanged", "verdict_reason": "identical_tree",
            "next": "confirm", "delivery": "target_hit"
        });
        let failure = GuardFailure {
            guard: "verdict",
            expected: "changed".into(),
            observed: "unchanged".into(),
        };
        let report = stopped(&macro_file, 0, &macro_file.steps[0], &response, Some(&failure), &[]);
        assert_eq!(report["ok"], false);
        assert_eq!(report["stopped_at"], 0);
        assert_eq!(report["guard"], "verdict");
        assert_eq!(report["expected"], "changed");
        assert_eq!(report["observed"], "unchanged");
        assert_eq!(report["next"], "confirm", "the action's own branch is carried");
        assert_eq!(report["verdict_reason"], "identical_tree");
        assert!(report["stop"].as_str().unwrap().contains("did not happen"));
    }

    /// A response-settled guard is read off the fields the action already wrote.
    #[test]
    fn the_response_guards_are_read_from_the_response() {
        let response = json!({"delivery": "target_hit", "verdict": "changed", "value": {"verbatim": true}});
        assert_eq!(observed_for("delivery", &response).as_deref(), Some("target_hit"));
        assert_eq!(observed_for("verdict", &response).as_deref(), Some("changed"));
        assert_eq!(observed_for("verbatim", &response).as_deref(), Some("true"));
        assert_eq!(observed_for("verbatim", &json!({})), None);
    }
}
