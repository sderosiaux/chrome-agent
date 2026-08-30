//! `WebMCP` tool discovery and invocation — `document.modelContext.getTools()` and
//! `.executeTool()` (W3C WICG; `navigator.modelContext` is deprecated since Chrome 150).
//!
//! This module only talks to the page; whether a call moved anything is the shared
//! accessibility-tree diff's answer. The protocol defines no `outputSchema`, so a declared
//! result has no contract to check it against and `list_tools` reports `output_schema: null`.
//!
//! Under a `frame` binding the isolated world sees the frame's DOM but not a
//! `document.modelContext` a main-world script assigned (measured; a NATIVE one is untested),
//! so both calls report `frame_scoped: true` and an empty list there means unproven.

use serde_json::{Value, json};

use crate::cdp::client::CdpClient;
use crate::commands::eval;

/// Thrown when `document.modelContext` is absent and no `frame` is bound. The literal text is
/// what `hints.rs` matches on.
pub const NO_MODEL_CONTEXT_MARKER: &str =
    "chrome-agent: document.modelContext is undefined on this page.";

/// Thrown instead of [`NO_MODEL_CONTEXT_MARKER`] when a `frame` is bound, where absence is not
/// evidence: a polyfill installed by the frame's main-world script is invisible here.
pub const NO_MODEL_CONTEXT_FRAME_MARKER: &str = "chrome-agent: document.modelContext is undefined \
     in the bound frame's isolated world — this does not prove the frame has no tools, since a \
     polyfill the frame's own main-world script installs is invisible here.";

/// Thrown when the requested name matches no tool `getTools()` reported. `hints.rs` matches
/// only this prefix; the tool names that follow are page content.
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
/// `inputSchema` is a JSON *string* per spec. It is kept verbatim in `input_schema_raw` and
/// additionally parsed into `input_schema` when it parses; an unparseable one is left raw.
fn tool_summary(t: &Value) -> Value {
    let raw_schema = t.get("inputSchema").and_then(Value::as_str);
    let mut obj = json!({
        "name": t.get("name").and_then(Value::as_str).unwrap_or_default(),
        "description": t.get("description").and_then(Value::as_str),
        "title": t.get("title").and_then(Value::as_str),
        "origin": t.get("origin").and_then(Value::as_str),
        "input_schema_raw": raw_schema,
        // The spec has no counterpart to inputSchema for the return value. Per tool, not once
        // per list, so an agent iterating tools sees it.
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

/// `getTools()` reshaped per [`tool_summary`], plus whether a `frame` was bound.
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

/// What `executeTool` returned — the tool's own claim. Whether the page moved is a separate
/// question, answered by the shared change report.
pub struct ToolCallOutcome {
    pub tool: String,
    pub declared_result: String,
    /// `executeTool` is specified to resolve to a string. `false` means the page's own
    /// implementation returned something else, which is `JSON.stringify`d rather than dropped.
    pub declared_result_was_string: bool,
    /// The call reached a bound frame's isolated world, not the top document's main world.
    pub frame_scoped: bool,
}

/// Call a tool by name, closing two of the spec's traps: `executeTool` needs the actual
/// `RegisteredTool` object (a bare name is `TypeError: not of type 'RegisteredTool'`) and a
/// JSON *string* second argument, so the name is resolved and the args validated first.
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

/// Text-mode `webmcp list`: one line per tool, with the absent `outputSchema` stated once.
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
