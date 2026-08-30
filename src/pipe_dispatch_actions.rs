//! Composite and form-control dispatchers, plus `run_batch`. Re-exported from
//! `pipe_dispatch` so callers use a single path.

use serde_json::{json, Value};

use crate::cdp::client::CdpClient;
use crate::commands;
use crate::session::{self, SessionStore};

use crate::pipe_dispatch::{attach_snapshot, cmd_max_depth, parse_xy};
use crate::run_helpers::{get_uid_map, merge_into};

// --- Composite dispatchers ---

pub async fn dispatch_navigate_and_read(
    client: &CdpClient, _store: &mut SessionStore, browser_name: &str, page_name: &str,
    _target_id: &str, timeout: u64, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let url = cmd.get("url").and_then(Value::as_str).ok_or("navigate_and_read: missing \"url\"")?;
    let truncate = cmd.get("truncate").and_then(Value::as_u64).map(|v| v as usize);
    let goto_result = commands::goto::run(client, url, timeout, &[]).await?;
    client.set_frame_context(None); // navigation invalidates any bound frame (issue #8)
    let _ = commands::history::append(&goto_result.url, &goto_result.title, page_name);
    let read_result = commands::read::run(client, false, truncate).await?;
    let mut out = json!({"ok": true, "url": goto_result.url, "title": goto_result.title, "content": read_result.text_content});
    // `landed` matters more here than on a bare `goto`: without it a login page's prose comes
    // back as if it were the article that was asked for.
    goto_result.landed.attach(&mut out, browser_name);
    Ok(out)
}

pub async fn dispatch_fill_and_submit(client: &CdpClient, timeout: u64, cmd: &Value) -> Result<Value, crate::BoxError> {
    let fields = cmd.get("fields").and_then(Value::as_array).ok_or("fill_and_submit: missing \"fields\" array")?;
    let submit_selector = cmd.get("submit").and_then(Value::as_str).ok_or("fill_and_submit: missing \"submit\" selector")?;
    let wait_for = cmd.get("wait_for").and_then(Value::as_str);
    let field_count = fields.len();
    let mut outcomes = Vec::new();
    for field in fields {
        let selector = field.get("selector").and_then(Value::as_str).ok_or("fill_and_submit: each field needs \"selector\"")?;
        let value = field.get("value").and_then(Value::as_str).ok_or("fill_and_submit: each field needs \"value\"")?;
        let outcome = crate::element::fill_selector(client, selector, value).await?;
        outcomes.push((selector.to_string(), outcome));
    }
    let submitted = crate::element::click_selector(
        client,
        submit_selector,
        crate::hit_test::OnIntercept::from_cmd(cmd, crate::hit_test::OnIntercept::default()),
    )
    .await?;
    if let Some(pattern) = wait_for {
        let is_selector = pattern.contains('.') || pattern.contains('#') || pattern.contains('[') || pattern.contains('>');
        let wait_type = if is_selector { "selector" } else { "text" };
        commands::wait::run(client, wait_type, pattern, timeout, 500).await?;
    }
    // The read below is best effort: Readability rejects plenty of legitimate pages, and the
    // fill and submit already landed. Failing here would invite a second submit.
    let message = format!("Filled {field_count} fields, submitted, waited for '{}'", wait_for.unwrap_or("none"));
    let mut out = json!({"ok": true, "message": message});
    // The submit's own delivery, at the top level where the verdict wiring reads it: a submit
    // button under a consent banner would otherwise report as a successful submit.
    merge_into(&mut out, Some(&submitted.report()));
    // The only witness this command has: the change report runs after the submit, by which
    // time a field the page rewrote on the way in is no longer visible.
    out["values"] = crate::run_helpers::bulk_fill_report("selector", &outcomes);
    match commands::read::run(client, false, None).await {
        Ok(read_result) => out["content"] = json!(read_result.text_content),
        Err(e) => out["read_error"] = json!(e.to_string()),
    }
    Ok(out)
}

pub fn dispatch_history(cmd: &Value) -> Result<Value, crate::BoxError> {
    let filter = cmd.get("filter").and_then(Value::as_str);
    let limit = cmd.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
    let entries = commands::history::run(filter, limit)?;
    let entries_json: Vec<Value> = entries.iter()
        .map(|e| json!({"ts": e.ts, "url": e.url, "title": e.title, "page": e.page})).collect();
    Ok(json!({"ok": true, "entries": entries_json}))
}

pub async fn dispatch_fill_form(
    client: &CdpClient, store: &mut SessionStore, browser_name: &str, page_name: &str,
    target_id: &str, global_max_depth: Option<usize>, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let pairs = cmd.get("pairs").and_then(Value::as_array)
        .ok_or("fill-form requires \"pairs\" array (e.g. [{\"uid\":\"n1\",\"value\":\"a\"}])")?;
    let uid_map = get_uid_map(store, browser_name, page_name);
    let mut outcomes = Vec::new();
    for pair in pairs {
        let uid = pair.get("uid").and_then(Value::as_str).ok_or("Each pair needs \"uid\"")?;
        let value = pair.get("value").and_then(Value::as_str).ok_or("Each pair needs \"value\"")?;
        let outcome = crate::element::fill(client, &uid_map, uid, value).await?;
        outcomes.push((uid.to_string(), outcome));
    }
    let inspect = cmd.get("inspect").and_then(Value::as_bool).unwrap_or(false);
    let mut obj = json!({"ok": true, "message": format!("Filled {} fields", pairs.len())});
    obj["values"] = crate::run_helpers::bulk_fill_report("uid", &outcomes);
    if inspect {
        let max_depth = cmd.get("max_depth").and_then(Value::as_u64).map(|v| v as usize).or(global_max_depth);
        let snapshot = commands::inspect::run(client, false, max_depth, None, None).await?;
        obj["snapshot"] = json!(snapshot.text);
        if let Some(browser_s) = store.browsers.get_mut(browser_name) {
            let page = session::ensure_page(browser_s, page_name, target_id);
            page.uid_map = snapshot.uid_map;
        }
    }
    Ok(obj)
}

pub async fn dispatch_hover(
    client: &CdpClient, store: &SessionStore, browser_name: &str, page_name: &str, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let uid = cmd.get("uid").and_then(Value::as_str).ok_or("hover requires \"uid\"")?;
    let uid_map = get_uid_map(store, browser_name, page_name);
    crate::element::hover(client, &uid_map, uid).await?;
    Ok(json!({"ok": true, "message": format!("Hovered uid={uid}")}))
}

// --- New command dispatchers ---

pub async fn dispatch_dblclick(
    client: &CdpClient, store: &mut SessionStore, browser_name: &str, page_name: &str,
    target_id: &str, global_max_depth: Option<usize>,
    report: crate::run_helpers::ReportPolicy, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let inspect = cmd.get("inspect").and_then(Value::as_bool).unwrap_or(false);
    let max_depth = cmd_max_depth(cmd).or(global_max_depth);
    // Hoisted out of the `else if let` below: the non-Send `?` residual must not be held
    // across an await, or the future stops being Send.
    let xy = parse_xy(cmd)?;
    let on_intercept = crate::hit_test::OnIntercept::from_cmd(cmd, report.on_intercept);
    // The node is resolved before the action: afterwards it may be detached, and the answer
    // would describe a different page.
    let (msg, details) = if let Some(sel) = cmd.get("selector").and_then(Value::as_str) {
        let outcome = crate::element::dblclick_selector(client, sel, on_intercept).await?;
        let target = format!("selector '{sel}'");
        (
            outcome
                .refusal_message("double-click", &target)
                .unwrap_or_else(|| format!("Double-clicked {target}")),
            Some(outcome.report()),
        )
    } else if let Some((x, y)) = xy {
        crate::element::dblclick_at_coords(client, x, y).await?;
        (format!("Double-clicked at ({x}, {y})"), None)
    } else if let Some(uid) = cmd.get("uid").and_then(Value::as_str) {
        let uid_map = get_uid_map(store, browser_name, page_name);
        let (msg, outcome) = commands::dblclick::run(client, &uid_map, uid, on_intercept).await?;
        (msg, Some(outcome.report()))
    } else {
        return Err("dblclick: provide \"uid\", \"selector\", or \"xy\"".into());
    };
    let mut obj = json!({"ok": true, "message": msg});
    merge_into(&mut obj, details.as_ref());
    if inspect {
        let snapshot = attach_snapshot(client, store, browser_name, page_name, target_id, max_depth).await?;
        obj["snapshot"] = json!(snapshot);
    }
    Ok(obj)
}

pub async fn dispatch_select(
    client: &CdpClient, store: &mut SessionStore, browser_name: &str, page_name: &str,
    target_id: &str, global_max_depth: Option<usize>, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let value = cmd.get("value").and_then(Value::as_str).ok_or("select: missing \"value\"")?;
    let inspect = cmd.get("inspect").and_then(Value::as_bool).unwrap_or(false);
    let max_depth = cmd_max_depth(cmd).or(global_max_depth);
    let target = crate::run_helpers::target_details(
        client,
        cmd.get("selector").and_then(Value::as_str),
        cmd.get("uid").and_then(Value::as_str),
    )
    .await;
    let (msg, outcome) = if let Some(sel) = cmd.get("selector").and_then(Value::as_str) {
        let outcome = crate::element::select_option_selector(client, sel, value).await?;
        (format!("Selected \"{}\" on selector '{sel}'", outcome.label()), outcome)
    } else if let Some(uid) = cmd.get("uid").and_then(Value::as_str) {
        let uid_map = get_uid_map(store, browser_name, page_name);
        let outcome = crate::element::select_option(client, &uid_map, uid, value).await?;
        (format!("Selected \"{}\" on uid={uid}", outcome.label()), outcome)
    } else {
        return Err("select: provide \"uid\" or \"selector\"".into());
    };
    let mut obj = json!({"ok": true, "message": msg});
    merge_into(&mut obj, Some(&crate::run_helpers::select_report(&outcome)));
    merge_into(&mut obj, target.as_ref());
    if inspect {
        let snapshot = attach_snapshot(client, store, browser_name, page_name, target_id, max_depth).await?;
        obj["snapshot"] = json!(snapshot);
    }
    Ok(obj)
}

/// `desired` comes from the caller, not from the command: the dispatcher already knows whether
/// the verb was `check` or `uncheck`, and reading it back out of a cloned Value with the field
/// inserted was the same decision made twice.
pub async fn dispatch_check(
    client: &CdpClient, store: &SessionStore, browser_name: &str, page_name: &str,
    report: crate::run_helpers::ReportPolicy, desired: bool, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let target = crate::run_helpers::target_details(
        client,
        cmd.get("selector").and_then(Value::as_str),
        cmd.get("uid").and_then(Value::as_str),
    )
    .await;
    let outcome = if let Some(sel) = cmd.get("selector").and_then(Value::as_str) {
        crate::element::set_checked_selector(client, sel, desired).await?
    } else if let Some(uid) = cmd.get("uid").and_then(Value::as_str) {
        let uid_map = get_uid_map(store, browser_name, page_name);
        let on_intercept = crate::hit_test::OnIntercept::from_cmd(cmd, report.on_intercept);
        crate::element::set_checked(client, &uid_map, uid, desired, on_intercept).await?
    } else {
        return Err("check: provide \"uid\" or \"selector\"".into());
    };
    let (message, details) = crate::run_helpers::check_report(outcome);
    let mut obj = json!({"ok": true, "message": message});
    if let Some(uid) = target.as_ref().and_then(|t| t.get("uid")) {
        obj["uid"] = uid.clone();
    }
    // `observed_after_ms` is absent when the element already held the state: nothing was
    // dispatched, so there is no post-action moment.
    merge_into(&mut obj, details.as_ref());
    Ok(obj)
}

pub async fn dispatch_upload(
    client: &CdpClient, store: &SessionStore, browser_name: &str, page_name: &str, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let files: Vec<String> = cmd.get("files").and_then(Value::as_array)
        .ok_or("upload: missing \"files\" array")?
        .iter().filter_map(|v| v.as_str().map(String::from)).collect();
    let target = crate::run_helpers::target_details(
        client,
        cmd.get("selector").and_then(Value::as_str),
        cmd.get("uid").and_then(Value::as_str),
    )
    .await;
    let msg = if let Some(uid) = cmd.get("uid").and_then(Value::as_str) {
        let uid_map = get_uid_map(store, browser_name, page_name);
        crate::element::set_file_input(client, &uid_map, uid, &files).await?;
        format!("Uploaded {} file(s) to uid={uid}", files.len())
    } else if let Some(sel) = cmd.get("selector").and_then(Value::as_str) {
        crate::element::set_file_input_selector(client, sel, &files).await?;
        format!("Uploaded {} file(s) to selector '{sel}'", files.len())
    } else {
        return Err("upload: provide \"uid\" or \"selector\"".into());
    };
    let mut obj = json!({"ok": true, "message": msg});
    merge_into(&mut obj, target.as_ref());
    Ok(obj)
}

pub async fn dispatch_drag(
    client: &CdpClient, store: &SessionStore, browser_name: &str, page_name: &str, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let from = cmd.get("from").and_then(Value::as_str).ok_or("drag: missing \"from\" uid")?;
    let to = cmd.get("to").and_then(Value::as_str).ok_or("drag: missing \"to\" uid")?;
    let uid_map = get_uid_map(store, browser_name, page_name);
    crate::element::drag(client, &uid_map, from, to).await?;
    Ok(json!({"ok": true, "message": format!("Dragged uid={from} to uid={to}")}))
}

// --- Assert ---

/// `assert` for pipe and batch. No exit code here, so `held` rides on `ok`: a claim that did
/// not hold answers `{"ok":false,"assertion":{…}}`, one that could not be checked answers
/// `{"ok":false,"error":…}`, and the presence of `assertion` tells them apart. Not in
/// `mutates_page`: a read, so no change report and no verdict.
pub async fn dispatch_assert(
    client: &CdpClient, store: &SessionStore, browser_name: &str, page_name: &str, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let assertion = commands::assert::from_json(cmd)?;
    let uid_map = get_uid_map(store, browser_name, page_name);
    let outcome = commands::assert::run(client, &uid_map, &assertion).await?;
    Ok(outcome.to_json())
}

// --- WebMCP ---

/// `webmcp_list` for pipe and batch. Not in `mutates_page`: it is a read, like `assert`.
pub async fn dispatch_webmcp_list(client: &CdpClient) -> Result<Value, crate::BoxError> {
    let tools = commands::webmcp::list_tools(client).await?;
    Ok(json!({"ok": true, "tools": tools.tools, "frame_scoped": tools.frame_scoped}))
}

/// `webmcp_call` for pipe and batch. IS in `mutates_page`: a tool's declared result is
/// freeform text with no schema, so the accessibility-tree delta attached after this returns
/// is the only corroboration. `declared_result` carries the tool's own claim.
pub async fn dispatch_webmcp_call(client: &CdpClient, cmd: &Value) -> Result<Value, crate::BoxError> {
    let name = cmd.get("name").and_then(Value::as_str).ok_or("webmcp_call: missing \"name\"")?;
    let args = cmd.get("args").cloned().unwrap_or_else(|| json!({}));
    let args_text = serde_json::to_string(&args)?;
    let outcome = commands::webmcp::call_tool(client, name, &args_text).await?;
    let mut out = json!({
        "ok": true,
        "message": format!("Called WebMCP tool '{}'", outcome.tool),
        "tool": outcome.tool,
        "declared_result": outcome.declared_result,
    });
    if let Ok(parsed) = serde_json::from_str::<Value>(&outcome.declared_result) {
        out["declared_result_parsed"] = parsed;
    }
    if !outcome.declared_result_was_string {
        out["declared_result_was_string"] = json!(false);
    }
    if outcome.frame_scoped {
        out["frame_scoped"] = json!(true);
    }
    Ok(out)
}

// --- Batch ---

/// Run commands through `dispatch_single`. The one loop behind both batch front ends: CLI
/// `batch` and `{"cmd":"batch",…}` in pipe mode.
///
/// `stop_on_error` is off by default — a batch also collects independent observations. When
/// on, the response carries `stopped_at` and `skipped`.
#[allow(clippy::too_many_arguments)]
pub async fn run_batch(
    client: &CdpClient,
    browser_client: &CdpClient,
    store: &mut SessionStore,
    browser_name: &str,
    page_name: &str,
    target_id: &str,
    timeout: u64,
    global_max_depth: Option<usize>,
    report: crate::run_helpers::ReportPolicy,
    commands_list: &[Value],
    stop_on_error: bool,
    emulation_recovery: &mut crate::pipe_dispatch::EmulationRecovery,
) -> Value {
    let mut results = Vec::with_capacity(commands_list.len());
    let mut stopped_at = None;
    // Each entry is checked independently: a reset repairs later entries, never earlier ones.
    for (index, c) in commands_list.iter().enumerate() {
        let r = if let Some(response) = emulation_recovery.refusal_for(c) {
            response
        } else {
            crate::pipe_dispatch::dispatch_single(
                client, browser_client, store, browser_name, page_name, target_id, timeout,
                global_max_depth, report, c, emulation_recovery,
            ).await
        };
        emulation_recovery.update_after(c, &r);
        let ok = r.get("ok").and_then(Value::as_bool).unwrap_or(false);
        results.push(r);
        if stop_on_error && !ok {
            stopped_at = Some(index);
            break;
        }
    }
    let all_ok = results.iter().all(|r| r.get("ok").and_then(Value::as_bool).unwrap_or(false));
    let mut obj = json!({"ok": all_ok, "results": results});
    if let Some(index) = stopped_at {
        obj["stopped_at"] = json!(index);
        obj["skipped"] = json!(commands_list.len() - index - 1);
    }
    obj
}
