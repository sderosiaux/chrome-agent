//! The pipe/batch protocol as a type.
//!
//! Every dispatcher used to hand-decode `serde_json::Value`, so an unknown key was simply
//! ignored: `{"cmd":"click","uidd":"n1"}` answered `click: provide "uid", "selector", or "xy"`,
//! naming a problem the caller did not have. One `deny_unknown_fields` struct per verb makes the
//! typo the error it is, and the accepted key set of each command a single declaration.
//!
//! The same shape `macros::Step` already had over the same command objects.
//!
//! What is still a raw `Value`, and why:
//!
//! - `_record` — a session-level directive, stripped by `pipe.rs` before the command is parsed.
//! - `batch.commands` — each entry is itself a command, parsed by `dispatch_single` in turn.
//! - `webmcp_call.args` — the tool's own arguments, serialised straight back to the page.
//! - `emulate`'s and `assert`'s VALUES — the keys are enumerated here (so a typo is refused by
//!   name), but each value keeps its existing validator, whose messages name the field and the
//!   type where serde's would not.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One line of the protocol.
#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum PipeCommand {
    Goto(GotoArgs),
    Click(PointerArgs),
    Dblclick(PointerArgs),
    Fill(FillArgs),
    Select(ValueArgs),
    Check(CheckArgs),
    Uncheck(TargetArgs),
    Upload(UploadArgs),
    Drag(DragArgs),
    Hover(HoverArgs),
    Inspect(InspectArgs),
    Diff(NoArgs),
    Eval(EvalArgs),
    Read(ReadArgs),
    Text(TextArgs),
    Screenshot(ScreenshotArgs),
    Pdf(PdfArgs),
    Download(DownloadArgs),
    Wait(WaitArgs),
    Back(NoArgs),
    Forward(NoArgs),
    Scroll(ScrollArgs),
    Type(TypeArgs),
    Press(PressArgs),
    Tabs(NoArgs),
    Network(NetworkArgs),
    Console(ConsoleArgs),
    Extract(ExtractArgs),
    #[serde(alias = "fill-form", alias = "fillform")]
    FillForm(FillFormArgs),
    #[serde(alias = "navigate-and-read")]
    NavigateAndRead(NavigateAndReadArgs),
    #[serde(alias = "fill-and-submit")]
    FillAndSubmit(FillAndSubmitArgs),
    History(HistoryArgs),
    Frame(FrameArgs),
    Emulate(EmulateArgs),
    Assert(AssertArgs),
    #[serde(alias = "webmcp-list")]
    WebmcpList(NoArgs),
    #[serde(alias = "webmcp-call")]
    WebmcpCall(WebmcpCallArgs),
    Batch(BatchArgs),
}

impl PipeCommand {
    /// The canonical verb, whichever spelling arrived. `mutates_page` and the change report read
    /// this rather than the caller's string, so an alias can never fall out of that allowlist.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Goto(_) => "goto",
            Self::Click(_) => "click",
            Self::Dblclick(_) => "dblclick",
            Self::Fill(_) => "fill",
            Self::Select(_) => "select",
            Self::Check(_) => "check",
            Self::Uncheck(_) => "uncheck",
            Self::Upload(_) => "upload",
            Self::Drag(_) => "drag",
            Self::Hover(_) => "hover",
            Self::Inspect(_) => "inspect",
            Self::Diff(_) => "diff",
            Self::Eval(_) => "eval",
            Self::Read(_) => "read",
            Self::Text(_) => "text",
            Self::Screenshot(_) => "screenshot",
            Self::Pdf(_) => "pdf",
            Self::Download(_) => "download",
            Self::Wait(_) => "wait",
            Self::Back(_) => "back",
            Self::Forward(_) => "forward",
            Self::Scroll(_) => "scroll",
            Self::Type(_) => "type",
            Self::Press(_) => "press",
            Self::Tabs(_) => "tabs",
            Self::Network(_) => "network",
            Self::Console(_) => "console",
            Self::Extract(_) => "extract",
            Self::FillForm(_) => "fill_form",
            Self::NavigateAndRead(_) => "navigate_and_read",
            Self::FillAndSubmit(_) => "fill_and_submit",
            Self::History(_) => "history",
            Self::Frame(_) => "frame",
            Self::Emulate(_) => "emulate",
            Self::Assert(_) => "assert",
            Self::WebmcpList(_) => "webmcp_list",
            Self::WebmcpCall(_) => "webmcp_call",
            Self::Batch(_) => "batch",
        }
    }

    fn validate(&self) -> Result<(), crate::BoxError> {
        crate::pipe_validate::validate(self)
    }
}

/// Parse one command object. The tag is read first so an absent or unknown `cmd` keeps the
/// wording every caller already branches on.
pub fn parse(cmd: &Value) -> Result<PipeCommand, crate::BoxError> {
    let Some(name) = cmd
        .get("cmd")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
    else {
        return Err("Missing \"cmd\" field".into());
    };
    let parsed = PipeCommand::deserialize(cmd).map_err(|e| describe(name, cmd, &e.to_string()))?;
    parsed.validate()?;
    Ok(parsed)
}

/// serde's message, made to say what this protocol's messages have always said.
///
/// serde's "missing field" becomes `goto: missing "url"` — the exact text the hand-decoded
/// dispatchers produced. An unknown variant is the unknown command it names. Everything else
/// (a wrong type) is prefixed with the command and, where the offending key can be identified,
/// with the key: serde's own "invalid type" carries neither.
fn describe(name: &str, cmd: &Value, raw: &str) -> crate::BoxError {
    if raw.starts_with("unknown variant") {
        return format!("Unknown command: {name}").into();
    }
    let quoted = raw.replace('`', "\"");
    let body = quoted
        .strip_prefix("missing field ")
        .map_or_else(|| quoted.clone(), |field| format!("missing {field}"));
    match blame_field(cmd, raw) {
        Some(key) => format!("{name}: \"{key}\": {body}").into(),
        None => format!("{name}: {body}").into(),
    }
}

/// Which key the parse tripped over, when serde will not say.
///
/// Removing a key that parsed cleanly leaves the identical error; removing the offending one
/// changes it (to success, or to `missing field <key>`). Only ever runs on the error path, over
/// one small object.
fn blame_field(cmd: &Value, original: &str) -> Option<String> {
    if original.starts_with("missing field") || original.starts_with("unknown field") {
        return None;
    }
    let object = cmd.as_object()?;
    for key in object.keys() {
        if key == "cmd" {
            continue;
        }
        let mut probe = object.clone();
        probe.remove(key);
        let still = match PipeCommand::deserialize(&Value::Object(probe)) {
            Ok(_) => return Some(key.clone()),
            Err(e) => e.to_string(),
        };
        if still != original {
            return Some(key.clone());
        }
    }
    None
}

// --- Argument structs: one per accepted key set ---

/// A verb that takes nothing. Still `deny_unknown_fields`: a unit variant would swallow any key.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoArgs {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GotoArgs {
    pub url: String,
    #[serde(default)]
    pub headers: Option<Vec<String>>,
    #[serde(default)]
    pub wait_for: Option<String>,
    #[serde(default)]
    pub inspect: Option<bool>,
    #[serde(default)]
    pub max_depth: Option<u64>,
}

/// `click` and `dblclick`: the same three ways to aim, the same two display flags.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointerArgs {
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default)]
    pub xy: Option<[f64; 2]>,
    #[serde(default)]
    pub on_intercept: Option<String>,
    #[serde(default)]
    pub inspect: Option<bool>,
    #[serde(default)]
    pub max_depth: Option<u64>,
}

/// `select`: a value written to a uid or a selector.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueArgs {
    pub value: String,
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default)]
    pub inspect: Option<bool>,
    #[serde(default)]
    pub max_depth: Option<u64>,
}

/// `fill`: `select`'s keys plus `secret`.
///
/// A separate struct rather than a field on `ValueArgs`: `select` reads secrecy off the element
/// and has no way to honour an asserted one, and an accepted key that does nothing is worse than
/// a refusal naming it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FillArgs {
    pub value: String,
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
    /// The caller's own claim that this value must never be printed, on top of what the element
    /// declares. It only ever ADDS redaction — there is deliberately no way to force a value out.
    #[serde(default)]
    pub secret: Option<bool>,
    #[serde(default)]
    pub inspect: Option<bool>,
    #[serde(default)]
    pub max_depth: Option<u64>,
}

/// `uncheck`. `desired` is `check`'s alone: the verb already decided, and `uncheck` carrying the
/// field meant one decision written twice.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetArgs {
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default)]
    pub on_intercept: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckArgs {
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default)]
    pub on_intercept: Option<String>,
    /// The one way to ask `check` for the opposite state.
    #[serde(default)]
    pub desired: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UploadArgs {
    #[serde(default)]
    pub files: Option<Vec<String>>,
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DragArgs {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HoverArgs {
    #[serde(default)]
    pub uid: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectArgs {
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub filter: Option<String>,
    #[serde(default)]
    pub verbose: Option<bool>,
    #[serde(default)]
    pub scroll: Option<bool>,
    #[serde(default)]
    pub urls: Option<bool>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub max_depth: Option<u64>,
    #[serde(default)]
    pub offset: Option<u64>,
    #[serde(default)]
    pub max_chars: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalArgs {
    pub expression: String,
    #[serde(default)]
    pub selector: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadArgs {
    #[serde(default)]
    pub html: Option<bool>,
    #[serde(default)]
    pub truncate: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextArgs {
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default)]
    pub truncate: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenshotArgs {
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub quality: Option<u32>,
    #[serde(default)]
    pub max_width: Option<u32>,
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PdfArgs {
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub landscape: Option<bool>,
    #[serde(default)]
    pub background: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DownloadArgs {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default)]
    pub out: Option<String>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub max_bytes: Option<u64>,
    #[serde(default)]
    pub on_intercept: Option<String>,
}

/// `wait` takes several shapes; `what`/`pattern` is the explicit one and the rest are shorthand.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitArgs {
    #[serde(default)]
    pub what: Option<String>,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub idle_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScrollArgs {
    pub target: String,
    #[serde(default)]
    pub px: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeArgs {
    pub text: String,
    #[serde(default)]
    pub selector: Option<String>,
    /// See [`FillArgs::secret`]. `type` has no read-back, so this withholds the length too.
    #[serde(default)]
    pub secret: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PressArgs {
    pub key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkArgs {
    #[serde(default)]
    pub filter: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub body: Option<bool>,
    #[serde(default)]
    pub live: Option<u64>,
    #[serde(default)]
    pub abort: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleArgs {
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub clear: Option<bool>,
    #[serde(default)]
    pub limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractArgs {
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub scroll: Option<bool>,
    #[serde(default)]
    pub a11y: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FillFormArgs {
    #[serde(default)]
    pub pairs: Option<Vec<PairArgs>>,
    #[serde(default)]
    pub inspect: Option<bool>,
    #[serde(default)]
    pub max_depth: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairArgs {
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavigateAndReadArgs {
    pub url: String,
    #[serde(default)]
    pub truncate: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FillAndSubmitArgs {
    #[serde(default)]
    pub fields: Option<Vec<FieldArgs>>,
    #[serde(default)]
    pub submit: Option<String>,
    #[serde(default)]
    pub wait_for: Option<String>,
    #[serde(default)]
    pub on_intercept: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldArgs {
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryArgs {
    #[serde(default)]
    pub filter: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameArgs {
    pub target: String,
}

/// `emulate`'s keys, typed; its values stay `Value` for `pipe_emulation`'s own validator, whose
/// messages name the field and the expected type (`"dpr" must be a number`) where serde's do not.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmulateArgs {
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub label: Option<Value>,
    #[serde(default)]
    pub width: Option<Value>,
    #[serde(default)]
    pub height: Option<Value>,
    #[serde(default)]
    pub dpr: Option<Value>,
    #[serde(default)]
    pub mobile: Option<Value>,
    #[serde(default)]
    pub touch: Option<Value>,
    #[serde(default)]
    pub orientation: Option<Value>,
}

/// `assert`'s keys, typed; its values stay `Value` and go back through
/// `commands::assert::from_json`, which owns every refusal `assert` makes and is shared with the
/// CLI. Serialised back rather than re-read so there is still one parser, not two.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssertArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub what: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matches: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unchecked: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<Value>,
}

impl AssertArgs {
    /// The object `commands::assert::from_json` reads. Only the keys the caller sent.
    #[must_use]
    pub fn as_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebmcpCallArgs {
    pub name: String,
    #[serde(default)]
    pub args: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchArgs {
    #[serde(default)]
    pub commands: Option<Vec<Value>>,
    #[serde(default)]
    pub stop_on_error: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;
    use serde_json::json;

    fn err(cmd: &Value) -> String {
        parse(cmd).expect_err("a refusal").to_string()
    }

    /// Every alias the hand-written match accepted still parses, to the same verb.
    #[test]
    fn every_accepted_spelling_still_parses() {
        for (spelling, canonical) in [
            ("fill-form", "fill_form"),
            ("fill_form", "fill_form"),
            ("fillform", "fill_form"),
        ] {
            let parsed = parse(&json!({"cmd": spelling, "pairs": []})).expect(spelling);
            assert_eq!(parsed.name(), canonical, "{spelling}");
        }
        for (spelling, canonical) in [
            ("navigate_and_read", "navigate_and_read"),
            ("navigate-and-read", "navigate_and_read"),
        ] {
            let parsed = parse(&json!({"cmd": spelling, "url": "http://x"})).expect(spelling);
            assert_eq!(parsed.name(), canonical, "{spelling}");
        }
        for spelling in ["fill_and_submit", "fill-and-submit"] {
            let parsed =
                parse(&json!({"cmd": spelling, "fields": [], "submit": "#go"})).expect(spelling);
            assert_eq!(parsed.name(), "fill_and_submit", "{spelling}");
        }
        for spelling in ["webmcp_list", "webmcp-list"] {
            assert_eq!(
                parse(&json!({"cmd": spelling})).expect(spelling).name(),
                "webmcp_list"
            );
        }
        for spelling in ["webmcp_call", "webmcp-call"] {
            let parsed = parse(&json!({"cmd": spelling, "name": "t"})).expect(spelling);
            assert_eq!(parsed.name(), "webmcp_call", "{spelling}");
        }
    }

    /// The canonical name is what the change-report allowlist reads, so no alias can fall out of
    /// it: `fill-form` mutates the page under every spelling.
    #[test]
    fn an_alias_carries_the_canonical_name_into_mutates_page() {
        for spelling in ["fill-form", "fill_form", "fillform"] {
            let parsed = parse(&json!({"cmd": spelling, "pairs": []})).unwrap();
            assert!(
                crate::pipe_report::mutates_page(parsed.name()),
                "{spelling}"
            );
        }
        assert!(crate::pipe_report::mutates_page(
            parse(&json!({"cmd": "webmcp-call", "name": "t"}))
                .unwrap()
                .name()
        ));
    }

    /// The whole point: a key nobody declared is an error naming the key, not a command that
    /// silently ignores it and then complains about something else.
    #[test]
    fn an_unknown_key_is_refused_by_name() {
        let message = err(&json!({"cmd": "click", "uidd": "n1"}));
        assert!(message.starts_with("click: "), "{message}");
        assert!(
            message.contains("\"uidd\""),
            "the key must be named: {message}"
        );
        // And the old answer — which named the wrong problem — is gone.
        assert!(!message.contains("provide"), "{message}");

        // A verb that takes nothing at all still refuses one.
        let message = err(&json!({"cmd": "back", "delta": -1}));
        assert!(message.contains("\"delta\""), "{message}");

        // `desired` belongs to `check`; `uncheck` already decided.
        let message = err(&json!({"cmd": "uncheck", "uid": "n1", "desired": true}));
        assert!(message.contains("\"desired\""), "{message}");
    }

    /// A missing required field names the command and the field, in the words it always used.
    #[test]
    fn a_missing_field_names_the_command_and_the_field() {
        assert_eq!(err(&json!({"cmd": "goto"})), "goto: missing \"url\"");
        assert_eq!(
            err(&json!({"cmd": "fill", "selector": "#a"})),
            "fill: missing \"value\""
        );
        assert_eq!(err(&json!({"cmd": "eval"})), "eval: missing \"expression\"");
        assert_eq!(err(&json!({"cmd": "type"})), "type: missing \"text\"");
        assert_eq!(err(&json!({"cmd": "press"})), "press: missing \"key\"");
        assert_eq!(err(&json!({"cmd": "scroll"})), "scroll: missing \"target\"");
        assert_eq!(err(&json!({"cmd": "frame"})), "frame: missing \"target\"");
        assert_eq!(
            err(&json!({"cmd": "select", "uid": "n1"})),
            "select: missing \"value\""
        );
    }

    /// A wrong type names the field too — serde's own message names neither.
    #[test]
    fn a_wrong_type_names_the_command_and_the_field() {
        let message = err(&json!({"cmd": "fill", "selector": "#a", "value": 42}));
        assert!(message.starts_with("fill: \"value\": "), "{message}");
        let message = err(&json!({"cmd": "click", "xy": "100,200"}));
        assert!(message.starts_with("click: \"xy\": "), "{message}");
        let message = err(&json!({"cmd": "inspect", "limit": "10"}));
        assert!(message.starts_with("inspect: \"limit\": "), "{message}");
    }

    #[test]
    fn an_absent_or_unknown_verb_keeps_its_wording() {
        assert_eq!(err(&json!({})), "Missing \"cmd\" field");
        assert_eq!(err(&json!({"cmd": ""})), "Missing \"cmd\" field");
        assert_eq!(
            err(&json!({"cmd": "frobnicate"})),
            "Unknown command: frobnicate"
        );
        assert_eq!(err(&json!([1, 2])), "Missing \"cmd\" field");
    }

    /// `xy` keeps its shape: two numbers, integral or not.
    #[test]
    fn xy_is_two_numbers_or_a_refusal() {
        let PipeCommand::Click(args) = parse(&json!({"cmd": "click", "xy": [12.5, 3]})).unwrap()
        else {
            panic!("click");
        };
        let [x, y] = args.xy.expect("a pair");
        assert!((x - 12.5).abs() < f64::EPSILON && (y - 3.0).abs() < f64::EPSILON);

        assert!(parse(&json!({"cmd": "click", "xy": [1, 2, 3]})).is_err());
        assert!(parse(&json!({"cmd": "click", "xy": [1]})).is_err());
        assert!(parse(&json!({"cmd": "click", "xy": "100,200"})).is_err());
        let PipeCommand::Click(args) =
            parse(&json!({"cmd": "click", "uid": "n1", "xy": null})).unwrap()
        else {
            panic!("click");
        };
        assert!(
            args.xy.is_none(),
            "an explicit null is an absent flag, as it always was"
        );
    }

    #[test]
    fn target_groups_are_exercised_through_both_parsers() {
        let cases: &[(&[&str], Value)] = &[
            (&["click"], json!({"cmd": "click"})),
            (
                &["click", "n1", "--selector", "#go"],
                json!({"cmd": "click", "uid": "n1", "selector": "#go"}),
            ),
            (&["click", "n1"], json!({"cmd": "click", "uid": "n1"})),
            (&["fill", "x"], json!({"cmd": "fill", "value": "x"})),
            (
                &["fill", "x", "--uid", "n1", "--selector", "#field"],
                json!({"cmd": "fill", "value": "x", "uid": "n1", "selector": "#field"}),
            ),
            (
                &["fill", "x", "--uid", "n1"],
                json!({"cmd": "fill", "value": "x", "uid": "n1"}),
            ),
            (
                &["select", "x", "--uid", "n1", "--selector", "select"],
                json!({"cmd": "select", "value": "x", "uid": "n1", "selector": "select"}),
            ),
            (
                &["check", "n1", "--selector", "#box"],
                json!({"cmd": "check", "uid": "n1", "selector": "#box"}),
            ),
            (
                &["uncheck", "n1", "--selector", "#box"],
                json!({"cmd": "uncheck", "uid": "n1", "selector": "#box"}),
            ),
            (
                &["upload", "--uid", "n1", "--selector", "input"],
                json!({"cmd": "upload", "files": [], "uid": "n1", "selector": "input"}),
            ),
            (
                &["text", "n1", "--selector", "main"],
                json!({"cmd": "text", "uid": "n1", "selector": "main"}),
            ),
            (&["text"], json!({"cmd": "text"})),
            (
                &["screenshot", "--uid", "n1", "--selector", "main"],
                json!({"cmd": "screenshot", "uid": "n1", "selector": "main"}),
            ),
            (&["screenshot"], json!({"cmd": "screenshot"})),
            (
                &["download", "https://example.test", "--uid", "n1"],
                json!({"cmd": "download", "url": "https://example.test", "uid": "n1"}),
            ),
            (
                &["download", "https://example.test"],
                json!({"cmd": "download", "url": "https://example.test"}),
            ),
        ];

        for (argv, command) in cases {
            let cli = crate::cli::Cli::try_parse_from(
                std::iter::once("chrome-agent").chain(argv.iter().copied()),
            );
            let pipe = parse(command);
            assert_eq!(
                cli.is_ok(),
                pipe.is_ok(),
                "CLI {argv:?} and pipe {command} disagree"
            );
        }
    }

    #[test]
    fn invalid_interception_policy_is_a_refusal() {
        for command in [
            json!({"cmd": "click", "uid": "n1", "on_intercept": "refuze"}),
            json!({"cmd": "check", "uid": "n1", "on_intercept": "refuze"}),
            json!({"cmd": "uncheck", "uid": "n1", "on_intercept": "refuze"}),
            json!({"cmd": "download", "url": "https://example.test", "on_intercept": "refuze"}),
            json!({"cmd": "fill_and_submit", "fields": [], "submit": "#go", "on_intercept": "refuze"}),
        ] {
            let message = err(&command);
            assert!(
                message.contains("Unknown --on-intercept value"),
                "{message}"
            );
        }
    }

    #[test]
    fn ambiguous_waits_and_irrelevant_emulation_fields_are_refused() {
        for command in [
            json!({"cmd": "wait", "what": "text", "pattern": "done", "url": "/done"}),
            json!({"cmd": "wait", "text": "done", "selector": ".done"}),
            json!({"cmd": "wait", "pattern": "done"}),
            json!({"cmd": "emulate", "action": "status", "width": 320}),
            json!({"cmd": "emulate", "action": "reset", "mobile": true}),
        ] {
            assert!(parse(&command).is_err(), "accepted {command}");
        }
    }

    #[test]
    fn screenshot_numbers_cannot_wrap_the_cdp_types() {
        let message = err(&json!({"cmd": "screenshot", "max_width": 4_294_967_296_u64}));
        assert!(message.contains("max_width"), "{message}");
        let message = err(&json!({"cmd": "screenshot", "quality": 4_294_967_296_u64}));
        assert!(message.contains("quality"), "{message}");
    }

    /// `assert` round-trips through the CLI's own parser, so the two cannot disagree.
    #[test]
    fn assert_keeps_only_the_keys_the_caller_sent() {
        let PipeCommand::Assert(args) =
            parse(&json!({"cmd": "assert", "what": "exists", "selector": "#a", "min": 1})).unwrap()
        else {
            panic!("assert");
        };
        let value = args.as_value();
        assert_eq!(value, json!({"what": "exists", "selector": "#a", "min": 1}));
        assert!(crate::commands::assert::from_json(&value).is_ok());

        let message = err(&json!({"cmd": "assert", "what": "exists", "mim": 1}));
        assert!(message.contains("\"mim\""), "{message}");
    }
}
