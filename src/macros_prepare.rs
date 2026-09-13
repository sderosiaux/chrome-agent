//! Offline preparation shared by `macro check` and `macro run`.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::macros::{Macro, Param, Step};

impl Macro {
    /// Resolve and validate the entire path before opening Chrome. Page-dependent selectors,
    /// JavaScript, permissions and guards still need evaluation during the run.
    pub fn prepare(&self, vars: &BTreeMap<String, String>) -> Result<Vec<Step>, crate::BoxError> {
        self.bind(vars)?;
        for key in vars.keys() {
            if !self.params.contains_key(key) {
                return Err(format!("Undeclared macro parameter '{key}'. Nothing was run.").into());
            }
        }
        self.steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                let prepare = || -> Result<Step, crate::BoxError> {
                    let mut resolved = step.clone();
                    resolved.action = self.resolve(step, vars)?;
                    resolved.expect = serde_json::from_value(substitute(
                        &serde_json::to_value(&step.expect)?,
                        &self.params,
                        vars,
                    )?)?;
                    validate_action(&resolved.action)?;
                    if let Some(delivery) = &resolved.expect.delivery
                        && crate::verdict::Delivery::parse(delivery).as_str() != delivery
                    {
                        return Err("Unknown delivery guard".into());
                    }
                    if let Some(word) = &resolved.expect.verdict {
                        use crate::verdict::Verdict as V;
                        if ![
                            V::Changed,
                            V::Navigated,
                            V::Intercepted,
                            V::NotKept,
                            V::NoEffect,
                            V::Unchanged,
                            V::Unknown,
                            V::NotChecked,
                        ]
                        .iter()
                        .any(|v| v.as_str() == word)
                        {
                            return Err("Unknown verdict guard".into());
                        }
                    }
                    if let Some(pattern) = &resolved.expect.url_matches {
                        regex_lite::Regex::new(pattern)?;
                    }
                    if let Some(selector) = resolved.expect.exists.as_ref().map(|e| &e.selector)
                        && selector.trim().is_empty()
                    {
                        return Err("exists guard requires a nonempty selector".into());
                    }
                    Ok(resolved)
                };
                prepare().map_err(|e| {
                    let message = format!("Macro step {index}: {e}. Nothing was run.");
                    redact_text(&message, self, vars).into()
                })
            })
            .collect()
    }
}

/// Role/name is a macro locator. Validate the underlying command using a stand-in uid;
/// uniqueness and actual identity are checked against a fresh snapshot at execution time.
pub fn validate_action(action: &Value) -> Result<(), crate::BoxError> {
    let mut typed = action.clone();
    if has_named_locator(action) {
        for key in ["role", "name"] {
            if action
                .get(key)
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            {
                return Err("A macro locator requires both a nonempty role and name".into());
            }
        }
        if ["uid", "selector", "xy"]
            .iter()
            .any(|key| action.get(key).is_some())
        {
            return Err("A role/name locator cannot also carry uid, selector or xy".into());
        }
        let map = typed
            .as_object_mut()
            .ok_or("A macro command must be an object")?;
        map.remove("role");
        map.remove("name");
        map.insert("uid".into(), Value::String("macro-preflight".into()));
    }
    if let crate::pipe_command::PipeCommand::Batch(args) = crate::pipe_command::parse(&typed)? {
        for (index, command) in args
            .commands
            .as_ref()
            .ok_or("batch: missing commands array")?
            .iter()
            .enumerate()
        {
            // Nested commands run directly through the pipe dispatcher, which has no
            // role/name resolution. Refuse instead of promising to resolve a hidden locator.
            if has_named_locator(command) {
                return Err("Role/name locators must be top-level macro steps".into());
            }
            validate_action(command).map_err(|e| format!("batch command {index}: {e}"))?;
        }
    }
    Ok(())
}

fn has_named_locator(action: &Value) -> bool {
    action.get("role").is_some()
        || (action.get("name").is_some()
            && !matches!(
                action.get("cmd").and_then(Value::as_str),
                Some("webmcp_call" | "webmcp-call")
            ))
}

pub fn substitute(
    value: &Value,
    params: &BTreeMap<String, Param>,
    vars: &BTreeMap<String, String>,
) -> Result<Value, crate::BoxError> {
    Ok(match value {
        Value::String(template) => {
            let mut rest = template.as_str();
            let mut result = String::new();
            while let Some(start) = rest.find("{{") {
                result.push_str(&rest[..start]);
                let tail = &rest[start + 2..];
                let end = tail
                    .find("}}")
                    .ok_or("Unclosed macro parameter placeholder")?;
                let key = &tail[..end];
                if !params.contains_key(key) {
                    return Err(format!("Undeclared macro placeholder '{{{{{key}}}}}'").into());
                }
                let replacement = vars
                    .get(key)
                    .ok_or_else(|| format!("Missing parameter '{key}': pass --var {key}=…"))?;
                result.push_str(replacement);
                rest = &tail[end + 2..];
            }
            result.push_str(rest);
            Value::String(result)
        }
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|v| substitute(v, params, vars))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(values) => {
            let mut result = serde_json::Map::new();
            for (key, value) in values {
                if key.contains("{{") {
                    return Err(
                        "Macro placeholders belong in string values, not object keys".into(),
                    );
                }
                result.insert(key.clone(), substitute(value, params, vars)?);
            }
            Value::Object(result)
        }
        value => value.clone(),
    })
}

/// Dispatchers already redact secret controls. Also redact declared secret inputs when a
/// page echoes them through a read, a URL or an error in the macro report.
pub fn redact_text(text: &str, macro_file: &Macro, vars: &BTreeMap<String, String>) -> String {
    let mut secrets: Vec<&str> = vars
        .iter()
        .filter_map(|(key, value)| {
            (macro_file.params.get(key).is_some_and(|p| p.secret) && !value.is_empty())
                .then_some(value.as_str())
        })
        .collect();
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut text = text.to_string();
    for secret in secrets {
        let encoded = serde_json::to_string(secret).expect("serialize string");
        text = text.replace(&encoded[1..encoded.len() - 1], "<redacted>");
        text = text.replace(secret, "<redacted>");
    }
    text
}

fn redact(value: Value, macro_file: &Macro, vars: &BTreeMap<String, String>) -> Value {
    match value {
        Value::String(s) => Value::String(redact_text(&s, macro_file, vars)),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|v| redact(v, macro_file, vars))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(k, v)| {
                    (
                        redact_text(&k, macro_file, vars),
                        redact(v, macro_file, vars),
                    )
                })
                .collect(),
        ),
        v => v,
    }
}

/// Preserve protocol keys and enum tokens even when a secret happens to be "ok" or "guard".
/// Free-form result data still gets full redaction, including its object keys.
pub fn redact_response(value: Value, macro_file: &Macro, vars: &BTreeMap<String, String>) -> Value {
    let Value::Object(fields) = value else {
        return redact(value, macro_file, vars);
    };
    Value::Object(
        fields
            .into_iter()
            .map(|(key, value)| {
                let value = match key.as_str() {
                    "cmd" | "verdict" | "next" | "delivery" | "via" | "kind" | "comparator" => {
                        value
                    }
                    "assertion" | "last_observation" | "wait" | "value" | "landed" => {
                        redact_response(value, macro_file, vars)
                    }
                    "results" => match value {
                        Value::Array(results) => Value::Array(
                            results
                                .into_iter()
                                .map(|v| redact_response(v, macro_file, vars))
                                .collect(),
                        ),
                        value => redact(value, macro_file, vars),
                    },
                    _ => redact(value, macro_file, vars),
                };
                (key, value)
            })
            .collect(),
    )
}

pub fn redact_stop(
    mut report: Value,
    macro_file: &Macro,
    vars: &BTreeMap<String, String>,
) -> Value {
    // Completed steps were redacted when collected. These fields can contain page data or
    // substituted inputs; routing fields (especially stopped_by, which determines exit 2) cannot.
    for key in [
        "error",
        "expected",
        "observed",
        "hint",
        "verdict_reason",
        "verdict_hint",
        "intercepted_by",
    ] {
        if let Some(value) = report.get_mut(key) {
            *value = redact(value.take(), macro_file, vars);
        }
    }
    if let Some(result) = report.get_mut("result") {
        *result = redact_response(result.take(), macro_file, vars);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn example(action: &Value) -> Macro {
        Macro::parse(
            &json!({"name":"example", "params":{"value":{}},
            "steps":[{"do":action}]})
            .to_string(),
        )
        .unwrap()
    }

    #[test]
    fn parameters_are_literal_data_in_actions_and_guards() {
        for value in ["ends-in-quote\"", "a\\b\n中文,é", "{{value}}", ""] {
            let mut file =
                example(&json!({"cmd":"fill", "selector":"#field", "value":"{{value}}"}));
            file.steps[0].expect.text_contains = Some("prefix {{value}} suffix".into());
            let vars = BTreeMap::from([("value".into(), value.into())]);
            let steps = file.prepare(&vars).unwrap();
            assert_eq!(steps[0].action["value"], value);
            assert_eq!(
                steps[0].expect.text_contains.as_deref(),
                Some(format!("prefix {value} suffix").as_str())
            );
        }
    }

    #[test]
    fn all_steps_and_guards_are_validated_before_execution() {
        for bad in [
            json!({"cmd":"typo_command"}),
            json!({"cmd":"fill", "selector":"#a"}),
            json!({"cmd":"assert", "what":"text", "matches":"["}),
            json!({"cmd":"wait", "what":"bogus"}),
            json!({"cmd":"batch", "commands":[{"cmd":"typo_command"}]}),
            json!({"cmd":"fill", "selector":"#a", "value":"{{missing}}"}),
        ] {
            let mut file = example(&json!({"cmd":"fill", "selector":"#a", "value":"first"}));
            file.params.clear();
            file.steps.push(Step {
                action: bad.clone(),
                expect: crate::macros::Guards::default(),
                unguarded: None,
            });
            let error = file.prepare(&BTreeMap::new()).unwrap_err().to_string();
            assert!(error.contains("step 1"), "{bad}: {error}");
        }
        let mut file = example(&json!({"cmd":"text"}));
        file.params.clear();
        file.steps[0].expect.url_matches = Some("[".into());
        assert!(file.prepare(&BTreeMap::new()).is_err());
    }

    #[test]
    fn durable_locators_are_checked_without_chrome() {
        assert!(validate_action(&json!({"cmd":"click", "role":"button", "name":"Save"})).is_ok());
        assert!(validate_action(&json!({"cmd":"click", "role":"button"})).is_err());
        assert!(
            validate_action(
                &json!({"cmd":"click", "role":"button", "name":"Save", "selector":"#a"})
            )
            .is_err()
        );
    }

    #[test]
    fn a_webmcp_tool_name_is_not_an_element_locator() {
        let call = json!({"cmd":"webmcp_call", "name":"search", "args":{"query":"news"}});
        assert!(validate_action(&call).is_ok());
        assert!(validate_action(&json!({"cmd":"batch", "commands":[call]})).is_ok());
    }

    #[test]
    fn secret_redaction_preserves_protocol_routing_and_redacts_opaque_data() {
        let mut file = example(&json!({"cmd":"text"}));
        file.params.get_mut("value").unwrap().secret = true;
        for secret in ["ok", "guard"] {
            let vars = BTreeMap::from([("value".into(), secret.into())]);
            let response = redact_response(
                json!({"ok":false,"assertion":{"held":false,"actual":secret},
                "result":{(secret):secret}}),
                &file,
                &vars,
            );
            assert_eq!(response["ok"], false);
            assert_eq!(response["assertion"]["actual"], "<redacted>");
            assert_eq!(response["result"], json!({"<redacted>":"<redacted>"}));
            let report = redact_stop(
                json!({"ok":false,"stopped_by":"guard","guard":"assertion","observed":secret}),
                &file,
                &vars,
            );
            assert_eq!(crate::macros_run::exit_code(&report), 2);
            assert_eq!(report["observed"], "<redacted>");
        }
    }
}
