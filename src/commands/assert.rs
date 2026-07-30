//! `assert` — check a claim about the page and answer with an exit code.
//!
//! The exit code is the whole point. A caller (a CI job, a recipe runner) needs to tell
//! three answers apart, and the binary could previously only say two: `0` and `1`.
//!
//! | code | meaning | who is at fault |
//! |------|---------|-----------------|
//! | 0 | the claim held when we looked | nobody |
//! | 2 | the claim did not hold | the page |
//! | 1 | the claim could not be answered | the tool, the browser, or the caller's arguments |
//!
//! Collapsing 2 into 1 makes "the form kept a different value" look like "Chrome never
//! started", and the recovery for those is not the same: the first is a fact about the page
//! to report or repair, the second is a retry. `2` is carried by [`NotHeld`], an error type
//! `main` recognises before its generic handler runs.
//!
//! A selector that matches nothing is a `1`, not a `2`: "the field holds X" is unanswerable
//! when there is no field, and answering `false` would let a typo in a selector read as a
//! statement about the page. `assert exists` is the command that turns presence itself into
//! a claim, and there a count of zero is a `2`.
//!
//! The state readers are deliberately not this module's own: `--checked` reads through
//! `element_controls::CHECKABLE_PROBE`, the same classification `check`/`uncheck` apply
//! before clicking, and `--selected` through `element_controls::SELECT_READ`, the same
//! reading `select` uses for its read-back. An assertion that disagreed with the action
//! about what "checked" means would be worse than no assertion at all.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::cdp::client::CdpClient;
use crate::element_ref::ElementRef;

// Split out for the 1000-line file cap; callers keep using `commands::assert::*`.
pub use super::assert_args::{from_cli, from_json};

/// Exit code for an assertion that ran and did not hold.
pub const EXIT_NOT_HELD: i32 = 2;

/// How much of a text read travels in the report. A whole-page `innerText` is thousands of
/// characters and the agent has `text` for reading it; what the report owes the caller is
/// enough of the page to recognise which page it was looking at.
const ACTUAL_TEXT_BUDGET: usize = 400;

// ---------------------------------------------------------------------------
// Comparators — pure, no Chrome
// ---------------------------------------------------------------------------

/// How an observed string is compared with the expected one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Comparator {
    Equals(String),
    Contains(String),
    /// A Rust regex (the `regex-lite` engine): `\d`, `\w` and `\s` are ASCII-only and
    /// there is no `\p{…}`. Documented in `llm-guide.txt` because the difference from a
    /// JS `RegExp` is invisible until an accented character fails to match `\w`.
    Matches(String),
}

impl Comparator {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Equals(_) => "equals",
            Self::Contains(_) => "contains",
            Self::Matches(_) => "matches",
        }
    }

    pub fn expected(&self) -> &str {
        match self {
            Self::Equals(s) | Self::Contains(s) | Self::Matches(s) => s,
        }
    }

    /// Whether `actual` satisfies this comparator.
    ///
    /// A malformed pattern is an operational error, not a failed assertion: nothing was
    /// compared, so there is nothing to report about the page.
    pub fn holds(&self, actual: &str) -> Result<bool, crate::BoxError> {
        match self {
            Self::Equals(expected) => Ok(actual == expected),
            Self::Contains(expected) => Ok(actual.contains(expected.as_str())),
            Self::Matches(pattern) => {
                let re = regex_lite::Regex::new(pattern).map_err(|e| {
                    format!("assert --matches: invalid regular expression /{pattern}/: {e}")
                })?;
                Ok(re.is_match(actual))
            }
        }
    }
}

/// Whether a found count satisfies the requested cardinality.
///
/// No `--count` and no `--min` means "at least one": `assert exists --selector x` reads as
/// a presence check, and the count is there for the callers who care how many.
#[must_use]
pub const fn count_holds(found: usize, count: Option<usize>, min: Option<usize>) -> bool {
    match (count, min) {
        (Some(exact), _) => found == exact,
        (None, Some(at_least)) => found >= at_least,
        (None, None) => found > 0,
    }
}

/// The state an element is asserted to be in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Want {
    Checked,
    Unchecked,
    /// The `<select>`'s current option, matched by `option.value` or by its trimmed text —
    /// the same two spellings `select` accepts when picking one.
    Selected(String),
    Enabled,
    Disabled,
    Visible,
}

impl Want {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Checked => "checked",
            Self::Unchecked => "unchecked",
            Self::Selected(_) => "selected",
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Visible => "visible",
        }
    }

    fn expected(&self) -> Value {
        match self {
            Self::Selected(option) => json!(option),
            other => json!(other.name()),
        }
    }
}

/// Whether a checkable's read state (`true` / `false` / `mixed`, straight out of the
/// shared probe) satisfies the wanted state.
///
/// `mixed` (an `indeterminate` checkbox, or `aria-checked="mixed"`) satisfies neither: it is
/// a third state, and calling it unchecked is how a form gets submitted with a box the user
/// never resolved.
#[must_use]
pub fn checked_holds(state: &str, want: &Want) -> bool {
    match want {
        Want::Checked => state == "true",
        Want::Unchecked => state == "false",
        _ => false,
    }
}

/// Whether the option the page currently holds is the one asserted.
///
/// Value first, then trimmed text — `select`'s own precedence. For a single option the two
/// orderings cannot disagree, which is why this reads as one predicate rather than two.
#[must_use]
pub fn selected_holds(value: Option<&str>, text: Option<&str>, expected: &str) -> bool {
    value == Some(expected) || text.map(str::trim) == Some(expected)
}

// ---------------------------------------------------------------------------
// The assertion
// ---------------------------------------------------------------------------

/// What is being claimed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Value(Comparator),
    Text(Comparator),
    Url(Comparator),
    State(Want),
    Exists { count: Option<usize>, min: Option<usize> },
}

impl Kind {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Value(_) => "value",
            Self::Text(_) => "text",
            Self::Url(_) => "url",
            Self::State(_) => "state",
            Self::Exists { .. } => "exists",
        }
    }
}

/// A claim about the page, plus the element it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assertion {
    pub kind: Kind,
    pub selector: Option<String>,
    pub uid: Option<String>,
}

impl Assertion {
    /// Reject an assertion whose target is missing or doubly specified, before any CDP call.
    fn require_target(&self) -> Result<(), crate::BoxError> {
        match (self.selector.as_deref(), self.uid.as_deref()) {
            (Some(_), Some(_)) => {
                Err(format!("assert {}: only one of --uid or --selector can be provided.", self.kind.name()).into())
            }
            (None, None) => Err(format!(
                "assert {}: Provide --uid or --selector to identify the element.",
                self.kind.name()
            )
            .into()),
            _ => Ok(()),
        }
    }
}

// ---------------------------------------------------------------------------
// The outcome
// ---------------------------------------------------------------------------

/// What the page answered, and whether that satisfies the claim.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub kind: &'static str,
    pub comparator: &'static str,
    pub expected: Value,
    pub actual: Value,
    pub held: bool,
    /// The element the read was taken from, named the way every action names its target:
    /// `uid` whenever it could be resolved, plus the selector the caller typed.
    pub target: Option<Value>,
    /// Extra fields the specific check owes the caller (a truncation flag, which flavour of
    /// hidden, how the value was redacted).
    pub details: Option<Value>,
}

impl Outcome {
    const fn new(kind: &'static str, comparator: &'static str, expected: Value, actual: Value, held: bool) -> Self {
        Self { kind, comparator, expected, actual, held, target: None, details: None }
    }

    fn with_target(mut self, target: Option<Value>) -> Self {
        self.target = target;
        self
    }

    fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    /// The `assertion` object every mode returns, in all three modes and both outcomes.
    #[must_use]
    pub fn assertion_json(&self) -> Value {
        let mut obj = json!({
            "kind": self.kind,
            "comparator": self.comparator,
            "expected": self.expected,
            "actual": self.actual,
            "held": self.held,
        });
        for extra in [self.target.as_ref(), self.details.as_ref()] {
            if let (Some(map), Some(fields)) = (obj.as_object_mut(), extra.and_then(Value::as_object)) {
                for (key, value) in fields {
                    map.insert(key.clone(), value.clone());
                }
            }
        }
        obj
    }

    /// The full response: `ok` mirrors `held`, so a batch's `all_ok` and `stop_on_error`
    /// treat a failed assertion as the failure it is without a second convention.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut obj = json!({"ok": self.held, "assertion": self.assertion_json()});
        if !self.held {
            obj["hint"] = json!(self.hint());
        }
        obj
    }

    /// One line for a human, on the same shape the JSON carries.
    #[must_use]
    pub fn message(&self) -> String {
        let held = if self.held { "held" } else { "did NOT hold" };
        let target = self
            .target
            .as_ref()
            .and_then(|t| {
                t.get("selector")
                    .or_else(|| t.get("uid"))
                    .and_then(Value::as_str)
                    .map(|s| format!(" on '{s}'"))
            })
            .unwrap_or_default();
        format!(
            "assert {} {}{target}: {held} — expected {}, actual {}",
            self.kind,
            self.comparator,
            compact(&self.expected),
            compact(&self.actual)
        )
    }

    /// What to do next when the claim did not hold. Every error in this tool carries one;
    /// a failed assertion is the answer an agent is most likely to act on.
    fn hint(&self) -> &'static str {
        match self.kind {
            "value" => "The page holds something else. Re-read it (`eval --selector \"…\" \"el.value\"`), or `wait` and assert again — a controlled component can rewrite a value after the write returns.",
            "text" => "Read what the page actually says: `text --selector \"…\"` for a region, `read` for article content. If the content loads late, `wait text \"…\"` first.",
            "url" => "Read the current location with `eval \"location.href\"`. A redirect or a pushState may have landed somewhere else.",
            "state" => "Read the element: `inspect --uid <uid>` for its a11y state, or `eval --selector \"…\" \"el.outerHTML\"` for the DOM truth.",
            _ => "Count the matches yourself with `eval \"document.querySelectorAll('…').length\"`, or `inspect` to see what the page renders.",
        }
    }
}

/// An assertion that ran and did not hold — the carrier for exit code 2.
///
/// It travels the error channel so no caller has to thread a second return type through
/// `run`, but it is not an error: nothing failed, the page is simply not in the asserted
/// state. `main` recognises it before its generic handler and exits [`EXIT_NOT_HELD`].
#[derive(Debug)]
pub struct NotHeld {
    outcome: Outcome,
    json_mode: bool,
}

impl NotHeld {
    /// Print the outcome on the stream the caller reads, and answer with the exit code.
    ///
    /// `--json` puts it on stdout, where the agent parses errors already; text mode puts it
    /// on stderr like every other refusal this binary prints, so a shell pipeline reading
    /// stdout sees nothing and the exit code is the whole answer.
    pub fn report(&self) -> i32 {
        if self.json_mode {
            crate::run_helpers::json_output(&self.outcome.to_json());
        } else {
            eprintln!("{}", self.outcome.message());
            eprintln!("hint: {}", self.outcome.hint());
        }
        EXIT_NOT_HELD
    }
}

impl std::fmt::Display for NotHeld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.outcome.message())
    }
}

impl std::error::Error for NotHeld {}

/// Render a JSON scalar for a human-readable line.
///
/// Capped well below the JSON's own budget: a whole-page text read is 400 characters in the
/// response, and 400 characters of page text on one terminal line is not a line anybody
/// reads. The full excerpt is still in `--json`. Shared with `render`, which generalises this
/// command's output shape to every action — two budgets would make the same value print two
/// lengths depending on which command reported it.
use crate::render::compact;

// ---------------------------------------------------------------------------
// Reading the page
// ---------------------------------------------------------------------------

/// Read a form control's value, and whether it is a secret.
///
/// The sensitivity test is the one `fill` applies (`element.rs`) — an assertion that printed
/// a password an action refuses to print would be a way around the redaction, and this
/// response reaches stdout, the agent transcript and any `--record` file just the same.
fn value_probe() -> String {
    VALUE_PROBE_TEMPLATE.replace("SECRET_EXPR", crate::element::SECRET_FIELD)
}

/// Template for `value_probe`: the shared secret predicate is substituted in, so the
/// assertion and the fill that made the value cannot disagree about what must stay hidden.
const VALUE_PROBE_TEMPLATE: &str = r"function (el) {
  const holder = ['INPUT', 'TEXTAREA', 'SELECT', 'PROGRESS', 'METER', 'OUTPUT'].indexOf(el.tagName) >= 0;
  if (!holder) {
    return { kind: 'novalue', tag: el.tagName.toLowerCase(), editable: !!el.isContentEditable };
  }
  return {
    kind: 'value',
    value: (el.value === undefined || el.value === null) ? null : String(el.value),
    sensitive: SECRET_EXPR
  };
}";

/// Read whether an element is disabled, and whether it is rendered.
///
/// `:disabled` rather than `el.disabled`: it is what `fill` refuses on, and it catches an
/// ancestor `<fieldset disabled>` that the property misses. `aria-disabled` is read too and
/// reported separately — a `<div role="button" aria-disabled="true">` is disabled to
/// everything that reads the page, and answering "enabled" there is the same class of lie
/// as reading `.checked` off a div.
///
/// `visible` means "rendered, not transparent, not `visibility:hidden`". It does NOT mean in
/// the viewport, and it does NOT mean nothing is stacked on top — that question needs a hit
/// test, which this command does not do.
const RENDER_PROBE: &str = r"function (el) {
  const cs = getComputedStyle(el);
  let visibility = 'visible';
  if (el.getClientRects().length === 0) visibility = 'no-box';
  else if (cs.visibility === 'hidden' || cs.visibility === 'collapse') visibility = 'visibility:' + cs.visibility;
  else if (parseFloat(cs.opacity) === 0) visibility = 'opacity:0';
  let enabled = 'enabled';
  if (el.matches(':disabled')) enabled = 'disabled';
  else if ((el.getAttribute('aria-disabled') || '').toLowerCase() === 'true') enabled = 'aria-disabled';
  return { enabled: enabled, visibility: visibility };
}";

/// Call a probe on the asserted element, whichever way it was named.
///
/// The selector path resolves in the page and throws when nothing matches — that is an
/// operational failure (exit 1), not a claim about the page.
async fn probe(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    assertion: &Assertion,
    body: &str,
) -> Result<Value, crate::BoxError> {
    let result: Value = if let Some(selector) = assertion.selector.as_deref() {
        let sel = serde_json::to_string(selector).unwrap_or_default();
        let expression = format!(
            "(() => {{ const el = document.querySelector({sel}); \
             if (!el) throw new Error('No element matches selector: ' + {sel}); \
             return ({body})(el); }})()"
        );
        client
            .call("Runtime.evaluate", json!({"expression": expression, "returnByValue": true}))
            .await?
    } else {
        let uid = assertion.uid.as_deref().unwrap_or_default();
        let resolved = crate::element::resolve_uid(client, uid_map, uid).await?;
        client
            .call(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": resolved.object_id,
                    "functionDeclaration": format!("function() {{ return ({body})(this); }}"),
                    "returnByValue": true,
                }),
            )
            .await?
    };
    crate::element::check_js_exception(&result)?;
    Ok(result.get("result").and_then(|r| r.get("value")).cloned().unwrap_or_default())
}

/// Evaluate an expression in the page and return its value.
async fn evaluate(client: &CdpClient, expression: &str) -> Result<Value, crate::BoxError> {
    let result: Value = client
        .call("Runtime.evaluate", json!({"expression": expression, "returnByValue": true}))
        .await?;
    crate::element::check_js_exception(&result)?;
    Ok(result.get("result").and_then(|r| r.get("value")).cloned().unwrap_or_default())
}

/// Run the assertion: read the page, compare, and report — without deciding anything about
/// exit codes. Both front ends call this, so both get the same answer for the same claim.
pub async fn run(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    assertion: &Assertion,
) -> Result<Outcome, crate::BoxError> {
    match &assertion.kind {
        Kind::Value(cmp) => assert_value(client, uid_map, assertion, cmp).await,
        Kind::Text(cmp) => assert_text(client, uid_map, assertion, cmp).await,
        Kind::Url(cmp) => {
            let url = evaluate(client, "location.href").await?;
            let actual = url.as_str().unwrap_or_default();
            let held = cmp.holds(actual)?;
            Ok(Outcome::new("url", cmp.name(), json!(cmp.expected()), json!(actual), held))
        }
        Kind::State(want) => assert_state(client, uid_map, assertion, want).await,
        Kind::Exists { count, min } => {
            let selector = assertion.selector.as_deref().ok_or(
                "assert exists: Provide --selector; presence is a claim about a CSS match, not about a uid (a uid you hold came from a snapshot of a page that already had it).",
            )?;
            let sel = serde_json::to_string(selector).unwrap_or_default();
            let found = evaluate(client, &format!("document.querySelectorAll({sel}).length"))
                .await?
                .as_u64()
                .unwrap_or(0) as usize;
            let held = count_holds(found, *count, *min);
            let expected = match (count, min) {
                (Some(exact), _) => json!(exact),
                (None, Some(at_least)) => json!(format!(">= {at_least}")),
                (None, None) => json!(">= 1"),
            };
            let comparator = if count.is_some() { "count" } else if min.is_some() { "min" } else { "present" };
            Ok(Outcome::new("exists", comparator, expected, json!(found), held)
                .with_target(Some(json!({"selector": selector}))))
        }
    }
}

async fn assert_value(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    assertion: &Assertion,
    cmp: &Comparator,
) -> Result<Outcome, crate::BoxError> {
    assertion.require_target()?;
    let target = target_fields(client, assertion).await;
    let read = probe(client, uid_map, assertion, &value_probe()).await?;
    if read.get("kind").and_then(Value::as_str) == Some("novalue") {
        let tag = read.get("tag").and_then(Value::as_str).unwrap_or("element");
        let editable = read.get("editable").and_then(Value::as_bool).unwrap_or(false);
        let alternative = if editable {
            "It is contenteditable, which has no value property — assert its text instead: `assert text --selector \"…\" --contains \"…\"`."
        } else {
            "Only form controls hold a value. For anything else, assert its text: `assert text --selector \"…\" --contains \"…\"`."
        };
        return Err(format!("assert value: <{tag}> has no value property. {alternative}").into());
    }
    let actual = read.get("value").and_then(Value::as_str);
    let held = cmp.holds(actual.unwrap_or_default())?;

    // A secret is compared but never echoed. Both lengths still travel: they are what
    // distinguishes "the mask reformatted it" from "the field is empty", which is the whole
    // reason the value report exists.
    if read.get("sensitive").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(Outcome::new("value", cmp.name(), json!("redacted"), json!("redacted"), held)
            .with_target(target)
            .with_details(json!({
                "redacted": true,
                "expected_length": cmp.expected().chars().count(),
                "actual_length": actual.map(|a| a.chars().count()),
            })));
    }
    Ok(Outcome::new("value", cmp.name(), json!(cmp.expected()), json!(actual), held).with_target(target))
}

async fn assert_text(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    assertion: &Assertion,
    cmp: &Comparator,
) -> Result<Outcome, crate::BoxError> {
    if assertion.selector.is_some() && assertion.uid.is_some() {
        return Err("assert text: only one of --uid or --selector can be provided.".into());
    }
    let target = if assertion.selector.is_some() || assertion.uid.is_some() {
        target_fields(client, assertion).await
    } else {
        None
    };
    // The same reader the `text` command uses, so "what the page says" means one thing.
    let text = crate::commands::text::run(client, assertion.uid.as_deref(), assertion.selector.as_deref(), uid_map).await?;
    let held = cmp.holds(&text)?;
    let full = text.chars().count();
    let excerpt = crate::truncate::truncate_str(&text, ACTUAL_TEXT_BUDGET, "…");
    let mut details = json!({"actual_chars": full});
    if full > ACTUAL_TEXT_BUDGET {
        details["actual_truncated"] = json!(true);
    }
    Ok(
        Outcome::new("text", cmp.name(), json!(cmp.expected()), json!(excerpt.as_ref()), held)
            .with_target(target)
            .with_details(details),
    )
}

async fn assert_state(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    assertion: &Assertion,
    want: &Want,
) -> Result<Outcome, crate::BoxError> {
    assertion.require_target()?;
    let target = target_fields(client, assertion).await;
    let outcome = match want {
        Want::Checked | Want::Unchecked => {
            // The classification check/uncheck apply before clicking, verbatim.
            let read = probe(client, uid_map, assertion, crate::element_controls::CHECKABLE_PROBE).await?;
            let probe_result = crate::element_controls::parse_probe_value(&read);
            crate::element_controls::refuse_uncheckable(&probe_result, true)?;
            let state = probe_result.state.as_str();
            Outcome::new("state", want.name(), want.expected(), json!(state), checked_holds(state, want))
                .with_details(json!({"reading": probe_result.kind}))
        }
        Want::Selected(option) => {
            // The reading `select` takes for its own read-back.
            let read = probe(client, uid_map, assertion, crate::element_controls::SELECT_READ).await?;
            let text = read.get("text").and_then(Value::as_str);
            let value = read.get("value").and_then(Value::as_str);
            let held = selected_holds(value, text, option);
            Outcome::new("state", "selected", json!(option), json!(text), held)
                .with_details(json!({"selected_value": value}))
        }
        Want::Enabled | Want::Disabled => {
            let read = probe(client, uid_map, assertion, RENDER_PROBE).await?;
            let state = read.get("enabled").and_then(Value::as_str).unwrap_or("enabled");
            // `aria-disabled` counts as disabled and never as enabled: the page is telling
            // every reader the control is inert, and only the CSS pseudo-class disagrees.
            let held = match want {
                Want::Enabled => state == "enabled",
                _ => state != "enabled",
            };
            Outcome::new("state", want.name(), want.expected(), json!(state), held)
        }
        Want::Visible => {
            let read = probe(client, uid_map, assertion, RENDER_PROBE).await?;
            let state = read.get("visibility").and_then(Value::as_str).unwrap_or("no-box");
            Outcome::new("state", "visible", want.expected(), json!(state), state == "visible")
                .with_details(json!({
                    "means": "rendered, opaque and not visibility:hidden — not 'in the viewport' and not 'nothing on top of it'"
                }))
        }
    };
    Ok(outcome.with_target(target))
}

/// Name the node the read came from: the uid whenever it resolves, plus the selector typed.
///
/// Same reason an action names its target — without it a report about `.total` and a delta
/// about `n42` cannot be tied together, and a selector matching several nodes gives no clue
/// which one answered.
async fn target_fields(client: &CdpClient, assertion: &Assertion) -> Option<Value> {
    let mut fields = crate::run_helpers::target_details(
        client,
        assertion.selector.as_deref(),
        assertion.uid.as_deref(),
    )
    .await
    .unwrap_or_else(|| json!({}));
    if let (Some(map), Some(selector)) = (fields.as_object_mut(), assertion.selector.as_deref()) {
        map.insert("selector".into(), json!(selector));
    }
    let empty = fields.as_object().is_none_or(serde_json::Map::is_empty);
    if empty { None } else { Some(fields) }
}

// ---------------------------------------------------------------------------
// CLI entry point
// ---------------------------------------------------------------------------

/// Run the assertion for the CLI, print it, and hand `main` the exit code to use.
///
/// A held assertion prints on stdout and returns `Ok`. One that did not hold returns
/// [`NotHeld`], which `main` turns into exit 2 — deliberately not an `Err` string, because
/// the generic error path would print it as a tool failure and exit 1.
pub async fn run_cli(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    what: &crate::cli::AssertWhat,
    json_mode: bool,
) -> Result<(), crate::BoxError> {
    let assertion = from_cli(what)?;
    let outcome = run(client, uid_map, &assertion).await?;
    if !outcome.held {
        return Err(Box::new(NotHeld { outcome, json_mode }));
    }
    if json_mode {
        crate::run_helpers::json_output(&outcome.to_json());
    } else {
        println!("{}", outcome.message());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests — the comparators are pure, so none of this needs Chrome
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equals_is_exact_including_whitespace_and_case() {
        let cmp = Comparator::Equals("hello@example.com".into());
        assert!(cmp.holds("hello@example.com").unwrap());
        assert!(!cmp.holds("hello@example.com ").unwrap());
        assert!(!cmp.holds("Hello@example.com").unwrap());
        // The failing case the E2E test pins: the page kept nothing.
        assert!(!cmp.holds("").unwrap());
    }

    #[test]
    fn contains_is_a_substring_test_not_a_word_test() {
        let cmp = Comparator::Contains("order".into());
        assert!(cmp.holds("Your order shipped").unwrap());
        assert!(cmp.holds("reorder").unwrap(), "substring, deliberately not word-bounded");
        assert!(!cmp.holds("ORDER").unwrap(), "case-sensitive; use --matches \"(?i)order\" instead");
    }

    #[test]
    fn matches_is_a_rust_regex_and_is_not_anchored() {
        let cmp = Comparator::Matches(r"^\d{3}-\d{4}$".into());
        assert!(cmp.holds("555-1234").unwrap());
        assert!(!cmp.holds("x555-1234").unwrap());
        // Unanchored by default, like every regex engine.
        assert!(Comparator::Matches("total".into()).holds("Subtotal: 12").unwrap());
        // Case-insensitive flag is supported inline.
        assert!(Comparator::Matches("(?i)total".into()).holds("TOTAL").unwrap());
    }

    #[test]
    fn a_malformed_pattern_is_an_error_not_a_failed_assertion() {
        // Nothing was compared, so there is nothing to say about the page: this must reach
        // the caller as exit 1, not as held:false.
        let err = Comparator::Matches("(unclosed".into()).holds("anything").unwrap_err().to_string();
        assert!(err.contains("invalid regular expression"), "{err}");
    }

    #[test]
    fn regex_lite_classes_are_ascii_only() {
        // Pinning the documented limitation rather than discovering it in a user's page:
        // `\w` does not cover accented letters in this engine.
        assert!(Comparator::Matches(r"^\w+$".into()).holds("Jean").unwrap());
        assert!(!Comparator::Matches(r"^\w+$".into()).holds("Jean-Sébastien").unwrap());
        // A literal accented character matches fine, which is the common case.
        assert!(Comparator::Matches("Sébastien".into()).holds("Jean-Sébastien").unwrap());
    }

    #[test]
    fn exists_cardinality() {
        // Bare presence: at least one.
        assert!(count_holds(1, None, None));
        assert!(count_holds(9, None, None));
        assert!(!count_holds(0, None, None));
        // Exact count.
        assert!(count_holds(3, Some(3), None));
        assert!(!count_holds(4, Some(3), None));
        assert!(!count_holds(2, Some(3), None));
        // `--count 0` is a legitimate absence claim, and must not be read as "unset".
        assert!(count_holds(0, Some(0), None));
        assert!(!count_holds(1, Some(0), None));
        // Minimum.
        assert!(count_holds(3, None, Some(3)));
        assert!(count_holds(30, None, Some(3)));
        assert!(!count_holds(2, None, Some(3)));
        // --count wins when both are given (clap forbids it; JSON callers can still try).
        assert!(count_holds(3, Some(3), Some(10)));
    }

    #[test]
    fn mixed_is_neither_checked_nor_unchecked() {
        assert!(checked_holds("true", &Want::Checked));
        assert!(!checked_holds("false", &Want::Checked));
        assert!(checked_holds("false", &Want::Unchecked));
        assert!(!checked_holds("true", &Want::Unchecked));
        // An indeterminate checkbox is a third state. Calling it unchecked is how a form is
        // submitted with a box nobody resolved.
        assert!(!checked_holds("mixed", &Want::Checked));
        assert!(!checked_holds("mixed", &Want::Unchecked));
    }

    #[test]
    fn selected_matches_value_or_visible_text() {
        // select picks by value first, then by trimmed text; for the one option a page
        // holds, the two orderings cannot disagree.
        assert!(selected_holds(Some("CA"), Some("California"), "CA"));
        assert!(selected_holds(Some("CA"), Some("California"), "California"));
        assert!(selected_holds(Some("CA"), Some("  California  "), "California"), "text is trimmed");
        assert!(!selected_holds(Some("CA"), Some("California"), "NY"));
        // Nothing selected at all.
        assert!(!selected_holds(None, None, "CA"));
    }

    #[test]
    fn json_shape_carries_the_verdict_and_the_two_sides() {
        let outcome = Outcome::new("value", "equals", json!("a@b.c"), json!(""), false)
            .with_target(Some(json!({"uid": "n12", "selector": "#email"})));
        let v = outcome.to_json();
        assert_eq!(v["ok"], false, "ok mirrors held so batch/stop_on_error need no second rule");
        assert_eq!(v["assertion"]["kind"], "value");
        assert_eq!(v["assertion"]["comparator"], "equals");
        assert_eq!(v["assertion"]["expected"], "a@b.c");
        assert_eq!(v["assertion"]["actual"], "");
        assert_eq!(v["assertion"]["held"], false);
        assert_eq!(v["assertion"]["uid"], "n12");
        assert!(v["hint"].is_string(), "a failed assertion tells the caller what to do next");

        let held = Outcome::new("exists", "count", json!(3), json!(3), true);
        assert_eq!(held.to_json()["ok"], true);
        assert!(held.to_json().get("hint").is_none(), "nothing to advise when it held");
    }

    #[test]
    fn a_target_is_required_where_a_read_needs_one() {
        let no_target = Assertion {
            kind: Kind::Value(Comparator::Equals("x".into())),
            selector: None,
            uid: None,
        };
        let err = no_target.require_target().unwrap_err().to_string();
        assert!(err.contains("Provide --uid"), "must hit the existing hint branch: {err}");
        let both = Assertion {
            kind: Kind::State(Want::Checked),
            selector: Some("#a".into()),
            uid: Some("n1".into()),
        };
        assert!(both.require_target().is_err());
    }

    #[test]
    fn the_human_line_names_the_target_and_both_sides() {
        let line = Outcome::new("value", "equals", json!("a@b.c"), json!(""), false)
            .with_target(Some(json!({"uid": "n12", "selector": "#email"})))
            .message();
        assert!(line.contains("did NOT hold"), "{line}");
        assert!(line.contains("#email"), "{line}");
        assert!(line.contains("\"a@b.c\""), "{line}");
    }
}
