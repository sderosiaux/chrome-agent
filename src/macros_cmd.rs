//! The `macro` surface: what an agent types, and what it reads back. `list`, `show` and `record`
//! touch files only and never open a page; only `run` needs a browser.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::cli::{Cli, MacroAction};
use crate::macros::Macro;
use crate::macros_record::{self, Observed};

/// `chrome-agent macro …`
pub async fn run_cli(cli: &Cli, action: &MacroAction) -> Result<(), crate::BoxError> {
    let json_mode = cli.json;
    match action {
        MacroAction::List => {
            let names = crate::macros::list();
            if json_mode {
                let summaries: Vec<Value> = names
                    .iter()
                    .map(|name| crate::macros::summary(name))
                    .collect();
                out_line!("{}", json!({"ok": true, "macros": summaries}));
            } else if names.is_empty() {
                out_line!(
                    "No macros yet. Record one from a session that worked: \
                     `chrome-agent macro record <name> --from-recording <file>`."
                );
            } else {
                for name in &names {
                    let summary = crate::macros::summary(name);
                    out_line!(
                        "{name}  steps={}  unguarded={}  site={}",
                        summary["steps"],
                        summary["unguarded_steps"],
                        summary["site"].as_str().unwrap_or("-")
                    );
                }
            }
        }
        MacroAction::Show { name } => {
            let macro_file = Macro::load(name)?;
            if json_mode {
                out_line!("{}", json!({"ok": true, "macro": macro_file}));
            } else {
                out!("{}", render(&macro_file));
            }
        }
        MacroAction::Record {
            name,
            from_recording,
            from,
        } => {
            let report = record_from_recording(name, from_recording, *from)?;
            if json_mode {
                out_line!("{report}");
            } else {
                out!("{}", render_record(&report));
            }
        }
        MacroAction::Run { name, var } => {
            let vars = parse_vars(var)?;
            let report = crate::macros_run::run(cli, name, &vars).await?;
            if report.get("ok").and_then(Value::as_bool) == Some(true) {
                if json_mode {
                    out_line!("{report}");
                } else {
                    out!("{}", crate::macros_run::render_run(&report));
                }
            } else {
                // A guard that did not hold fails the run, so a chaining shell sees it. Which
                // code it fails with is `Stopped`'s to answer, off the report's own
                // `stopped_by`: `2` when a guard ran and the page disagreed (the same claim
                // class as `assert`), `1` when the run never got that far.
                return Err(Box::new(crate::macros_run::Stopped::new(report, json_mode)));
            }
        }
    }
    Ok(())
}

/// `--var k=v` into a map, refusing the shapes that silently do nothing.
pub fn parse_vars(pairs: &[String]) -> Result<BTreeMap<String, String>, crate::BoxError> {
    let mut vars = BTreeMap::new();
    for pair in pairs {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| format!("--var expects name=value, got '{pair}'. Nothing was run."))?;
        if key.is_empty() {
            return Err(format!("--var '{pair}' has no name. Nothing was run.").into());
        }
        vars.insert(key.to_string(), value.to_string());
    }
    Ok(vars)
}

/// Distil a `_record` file. The pure part is `macros_record::distil`; this is the file half.
pub fn record_from_recording(
    name: &str,
    path: &str,
    from: Option<usize>,
) -> Result<Value, crate::BoxError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Cannot read the recording '{path}': {e}"))?;
    // A step's uid resolves against the last snapshot the session took, which the recording
    // kept with its `inspect` responses.
    let mut snapshot: Option<String> = None;
    let mut history: Vec<Observed> = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let cmd = entry.get("cmd").unwrap_or(&Value::Null);
        let response = entry.get("response").unwrap_or(&Value::Null);
        history.push(Observed::read_with_snapshot(
            cmd,
            response,
            snapshot.as_deref(),
        ));
        if let Some(fresh) = response.get("snapshot").and_then(Value::as_str) {
            snapshot = Some(fresh.to_string());
        }
    }
    if history.is_empty() {
        return Err(format!(
            "The recording '{path}' holds no command. A pipe session records the commands that \
             carry `_record`, so a session recorded from its second command starts there."
        )
        .into());
    }
    save_distilled(name, &history, from)
}

/// Shared by the CLI and by the pipe command: distil, save, and report what did not make it.
pub fn save_distilled(
    name: &str,
    history: &[Observed],
    from: Option<usize>,
) -> Result<Value, crate::BoxError> {
    crate::macros::check_name(name)?;
    let start = from.unwrap_or_else(|| macros_record::default_start(history));
    let distilled = macros_record::distil(name, history, start)?;
    let path = distilled.macro_file.save()?;
    let unguarded = distilled
        .macro_file
        .steps
        .iter()
        .filter(|s| s.expect.is_empty())
        .count();
    Ok(json!({
        "ok": true,
        "macro": distilled.macro_file.name,
        "path": path.display().to_string(),
        // Which entry the task was taken to start at, and whether the caller chose it.
        "started_at": start,
        "started_by": if from.is_some() { "you" } else { "the last navigation" },
        "steps": distilled.macro_file.steps.len(),
        "unguarded_steps": unguarded,
        "params": distilled.macro_file.params.keys().collect::<Vec<_>>(),
        "dropped": distilled.dropped.iter().map(|r| json!({"index": r.index, "reason": r.reason})).collect::<Vec<_>>(),
        // Not dropped: these acted on the page and could not be written down, so the macro is
        // SHORTER than the task.
        "refused": distilled.refused.iter().map(|r| json!({"index": r.index, "reason": r.reason})).collect::<Vec<_>>(),
    }))
}

/// `{"cmd":"macro", …}` inside a pipe session, distilling the session's own history — which is
/// what lets an agent ask for a task to be kept only after finding out that it worked.
pub fn dispatch_pipe(cmd: &Value, history: &[Observed]) -> Result<Value, crate::BoxError> {
    let action = cmd
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("record");
    match action {
        "record" => {
            let name = cmd
                .get("name")
                .and_then(Value::as_str)
                .ok_or("macro record: give it a \"name\".")?;
            let from = cmd.get("from").and_then(Value::as_u64).map(|n| n as usize);
            save_distilled(name, history, from)
        }
        "list" => Ok(json!({
            "ok": true,
            "macros": crate::macros::list().iter().map(|n| crate::macros::summary(n)).collect::<Vec<_>>()
        })),
        "show" => {
            let name = cmd
                .get("name")
                .and_then(Value::as_str)
                .ok_or("macro show: give it a \"name\".")?;
            Ok(json!({"ok": true, "macro": Macro::load(name)?}))
        }
        other => Err(format!(
            "macro: unknown action {other:?}. In a pipe session the actions are \"record\", \
             \"list\" and \"show\" — running a macro inside the session that is recording it \
             would be recording the run."
        )
        .into()),
    }
}

/// One macro, for a person.
fn render(macro_file: &Macro) -> String {
    let mut out = format!("{}\n", macro_file.name);
    if let Some(site) = &macro_file.site {
        out.push_str(&format!("site: {site}\n"));
    }
    for (name, param) in &macro_file.params {
        out.push_str(&format!(
            "param: {name}{}{}\n",
            if param.required { " (required)" } else { "" },
            if param.secret {
                " SECRET — never stored, pass it every run"
            } else {
                ""
            }
        ));
    }
    for (index, step) in macro_file.steps.iter().enumerate() {
        let verb = step
            .action
            .get("cmd")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let guards = serde_json::to_string(&step.expect).unwrap_or_default();
        out.push_str(&format!(
            "{index}. {verb} {}\n",
            compact_action(&step.action)
        ));
        if let Some(reason) = &step.unguarded {
            out.push_str(&format!("   UNGUARDED — {reason}\n"));
        } else {
            out.push_str(&format!("   expect {guards}\n"));
        }
    }
    out
}

fn compact_action(action: &Value) -> String {
    let mut parts = Vec::new();
    for key in ["url", "selector", "role", "name", "value", "key", "text"] {
        if let Some(value) = action.get(key).and_then(Value::as_str) {
            parts.push(format!("{key}={value}"));
        }
    }
    parts.join(" ")
}

fn render_record(report: &Value) -> String {
    let mut out = format!(
        "Recorded {} step(s) as '{}' ({})\n",
        report["steps"],
        report["macro"],
        report["path"].as_str().unwrap_or_default()
    );
    out.push_str(&format!(
        "started at entry {} ({})\n",
        report["started_at"],
        report["started_by"].as_str().unwrap_or("")
    ));
    if report["unguarded_steps"].as_u64().unwrap_or(0) > 0 {
        out.push_str(&format!(
            "{} step(s) promise nothing: `macro show` names them, and a run cannot verify them.\n",
            report["unguarded_steps"]
        ));
    }
    for refusal in report["refused"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "REFUSED entry {}: {}\n",
            refusal["index"],
            refusal["reason"].as_str().unwrap_or_default()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_var_without_an_equals_is_refused_before_anything_runs() {
        let error = parse_vars(&["email".to_string()])
            .expect_err("no =")
            .to_string();
        assert!(error.contains("name=value"), "{error}");
        assert!(error.contains("Nothing was run"), "{error}");
        let vars = parse_vars(&["email=a@b.c".to_string(), "q=x=y".to_string()]).unwrap();
        assert_eq!(vars["email"], "a@b.c");
        assert_eq!(vars["q"], "x=y", "only the first = separates");
    }

    /// Running a macro from inside the session that is recording it would record the run.
    #[test]
    fn the_pipe_surface_refuses_to_run_a_macro() {
        let error = dispatch_pipe(&json!({"cmd": "macro", "action": "run", "name": "x"}), &[])
            .expect_err("refused")
            .to_string();
        assert!(error.contains("recording the run"), "{error}");
    }
}
