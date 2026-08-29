use std::collections::HashMap;

use serde_json::{json, Value};

use crate::cdp::client::CdpClient;
use crate::element_ref::ElementRef;
use crate::session::{self, SessionStore};
use crate::commands;
pub use crate::pipe_emulation::{EmulationRecovery, dispatch_emulate};
pub use crate::pipe_report::{attach_change_report, mutates_page};

// Split out to stay under the 1000-line file cap; callers keep using `pipe_dispatch::*`.
pub use crate::pipe_dispatch_actions::{
    dispatch_assert, dispatch_check, dispatch_dblclick, dispatch_drag, dispatch_fill_and_submit,
    dispatch_fill_form, dispatch_history, dispatch_hover, dispatch_navigate_and_read,
    dispatch_select, dispatch_upload, run_batch,
};

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
    let parsed_headers = cmd
        .get("headers")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(commands::goto::parse_header)
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();

    let result = commands::goto::run(client, url, timeout, &parsed_headers).await?;
    // Mirror the CLI: after navigation, optionally wait for a CSS selector.
    if let Some(selector) = cmd.get("wait_for").and_then(Value::as_str) {
        commands::wait::run(client, "selector", selector, timeout, 500).await?;
    }
    // Navigation destroys any bound frame's isolated world — clear it so
    // subsequent eval/inspect target the freshly loaded top document (issue #8).
    client.set_frame_context(None);
    let _ = commands::history::append(&result.url, &result.title, page_name);

    let mut obj = json!({"ok": true, "url": result.url, "title": result.title});
    // `goto` stays out of `mutates_page`, so nothing else will speak for it: `landed` rides
    // on its own response, in the one dispatcher pipe and batch share.
    result.landed.attach(&mut obj);
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
    report: crate::run_helpers::ReportPolicy,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let inspect = cmd.get("inspect").and_then(Value::as_bool).unwrap_or(false);
    let max_depth = cmd_max_depth(cmd).or(global_max_depth);
    // Hoist the `?` out of the `else if let` so the non-Send ControlFlow residual
    // isn't held across the awaits below (keeps the future Send).
    let xy = parse_xy(cmd)?;
    let on_intercept = crate::hit_test::OnIntercept::from_cmd(cmd, report.on_intercept);

    let (msg, details) = if let Some(sel) = cmd.get("selector").and_then(Value::as_str) {
        let outcome = crate::element::click_selector(client, sel, on_intercept).await?;
        let target = format!("selector '{sel}'");
        (
            outcome.refusal_message("click", &target).unwrap_or_else(|| format!("Clicked {target}")),
            Some(outcome.report()),
        )
    } else if let Some((x, y)) = xy {
        crate::element::click_at_coords(client, x, y).await?;
        (format!("Clicked at ({x}, {y})"), None)
    } else if let Some(uid) = cmd.get("uid").and_then(Value::as_str) {
        let uid_map = get_uid_map(store, browser_name, page_name);
        let (msg, outcome) = commands::click::run(client, &uid_map, uid, on_intercept).await?;
        (msg, Some(outcome.report()))
    } else {
        return Err("click: provide \"uid\", \"selector\", or \"xy\"".into());
    };

    let mut obj = json!({"ok": true, "message": msg});
    merge_into(&mut obj, details.as_ref());
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

    let target = crate::run_helpers::target_details(
        client,
        cmd.get("selector").and_then(Value::as_str),
        cmd.get("uid").and_then(Value::as_str),
    )
    .await;
    let (msg, outcome) = if let Some(sel) = cmd.get("selector").and_then(Value::as_str) {
        let outcome = crate::element::fill_selector(client, sel, value).await?;
        (format!("Filled selector '{sel}'"), outcome)
    } else if let Some(uid) = cmd.get("uid").and_then(Value::as_str) {
        let uid_map = get_uid_map(store, browser_name, page_name);
        commands::fill::run(client, &uid_map, uid, value).await?
    } else {
        return Err("fill: provide \"uid\" or \"selector\"".into());
    };

    let mut obj = json!({"ok": true, "message": msg});
    merge_into(&mut obj, target.as_ref());
    obj["value"] = crate::run_helpers::fill_value_report(&outcome);
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
    let verbose = cmd.get("verbose").and_then(Value::as_bool).unwrap_or(false);
    let max_depth = cmd_max_depth(cmd);
    let uid = cmd.get("uid").and_then(Value::as_str);
    let scroll = cmd.get("scroll").and_then(Value::as_bool).unwrap_or(false);
    let limit = cmd.get("limit").and_then(Value::as_u64).map(|v| v as usize);
    let urls = cmd.get("urls").and_then(Value::as_bool).unwrap_or(false);
    let filter_str = cmd.get("filter").and_then(Value::as_str);
    let role_filter: Option<Vec<&str>> = filter_str.map(|f| f.split(',').map(str::trim).collect());

    if scroll {
        commands::extract::scroll_to_load(client).await?;
    }
    let (mut text, uid_map, doc_identity) = if let Some(max) = limit {
        let result = commands::inspect::scroll_collect(client, verbose, uid, role_filter.as_deref(), max).await?;
        (result.text, result.uid_map, result.identity)
    } else {
        let s = commands::inspect::run(client, verbose, max_depth, uid, role_filter.as_deref()).await?;
        (s.text, s.uid_map, s.identity)
    };
    if urls {
        text = commands::inspect::resolve_urls(client, &text, &uid_map).await;
    }

    // Persist the FULL snapshot so diff and uid lookups stay complete;
    // paging only affects what we return.
    if let Some(browser_s) = store.browsers.get_mut(browser_name) {
        let page = session::ensure_page(browser_s, page_name, target_id);
        page.uid_map = uid_map;
        page.last_snapshot = Some(text.clone());
        let (f, l) = doc_identity.map_or((None, None), |(f, l)| (Some(f), Some(l)));
        page.last_snapshot_frame = f;
        page.last_snapshot_loader = l;
    }

    let offset = cmd.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let max_chars = cmd.get("max_chars").and_then(Value::as_u64).map(|n| n as usize);
    let paged = commands::inspect::paginate(&text, offset, max_chars);
    Ok(json!({
        "ok": true,
        "snapshot": paged.text,
        "total_chars": paged.total_chars,
        "truncated": paged.truncated,
        "next_offset": paged.next_offset,
    }))
}

pub async fn dispatch_diff(
    client: &CdpClient,
    store: &mut SessionStore,
    browser_name: &str,
    page_name: &str,
    target_id: &str,
) -> Result<Value, crate::BoxError> {
    let page_state = store
        .browsers.get(browser_name)
        .and_then(|b| b.pages.get(page_name));
    let old_text = page_state
        .and_then(|p| p.last_snapshot.clone())
        .ok_or("No previous snapshot. Run inspect first.")?;
    let stored = page_state
        .and_then(|p| p.last_snapshot_frame.clone().zip(p.last_snapshot_loader.clone()));

    let snapshot = commands::inspect::run(client, false, None, None, None).await?;
    let identity = commands::diff::Identity::from_loader(
        stored.as_ref().map(|(f, l)| (f.as_str(), l.as_str())),
        snapshot.identity.as_ref().map(|(f, l)| (f.as_str(), l.as_str())),
    );
    let result = commands::diff::compare(identity, &old_text, &snapshot.text);

    if let Some(browser_s) = store.browsers.get_mut(browser_name) {
        let page = session::ensure_page(browser_s, page_name, target_id);
        page.uid_map = snapshot.uid_map;
        page.last_snapshot = Some(snapshot.text);
            let (f, l) = snapshot.identity.map_or((None, None), |(f, l)| (Some(f), Some(l)));
            page.last_snapshot_frame = f;
            page.last_snapshot_loader = l;
    }

    let mut out = json!({
        "ok": true,
        "document_changed": result.document_changed,
        "identity_known": result.identity_known,
        "added": result.added,
        "removed": result.removed,
        "changed": result.changed,
        "unchanged": result.unchanged,
                    "moved": result.moved,
                    "anonymous": result.anonymous,
        "diff": result.text.trim_end(),
    });
    if result.focus_from.is_some() || result.focus_to.is_some() {
        out["focus"] = json!({"from": result.focus_from, "to": result.focus_to});
    }
    if let Some(hint) = result.hint {
        out["hint"] = json!(hint);
    }
    Ok(out)
}

pub async fn dispatch_eval(client: &CdpClient, cmd: &Value) -> Result<Value, crate::BoxError> {
    let expression = cmd.get("expression").and_then(Value::as_str).ok_or("eval: missing \"expression\"")?;
    let expr = if let Some(sel) = cmd.get("selector").and_then(Value::as_str) {
        let escaped = serde_json::to_string(sel).unwrap_or_default();
        format!("((el) => {{ if (!el) throw new Error('No element matches selector ' + {escaped}); return {expression} }})(document.querySelector({escaped}))")
    } else {
        expression.to_string()
    };
    let raw = commands::eval::run_raw(client, &expr).await?;
    Ok(json!({"ok": true, "result": raw}))
}

pub async fn dispatch_read(client: &CdpClient, cmd: &Value) -> Result<Value, crate::BoxError> {
    let html = cmd.get("html").and_then(Value::as_bool).unwrap_or(false);
    let truncate = cmd.get("truncate").and_then(Value::as_u64).map(|v| v as usize);
    let result = commands::read::run(client, html, truncate).await?;
    let mut obj = json!({"ok": true, "title": result.title, "text": result.text_content});
    if let Some(excerpt) = &result.excerpt { obj["excerpt"] = json!(excerpt); }
    if let Some(byline) = &result.byline { obj["byline"] = json!(byline); }
    // When --html is requested, `read::run` keeps the cleaned HTML; surface it
    // (pipe/batch is JSON-only, so this is the only place --html can be observed).
    if let Some(content) = &result.content { obj["content"] = json!(content); }
    Ok(obj)
}

pub async fn dispatch_text(
    client: &CdpClient, store: &SessionStore, browser_name: &str, page_name: &str, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let TextArgs { uid, selector, truncate } = parse_text(cmd);
    let uid_map = get_uid_map(store, browser_name, page_name);
    let text = commands::text::run(client, uid, selector, &uid_map).await?;
    let full_length = text.chars().count();
    let (text, truncated) = if let Some(n) = truncate {
        if full_length > n { (crate::truncate::truncate_str(&text, n, "...").into_owned(), true) }
        else { (text, false) }
    } else { (text, false) };
    let mut obj = json!({"ok": true, "text": text});
    if truncated { obj["truncated"] = json!(true); obj["fullLength"] = json!(full_length); }
    Ok(obj)
}

pub async fn dispatch_screenshot(
    client: &CdpClient,
    store: &SessionStore,
    browser_name: &str,
    page_name: &str,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let format = commands::screenshot::ImgFormat::parse(
        cmd.get("format").and_then(Value::as_str).unwrap_or("png"),
    )?;
    let quality = cmd.get("quality").and_then(Value::as_u64).map(|q| q as u32);
    let max_width = cmd.get("max_width").and_then(Value::as_u64).map(|w| w as u32);
    let uid = cmd.get("uid").and_then(Value::as_str);
    let selector = cmd.get("selector").and_then(Value::as_str);

    let clip = if let Some(u) = uid {
        let uid_map = get_uid_map(store, browser_name, page_name);
        Some(crate::geometry::clip_for_uid(client, &uid_map, u).await?)
    } else if let Some(sel) = selector {
        Some(crate::geometry::clip_for_selector(client, sel).await?)
    } else {
        None
    };

    let opts = commands::screenshot::ScreenshotOpts {
        filename: cmd.get("filename").and_then(Value::as_str),
        format,
        quality,
        max_width,
        clip,
    };
    let path = commands::screenshot::run(client, &opts).await?;
    Ok(json!({"ok": true, "path": path}))
}

pub async fn dispatch_download(client: &CdpClient, default_timeout: u64, cmd: &Value) -> Result<Value, crate::BoxError> {
    let url = cmd.get("url").and_then(Value::as_str).ok_or("download: missing \"url\"")?;
    let out = cmd.get("out").and_then(Value::as_str);
    let timeout = cmd.get("timeout").and_then(Value::as_u64).unwrap_or(default_timeout);
    let max_bytes = parse_download_max_bytes(cmd)?;
    let result = commands::download::run(client, url, out, timeout, max_bytes).await?;
    Ok(json!({"ok": true, "path": result.path, "bytes": result.bytes, "mime": result.mime}))
}

fn parse_download_max_bytes(cmd: &Value) -> Result<usize, crate::BoxError> {
    let value = match cmd.get("max_bytes") {
        Some(value) => value
            .as_u64()
            .ok_or("download: max_bytes must be a positive integer")?,
        None => commands::download::DEFAULT_MAX_BYTES as u64,
    };
    let value = usize::try_from(value).map_err(|_| "download: max_bytes exceeds platform limits")?;
    if value == 0 {
        return Err("download: max_bytes must be greater than zero".into());
    }
    Ok(value)
}

#[cfg(test)]
mod download_limit_tests {
    use super::*;

    #[test]
    fn pipe_download_max_bytes_defaults_and_rejects_zero() {
        assert_eq!(
            parse_download_max_bytes(&serde_json::json!({"cmd": "download"})).unwrap(),
            67_108_864
        );
        assert_eq!(
            parse_download_max_bytes(
                &serde_json::json!({"cmd": "download", "max_bytes": 10})
            )
            .unwrap(),
            10
        );
        assert!(
            parse_download_max_bytes(
                &serde_json::json!({"cmd": "download", "max_bytes": 0})
            )
            .is_err()
        );
        assert!(
            parse_download_max_bytes(
                &serde_json::json!({"cmd": "download", "max_bytes": "10"})
            )
            .is_err()
        );
    }
}

pub async fn dispatch_pdf(client: &CdpClient, cmd: &Value) -> Result<Value, crate::BoxError> {
    let opts = commands::pdf::PdfOpts {
        filename: cmd.get("filename").and_then(Value::as_str),
        landscape: cmd.get("landscape").and_then(Value::as_bool).unwrap_or(false),
        background: cmd.get("background").and_then(Value::as_bool).unwrap_or(false),
    };
    let path = commands::pdf::run(client, &opts).await?;
    Ok(json!({"ok": true, "path": path}))
}

pub async fn dispatch_wait(client: &CdpClient, _default_timeout: u64, cmd: &Value) -> Result<Value, crate::BoxError> {
    let (what, pattern) = parse_wait(cmd)?;
    // Match the CLI `wait --timeout` default (10s), not the global page-load
    // timeout (30s): waits are per-condition and should not inherit --timeout.
    let timeout = cmd.get("timeout").and_then(Value::as_u64).unwrap_or(WAIT_DEFAULT_TIMEOUT);
    let idle_ms = cmd.get("idle_ms").and_then(Value::as_u64).unwrap_or(500);
    let msg = commands::wait::run(client, &what, &pattern, timeout, idle_ms).await?;
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
    // Subscribe BEFORE navigating: a fast (cached) history entry can fire
    // Page.loadEventFired before a late subscription exists, which would stall
    // until the timeout (same race the CLI history path has).
    let mut rx = client.events();
    client.send("Page.navigateToHistoryEntry", json!({"entryId": prev_entry_id})).await?;
    client.set_frame_context(None); // history navigation invalidates any bound frame
    let _ = CdpClient::wait_for_event_on(&mut rx, "Page.loadEventFired", std::time::Duration::from_secs(5)).await;
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
    // Subscribe BEFORE navigating to avoid missing a fast loadEventFired (see dispatch_back).
    let mut rx = client.events();
    client.send("Page.navigateToHistoryEntry", json!({"entryId": next_entry_id})).await?;
    client.set_frame_context(None); // history navigation invalidates any bound frame
    let _ = CdpClient::wait_for_event_on(&mut rx, "Page.loadEventFired", std::time::Duration::from_secs(5)).await;
    let title: crate::cdp::types::EvaluateResult = client
        .call("Runtime.evaluate", json!({"expression": "document.title", "returnByValue": true})).await?;
    let title_str = title.result.value.as_ref().and_then(|v| v.as_str()).unwrap_or("");
    Ok(json!({"ok": true, "title": title_str}))
}

pub async fn dispatch_scroll(
    client: &CdpClient, store: &SessionStore, browser_name: &str, page_name: &str, cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let ScrollArgs { target, px } = parse_scroll(cmd)?;
    let msg = match target {
        "down" => { let _: Value = client.call("Runtime.evaluate", json!({"expression": format!("window.scrollBy(0, {px})"), "returnByValue": true})).await?; format!("Scrolled down {px}px") }
        "up" => { let _: Value = client.call("Runtime.evaluate", json!({"expression": format!("window.scrollBy(0, -{px})"), "returnByValue": true})).await?; format!("Scrolled up {px}px") }
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
    crate::element::require_editable_focus(client).await?;
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
    let body = cmd.get("body").and_then(Value::as_bool).unwrap_or(false);
    let live = cmd.get("live").and_then(Value::as_u64);
    let abort = cmd.get("abort").and_then(Value::as_str);

    if let Some(pattern) = abort {
        // Mirror the CLI: --live doubles as the abort window (default 30s).
        let timeout_secs = live.unwrap_or(30);
        let blocked = commands::network::run_route_abort(client, pattern, timeout_secs).await?;
        Ok(json!({"ok": true, "blocked": blocked.len(), "urls": blocked}))
    } else if let Some(secs) = live {
        let entries = commands::network::run_live(client, filter, body, limit, secs).await?;
        Ok(json!({"ok": true, "requests": entries}))
    } else {
        let entries = commands::network::run_retroactive(client, filter, limit).await?;
        Ok(json!({"ok": true, "requests": entries}))
    }
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
    let a11y = cmd.get("a11y").and_then(Value::as_bool).unwrap_or(false);
    // Match the CLI: `run_a11y` scrolls internally, so only the DOM path needs
    // an explicit scroll_to_load — otherwise --a11y --scroll would scroll twice.
    let result = if a11y {
        commands::extract::run_a11y(client, limit, scroll).await?
    } else {
        if scroll { commands::extract::scroll_to_load(client).await?; }
        commands::extract::run(client, selector, limit).await?
    };
    Ok(commands::extract::to_json(&result))
}


// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub async fn attach_snapshot(
    client: &CdpClient, store: &mut SessionStore, browser_name: &str, page_name: &str,
    target_id: &str, max_depth: Option<usize>,
) -> Result<String, crate::BoxError> {
    let snapshot = commands::inspect::run(client, false, max_depth, None, None).await?;
    if let Some(browser_s) = store.browsers.get_mut(browser_name) {
        let page = session::ensure_page(browser_s, page_name, target_id);
        page.uid_map = snapshot.uid_map;
        page.last_snapshot = Some(snapshot.text.clone());
        let (f, l) = snapshot.identity.clone().map_or((None, None), |(f, l)| (Some(f), Some(l)));
        page.last_snapshot_frame = f;
        page.last_snapshot_loader = l;
    }
    Ok(snapshot.text)
}

pub fn get_uid_map(store: &SessionStore, browser_name: &str, page_name: &str) -> HashMap<String, ElementRef> {
    store.browsers.get(browser_name)
        .and_then(|b| b.pages.get(page_name))
        .map(|p| p.uid_map.clone())
        .unwrap_or_default()
}

pub fn cmd_max_depth(cmd: &Value) -> Option<usize> {
    cmd.get("max_depth").and_then(Value::as_u64).map(|v| v as usize)
}

/// Default `wait` timeout in seconds — mirrors the CLI `wait --timeout` default.
const WAIT_DEFAULT_TIMEOUT: u64 = 10;

/// Parsed `scroll` arguments. `px` defaults to 500 (matches the CLI).
#[cfg_attr(test, derive(Debug))]
struct ScrollArgs<'a> {
    target: &'a str,
    px: u64,
}

fn parse_scroll(cmd: &Value) -> Result<ScrollArgs<'_>, crate::BoxError> {
    let target = cmd.get("target").and_then(Value::as_str).ok_or("scroll: missing \"target\"")?;
    let px = cmd.get("px").and_then(Value::as_u64).unwrap_or(500);
    Ok(ScrollArgs { target, px })
}

/// Parsed `text` arguments.
struct TextArgs<'a> {
    uid: Option<&'a str>,
    selector: Option<&'a str>,
    truncate: Option<usize>,
}

fn parse_text(cmd: &Value) -> TextArgs<'_> {
    TextArgs {
        uid: cmd.get("uid").and_then(Value::as_str),
        selector: cmd.get("selector").and_then(Value::as_str),
        truncate: cmd.get("truncate").and_then(Value::as_u64).map(|v| v as usize),
    }
}

/// Parse an optional `xy` coordinate pair (`"xy": [x, y]`) for click/dblclick.
/// Returns `Ok(None)` when absent/null, an error when malformed.
pub fn parse_xy(cmd: &Value) -> Result<Option<(f64, f64)>, crate::BoxError> {
    match cmd.get("xy") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let arr = v.as_array().ok_or("xy must be an array [x, y]")?;
            if arr.len() != 2 {
                return Err("xy requires exactly 2 values: [x, y]".into());
            }
            let x = arr[0].as_f64().ok_or("xy values must be numbers")?;
            let y = arr[1].as_f64().ok_or("xy values must be numbers")?;
            Ok(Some((x, y)))
        }
    }
}

/// Resolve `wait`'s (what, pattern) from the several accepted shapes.
/// `network-idle` needs no pattern; every other condition requires one.
fn parse_wait(cmd: &Value) -> Result<(String, String), crate::BoxError> {
    if let Some(w) = cmd.get("what").and_then(Value::as_str) {
        if w == "network-idle" {
            Ok((w.to_string(), String::new()))
        } else {
            let p = cmd.get("pattern").and_then(Value::as_str)
                .ok_or("wait: missing \"pattern\" (use {\"what\":\"text\",\"pattern\":\"...\"})")?;
            Ok((w.to_string(), p.to_string()))
        }
    } else if let Some(p) = cmd.get("text").and_then(Value::as_str) {
        Ok(("text".into(), p.into()))
    } else if let Some(p) = cmd.get("url").and_then(Value::as_str) {
        Ok(("url".into(), p.into()))
    } else if let Some(p) = cmd.get("selector").and_then(Value::as_str) {
        Ok(("selector".into(), p.into()))
    } else {
        Err("wait: specify {\"what\":\"text\",\"pattern\":\"...\"} or {\"text\":\"...\"} or {\"url\":\"...\"} or {\"selector\":\"...\"} or {\"what\":\"network-idle\"}".into())
    }
}

/// Error for an empty or unrecognized `cmd` name.
fn unknown_cmd_error(name: &str) -> crate::BoxError {
    if name.is_empty() {
        "Missing \"cmd\" field".into()
    } else {
        format!("Unknown command: {name}").into()
    }
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
    report: crate::run_helpers::ReportPolicy,
    cmd: &Value,
    emulation_recovery: &mut EmulationRecovery,
) -> Result<Value, crate::BoxError> {
    let cmds = cmd.get("commands").and_then(Value::as_array)
        .ok_or("batch: missing \"commands\" array")?;
    let stop_on_error = cmd.get("stop_on_error").and_then(Value::as_bool).unwrap_or(false);
    Ok(run_batch(
        client, browser_client, store, browser_name, page_name, target_id, timeout,
        global_max_depth, report, cmds, stop_on_error, emulation_recovery,
    )
    .await)
}

/// Copy an optional field set into a response object.
pub fn merge_into(obj: &mut Value, details: Option<&Value>) {
    if let (Some(target), Some(fields)) = (obj.as_object_mut(), details.and_then(Value::as_object)) {
        for (key, value) in fields {
            target.insert(key.clone(), value.clone());
        }
    }
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
    report: crate::run_helpers::ReportPolicy,
    cmd: &Value,
) -> Value {
    let cmd_name = cmd.get("cmd").and_then(Value::as_str).unwrap_or("");
    // Capture the baseline before dispatching: a command run with `inspect` refreshes it
    // itself, and comparing against the refreshed copy would report that nothing moved.
    let baseline = if report.changes && mutates_page(cmd_name) {
        store
            .browsers
            .get(browser_name)
            .and_then(|b| b.pages.get(page_name))
            .map(|p| {
                (
                    p.last_snapshot.clone(),
                    p.last_snapshot_frame.clone().zip(p.last_snapshot_loader.clone()),
                )
            })
    } else {
        None
    };
    let mut value = {
    let result: Result<Value, crate::BoxError> = match cmd_name {
        "goto" => dispatch_goto(client, store, browser_name, page_name, target_id, timeout, global_max_depth, cmd).await,
        "click" => dispatch_click(client, store, browser_name, page_name, target_id, global_max_depth, report, cmd).await,
        "fill" => dispatch_fill(client, store, browser_name, page_name, target_id, global_max_depth, cmd).await,
        "inspect" => dispatch_inspect(client, store, browser_name, page_name, target_id, cmd).await,
        "eval" => dispatch_eval(client, cmd).await,
        "read" => dispatch_read(client, cmd).await,
        "text" => dispatch_text(client, store, browser_name, page_name, cmd).await,
        "screenshot" => dispatch_screenshot(client, store, browser_name, page_name, cmd).await,
        "pdf" => dispatch_pdf(client, cmd).await,
        "download" => dispatch_download(client, timeout, cmd).await,
        "wait" => dispatch_wait(client, timeout, cmd).await,
        "back" => dispatch_back(client).await,
        "forward" => dispatch_forward(client).await,
        "scroll" => dispatch_scroll(client, store, browser_name, page_name, cmd).await,
        "type" => dispatch_type(client, cmd).await,
        "press" => dispatch_press(client, cmd).await,
        "dblclick" => dispatch_dblclick(client, store, browser_name, page_name, target_id, global_max_depth, report, cmd).await,
        "select" => dispatch_select(client, store, browser_name, page_name, target_id, global_max_depth, cmd).await,
        "check" => dispatch_check(client, store, browser_name, page_name, report, cmd).await,
        "uncheck" => {
            let mut c = cmd.clone();
            if let Some(m) = c.as_object_mut() { m.insert("desired".into(), Value::Bool(false)); }
            dispatch_check(client, store, browser_name, page_name, report, &c).await
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
        "emulate" => dispatch_emulate(client, store, browser_name, page_name, cmd).await,
        "assert" => dispatch_assert(client, store, browser_name, page_name, cmd).await,
        other => Err(unknown_cmd_error(other)),
    };
    // `result` must not outlive this block: BoxError is not Send, and an await with it
    // still in scope would make every caller's future non-Send.
    match result {
        Ok(v) => v,
        Err(e) => {
            let msg = e.to_string();
            let mut obj = json!({"ok": false, "error": msg});
            if let Some(h) = crate::run_helpers::error_hint(&msg, browser_name) { obj["hint"] = json!(h); }
            return obj;
        }
    }
    };
    // Same as pipe: switching the report off must not read like an empty page.
    if !report.changes && mutates_page(cmd_name) {
        // The hit test still ran: it is part of aiming the action, not part of the report.
        // An intercepted click says so even here, where the page was never re-read.
        crate::pipe_report::attach_verdict_for(
            client,
            &mut value,
            crate::verdict::Observation::ReportingDisabled,
        );
    }
    if let Some((old_text, old_url)) = baseline {
        attach_change_report(
            client, store, browser_name, page_name, target_id, report, old_text.as_deref(),
            old_url, &mut value,
        )
        .await;
    }
    value
}

// ---------------------------------------------------------------------------
// Tests — pure JSON→typed-args parsing (no live Chrome required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_honors_px() {
        let cmd = json!({"target": "down", "px": 1200});
        let args = parse_scroll(&cmd).unwrap();
        assert_eq!(args.target, "down");
        assert_eq!(args.px, 1200);
    }

    #[test]
    fn scroll_px_defaults_to_500() {
        let cmd = json!({"target": "up"});
        let args = parse_scroll(&cmd).unwrap();
        assert_eq!(args.target, "up");
        assert_eq!(args.px, 500);
    }

    #[test]
    fn scroll_missing_target_errors() {
        let err = parse_scroll(&json!({"px": 300})).unwrap_err().to_string();
        assert!(err.contains("target"), "unexpected error: {err}");
    }

    #[test]
    fn text_maps_uid_and_selector_and_truncate() {
        let cmd = json!({"uid": "n47", "selector": "main", "truncate": 80});
        let args = parse_text(&cmd);
        assert_eq!(args.uid, Some("n47"));
        assert_eq!(args.selector, Some("main"));
        assert_eq!(args.truncate, Some(80));
    }

    #[test]
    fn text_uid_present_selector_absent() {
        let cmd = json!({"uid": "n1"});
        let args = parse_text(&cmd);
        assert_eq!(args.uid, Some("n1"));
        assert_eq!(args.selector, None);
        assert_eq!(args.truncate, None);
    }

    #[test]
    fn text_defaults_all_none() {
        let cmd = json!({});
        let args = parse_text(&cmd);
        assert!(args.uid.is_none() && args.selector.is_none() && args.truncate.is_none());
    }

    #[test]
    fn xy_parses_valid_pair() {
        let (x, y) = parse_xy(&json!({"xy": [100, 200]})).unwrap().unwrap();
        assert!((x - 100.0).abs() < f64::EPSILON && (y - 200.0).abs() < f64::EPSILON);
        // Fractional coordinates round-trip too.
        let (x, y) = parse_xy(&json!({"xy": [12.5, 3.0]})).unwrap().unwrap();
        assert!((x - 12.5).abs() < f64::EPSILON && (y - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn xy_absent_is_none() {
        assert!(parse_xy(&json!({"uid": "n1"})).unwrap().is_none());
        assert!(parse_xy(&json!({"xy": null})).unwrap().is_none());
    }

    #[test]
    fn xy_wrong_length_errors() {
        assert!(parse_xy(&json!({"xy": [1, 2, 3]})).is_err());
        assert!(parse_xy(&json!({"xy": [1]})).is_err());
    }

    #[test]
    fn xy_non_array_errors() {
        assert!(parse_xy(&json!({"xy": "100,200"})).is_err());
    }

    #[test]
    fn wait_network_idle_needs_no_pattern() {
        let (what, pattern) = parse_wait(&json!({"what": "network-idle"})).unwrap();
        assert_eq!(what, "network-idle");
        assert!(pattern.is_empty());
    }

    #[test]
    fn wait_explicit_what_pattern() {
        let (what, pattern) = parse_wait(&json!({"what": "text", "pattern": "Welcome"})).unwrap();
        assert_eq!(what, "text");
        assert_eq!(pattern, "Welcome");
    }

    #[test]
    fn wait_shorthand_fields() {
        let (what, pattern) = parse_wait(&json!({"selector": ".done"})).unwrap();
        assert_eq!(what, "selector");
        assert_eq!(pattern, ".done");
    }

    #[test]
    fn wait_missing_pattern_for_text_errors() {
        assert!(parse_wait(&json!({"what": "text"})).is_err());
    }

    #[test]
    fn wait_empty_errors() {
        assert!(parse_wait(&json!({})).is_err());
    }

    #[test]
    fn wait_default_timeout_is_ten() {
        // Regression: pipe/batch `wait` must default to the CLI's 10s, not the
        // global 30s page-load timeout.
        assert_eq!(WAIT_DEFAULT_TIMEOUT, 10);
    }

    #[test]
    fn unknown_cmd_error_messages() {
        assert!(unknown_cmd_error("").to_string().contains("Missing"));
        assert_eq!(unknown_cmd_error("frobnicate").to_string(), "Unknown command: frobnicate");
    }
}
