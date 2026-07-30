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
    let mut out = json!({"ok": true, "url": goto_result.url, "title": goto_result.title, "content": read_result.text_content});
    // The bounce this reports matters more here than on a bare `goto`: without it the caller
    // gets a login page's prose back as if it were the article they asked for.
    goto_result.landed.attach(&mut out);
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
    // Best effort: Readability rejects plenty of legitimate pages, and the fill and the
    // submit have already landed. Failing the whole command there tells an agent its
    // mutation did not happen, and the natural response to that is to submit again.
    let message = format!("Filled {field_count} fields, submitted, waited for '{}'", wait_for.unwrap_or("none"));
    let mut out = json!({"ok": true, "message": message});
    // The submit's own delivery, at the top level where the verdict wiring reads it: a submit
    // button under a consent banner is the shape this command exists for, and it used to be
    // reported as a successful submit.
    merge_into(&mut out, Some(&submitted.report()));
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
    target_id: &str, global_max_depth: Option<usize>,
    report: crate::run_helpers::ReportPolicy, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let inspect = cmd.get("inspect").and_then(Value::as_bool).unwrap_or(false);
    let max_depth = cmd_max_depth(cmd).or(global_max_depth);
    // Hoist the `?` out of the `else if let` so the non-Send ControlFlow residual
    // isn't held across the awaits below (keeps the future Send).
    let xy = parse_xy(cmd)?;
    let on_intercept = crate::hit_test::OnIntercept::from_cmd(cmd, report.on_intercept);
    // The node is resolved before the action, from the handle the probe and the dispatch use:
    // afterwards the element may be detached and the answer would describe a different page.
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

pub async fn dispatch_check(
    client: &CdpClient, store: &SessionStore, browser_name: &str, page_name: &str,
    report: crate::run_helpers::ReportPolicy, cmd: &Value,
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
    // dispatched, so there was no post-action moment to report.
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

// ---------------------------------------------------------------------------
// Assert
// ---------------------------------------------------------------------------

/// `assert` for pipe and batch.
///
/// There is no exit code here, so `held` rides on `ok`: a claim that did not hold answers
/// `{"ok":false,"assertion":{…},"hint":…}` and one that could not be checked answers the
/// usual `{"ok":false,"error":…}`. The two are told apart by the presence of `assertion`,
/// and `batch`'s `all_ok` and `stop_on_error` treat both as the failures they are without
/// needing a second convention. `assert` is not in `mutates_page`: it is a read, so no
/// change report and no verdict ride on the response.
pub async fn dispatch_assert(
    client: &CdpClient, store: &SessionStore, browser_name: &str, page_name: &str, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let assertion = commands::assert::from_json(cmd)?;
    let uid_map = get_uid_map(store, browser_name, page_name);
    let outcome = commands::assert::run(client, &uid_map, &assertion).await?;
    Ok(outcome.to_json())
}

// ---------------------------------------------------------------------------
// Batch
// ---------------------------------------------------------------------------

/// Run a list of commands through `dispatch_single`, optionally stopping at the first
/// failure.
///
/// The one loop behind both batch front ends (`chrome-agent batch` reading a JSON array from
/// stdin, and `{"cmd":"batch","commands":[…]}` in pipe mode) — they used to keep a copy each,
/// so `stop_on_error` would have had to be implemented, and kept in step, twice.
///
/// `stop_on_error` is opt-in and off by default: a batch is also used to collect independent
/// observations, where one failure is not a reason to abandon the rest. When it is on, the
/// response says where it stopped rather than leaving the caller to infer it from a short
/// array.
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
) -> Value {
    let mut results = Vec::with_capacity(commands_list.len());
    let mut stopped_at = None;
    for (index, c) in commands_list.iter().enumerate() {
        let r = crate::pipe_dispatch::dispatch_single(
            client, browser_client, store, browser_name, page_name, target_id, timeout,
            global_max_depth, report, c,
        )
        .await;
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
