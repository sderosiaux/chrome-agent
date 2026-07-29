//! Composite and form-control dispatchers, split out of `pipe_dispatch.rs` to stay
//! under the repo's 1000-line file cap. Re-exported from `pipe_dispatch` so callers
//! keep using a single path.

use serde_json::{json, Value};

use crate::cdp::client::CdpClient;
use crate::commands;
use crate::session::{self, SessionStore};

use crate::pipe_dispatch::{attach_snapshot, cmd_max_depth, get_uid_map, merge_into, parse_xy};

// ---------------------------------------------------------------------------
// Composite dispatchers
// ---------------------------------------------------------------------------

pub async fn dispatch_navigate_and_read(
    client: &CdpClient, _store: &mut SessionStore, _browser_name: &str, page_name: &str,
    _target_id: &str, timeout: u64, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let url = cmd.get("url").and_then(Value::as_str).ok_or("navigate_and_read: missing \"url\"")?;
    let truncate = cmd.get("truncate").and_then(Value::as_u64).map(|v| v as usize);
    let goto_result = commands::goto::run(client, url, timeout, &[]).await?;
    client.set_frame_context(None); // navigation invalidates any bound frame (issue #8)
    let _ = commands::history::append(&goto_result.url, &goto_result.title, page_name);
    let read_result = commands::read::run(client, false, truncate).await?;
    Ok(json!({"ok": true, "url": goto_result.url, "title": goto_result.title, "content": read_result.text_content}))
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
    crate::element::click_selector(client, submit_selector).await?;
    if let Some(pattern) = wait_for {
        let is_selector = pattern.contains('.') || pattern.contains('#') || pattern.contains('[') || pattern.contains('>');
        let wait_type = if is_selector { "selector" } else { "text" };
        commands::wait::run(client, wait_type, pattern, timeout, 500).await?;
    }
    // Best effort: Readability rejects plenty of legitimate pages, and the fill and the
    // submit have already landed. Failing the whole command there tells an agent its
    // mutation did not happen, and the natural response to that is to submit again.
    let message = format!("Filled {field_count} fields, submitted, waited for '{}'", wait_for.unwrap_or("none"));
    let mut out = json!({"ok": true, "message": message});
    // The only witness this command has. The change report runs after the submit, so a
    // field the page rewrote on the way in is no longer visible anywhere by then.
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
    let uid_map = crate::run_helpers::get_uid_map(store, browser_name, page_name);
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
    let uid_map = crate::run_helpers::get_uid_map(store, browser_name, page_name);
    crate::element::hover(client, &uid_map, uid).await?;
    Ok(json!({"ok": true, "message": format!("Hovered uid={uid}")}))
}

// ---------------------------------------------------------------------------
// New command dispatchers
// ---------------------------------------------------------------------------

pub async fn dispatch_dblclick(
    client: &CdpClient, store: &mut SessionStore, browser_name: &str, page_name: &str,
    target_id: &str, global_max_depth: Option<usize>, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let inspect = cmd.get("inspect").and_then(Value::as_bool).unwrap_or(false);
    let max_depth = cmd_max_depth(cmd).or(global_max_depth);
    // Hoist the `?` out of the `else if let` so the non-Send ControlFlow residual
    // isn't held across the awaits below (keeps the future Send).
    let xy = parse_xy(cmd)?;
    // Resolved before the action, like every other targeted command: afterwards the
    // element may be detached and the answer would describe a different page.
    let target = crate::run_helpers::target_details(
        client,
        cmd.get("selector").and_then(Value::as_str),
        cmd.get("uid").and_then(Value::as_str),
    )
    .await;
    let msg = if let Some(sel) = cmd.get("selector").and_then(Value::as_str) {
        crate::element::dblclick_selector(client, sel).await?;
        format!("Double-clicked selector '{sel}'")
    } else if let Some((x, y)) = xy {
        crate::element::dblclick_at_coords(client, x, y).await?;
        format!("Double-clicked at ({x}, {y})")
    } else if let Some(uid) = cmd.get("uid").and_then(Value::as_str) {
        let uid_map = get_uid_map(store, browser_name, page_name);
        crate::element::dblclick(client, &uid_map, uid).await?;
        format!("Double-clicked uid={uid}")
    } else {
        return Err("dblclick: provide \"uid\", \"selector\", or \"xy\"".into());
    };
    let mut obj = json!({"ok": true, "message": msg});
    merge_into(&mut obj, target.as_ref());
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
        (format!("Selected \"{}\" on selector '{sel}'", outcome.text), outcome)
    } else if let Some(uid) = cmd.get("uid").and_then(Value::as_str) {
        let uid_map = get_uid_map(store, browser_name, page_name);
        let outcome = crate::element::select_option(client, &uid_map, uid, value).await?;
        (format!("Selected \"{}\" on uid={uid}", outcome.text), outcome)
    } else {
        return Err("select: provide \"uid\" or \"selector\"".into());
    };
    let mut obj = json!({"ok": true, "message": msg, "observed_after_ms": outcome.observed_after_ms});
    merge_into(&mut obj, target.as_ref());
    if inspect {
        let snapshot = attach_snapshot(client, store, browser_name, page_name, target_id, max_depth).await?;
        obj["snapshot"] = json!(snapshot);
    }
    Ok(obj)
}

pub async fn dispatch_check(
    client: &CdpClient, store: &SessionStore, browser_name: &str, page_name: &str, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let desired = cmd.get("desired").and_then(Value::as_bool).unwrap_or(true);
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
        crate::element::set_checked(client, &uid_map, uid, desired).await?
    } else {
        return Err("check: provide \"uid\" or \"selector\"".into());
    };
    let mut obj = json!({"ok": true, "message": outcome.message});
    if let Some(uid) = target.as_ref().and_then(|t| t.get("uid")) {
        obj["uid"] = uid.clone();
    }
    // Absent when the element already held the state: nothing was dispatched, so there was
    // no post-action moment to report.
    if let Some(ms) = outcome.observed_after_ms {
        obj["observed_after_ms"] = json!(ms);
    }
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
