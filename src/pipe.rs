use std::collections::HashMap;
use std::io::Write as _;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::browser::{self, BrowserOptions};
use crate::cdp::client::CdpClient;
use crate::commands;
use crate::element_ref::ElementRef;
use crate::run_helpers::{error_hint, resolve_page_target};
use crate::session::{self, SessionStore};
use crate::Cli;

/// Run pipe mode: persistent CDP connection, reading JSON commands from stdin.
pub async fn run_pipe(cli: &Cli) -> Result<(), crate::BoxError> {
    let mut store = session::load_session()?;
    let want_headless = !cli.headed;

    let (conn, browser_client) = connect_browser(&mut store, cli, want_headless).await?;

    let http_endpoint = conn.http_endpoint.as_deref().ok_or(
        "No HTTP endpoint available. Cannot resolve page WebSocket URL.",
    )?;

    let target_id = {
        let browser_session = session::ensure_browser(
            &mut store,
            &cli.browser,
            &conn.ws_endpoint,
            conn.pid,
            want_headless,
        );
        resolve_page_target(&browser_client, browser_session, &cli.page).await?
    };
    let _ = session::save_session(&mut store);

    let page_ws = browser::get_page_ws_url(http_endpoint, &target_id).await?;
    let client = CdpClient::connect(&page_ws).await?;
    client.enable("Page").await?;

    // Console interceptor (stealth-safe)
    commands::console::inject(&client).await;

    if cli.stealth {
        crate::setup::apply_stealth(&client).await;
    } else {
        client.enable("Runtime").await?;
    }

    // Main loop: read JSON commands from stdin
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let cmd: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                emit(&json!({"ok": false, "error": format!("Invalid JSON: {e}")}));
                continue;
            }
        };

        // Check for _record field to start/continue recording
        let record_path = cmd.get("_record").and_then(Value::as_str).map(String::from);
        if let Some(ref path) = record_path {
            let _ = commands::record::start_recording(path);
        }

        let response = dispatch(
            &client,
            &browser_client,
            &mut store,
            &cli.browser,
            &cli.page,
            &target_id,
            cli.timeout,
            cli.max_depth,
            &cmd,
        )
        .await;

        // Log to recording file if _record was specified
        if let Some(ref path) = record_path {
            let _ = commands::record::log_entry(path, &cmd, &response);
        }

        emit(&response);
    }

    // EOF: save session and exit cleanly
    let _ = session::save_session(&mut store);
    Ok(())
}

/// Replay a recorded session file, optionally substituting variables.
pub async fn run_replay(
    cli: &Cli,
    file: &str,
    vars: Option<&[String]>,
) -> Result<(), crate::BoxError> {
    let content = std::fs::read_to_string(file)
        .map_err(|e| format!("Cannot read replay file '{file}': {e}"))?;

    // Parse variable substitutions: ["key=val", "key2=val2"]
    let replacements: Vec<(&str, &str)> = vars
        .unwrap_or(&[])
        .iter()
        .filter_map(|pair| pair.split_once('='))
        .collect();

    let mut store = session::load_session()?;
    let want_headless = !cli.headed;
    let (conn, browser_client) = connect_browser(&mut store, cli, want_headless).await?;

    let http_endpoint = conn.http_endpoint.as_deref().ok_or(
        "No HTTP endpoint available.",
    )?;
    let target_id = {
        let browser_session = session::ensure_browser(
            &mut store,
            &cli.browser,
            &conn.ws_endpoint,
            conn.pid,
            want_headless,
        );
        resolve_page_target(&browser_client, browser_session, &cli.page).await?
    };
    let _ = session::save_session(&mut store);

    let page_ws = browser::get_page_ws_url(http_endpoint, &target_id).await?;
    let client = CdpClient::connect(&page_ws).await?;
    client.enable("Page").await?;
    commands::console::inject(&client).await;
    if cli.stealth {
        crate::setup::apply_stealth(&client).await;
    } else {
        client.enable("Runtime").await?;
    }

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Apply variable substitutions
        let mut resolved = line.to_string();
        for (key, val) in &replacements {
            resolved = resolved.replace(&format!("{{{{{key}}}}}"), val);
        }

        let parsed: Value = serde_json::from_str(&resolved)
            .map_err(|e| format!("Invalid JSON in replay: {e}"))?;

        // Support both recording format {"cmd":..., "response":...} and raw command format
        let cmd = if parsed.get("cmd").is_some_and(Value::is_object) && parsed.get("response").is_some() {
            parsed.get("cmd").cloned().unwrap_or_default()
        } else {
            parsed
        };

        let response = dispatch(
            &client,
            &browser_client,
            &mut store,
            &cli.browser,
            &cli.page,
            &target_id,
            cli.timeout,
            cli.max_depth,
            &cmd,
        )
        .await;

        emit(&response);
    }

    let _ = session::save_session(&mut store);
    Ok(())
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn dispatch(
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
        "scroll" => dispatch_scroll(client, store, browser_name, page_name, cmd).await,
        "type" => dispatch_type(client, cmd).await,
        "press" => dispatch_press(client, cmd).await,
        "fill-form" | "fill_form" | "fillform" => dispatch_fill_form(client, store, browser_name, page_name, target_id, global_max_depth, cmd).await,
        "hover" => dispatch_hover(client, store, browser_name, page_name, cmd).await,
        "tabs" => dispatch_tabs(browser_client, store).await,
        "network" => dispatch_network(client, cmd).await,
        "console" => dispatch_console(client, cmd).await,
        "diff" => dispatch_diff(client, store, browser_name, page_name, target_id).await,
        "extract" => dispatch_extract(client, cmd).await,
        "navigate_and_read" | "navigate-and-read" => dispatch_navigate_and_read(client, store, browser_name, page_name, target_id, timeout, cmd).await,
        "fill_and_submit" | "fill-and-submit" => dispatch_fill_and_submit(client, timeout, cmd).await,
        "history" => dispatch_history(cmd),
        "" => Err("Missing \"cmd\" field".into()),
        other => Err(format!("Unknown command: {other}").into()),
    };

    match result {
        Ok(v) => v,
        Err(e) => {
            let msg = e.to_string();
            let mut obj = json!({"ok": false, "error": msg});
            if let Some(h) = error_hint(&msg) {
                obj["hint"] = json!(h);
            }
            obj
        }
    }
}

// ---------------------------------------------------------------------------
// Per-command dispatchers
// ---------------------------------------------------------------------------

async fn dispatch_goto(
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

    // Log to browsing history
    let _ = commands::history::append(&result.url, &result.title, page_name);

    let mut obj = json!({"ok": true, "url": result.url, "title": result.title});

    if inspect {
        let snapshot = attach_snapshot(client, store, browser_name, page_name, target_id, max_depth).await?;
        obj["snapshot"] = json!(snapshot);
    }
    Ok(obj)
}

async fn dispatch_click(
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

async fn dispatch_fill(
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

async fn dispatch_inspect(
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

async fn dispatch_diff(
    client: &CdpClient,
    store: &mut SessionStore,
    browser_name: &str,
    page_name: &str,
    target_id: &str,
) -> Result<Value, crate::BoxError> {
    let old_text = store
        .browsers
        .get(browser_name)
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

    Ok(json!({
        "ok": true,
        "added": stats.added,
        "removed": stats.removed,
        "changed": stats.changed,
        "diff": diff.trim_end(),
    }))
}

async fn dispatch_eval(
    client: &CdpClient,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
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

async fn dispatch_read(
    client: &CdpClient,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let truncate = cmd.get("truncate").and_then(Value::as_u64).map(|v| v as usize);
    let result = commands::read::run(client, false, truncate).await?;

    let mut obj = json!({"ok": true, "title": result.title, "text": result.text_content});
    if let Some(excerpt) = &result.excerpt {
        obj["excerpt"] = json!(excerpt);
    }
    if let Some(byline) = &result.byline {
        obj["byline"] = json!(byline);
    }
    Ok(obj)
}

async fn dispatch_text(
    client: &CdpClient,
    store: &SessionStore,
    browser_name: &str,
    page_name: &str,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let selector = cmd.get("selector").and_then(Value::as_str);
    let truncate = cmd.get("truncate").and_then(Value::as_u64).map(|v| v as usize);
    let uid_map = get_uid_map(store, browser_name, page_name);

    let text = commands::text::run(client, None, selector, &uid_map).await?;
    let full_length = text.chars().count();
    let (text, truncated) = if let Some(n) = truncate {
        if full_length > n {
            (crate::truncate::truncate_str(&text, n, "...").into_owned(), true)
        } else {
            (text, false)
        }
    } else {
        (text, false)
    };

    let mut obj = json!({"ok": true, "text": text});
    if truncated {
        obj["truncated"] = json!(true);
        obj["fullLength"] = json!(full_length);
    }
    Ok(obj)
}

async fn dispatch_screenshot(
    client: &CdpClient,
) -> Result<Value, crate::BoxError> {
    let path = commands::screenshot::run(client, None).await?;
    Ok(json!({"ok": true, "path": path}))
}

async fn dispatch_wait(
    client: &CdpClient,
    default_timeout: u64,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    // Accept both {"what":"text","pattern":"X"} and {"text":"X"} / {"url":"X"} / {"selector":"X"}
    let (what, pattern) = if let Some(w) = cmd.get("what").and_then(Value::as_str) {
        let p = cmd.get("pattern").and_then(Value::as_str)
            .ok_or("wait: missing \"pattern\" (use {\"what\":\"text\",\"pattern\":\"...\"})")?;
        (w.to_string(), p.to_string())
    } else if let Some(p) = cmd.get("text").and_then(Value::as_str) {
        ("text".into(), p.into())
    } else if let Some(p) = cmd.get("url").and_then(Value::as_str) {
        ("url".into(), p.into())
    } else if let Some(p) = cmd.get("selector").and_then(Value::as_str) {
        ("selector".into(), p.into())
    } else {
        return Err("wait: specify {\"what\":\"text\",\"pattern\":\"...\"} or {\"text\":\"...\"} or {\"url\":\"...\"} or {\"selector\":\"...\"}".into());
    };
    let timeout = cmd.get("timeout").and_then(Value::as_u64).unwrap_or(default_timeout);

    let msg = commands::wait::run(client, &what, &pattern, timeout).await?;
    Ok(json!({"ok": true, "message": msg}))
}

async fn dispatch_back(
    client: &CdpClient,
) -> Result<Value, crate::BoxError> {
    // Use CDP Page.getNavigationHistory + Page.navigateToHistoryEntry instead of
    // history.back() — the JS approach can break the WebSocket connection in pipe mode
    // because the page target changes during navigation.
    let history: Value = client
        .call("Page.getNavigationHistory", json!({}))
        .await?;
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
        .call("Runtime.evaluate", json!({"expression": "document.title", "returnByValue": true}))
        .await?;
    let title_str = title.result.value.as_ref().and_then(|v| v.as_str()).unwrap_or("");
    Ok(json!({"ok": true, "title": title_str}))
}

async fn dispatch_scroll(
    client: &CdpClient,
    store: &SessionStore,
    browser_name: &str,
    page_name: &str,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let target = cmd.get("target").and_then(Value::as_str).ok_or("scroll: missing \"target\"")?;

    let msg = match target {
        "down" => {
            let _: Value = client
                .call("Runtime.evaluate", json!({"expression": "window.scrollBy(0, 500)", "returnByValue": true}))
                .await?;
            "Scrolled down".to_string()
        }
        "up" => {
            let _: Value = client
                .call("Runtime.evaluate", json!({"expression": "window.scrollBy(0, -500)", "returnByValue": true}))
                .await?;
            "Scrolled up".to_string()
        }
        uid => {
            let uid_map = get_uid_map(store, browser_name, page_name);
            let element_ref = uid_map.get(uid).ok_or_else(|| {
                format!("Element uid={uid} not found. Run 'aibrowsr inspect' to get fresh uids.")
            })?;
            let backend_node_id = element_ref.backend_node_id().ok_or_else(|| {
                format!("Element uid={uid} has no resolvable backend node.")
            })?;
            let resolve_result: crate::cdp::types::ResolveNodeResult = client
                .call(
                    "DOM.resolveNode",
                    crate::cdp::types::ResolveNodeParams {
                        node_id: None,
                        backend_node_id: Some(backend_node_id),
                        object_group: Some("aibrowsr".into()),
                        execution_context_id: None,
                    },
                )
                .await?;
            let object_id = resolve_result.object.object_id.ok_or_else(|| {
                format!("Element uid={uid} could not be resolved to a JS object.")
            })?;
            let _: Value = client
                .call(
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": object_id,
                        "functionDeclaration": "function() { this.scrollIntoView({block: 'center'}); }",
                        "returnByValue": true,
                    }),
                )
                .await?;
            format!("Scrolled uid={uid} into view")
        }
    };

    Ok(json!({"ok": true, "message": msg}))
}

async fn dispatch_type(
    client: &CdpClient,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let text = cmd.get("text").and_then(Value::as_str).ok_or("type: missing \"text\"")?;
    let selector = cmd.get("selector").and_then(Value::as_str);

    if let Some(sel) = selector {
        crate::element::focus_selector(client, sel).await?;
    }
    crate::element::type_text(client, text).await?;

    let msg = if let Some(sel) = selector {
        format!("Typed {} chars into selector '{sel}'", text.len())
    } else {
        format!("Typed {} chars", text.len())
    };
    Ok(json!({"ok": true, "message": msg}))
}

async fn dispatch_press(
    client: &CdpClient,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let key = cmd.get("key").and_then(Value::as_str).ok_or("press: missing \"key\"")?;
    crate::element::press_key(client, key).await?;
    Ok(json!({"ok": true, "message": format!("Pressed {key}")}))
}

async fn dispatch_tabs(
    browser_client: &CdpClient,
    store: &session::SessionStore,
) -> Result<Value, crate::BoxError> {
    let tabs = commands::tabs::run_structured(browser_client, store).await?;
    Ok(json!({"ok": true, "tabs": tabs}))
}

async fn dispatch_network(
    client: &CdpClient,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let filter = cmd.get("filter").and_then(Value::as_str);
    let limit = cmd.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
    let entries = commands::network::run_retroactive(client, filter, limit).await?;
    Ok(json!({"ok": true, "requests": entries}))
}

async fn dispatch_console(
    client: &CdpClient,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let level = cmd.get("level").and_then(Value::as_str);
    let clear = cmd.get("clear").and_then(Value::as_bool).unwrap_or(false);
    let limit = cmd.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
    let entries = commands::console::run(client, level, clear, limit).await?;
    let messages: Vec<Value> = entries
        .iter()
        .map(|e| json!({"level": e.level, "message": e.message, "timestamp": e.timestamp}))
        .collect();
    Ok(json!({"ok": true, "messages": messages}))
}

async fn dispatch_extract(
    client: &CdpClient,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let selector = cmd.get("selector").and_then(Value::as_str);
    let limit = cmd.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize;
    let scroll = cmd.get("scroll").and_then(Value::as_bool).unwrap_or(false);
    if scroll {
        commands::extract::scroll_to_load(client).await?;
    }
    let result = commands::extract::run(client, selector, limit).await?;
    Ok(commands::extract::to_json(&result))
}

// ---------------------------------------------------------------------------
// Composite dispatchers
// ---------------------------------------------------------------------------

async fn dispatch_navigate_and_read(
    client: &CdpClient,
    _store: &mut SessionStore,
    _browser_name: &str,
    _page_name: &str,
    _target_id: &str,
    timeout: u64,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let url = cmd.get("url").and_then(Value::as_str)
        .ok_or("navigate_and_read: missing \"url\"")?;
    let truncate = cmd.get("truncate").and_then(Value::as_u64).map(|v| v as usize);

    // Step 1: goto + wait for load
    let goto_result = commands::goto::run(client, url, timeout).await?;

    // Log to browsing history
    let _ = commands::history::append(&goto_result.url, &goto_result.title, _page_name);

    // Step 2: read with optional truncation
    let read_result = commands::read::run(client, false, truncate).await?;

    Ok(json!({
        "ok": true,
        "url": goto_result.url,
        "title": goto_result.title,
        "content": read_result.text_content,
    }))
}

async fn dispatch_fill_and_submit(
    client: &CdpClient,
    timeout: u64,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let fields = cmd.get("fields").and_then(Value::as_array)
        .ok_or("fill_and_submit: missing \"fields\" array")?;
    let submit_selector = cmd.get("submit").and_then(Value::as_str)
        .ok_or("fill_and_submit: missing \"submit\" selector")?;
    let wait_for = cmd.get("wait_for").and_then(Value::as_str);

    // Step 1: fill each field
    let field_count = fields.len();
    for field in fields {
        let selector = field.get("selector").and_then(Value::as_str)
            .ok_or("fill_and_submit: each field needs \"selector\"")?;
        let value = field.get("value").and_then(Value::as_str)
            .ok_or("fill_and_submit: each field needs \"value\"")?;
        crate::element::fill_selector(client, selector, value).await?;
    }

    // Step 2: click submit
    crate::element::click_selector(client, submit_selector).await?;

    // Step 3: wait for condition if specified
    if let Some(pattern) = wait_for {
        // Heuristic: if it looks like a CSS selector, wait for selector; else wait for text
        let is_selector = pattern.contains('.') || pattern.contains('#')
            || pattern.contains('[') || pattern.contains('>');
        let wait_type = if is_selector { "selector" } else { "text" };
        commands::wait::run(client, wait_type, pattern, timeout).await?;
    }

    // Step 4: read page content
    let read_result = commands::read::run(client, false, None).await?;

    let message = format!(
        "Filled {field_count} fields, submitted, waited for '{}'",
        wait_for.unwrap_or("none")
    );

    Ok(json!({
        "ok": true,
        "message": message,
        "content": read_result.text_content,
    }))
}

fn dispatch_history(
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let filter = cmd.get("filter").and_then(Value::as_str);
    let limit = cmd.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
    let entries = commands::history::run(filter, limit)?;
    let entries_json: Vec<Value> = entries
        .iter()
        .map(|e| json!({"ts": e.ts, "url": e.url, "title": e.title, "page": e.page}))
        .collect();
    Ok(json!({"ok": true, "entries": entries_json}))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Take a snapshot, update the session `uid_map`, and return the snapshot text.
async fn attach_snapshot(
    client: &CdpClient,
    store: &mut SessionStore,
    browser_name: &str,
    page_name: &str,
    target_id: &str,
    max_depth: Option<usize>,
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
    store
        .browsers
        .get(browser_name)
        .and_then(|b| b.pages.get(page_name))
        .map(|p| p.uid_map.clone())
        .unwrap_or_default()
}

fn cmd_max_depth(cmd: &Value) -> Option<usize> {
    cmd.get("max_depth").and_then(Value::as_u64).map(|v| v as usize)
}

/// Emit a single JSON line to stdout, flushing immediately.
fn emit(value: &Value) {
    let line = serde_json::to_string(value).unwrap_or_default();
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let _ = writeln!(handle, "{line}");
    let _ = handle.flush();
}

/// Connect to the browser, reusing existing session or launching new.
async fn connect_browser(
    store: &mut SessionStore,
    cli: &Cli,
    want_headless: bool,
) -> Result<(browser::BrowserConnection, CdpClient), crate::BoxError> {
    if let Some(existing) = store.browsers.get(&cli.browser) {
        let mode_matches = existing.headless == want_headless;
        let ws = &existing.ws_endpoint;
        let http = browser::extract_http_from_ws(ws);

        if mode_matches {
            if let Ok(client) = CdpClient::connect(ws).await {
                let conn = browser::BrowserConnection {
                    ws_endpoint: ws.clone(),
                    http_endpoint: Some(http),
                    pid: existing.pid,
                };
                return Ok((conn, client));
            }
        } else if let Some(pid) = existing.pid {
            #[cfg(unix)]
            {
                let _ = std::process::Command::new("kill")
                    .arg(pid.to_string())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
        store.browsers.remove(&cli.browser);
    }

    let opts = BrowserOptions {
        name: cli.browser.clone(),
        headless: want_headless,
        ignore_https_errors: cli.ignore_https_errors,
        stealth: cli.stealth,
        connect: cli.connect.clone(),
    };
    let conn = browser::resolve_browser(&opts).await?;
    let client = CdpClient::connect(&conn.ws_endpoint).await?;
    Ok((conn, client))
}

async fn dispatch_fill_form(
    client: &CdpClient,
    store: &mut session::SessionStore,
    browser_name: &str,
    page_name: &str,
    target_id: &str,
    global_max_depth: Option<usize>,
    cmd: &Value,
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
        let snapshot = crate::commands::inspect::run(client, false, max_depth, None, None).await?;
        obj["snapshot"] = json!(snapshot.text);
        if let Some(browser_s) = store.browsers.get_mut(browser_name) {
            let page = session::ensure_page(browser_s, page_name, target_id);
            page.uid_map = snapshot.uid_map;
        }
    }
    Ok(obj)
}

async fn dispatch_hover(
    client: &CdpClient,
    store: &session::SessionStore,
    browser_name: &str,
    page_name: &str,
    cmd: &Value,
) -> Result<Value, crate::BoxError> {
    let uid = cmd.get("uid").and_then(Value::as_str).ok_or("hover requires \"uid\"")?;
    let uid_map = crate::run_helpers::get_uid_map(store, browser_name, page_name);
    crate::element::hover(client, &uid_map, uid).await?;
    Ok(json!({"ok": true, "message": format!("Hovered uid={uid}")}))
}
