use std::collections::HashMap;

use serde_json::{json, Value};

use crate::cdp::client::CdpClient;
use crate::element_ref::ElementRef;
use crate::session::{self, SessionStore};
use crate::commands;

// ---------------------------------------------------------------------------
// Per-command dispatchers
// ---------------------------------------------------------------------------

pub async fn dispatch_goto(
    client: &CdpClient,
    store: &mut SessionStore,
    browser_name: &str,
    page_name: &str,
    target_id: &str,
    timeout: u64,
    global_max_depth: Option<usize>,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let url = cmd.get("url").and_then(Value::as_str).ok_or("goto: missing \"url\"")?;
    let inspect = cmd.get("inspect").and_then(Value::as_bool).unwrap_or(false);
    let max_depth = cmd_max_depth(cmd).or(global_max_depth);

    let result = commands::goto::run(client, url, timeout).await?;
    let _ = commands::history::append(&result.url, &result.title, page_name);

    let mut obj = json!({"ok": true, "url": result.url, "title": result.title});
    if inspect {
        let snapshot = attach_snapshot(client, store, browser_name, page_name, target_id, max_depth).await?;
        obj["snapshot"] = json!(snapshot);
    }
    Ok(obj)
}

pub async fn dispatch_click(
    client: &CdpClient,
    store: &mut SessionStore,
    browser_name: &str,
    page_name: &str,
    target_id: &str,
    global_max_depth: Option<usize>,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let inspect = cmd.get("inspect").and_then(Value::as_bool).unwrap_or(false);
    let max_depth = cmd_max_depth(cmd).or(global_max_depth);

    let msg = if let Some(sel) = cmd.get("selector").and_then(Value::as_str) {
        crate::element::click_selector(client, sel).await?;
        format!("Clicked selector '{sel}'")
    } else if let Some(uid) = cmd.get("uid").and_then(Value::as_str) {
        let uid_map = get_uid_map(store, browser_name, page_name);
        commands::click::run(client, &uid_map, uid).await?
    } else {
        return Err("click: provide \"uid\" or \"selector\"".into());
    };

    let mut obj = json!({"ok": true, "message": msg});
    if inspect {
        let snapshot = attach_snapshot(client, store, browser_name, page_name, target_id, max_depth).await?;
        obj["snapshot"] = json!(snapshot);
    }
    Ok(obj)
}

pub async fn dispatch_fill(
    client: &CdpClient,
    store: &mut SessionStore,
    browser_name: &str,
    page_name: &str,
    target_id: &str,
    global_max_depth: Option<usize>,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let value = cmd.get("value").and_then(Value::as_str).ok_or("fill: missing \"value\"")?;
    let inspect = cmd.get("inspect").and_then(Value::as_bool).unwrap_or(false);
    let max_depth = cmd_max_depth(cmd).or(global_max_depth);

    let msg = if let Some(sel) = cmd.get("selector").and_then(Value::as_str) {
        crate::element::fill_selector(client, sel, value).await?;
        format!("Filled selector '{sel}'")
    } else if let Some(uid) = cmd.get("uid").and_then(Value::as_str) {
        let uid_map = get_uid_map(store, browser_name, page_name);
        commands::fill::run(client, &uid_map, uid, value).await?
    } else {
        return Err("fill: provide \"uid\" or \"selector\"".into());
    };

    let mut obj = json!({"ok": true, "message": msg});
    if inspect {
        let snapshot = attach_snapshot(client, store, browser_name, page_name, target_id, max_depth).await?;
        obj["snapshot"] = json!(snapshot);
    }
    Ok(obj)
}

pub async fn dispatch_inspect(
    client: &CdpClient,
    store: &mut SessionStore,
    browser_name: &str,
    page_name: &str,
    target_id: &str,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let max_depth = cmd_max_depth(cmd);
    let filter_str = cmd.get("filter").and_then(Value::as_str);
    let role_filter: Option<Vec<&str>> = filter_str.map(|f| f.split(',').map(str::trim).collect());

    let snapshot = commands::inspect::run(client, false, max_depth, None, role_filter.as_deref()).await?;

    if let Some(browser_s) = store.browsers.get_mut(browser_name) {
        let page = session::ensure_page(browser_s, page_name, target_id);
        page.uid_map = snapshot.uid_map;
        page.last_snapshot = Some(snapshot.text.clone());
    }

    Ok(json!({"ok": true, "snapshot": snapshot.text}))
}

pub async fn dispatch_diff(
    client: &CdpClient,
    store: &mut SessionStore,
    browser_name: &str,
    page_name: &str,
    target_id: &str,
) -> Result<Value, crate::BoxError> {
    let old_text = store
        .browsers.get(browser_name)
        .and_then(|b| b.pages.get(page_name))
        .and_then(|p| p.last_snapshot.clone())
        .ok_or("No previous snapshot. Run inspect first.")?;

    let snapshot = commands::inspect::run(client, false, None, None, None).await?;
    let diff = commands::diff::diff_snapshots(&old_text, &snapshot.text);
    let stats = commands::diff::diff_stats(&diff);

    if let Some(browser_s) = store.browsers.get_mut(browser_name) {
        let page = session::ensure_page(browser_s, page_name, target_id);
        page.uid_map = snapshot.uid_map;
        page.last_snapshot = Some(snapshot.text);
    }

    Ok(json!({"ok": true, "added": stats.added, "removed": stats.removed, "changed": stats.changed, "diff": diff.trim_end()}))
}

pub async fn dispatch_eval(client: &CdpClient, cmd: &Value) -> Result<Value, crate::BoxError> {
    let expression = cmd.get("expression").and_then(Value::as_str).ok_or("eval: missing \"expression\"")?;
    let expr = if let Some(sel) = cmd.get("selector").and_then(Value::as_str) {
        let escaped = serde_json::to_string(sel).unwrap_or_default();
        format!("((el) => {{ return {expression} }})(document.querySelector({escaped}))")
    } else {
        expression.to_string()
    };
    let raw = commands::eval::run_raw(client, &expr).await?;
    Ok(json!({"ok": true, "result": raw}))
}

pub async fn dispatch_read(client: &CdpClient, cmd: &Value) -> Result<Value, crate::BoxError> {
    let truncate = cmd.get("truncate").and_then(Value::as_u64).map(|v| v as usize);
    let result = commands::read::run(client, false, truncate).await?;
    let mut obj = json!({"ok": true, "title": result.title, "text": result.text_content});
    if let Some(excerpt) = &result.excerpt { obj["excerpt"] = json!(excerpt); }
    if let Some(byline) = &result.byline { obj["byline"] = json!(byline); }
    Ok(obj)
}

pub async fn dispatch_text(
    client: &CdpClient, store: &SessionStore, browser_name: &str, page_name: &str, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let selector = cmd.get("selector").and_then(Value::as_str);
    let truncate = cmd.get("truncate").and_then(Value::as_u64).map(|v| v as usize);
    let uid_map = get_uid_map(store, browser_name, page_name);
    let text = commands::text::run(client, None, selector, &uid_map).await?;
    let full_length = text.chars().count();
    let (text, truncated) = if let Some(n) = truncate {
        if full_length > n { (crate::truncate::truncate_str(&text, n, "...").into_owned(), true) }
        else { (text, false) }
    } else { (text, false) };
    let mut obj = json!({"ok": true, "text": text});
    if truncated { obj["truncated"] = json!(true); obj["fullLength"] = json!(full_length); }
    Ok(obj)
}

pub async fn dispatch_screenshot(client: &CdpClient) -> Result<Value, crate::BoxError> {
    let path = commands::screenshot::run(client, None).await?;
    Ok(json!({"ok": true, "path": path}))
}

pub async fn dispatch_wait(client: &CdpClient, default_timeout: u64, cmd: &Value) -> Result<Value, crate::BoxError> {
    let (what, pattern) = if let Some(w) = cmd.get("what").and_then(Value::as_str) {
        let p = cmd.get("pattern").and_then(Value::as_str)
            .ok_or("wait: missing \"pattern\" (use {\"what\":\"text\",\"pattern\":\"...\"})")?;
        (w.to_string(), p.to_string())
    } else if let Some(p) = cmd.get("text").and_then(Value::as_str) { ("text".into(), p.into()) }
    else if let Some(p) = cmd.get("url").and_then(Value::as_str) { ("url".into(), p.into()) }
    else if let Some(p) = cmd.get("selector").and_then(Value::as_str) { ("selector".into(), p.into()) }
    else { return Err("wait: specify {\"what\":\"text\",\"pattern\":\"...\"} or {\"text\":\"...\"} or {\"url\":\"...\"} or {\"selector\":\"...\"}".into()); };
    let timeout = cmd.get("timeout").and_then(Value::as_u64).unwrap_or(default_timeout);
    let msg = commands::wait::run(client, &what, &pattern, timeout).await?;
    Ok(json!({"ok": true, "message": msg}))
}

pub async fn dispatch_back(client: &CdpClient) -> Result<Value, crate::BoxError> {
    let history: Value = client.call("Page.getNavigationHistory", json!({})).await?;
    let current_index = history.get("currentIndex").and_then(Value::as_i64).unwrap_or(0);
    if current_index <= 0 {
        return Ok(json!({"ok": true, "title": "", "message": "Already at first history entry"}));
    }
    let entries = history.get("entries").and_then(Value::as_array);
    let prev_entry_id = entries
        .and_then(|e| e.get(usize::try_from(current_index - 1).unwrap_or(0)))
        .and_then(|e| e.get("id"))
        .and_then(Value::as_i64)
        .ok_or("Could not find previous history entry")?;
    client.send("Page.navigateToHistoryEntry", json!({"entryId": prev_entry_id})).await?;
    let _ = client.wait_for_event("Page.loadEventFired", std::time::Duration::from_secs(5)).await;
    let title: crate::cdp::types::EvaluateResult = client
        .call("Runtime.evaluate", json!({"expression": "document.title", "returnByValue": true})).await?;
    let title_str = title.result.value.as_ref().and_then(|v| v.as_str()).unwrap_or("");
    Ok(json!({"ok": true, "title": title_str}))
}

pub async fn dispatch_forward(client: &CdpClient) -> Result<Value, crate::BoxError> {
    let history: Value = client.call("Page.getNavigationHistory", json!({})).await?;
    let current_index = history.get("currentIndex").and_then(Value::as_i64).unwrap_or(0);
    let entries = history.get("entries").and_then(Value::as_array);
    let entry_count = entries.map_or(0, Vec::len) as i64;
    if current_index >= entry_count - 1 {
        return Ok(json!({"ok": true, "title": "", "message": "Already at last history entry"}));
    }
    let next_entry_id = entries
        .and_then(|e| e.get(usize::try_from(current_index + 1).unwrap_or(0)))
        .and_then(|e| e.get("id"))
        .and_then(Value::as_i64)
        .ok_or("Could not find next history entry")?;
    client.send("Page.navigateToHistoryEntry", json!({"entryId": next_entry_id})).await?;
    let _ = client.wait_for_event("Page.loadEventFired", std::time::Duration::from_secs(5)).await;
    let title: crate::cdp::types::EvaluateResult = client
        .call("Runtime.evaluate", json!({"expression": "document.title", "returnByValue": true})).await?;
    let title_str = title.result.value.as_ref().and_then(|v| v.as_str()).unwrap_or("");
    Ok(json!({"ok": true, "title": title_str}))
}

pub async fn dispatch_scroll(
    client: &CdpClient, store: &SessionStore, browser_name: &str, page_name: &str, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let target = cmd.get("target").and_then(Value::as_str).ok_or("scroll: missing \"target\"")?;
    let msg = match target {
        "down" => { let _: Value = client.call("Runtime.evaluate", json!({"expression": "window.scrollBy(0, 500)", "returnByValue": true})).await?; "Scrolled down".to_string() }
        "up" => { let _: Value = client.call("Runtime.evaluate", json!({"expression": "window.scrollBy(0, -500)", "returnByValue": true})).await?; "Scrolled up".to_string() }
        uid => {
            let uid_map = get_uid_map(store, browser_name, page_name);
            let element_ref = uid_map.get(uid).ok_or_else(|| format!("Element uid={uid} not found. Run 'chrome-agent inspect' to get fresh uids."))?;
            let backend_node_id = element_ref.backend_node_id().ok_or_else(|| format!("Element uid={uid} has no resolvable backend node."))?;
            let resolve_result: crate::cdp::types::ResolveNodeResult = client.call("DOM.resolveNode", crate::cdp::types::ResolveNodeParams { node_id: None, backend_node_id: Some(backend_node_id), object_group: Some("chrome-agent".into()), execution_context_id: None }).await?;
            let object_id = resolve_result.object.object_id.ok_or_else(|| format!("Element uid={uid} could not be resolved to a JS object."))?;
            let _: Value = client.call("Runtime.callFunctionOn", json!({"objectId": object_id, "functionDeclaration": "function() { this.scrollIntoView({block: 'center'}); }", "returnByValue": true})).await?;
            format!("Scrolled uid={uid} into view")
        }
    };
    Ok(json!({"ok": true, "message": msg}))
}

pub async fn dispatch_type(client: &CdpClient, cmd: &Value) -> Result<Value, crate::BoxError> {
    let text = cmd.get("text").and_then(Value::as_str).ok_or("type: missing \"text\"")?;
    let selector = cmd.get("selector").and_then(Value::as_str);
    if let Some(sel) = selector { crate::element::focus_selector(client, sel).await?; }
    crate::element::type_text(client, text).await?;
    let msg = if let Some(sel) = selector { format!("Typed {} chars into selector '{sel}'", text.len()) }
    else { format!("Typed {} chars", text.len()) };
    Ok(json!({"ok": true, "message": msg}))
}

pub async fn dispatch_press(client: &CdpClient, cmd: &Value) -> Result<Value, crate::BoxError> {
    let key = cmd.get("key").and_then(Value::as_str).ok_or("press: missing \"key\"")?;
    crate::element::press_key(client, key).await?;
    Ok(json!({"ok": true, "message": format!("Pressed {key}")}))
}

pub async fn dispatch_tabs(browser_client: &CdpClient, store: &SessionStore) -> Result<Value, crate::BoxError> {
    let tabs = commands::tabs::run_structured(browser_client, store).await?;
    Ok(json!({"ok": true, "tabs": tabs}))
}

pub async fn dispatch_network(client: &CdpClient, cmd: &Value) -> Result<Value, crate::BoxError> {
    let filter = cmd.get("filter").and_then(Value::as_str);
    let limit = cmd.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
    let entries = commands::network::run_retroactive(client, filter, limit).await?;
    Ok(json!({"ok": true, "requests": entries}))
}

pub async fn dispatch_console(client: &CdpClient, cmd: &Value) -> Result<Value, crate::BoxError> {
    let level = cmd.get("level").and_then(Value::as_str);
    let clear = cmd.get("clear").and_then(Value::as_bool).unwrap_or(false);
    let limit = cmd.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
    let entries = commands::console::run(client, level, clear, limit).await?;
    let messages: Vec<Value> = entries.iter()
        .map(|e| json!({"level": e.level, "message": e.message, "timestamp": e.timestamp})).collect();
    Ok(json!({"ok": true, "messages": messages}))
}

pub async fn dispatch_extract(client: &CdpClient, cmd: &Value) -> Result<Value, crate::BoxError> {
    let selector = cmd.get("selector").and_then(Value::as_str);
    let limit = cmd.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize;
    let scroll = cmd.get("scroll").and_then(Value::as_bool).unwrap_or(false);
    if scroll { commands::extract::scroll_to_load(client).await?; }
    let a11y = cmd.get("a11y").and_then(Value::as_bool).unwrap_or(false);
    let result = if a11y { commands::extract::run_a11y(client, limit, scroll).await? }
    else { commands::extract::run(client, selector, limit).await? };
    Ok(commands::extract::to_json(&result))
}

// ---------------------------------------------------------------------------
// Composite dispatchers
// ---------------------------------------------------------------------------

pub async fn dispatch_navigate_and_read(
    client: &CdpClient, _store: &mut SessionStore, _browser_name: &str, page_name: &str,
    _target_id: &str, timeout: u64, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let url = cmd.get("url").and_then(Value::as_str).ok_or("navigate_and_read: missing \"url\"")?;
    let truncate = cmd.get("truncate").and_then(Value::as_u64).map(|v| v as usize);
    let goto_result = commands::goto::run(client, url, timeout).await?;
    let _ = commands::history::append(&goto_result.url, &goto_result.title, page_name);
    let read_result = commands::read::run(client, false, truncate).await?;
    Ok(json!({"ok": true, "url": goto_result.url, "title": goto_result.title, "content": read_result.text_content}))
}

pub async fn dispatch_fill_and_submit(client: &CdpClient, timeout: u64, cmd: &Value) -> Result<Value, crate::BoxError> {
    let fields = cmd.get("fields").and_then(Value::as_array).ok_or("fill_and_submit: missing \"fields\" array")?;
    let submit_selector = cmd.get("submit").and_then(Value::as_str).ok_or("fill_and_submit: missing \"submit\" selector")?;
    let wait_for = cmd.get("wait_for").and_then(Value::as_str);
    let field_count = fields.len();
    for field in fields {
        let selector = field.get("selector").and_then(Value::as_str).ok_or("fill_and_submit: each field needs \"selector\"")?;
        let value = field.get("value").and_then(Value::as_str).ok_or("fill_and_submit: each field needs \"value\"")?;
        crate::element::fill_selector(client, selector, value).await?;
    }
    crate::element::click_selector(client, submit_selector).await?;
    if let Some(pattern) = wait_for {
        let is_selector = pattern.contains('.') || pattern.contains('#') || pattern.contains('[') || pattern.contains('>');
        let wait_type = if is_selector { "selector" } else { "text" };
        commands::wait::run(client, wait_type, pattern, timeout).await?;
    }
    let read_result = commands::read::run(client, false, None).await?;
    let message = format!("Filled {field_count} fields, submitted, waited for '{}'", wait_for.unwrap_or("none"));
    Ok(json!({"ok": true, "message": message, "content": read_result.text_content}))
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
    for pair in pairs {
        let uid = pair.get("uid").and_then(Value::as_str).ok_or("Each pair needs \"uid\"")?;
        let value = pair.get("value").and_then(Value::as_str).ok_or("Each pair needs \"value\"")?;
        crate::element::fill(client, &uid_map, uid, value).await?;
    }
    let inspect = cmd.get("inspect").and_then(Value::as_bool).unwrap_or(false);
    let mut obj = json!({"ok": true, "message": format!("Filled {} fields", pairs.len())});
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
    let msg = if let Some(uid) = cmd.get("uid").and_then(Value::as_str) {
        let uid_map = get_uid_map(store, browser_name, page_name);
        crate::element::dblclick(client, &uid_map, uid).await?;
        format!("Double-clicked uid={uid}")
    } else if let Some(sel) = cmd.get("selector").and_then(Value::as_str) {
        crate::element::click_selector(client, sel).await?;
        format!("Double-clicked selector '{sel}'")
    } else {
        return Err("dblclick: provide \"uid\" or \"selector\"".into());
    };
    let mut obj = json!({"ok": true, "message": msg});
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
    let msg = if let Some(sel) = cmd.get("selector").and_then(Value::as_str) {
        let text = crate::element::select_option_selector(client, sel, value).await?;
        format!("Selected \"{text}\" on selector '{sel}'")
    } else if let Some(uid) = cmd.get("uid").and_then(Value::as_str) {
        let uid_map = get_uid_map(store, browser_name, page_name);
        let text = crate::element::select_option(client, &uid_map, uid, value).await?;
        format!("Selected \"{text}\" on uid={uid}")
    } else {
        return Err("select: provide \"uid\" or \"selector\"".into());
    };
    let mut obj = json!({"ok": true, "message": msg});
    if inspect {
        let snapshot = attach_snapshot(client, store, browser_name, page_name, target_id, max_depth).await?;
        obj["snapshot"] = json!(snapshot);
    }
    Ok(obj)
}

pub async fn dispatch_check(
    client: &CdpClient, store: &SessionStore, browser_name: &str, page_name: &str, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let uid = cmd.get("uid").and_then(Value::as_str).ok_or("check: missing \"uid\"")?;
    let desired = cmd.get("desired").and_then(Value::as_bool).unwrap_or(true);
    let uid_map = get_uid_map(store, browser_name, page_name);
    let msg = crate::element::set_checked(client, &uid_map, uid, desired).await?;
    Ok(json!({"ok": true, "message": msg}))
}

pub async fn dispatch_upload(
    client: &CdpClient, store: &SessionStore, browser_name: &str, page_name: &str, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let files: Vec<String> = cmd.get("files").and_then(Value::as_array)
        .ok_or("upload: missing \"files\" array")?
        .iter().filter_map(|v| v.as_str().map(String::from)).collect();
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
    Ok(json!({"ok": true, "message": msg}))
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
// Helpers
// ---------------------------------------------------------------------------

async fn attach_snapshot(
    client: &CdpClient, store: &mut SessionStore, browser_name: &str, page_name: &str,
    target_id: &str, max_depth: Option<usize>,
) -> Result<String, crate::BoxError> {
    let snapshot = commands::inspect::run(client, false, max_depth, None, None).await?;
    if let Some(browser_s) = store.browsers.get_mut(browser_name) {
        let page = session::ensure_page(browser_s, page_name, target_id);
        page.uid_map = snapshot.uid_map;
        page.last_snapshot = Some(snapshot.text.clone());
    }
    Ok(snapshot.text)
}

fn get_uid_map(store: &SessionStore, browser_name: &str, page_name: &str) -> HashMap<String, ElementRef> {
    store.browsers.get(browser_name)
        .and_then(|b| b.pages.get(page_name))
        .map(|p| p.uid_map.clone())
        .unwrap_or_default()
}

fn cmd_max_depth(cmd: &Value) -> Option<usize> {
    cmd.get("max_depth").and_then(Value::as_u64).map(|v| v as usize)
}

// ---------------------------------------------------------------------------
// Frame
// ---------------------------------------------------------------------------

pub async fn dispatch_frame(
    client: &CdpClient,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let target = cmd.get("target").and_then(Value::as_str).ok_or("frame: missing \"target\"")?;
    let msg = commands::frame::run(client, target).await?;
    Ok(json!({"ok": true, "message": msg}))
}

// ---------------------------------------------------------------------------
// Batch
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub async fn dispatch_batch(
    client: &CdpClient,
    browser_client: &CdpClient,
    store: &mut SessionStore,
    browser_name: &str,
    page_name: &str,
    target_id: &str,
    timeout: u64,
    global_max_depth: Option<usize>,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let cmds = cmd.get("commands").and_then(Value::as_array)
        .ok_or("batch: missing \"commands\" array")?;
    let mut results = Vec::new();
    for c in cmds {
        let r = dispatch_single(client, browser_client, store, browser_name, page_name, target_id, timeout, global_max_depth, c).await;
        results.push(r);
    }
    Ok(json!({"ok": true, "results": results}))
}

/// Public entry point for dispatching a single pipe command.
/// Used by batch mode (both CLI and pipe).
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_single(
    client: &CdpClient,
    browser_client: &CdpClient,
    store: &mut SessionStore,
    browser_name: &str,
    page_name: &str,
    target_id: &str,
    timeout: u64,
    global_max_depth: Option<usize>,
    cmd: &Value,
) -> Value {
    let cmd_name = cmd.get("cmd").and_then(Value::as_str).unwrap_or("");
    let result: Result<Value, crate::BoxError> = match cmd_name {
        "goto" => dispatch_goto(client, store, browser_name, page_name, target_id, timeout, global_max_depth, cmd).await,
        "click" => dispatch_click(client, store, browser_name, page_name, target_id, global_max_depth, cmd).await,
        "fill" => dispatch_fill(client, store, browser_name, page_name, target_id, global_max_depth, cmd).await,
        "inspect" => dispatch_inspect(client, store, browser_name, page_name, target_id, cmd).await,
        "eval" => dispatch_eval(client, cmd).await,
        "read" => dispatch_read(client, cmd).await,
        "text" => dispatch_text(client, store, browser_name, page_name, cmd).await,
        "screenshot" => dispatch_screenshot(client).await,
        "wait" => dispatch_wait(client, timeout, cmd).await,
        "back" => dispatch_back(client).await,
        "forward" => dispatch_forward(client).await,
        "scroll" => dispatch_scroll(client, store, browser_name, page_name, cmd).await,
        "type" => dispatch_type(client, cmd).await,
        "press" => dispatch_press(client, cmd).await,
        "dblclick" => dispatch_dblclick(client, store, browser_name, page_name, target_id, global_max_depth, cmd).await,
        "select" => dispatch_select(client, store, browser_name, page_name, target_id, global_max_depth, cmd).await,
        "check" => dispatch_check(client, store, browser_name, page_name, cmd).await,
        "uncheck" => {
            let mut c = cmd.clone();
            if let Some(m) = c.as_object_mut() { m.insert("desired".into(), Value::Bool(false)); }
            dispatch_check(client, store, browser_name, page_name, &c).await
        }
        "upload" => dispatch_upload(client, store, browser_name, page_name, cmd).await,
        "drag" => dispatch_drag(client, store, browser_name, page_name, cmd).await,
        "hover" => dispatch_hover(client, store, browser_name, page_name, cmd).await,
        "fill-form" | "fill_form" | "fillform" => dispatch_fill_form(client, store, browser_name, page_name, target_id, global_max_depth, cmd).await,
        "tabs" => dispatch_tabs(browser_client, store).await,
        "network" => dispatch_network(client, cmd).await,
        "console" => dispatch_console(client, cmd).await,
        "diff" => dispatch_diff(client, store, browser_name, page_name, target_id).await,
        "extract" => dispatch_extract(client, cmd).await,
        "navigate_and_read" | "navigate-and-read" => dispatch_navigate_and_read(client, store, browser_name, page_name, target_id, timeout, cmd).await,
        "fill_and_submit" | "fill-and-submit" => dispatch_fill_and_submit(client, timeout, cmd).await,
        "history" => dispatch_history(cmd),
        "frame" => dispatch_frame(client, cmd).await,
        "" => Err("Missing \"cmd\" field".into()),
        other => Err(format!("Unknown command: {other}").into()),
    };
    match result {
        Ok(v) => v,
        Err(e) => {
            let msg = e.to_string();
            let mut obj = json!({"ok": false, "error": msg});
            if let Some(h) = crate::run_helpers::error_hint(&msg) { obj["hint"] = json!(h); }
            obj
        }
    }
}
