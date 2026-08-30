//! `WebMCP` tool discovery and invocation — `document.modelContext.getTools()` and
//! `.executeTool()` (W3C WICG `WebMCP`; `navigator.modelContext` is deprecated since Chrome 150).
//!
//! This module only talks to the page. Whether a call actually moved anything is a *separate*
//! question, answered by the same accessibility-tree diff every other mutating command gets
//! (`webmcp_call` is in `pipe_report::mutates_page`) — this module never judges that itself.
//!
//! **What the protocol does not give an agent**, measured against 17 real tools across 6 sites
//! while this module was built: there is no `outputSchema`. A tool's declared return is a
//! freeform string with no contract to check it against — `{"success":true}` and nothing having
//! moved is not a protocol violation, it is a tool no more suspicious-looking than a correct one.
//! `list_tools` reports `output_schema: null` on every entry for exactly this reason: it is
//! information a caller needs before it trusts a declared result, not a gap to paper over.
//!
//! **Under a `frame` binding**, `eval` already runs in that frame's isolated world (`frame`'s own
//! doc comment), which shares the frame's DOM but not plain JS properties a MAIN-world script
//! assigned to it. Measured against `webmcp_iframe_host.html` (top page with no tools, iframe
//! whose own main-world script installs the fixture's polyfill on ITS `document.modelContext`):
//! bound into that iframe with `frame`, `typeof document.modelContext` is `"undefined"` — the
//! same blindness `eval` already has for main-world variables, just hitting a property instead
//! of a variable. A NATIVE, platform-implemented `document.modelContext` (the real Chrome flag,
//! not a polyfill) is a `WebIDL` attribute rather than a page-assigned property and may or may not
//! share that visibility — Blink's isolated worlds share the underlying DOM objects, so a native
//! IDL attribute is the more likely case to be visible, but this was NOT verified: it needs a
//! Chrome build new enough to carry the flag AND an iframe registering tools natively, and is
//! left for whoever needs the native path next. What IS verified either way: `list_tools` and
//! `call_tool` report `frame_scoped: true` whenever a frame is bound, so an empty tool list next
//! to that flag reads as "unproven", never as "this page has none".

use serde_json::{Value, json};

use crate::cdp::client::CdpClient;
use crate::commands::eval;

/// Thrown when `document.modelContext` is absent and no `frame` is bound. The literal text is
/// unique to this module, which is what lets `hints.rs` recognise it without matching some
/// unrelated page error.
pub const NO_MODEL_CONTEXT_MARKER: &str =
    "chrome-agent: document.modelContext is undefined on this page.";

/// Thrown instead of [`NO_MODEL_CONTEXT_MARKER`] when a `frame` is bound: the absence is real
/// evidence for the top document, but not for a frame whose own main-world script may have
/// installed a polyfill this isolated-world check cannot see (module doc).
pub const NO_MODEL_CONTEXT_FRAME_MARKER: &str = "chrome-agent: document.modelContext is undefined \
     in the bound frame's isolated world — this does not prove the frame has no tools, since a \
     polyfill the frame's own main-world script installs is invisible here.";

/// Thrown when `--name`/`"name"` matches no tool `getTools()` reported. Only the prefix is
/// matched by `hints.rs` — the tool name and the list of known tools that follow are page
/// content, and vary.
pub const UNKNOWN_TOOL_PREFIX: &str = "chrome-agent: no WebMCP tool named";

fn guard_js(frame_scoped: bool) -> String {
    let marker = if frame_scoped { NO_MODEL_CONTEXT_FRAME_MARKER } else { NO_MODEL_CONTEXT_MARKER };
    format!(
        "if (typeof document.modelContext === 'undefined') {{ throw new Error({}); }}",
        serde_json::to_string(marker).expect("string literal always encodes")
    )
}

/// One tool as `getTools()` reported it, reshaped for JSON output.
///
/// `inputSchema` is a JSON *string* per spec (never an object) — kept verbatim in
/// `input_schema_raw` because that is what the tool actually declared, and additionally parsed
/// into `input_schema` on a best-effort basis for readability. A tool whose schema does not even
/// parse is itself something worth seeing, not a reason to hide the raw text.
fn tool_summary(t: &Value) -> Value {
    let raw_schema = t.get("inputSchema").and_then(Value::as_str);
    let mut obj = json!({
        "name": t.get("name").and_then(Value::as_str).unwrap_or_default(),
        "description": t.get("description").and_then(Value::as_str),
        "title": t.get("title").and_then(Value::as_str),
        "origin": t.get("origin").and_then(Value::as_str),
        "input_schema_raw": raw_schema,
        // The spec has no counterpart to inputSchema for the return value. Reported per tool
        // (not once for the whole list) so an agent iterating tools sees it without having to
        // recall a note stated somewhere else.
        "output_schema": Value::Null,
    });
    if let Some(raw) = raw_schema
        && let Ok(parsed) = serde_json::from_str::<Value>(raw)
        && let Some(map) = obj.as_object_mut()
    {
        map.insert("input_schema".into(), parsed);
    }
    obj
}

/// `getTools()`, reshaped per [`tool_summary`], plus whether this call is scoped to a bound
/// `frame` — see the module doc for what that means for visibility.
pub struct ToolList {
    pub tools: Vec<Value>,
    pub frame_scoped: bool,
}

/// `document.modelContext.getTools()`, reshaped per [`tool_summary`].
pub async fn list_tools(client: &CdpClient) -> Result<ToolList, crate::BoxError> {
    let frame_scoped = client.frame_context().is_some();
    let expr = format!(
        "(async () => {{ {guard}
          const tools = await document.modelContext.getTools();
          return tools.map(t => ({{
            name: t.name, description: t.description || null, title: t.title || null,
            origin: t.origin || null, inputSchema: t.inputSchema,
          }}));
        }})()",
        guard = guard_js(frame_scoped),
    );
    let raw = eval::run_raw(client, &expr).await?;
    let tools = raw.as_array().map(|arr| arr.iter().map(tool_summary).collect()).unwrap_or_default();
    Ok(ToolList { tools, frame_scoped })
}

/// What `executeTool` itself returned — the tool's side of the story. Whether the page
/// measurably moved is a separate question the caller answers via the shared change report.
pub struct ToolCallOutcome {
    pub tool: String,
    pub declared_result: String,
    /// `executeTool` is specified to always resolve to a string. `false` means this page's own
    /// tool implementation (typically a polyfill — the spec does not constrain what one
    /// returns) handed back something else, and chrome-agent used `JSON.stringify` so there is
    /// still something to report instead of silently discarding it.
    pub declared_result_was_string: bool,
    /// See the module doc: under a `frame` binding this call reached that frame's isolated
    /// world, not the top document's main world.
    pub frame_scoped: bool,
}

/// Call a tool by name. Resolves the name to the `RegisteredTool` `executeTool` requires
/// (passing a bare name is the exact mistake the spec's own error message does not explain:
/// `TypeError: The provided value is not of type 'RegisteredTool'.`) and always hands
/// `executeTool` a validated JSON string for its second argument (its own complaint about a
/// non-string argument — "Failed to parse input arguments" on native Chrome — is therefore
/// unreachable through this path; the validation below is what replaces it with a clearer one).
pub async fn call_tool(
    client: &CdpClient,
    name: &str,
    args_json: &str,
) -> Result<ToolCallOutcome, crate::BoxError> {
    if serde_json::from_str::<Value>(args_json).is_err() {
        return Err(format!("webmcp call: --args is not valid JSON: {args_json}").into());
    }
    let frame_scoped = client.frame_context().is_some();
    let name_lit = serde_json::to_string(name)?;
    let args_lit = serde_json::to_string(args_json)?;
    let unknown_prefix_lit =
        serde_json::to_string(UNKNOWN_TOOL_PREFIX).expect("string literal always encodes");
    let expr = format!(
        "(async () => {{ {guard}
          const tools = await document.modelContext.getTools();
          const tool = tools.find(t => t.name === {name_lit});
          if (!tool) {{
            const names = tools.map(t => t.name).join(', ');
            throw new Error({unknown_prefix_lit} + ' ' + {name_lit} + '. Known tools: ' + (names || '(none registered)') + '.');
          }}
          const result = await document.modelContext.executeTool(tool, {args_lit});
          return {{
            name: tool.name,
            result: typeof result === 'string' ? result : JSON.stringify(result),
            wasString: typeof result === 'string',
          }};
        }})()",
        guard = guard_js(frame_scoped),
    );
    let raw = eval::run_raw(client, &expr).await?;
    let tool = raw.get("name").and_then(Value::as_str).unwrap_or(name).to_string();
    let declared_result = raw
        .get("result")
        .and_then(Value::as_str)
        .ok_or("webmcp call: executeTool() produced no readable result")?
        .to_string();
    let declared_result_was_string = raw.get("wasString").and_then(Value::as_bool).unwrap_or(false);
    Ok(ToolCallOutcome { tool, declared_result, declared_result_was_string, frame_scoped })
}

/// Text-mode rendering for `webmcp list` — one line per tool, the absent `outputSchema` stated
/// once rather than repeated per line.
#[must_use]
pub fn render_list_text(tools: &[Value]) -> String {
    if tools.is_empty() {
        return "No WebMCP tools registered on this page.".to_string();
    }
    let mut out = String::new();
    for t in tools {
        let name = t.get("name").and_then(Value::as_str).unwrap_or("?");
        let desc = t.get("description").and_then(Value::as_str).unwrap_or("");
        out.push_str(&format!("tool={name} \"{desc}\"\n"));
    }
    out.push_str(
        "note: no tool here carries an outputSchema — the protocol defines none. A declared \
         result is never checked against a shape; `webmcp call` reports what the page measurably \
         did alongside it, and that is the only corroboration available.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_summary_parses_a_wellformed_schema_and_keeps_the_raw_text() {
        let raw = json!({
            "name": "add_to_cart",
            "description": "Add a product to the cart.",
            "title": null,
            "origin": "https://shop.example",
            "inputSchema": "{\"type\":\"object\",\"properties\":{\"item\":{\"type\":\"string\"}}}",
        });
        let summary = tool_summary(&raw);
        assert_eq!(summary["name"], "add_to_cart");
        assert_eq!(summary["output_schema"], Value::Null);
        assert_eq!(summary["input_schema"]["type"], "object");
        assert_eq!(
            summary["input_schema_raw"],
            "{\"type\":\"object\",\"properties\":{\"item\":{\"type\":\"string\"}}}"
        );
    }

    #[test]
    fn tool_summary_keeps_the_raw_text_when_the_schema_does_not_parse() {
        let raw = json!({
            "name": "broken_schema_tool",
            "description": "x",
            "inputSchema": "{not json",
        });
        let summary = tool_summary(&raw);
        assert_eq!(summary["input_schema_raw"], "{not json");
        assert!(summary.get("input_schema").is_none(), "an unparseable schema must not fabricate one");
    }

    #[test]
    fn render_list_text_states_the_absent_output_schema_once() {
        let tools = vec![json!({"name": "a", "description": "d"})];
        let text = render_list_text(&tools);
        assert!(text.contains("tool=a \"d\""));
        assert_eq!(text.matches("outputSchema").count(), 1, "stated once, not per tool: {text}");
    }

    #[test]
    fn render_list_text_names_an_empty_page_plainly() {
        assert_eq!(render_list_text(&[]), "No WebMCP tools registered on this page.");
    }
}
