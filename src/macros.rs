//! A named, parameterised path that already worked once, and what it is allowed to promise.
//!
//! Three pieces of this tool existed and ignored each other: `batch` (a list of commands on
//! stdin, with no name, no file and no parameter), `replay` (a file and `--vars`, but of a raw
//! recording), and the verdict (per action, never per scenario). A macro is the three of them
//! in one artefact: a file the agent writes after succeeding once, whose every step carries the
//! postcondition observed on that success.
//!
//! # What it is not
//!
//! Not a replayed recording. A recording keeps everything, dead ends included; a macro keeps the
//! path that arrived, with durable locators and expectations that survive tomorrow.
//!
//! Not a repair loop. A guard that does not hold stops the run at that step and says what was
//! expected, what was observed and what the action's own `next` was. There is no branch, no
//! retry, no repair — claiming otherwise would be the kind of false success this project exists
//! to remove.
//!
//! # Why JSON and not the YAML of the design note
//!
//! The design sketches the file in YAML. YAML earns its keep when a HUMAN writes the file, and
//! the design's own premise is that the agent writes it. Against that, YAML costs a dependency
//! (`serde_yaml` is unmaintained; the crate graph is guarded in CI and is one pure-Rust regex
//! away from empty), and every other artefact this tool reads or writes — pipe, batch, the
//! session store, the recording — is JSON. So a macro is JSON, and a step's `do` is EXACTLY the
//! command object `batch`/`pipe` already accept: `macro run` dispatches it through the same
//! dispatcher, which is the difference between reusing the execution semantics and inventing a
//! second set. Reported as a deviation rather than taken silently.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// A parameter the macro declares.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Param {
    #[serde(default = "yes")]
    pub required: bool,
    /// Never stored, never written to the file, and the run refuses without it.
    ///
    /// The predicate that decides which fields are secret is `element::SECRET_FIELD`, the one
    /// the fill report already uses; this flag is what the recorder writes when that predicate
    /// fired, so the two cannot disagree about what a secret is.
    #[serde(default)]
    pub secret: bool,
}

const fn yes() -> bool {
    true
}

/// One step: a command this tool already knows, and what was true after it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// The command object, in the shape `pipe` and `batch` take.
    #[serde(rename = "do")]
    pub action: Value,
    /// The guards, all from the whitelist. An empty object is a step that promises nothing.
    #[serde(default)]
    pub expect: Guards,
    /// Why this step carries no guard, when it carries none.
    ///
    /// The design's open question, answered in the file rather than in silence: a step whose
    /// observation produced nothing from the whitelist is not a verified step, and a reader
    /// that cannot tell it from a guarded one will trust it the same.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unguarded: Option<String>,
}

/// The whitelist, and only the whitelist.
///
/// Every field here answers the same question: would this still be true tomorrow, on the same
/// task, if it succeeded again? What is deliberately absent is argued in `macros_record`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Guards {
    /// `target_hit`, and nothing else — the strongest guard available, and binary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<String>,
    /// The verdict WORD (`changed`, `navigated`, `not_kept`…), never the reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    /// The page kept what was written. For a secret field the guard stays `verbatim`; the
    /// value never appears.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verbatim: Option<bool>,
    /// A pattern (`regex-lite`, like `assert --matches`), never a whole URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_matches: Option<String>,
    /// A short landmark, never a paragraph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_contains: Option<String>,
    /// A selector and a floor, never an exact count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exists: Option<Exists>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Exists {
    pub selector: String,
    #[serde(default = "one")]
    pub min: usize,
}

const fn one() -> usize {
    1
}

impl Guards {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.delivery.is_none()
            && self.verdict.is_none()
            && self.verbatim.is_none()
            && self.url_matches.is_none()
            && self.text_contains.is_none()
            && self.exists.is_none()
    }

    /// The guards that can be settled from the response alone, in the order they are checked.
    ///
    /// Kept separate from the ones that need a page read: a response-only guard costs nothing,
    /// and checking it first means a step that already failed does not pay for a `text` read.
    #[must_use]
    pub fn response_guards(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if let Some(delivery) = &self.delivery {
            out.push(("delivery", delivery.clone()));
        }
        if let Some(verdict) = &self.verdict {
            out.push(("verdict", verdict.clone()));
        }
        if let Some(verbatim) = self.verbatim {
            out.push(("verbatim", verbatim.to_string()));
        }
        out
    }
}

/// A macro as it sits on disk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Macro {
    pub name: String,
    /// Where it was recorded. Context for a reader, never a guard: a macro is not refused
    /// because the host moved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_at: Option<String>,
    #[serde(default)]
    pub params: BTreeMap<String, Param>,
    pub steps: Vec<Step>,
}

/// What a name may contain.
///
/// It becomes a file name under the store, so a name that walks out of the directory is
/// refused rather than sanitised: silently renaming what the caller asked for is how a macro
/// ends up somewhere nobody looks.
pub fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("A macro name cannot be empty.".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "Invalid macro name '{name}': use letters, digits, '-' and '_' only. The name is \
             the file name under ~/.chrome-agent/macros, so a path separator is refused rather \
             than rewritten."
        ));
    }
    Ok(())
}

/// `~/.chrome-agent/macros`.
#[must_use]
pub fn store_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    home.join(".chrome-agent").join("macros")
}

#[must_use]
pub fn path_of(name: &str) -> PathBuf {
    store_dir().join(format!("{name}.json"))
}

impl Macro {
    /// Read one back, refusing anything the whitelist does not know.
    pub fn load(name: &str) -> Result<Self, crate::BoxError> {
        check_name(name)?;
        let path = path_of(name);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "No macro named '{name}' ({}): {e}. `macro list` shows what exists.",
                path.display()
            )
        })?;
        Self::parse(&text)
    }

    /// Parse and validate. `deny_unknown_fields` is the point: a guard this build does not
    /// know is refused loudly, never ignored — an ignored guard is a promise nobody checks.
    pub fn parse(text: &str) -> Result<Self, crate::BoxError> {
        let parsed: Self = serde_json::from_str(text)
            .map_err(|e| format!("Not a usable macro file: {e}"))?;
        check_name(&parsed.name)?;
        if parsed.steps.is_empty() {
            return Err("This macro has no steps.".into());
        }
        Ok(parsed)
    }

    /// Write it under the store, 0600.
    ///
    /// Same permission as the recording and the session store, and for a weaker but real
    /// reason: a macro holds no secret by construction, and it does hold the values a fill
    /// wrote — an email address, an account number — which is not public either.
    pub fn save(&self) -> Result<PathBuf, crate::BoxError> {
        check_name(&self.name)?;
        let dir = store_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
        let path = path_of(&self.name);
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, text).map_err(|e| format!("Cannot write {}: {e}", path.display()))?;
        restrict(&path);
        Ok(path)
    }

    /// The parameters a run must be given, and the reason it may not be defaulted.
    ///
    /// A secret is declared and never stored, so a missing one is a refusal and not a blank:
    /// filling a password field with an empty string is a real edit of a real page.
    pub fn bind(&self, vars: &BTreeMap<String, String>) -> Result<(), crate::BoxError> {
        let mut missing: Vec<&str> = Vec::new();
        for (name, param) in &self.params {
            if param.required && !vars.contains_key(name) {
                missing.push(name);
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        let secret: Vec<&str> = missing
            .iter()
            .copied()
            .filter(|name| self.params.get(*name).is_some_and(|p| p.secret))
            .collect();
        let mut message = format!(
            "This macro needs {}: {}.",
            if missing.len() == 1 { "a value" } else { "values" },
            missing.join(", ")
        );
        if !secret.is_empty() {
            message.push_str(&format!(
                " {} declared secret, so {} never stored in the file and there is nothing to \
                 fall back on.",
                secret.join(", "),
                if secret.len() == 1 { "it is" } else { "they are" }
            ));
        }
        // The names, not a template: `hints.rs`'s rule 2 is that a caller who copies the
        // advice runs a command, and `--var name=value` is a shape to fill in rather than a
        // command to run.
        message.push_str(&format!(
            " Pass {}: {}.",
            if missing.len() == 1 { "it" } else { "them" },
            missing.iter().map(|name| format!("--var {name}=…")).collect::<Vec<_>>().join(" ")
        ));
        Err(message.into())
    }

    /// One step's command with `{{param}}` replaced.
    ///
    /// Substitution is textual and happens on the serialised command, which is what `replay`
    /// already does — the same spelling in the same tool, rather than a second convention.
    pub fn resolve(&self, step: &Step, vars: &BTreeMap<String, String>) -> Result<Value, crate::BoxError> {
        let mut text = serde_json::to_string(&step.action)?;
        for (key, value) in vars {
            let escaped = serde_json::to_string(value)?;
            // The value arrives as a JSON string; splice its INSIDE, so a quote or a backslash
            // in a password cannot end the string it is being written into.
            let inner = escaped.trim_matches('"');
            text = text.replace(&format!("{{{{{key}}}}}"), inner);
        }
        let resolved: Value = serde_json::from_str(&text)?;
        if let Some(left) = unresolved_placeholder(&text) {
            return Err(format!(
                "Step still carries {{{{{left}}}}} after substitution: pass --var {left}=… . A \
                 macro never runs with a placeholder in it — the page would receive the braces."
            )
            .into());
        }
        Ok(resolved)
    }
}

/// The first `{{name}}` left in a serialised step, if any.
fn unresolved_placeholder(text: &str) -> Option<String> {
    let start = text.find("{{")?;
    let rest = &text[start + 2..];
    let end = rest.find("}}")?;
    Some(rest[..end].to_string())
}

/// Every macro in the store, by name.
pub fn list() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(store_dir()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                path.file_stem().map(|stem| stem.to_string_lossy().into_owned())
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names
}

/// A summary for `macro list`: enough to choose one without opening it.
#[must_use]
pub fn summary(name: &str) -> Value {
    match Macro::load(name) {
        Ok(macro_file) => {
            let unguarded = macro_file.steps.iter().filter(|s| s.expect.is_empty()).count();
            json!({
                "name": macro_file.name,
                "site": macro_file.site,
                "steps": macro_file.steps.len(),
                "unguarded_steps": unguarded,
                "params": macro_file.params.keys().collect::<Vec<_>>(),
            })
        }
        Err(e) => json!({"name": name, "error": e.to_string()}),
    }
}

fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
    }

    const CANCEL: &str = r##"{
        "name": "cancel-subscription",
        "site": "example.com",
        "params": {"email": {"required": true, "secret": false},
                   "password": {"required": true, "secret": true}},
        "steps": [
            {"do": {"cmd": "goto", "url": "https://example.com/account"},
             "expect": {"url_matches": "/account"}},
            {"do": {"cmd": "fill", "selector": "#email", "value": "{{email}}"},
             "expect": {"verbatim": true}}
        ]
    }"##;

    #[test]
    fn a_macro_round_trips_through_the_file_it_declares() {
        let parsed = Macro::parse(CANCEL).expect("parses");
        assert_eq!(parsed.steps.len(), 2);
        assert!(parsed.params["password"].secret);
        assert_eq!(parsed.steps[0].expect.url_matches.as_deref(), Some("/account"));
        let again = Macro::parse(&serde_json::to_string(&parsed).unwrap()).expect("re-parses");
        assert_eq!(parsed, again);
    }

    /// A guard this build does not know must be refused, not ignored: an ignored guard is a
    /// promise on the response that nothing ever checks.
    #[test]
    fn an_unknown_guard_is_refused_rather_than_dropped() {
        let text = r#"{"name":"x","steps":[{"do":{"cmd":"click"},"expect":{"added":450}}]}"#;
        let error = Macro::parse(text).expect_err("must refuse").to_string();
        assert!(error.contains("added"), "the refusal names the field: {error}");
    }

    /// The counters are the headline case of the blacklist, and the parser is where the file
    /// format says no.
    #[test]
    fn a_macro_with_no_steps_is_not_a_macro() {
        assert!(Macro::parse(r#"{"name":"x","steps":[]}"#).is_err());
    }

    #[test]
    fn a_name_that_leaves_the_store_is_refused_not_sanitised() {
        assert!(check_name("cancel-subscription").is_ok());
        assert!(check_name("../../etc/passwd").is_err());
        assert!(check_name("a/b").is_err());
        assert!(check_name("").is_err());
        let error = check_name("a b").expect_err("space");
        assert!(error.contains("file name"), "{error}");
    }

    /// A missing secret is a refusal with its reason, because there is deliberately nothing
    /// stored to fall back on — and an empty password typed into a real form is a real edit.
    #[test]
    fn a_missing_secret_says_why_it_cannot_be_defaulted() {
        let parsed = Macro::parse(CANCEL).unwrap();
        let error = parsed
            .bind(&vars(&[("email", "ada@example.com")]))
            .expect_err("password is missing")
            .to_string();
        assert!(error.contains("password"), "{error}");
        assert!(error.contains("never stored"), "{error}");
        assert!(error.contains("--var password="), "{error}");
        assert!(parsed.bind(&vars(&[("email", "a@b.c"), ("password", "hunter2")])).is_ok());
    }

    #[test]
    fn substitution_puts_the_value_inside_the_json_string_it_replaces() {
        let parsed = Macro::parse(CANCEL).unwrap();
        let resolved = parsed
            .resolve(&parsed.steps[1], &vars(&[("email", "ada@example.com")]))
            .expect("resolves");
        assert_eq!(resolved["value"], "ada@example.com");

        // A password with a quote in it must not end the string it lands in.
        let awkward = parsed
            .resolve(&parsed.steps[1], &vars(&[("email", "a\"b\\c")]))
            .expect("stays valid JSON");
        assert_eq!(awkward["value"], "a\"b\\c");
    }

    /// The braces reaching the page is the failure this refuses: a form that receives
    /// `{{email}}` accepts it, and the macro looks like it worked.
    #[test]
    fn a_placeholder_nobody_bound_stops_the_step() {
        let parsed = Macro::parse(CANCEL).unwrap();
        let error = parsed
            .resolve(&parsed.steps[1], &BTreeMap::new())
            .expect_err("must refuse")
            .to_string();
        assert!(error.contains("email"), "{error}");
        assert!(error.contains("--var email="), "{error}");
    }

    #[test]
    fn an_empty_expect_is_the_shape_that_promises_nothing() {
        assert!(Guards::default().is_empty());
        let guards = Guards { verdict: Some("changed".into()), ..Guards::default() };
        assert!(!guards.is_empty());
        assert_eq!(guards.response_guards(), vec![("verdict", "changed".to_string())]);
    }
}
