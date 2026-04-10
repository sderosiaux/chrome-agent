use serde_json::json;

use crate::BoxError;
use crate::browser::{self, BrowserOptions};
use crate::cdp::client::CdpClient;
use crate::cli::{Cli, Command, DaemonAction};
use crate::run_helpers::{cmd_close, cmd_status, cmd_stop, connect_page, get_uid_map, json_output, output_action, output_goto, resolve_page_target};
use crate::{commands, pipe, session};

pub async fn run(cli: Cli) -> Result<(), BoxError> {
    match cli.command {
        Command::Daemon { action } => {
            match action {
                DaemonAction::Start => {
                    #[cfg(unix)]
                    {
                        let socket_path = session::daemon_socket_path()?;
                        crate::daemon::run_daemon(&socket_path).await?;
                    }
                    #[cfg(not(unix))]
                    {
                        return Err("Daemon is not supported on Windows. Commands work without a daemon.".into());
                    }
                }
            }
            return Ok(());
        }

        Command::Status => {
            return cmd_status(cli.json);
        }

        Command::Stop => {
            return cmd_stop(cli.json).await;
        }

        Command::Close { purge } => {
            return cmd_close(&cli.browser, purge, cli.json);
        }

        Command::Pipe => {
            return pipe::run_pipe(&cli).await;
        }

        Command::Replay { ref file, ref vars } => {
            return pipe::run_replay(&cli, file, vars.as_deref()).await;
        }

        Command::History { ref filter, limit } => {
            let entries = commands::history::run(filter.as_deref(), limit)?;
            if cli.json {
                let entries_json: Vec<serde_json::Value> = entries
                    .iter()
                    .map(|e| json!({"ts": e.ts, "url": e.url, "title": e.title, "page": e.page}))
                    .collect();
                json_output(&json!({"ok": true, "entries": entries_json}));
            } else {
                let text = commands::history::format_text(&entries);
                if text.is_empty() {
                    println!("No history entries found.");
                } else {
                    println!("{text}");
                }
            }
            return Ok(());
        }

        _ => {}
    }

    // All other commands need a browser connection + CDP client
    let mut store = session::load_session()?;

    let existing_mode = store.browsers.get(&cli.browser).map(|b| b.headless);
    let want_headless = existing_mode.unwrap_or(!cli.headed);

    let (conn, browser_client) = if let Some(existing) = store.browsers.get(&cli.browser) {
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
                (conn, client)
            } else {
                store.browsers.remove(&cli.browser);
                let opts = BrowserOptions {
                    name: cli.browser.clone(),
                    headless: want_headless,
                    ignore_https_errors: cli.ignore_https_errors,
                    stealth: cli.stealth,
                    connect: cli.connect.clone(),
                    copy_cookies: cli.copy_cookies,
                };
                let conn = browser::resolve_browser(&opts).await?;
                let client = CdpClient::connect(&conn.ws_endpoint).await?;
                (conn, client)
            }
        } else {
            if let Some(pid) = existing.pid {
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
            let opts = BrowserOptions {
                name: cli.browser.clone(),
                headless: want_headless,
                ignore_https_errors: cli.ignore_https_errors,
                stealth: cli.stealth,
                connect: cli.connect.clone(),
                copy_cookies: cli.copy_cookies,
            };
            let conn = browser::resolve_browser(&opts).await?;
            let client = CdpClient::connect(&conn.ws_endpoint).await?;
            (conn, client)
        }
    } else {
        let needs_existing = !matches!(
            cli.command,
            Command::Goto { .. } | Command::Pipe
        );
        if needs_existing {
            return Err(format!(
                "No browser session '{}'. Run `chrome-agent --browser {} goto <url>` first.",
                cli.browser, cli.browser
            ).into());
        }
        let opts = BrowserOptions {
            name: cli.browser.clone(),
            headless: want_headless,
            ignore_https_errors: cli.ignore_https_errors,
            stealth: cli.stealth,
            connect: cli.connect.clone(),
            copy_cookies: cli.copy_cookies,
        };
        let conn = browser::resolve_browser(&opts).await?;
        let client = CdpClient::connect(&conn.ws_endpoint).await?;
        (conn, client)
    };

    let http_endpoint = conn.http_endpoint.as_deref().ok_or(
        "No HTTP endpoint available. Cannot resolve page WebSocket URL."
    )?;

    let target_id = {
        let browser_session = session::ensure_browser(
            &mut store,
            &cli.browser,
            &conn.ws_endpoint,
            conn.pid,
            !cli.headed,
        );
        resolve_page_target(&browser_client, browser_session, &cli.page).await?
    };
    let _ = session::save_session(&mut store);

    let client = connect_page(http_endpoint, &target_id, cli.stealth).await?;

    let json_mode = cli.json;
    match cli.command {
        Command::Goto { url, inspect, max_depth, wait_for } => {
            let depth = max_depth.or(cli.max_depth);
            let result = commands::goto::run(&client, &url, cli.timeout).await?;
            if let Some(ref selector) = wait_for {
                commands::wait::run(&client, "selector", selector, cli.timeout).await?;
            }
            let _ = commands::history::append(&result.url, &result.title, &cli.page);
            output_goto(&client, &mut store, &cli.browser, &cli.page, &target_id, &result.url, &result.title, inspect, depth, json_mode).await?;
        }

        Command::Click { uid, selector, xy, inspect, max_depth } => {
            let depth = max_depth.or(cli.max_depth);
            let provided = u8::from(uid.is_some()) + u8::from(selector.is_some()) + u8::from(xy.is_some());
            if provided == 0 {
                return Err("Provide a uid, --selector, or --xy to identify the click target.".into());
            }
            if provided > 1 {
                return Err("Only one of uid, --selector, or --xy can be provided.".into());
            }

            let msg = if let Some(ref sel) = selector {
                crate::element::click_selector(&client, sel).await?;
                format!("Clicked selector '{sel}'")
            } else if let Some(ref coords) = xy {
                if coords.len() != 2 {
                    return Err("--xy requires exactly 2 values: x,y".into());
                }
                crate::element::click_at_coords(&client, coords[0], coords[1]).await?;
                format!("Clicked at ({}, {})", coords[0], coords[1])
            } else {
                let uid = uid.as_ref().unwrap();
                let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
                commands::click::run(&client, &uid_map, uid).await?
            };

            output_action(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, inspect, depth, json_mode).await?;
        }

        Command::Fill { uid, selector, value, inspect, max_depth } => {
            let depth = max_depth.or(cli.max_depth);
            let provided = u8::from(uid.is_some()) + u8::from(selector.is_some());
            if provided == 0 {
                return Err("Provide --uid or --selector to identify the element.".into());
            }
            if provided > 1 {
                return Err("Only one of --uid or --selector can be provided.".into());
            }

            let msg = if let Some(ref sel) = selector {
                crate::element::fill_selector(&client, sel, &value).await?;
                format!("Filled selector '{sel}'")
            } else {
                let uid = uid.as_ref().unwrap();
                let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
                commands::fill::run(&client, &uid_map, uid, &value).await?
            };

            output_action(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, inspect, depth, json_mode).await?;
        }

        Command::FillForm { pairs, inspect, max_depth } => {
            let depth = max_depth.or(cli.max_depth);
            let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
            let parsed: Result<Vec<(&str, &str)>, _> = pairs
                .iter()
                .map(|p| {
                    p.split_once('=')
                        .ok_or_else(|| format!("Invalid pair (expected uid=value): {p}"))
                })
                .collect();
            let parsed = parsed?;
            let msg = commands::fill::run_form(&client, &uid_map, &parsed).await?;
            output_action(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, inspect, depth, json_mode).await?;
        }

        Command::Text { uid, selector, truncate } => {
            if uid.is_some() && selector.is_some() {
                return Err("Only one of uid or --selector can be provided.".into());
            }
            let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
            let text = commands::text::run(&client, uid.as_deref(), selector.as_deref(), &uid_map).await?;
            let full_length = text.chars().count();
            let (text, truncated) = if let Some(n) = truncate
                && full_length > n {
                    (crate::truncate::truncate_str(&text, n, "...").into_owned(), true)
                } else {
                    (text, false)
                };
            if json_mode {
                let mut obj = json!({"ok": true, "text": text});
                if truncated {
                    obj["truncated"] = json!(true);
                    obj["fullLength"] = json!(full_length);
                }
                json_output(&obj);
            } else {
                println!("{text}");
            }
        }

        Command::Read { html, truncate } => {
            let result = commands::read::run(&client, html, truncate).await?;
            if json_mode {
                let mut obj = json!({"ok": true, "title": result.title, "text": result.text_content});
                if let Some(excerpt) = &result.excerpt {
                    obj["excerpt"] = json!(excerpt);
                }
                if let Some(byline) = &result.byline {
                    obj["byline"] = json!(byline);
                }
                json_output(&obj);
            } else {
                if !result.title.is_empty() {
                    println!("# {}", result.title);
                    println!();
                }
                if html {
                    if let Some(content) = &result.content {
                        println!("{content}");
                    }
                } else {
                    println!("{}", result.text_content);
                }
            }
        }

        Command::Back => {
            client.send("Runtime.evaluate", json!({"expression": "history.back()"})).await?;
            let _ = client.wait_for_event("Page.loadEventFired", std::time::Duration::from_secs(5)).await;
            let title: crate::cdp::types::EvaluateResult = client
                .call("Runtime.evaluate", json!({"expression": "document.title", "returnByValue": true}))
                .await?;
            let title_str = title.result.value.as_ref().and_then(|v| v.as_str()).unwrap_or("");
            if json_mode {
                json_output(&json!({"ok": true, "title": title_str}));
            } else {
                println!("Navigated back — {title_str}");
            }
        }

        Command::Forward => {
            let history: serde_json::Value = client
                .call("Page.getNavigationHistory", json!({}))
                .await?;
            let current_index = history.get("currentIndex").and_then(serde_json::Value::as_i64).unwrap_or(0);
            let entries = history.get("entries").and_then(serde_json::Value::as_array);
            let entry_count = entries.map_or(0, Vec::len) as i64;
            if current_index >= entry_count - 1 {
                if json_mode {
                    json_output(&json!({"ok": true, "title": "", "message": "Already at last history entry"}));
                } else {
                    println!("Already at last history entry");
                }
            } else {
                let next_entry_id = entries
                    .and_then(|e| e.get(usize::try_from(current_index + 1).unwrap_or(0)))
                    .and_then(|e| e.get("id"))
                    .and_then(serde_json::Value::as_i64)
                    .ok_or("Could not find next history entry")?;
                client.send("Page.navigateToHistoryEntry", json!({"entryId": next_entry_id})).await?;
                let _ = client.wait_for_event("Page.loadEventFired", std::time::Duration::from_secs(5)).await;
                let title: crate::cdp::types::EvaluateResult = client
                    .call("Runtime.evaluate", json!({"expression": "document.title", "returnByValue": true}))
                    .await?;
                let title_str = title.result.value.as_ref().and_then(|v| v.as_str()).unwrap_or("");
                if json_mode {
                    json_output(&json!({"ok": true, "title": title_str}));
                } else {
                    println!("Navigated forward — {title_str}");
                }
            }
        }

        Command::Dblclick { uid, selector, xy, inspect, max_depth } => {
            let depth = max_depth.or(cli.max_depth);
            let provided = u8::from(uid.is_some()) + u8::from(selector.is_some()) + u8::from(xy.is_some());
            if provided == 0 {
                return Err("Provide a uid, --selector, or --xy.".into());
            }
            if provided > 1 {
                return Err("Only one of uid, --selector, or --xy can be provided.".into());
            }

            let msg = if let Some(ref sel) = selector {
                crate::element::click_selector(&client, sel).await?;
                format!("Double-clicked selector '{sel}'")
            } else if let Some(ref coords) = xy {
                if coords.len() != 2 {
                    return Err("--xy requires exactly 2 values: x,y".into());
                }
                crate::element::dblclick_at_coords(&client, coords[0], coords[1]).await?;
                format!("Double-clicked at ({}, {})", coords[0], coords[1])
            } else {
                let uid = uid.as_ref().unwrap();
                let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
                commands::dblclick::run(&client, &uid_map, uid).await?
            };

            output_action(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, inspect, depth, json_mode).await?;
        }

        Command::Select { value, uid, selector, inspect, max_depth } => {
            let depth = max_depth.or(cli.max_depth);
            let provided = u8::from(uid.is_some()) + u8::from(selector.is_some());
            if provided == 0 {
                return Err("Provide --uid or --selector to identify the <select>.".into());
            }
            if provided > 1 {
                return Err("Only one of --uid or --selector can be provided.".into());
            }

            let msg = if let Some(ref sel) = selector {
                let text = crate::element::select_option_selector(&client, sel, &value).await?;
                format!("Selected \"{text}\" on selector '{sel}'")
            } else {
                let uid = uid.as_ref().unwrap();
                let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
                commands::select::run(&client, &uid_map, uid, &value).await?
            };

            output_action(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, inspect, depth, json_mode).await?;
        }

        Command::Check { uid, selector, inspect, max_depth } => {
            let depth = max_depth.or(cli.max_depth);
            if uid.is_none() && selector.is_none() {
                return Err("Provide a uid or --selector.".into());
            }
            let msg = if selector.is_some() {
                return Err("check --selector not yet supported. Use --uid instead.".into());
            } else {
                let uid = uid.as_ref().unwrap();
                let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
                commands::check::run(&client, &uid_map, uid, true).await?
            };
            output_action(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, inspect, depth, json_mode).await?;
        }

        Command::Uncheck { uid, selector, inspect, max_depth } => {
            let depth = max_depth.or(cli.max_depth);
            if uid.is_none() && selector.is_none() {
                return Err("Provide a uid or --selector.".into());
            }
            let msg = if selector.is_some() {
                return Err("uncheck --selector not yet supported. Use --uid instead.".into());
            } else {
                let uid = uid.as_ref().unwrap();
                let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
                commands::check::run(&client, &uid_map, uid, false).await?
            };
            output_action(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, inspect, depth, json_mode).await?;
        }

        Command::Upload { files, uid, selector } => {
            if uid.is_none() && selector.is_none() {
                return Err("Provide --uid or --selector to identify the file input.".into());
            }
            let msg = if let Some(ref sel) = selector {
                crate::element::set_file_input_selector(&client, sel, &files).await?;
                format!("Uploaded {} file(s) to selector '{sel}'", files.len())
            } else {
                let uid = uid.as_ref().unwrap();
                let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
                commands::upload::run(&client, &uid_map, uid, &files).await?
            };
            if json_mode {
                json_output(&json!({"ok": true, "message": msg}));
            } else {
                println!("{msg}");
            }
        }

        Command::Drag { from, to, inspect, max_depth } => {
            let depth = max_depth.or(cli.max_depth);
            let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
            let msg = commands::drag::run(&client, &uid_map, &from, &to).await?;
            output_action(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, inspect, depth, json_mode).await?;
        }

        Command::Inspect { verbose, max_depth, uid, filter, scroll, limit, urls } => {
            if scroll {
                commands::extract::scroll_to_load(&client).await?;
            }
            let role_filter: Option<Vec<&str>> = filter.as_deref().map(|f| f.split(',').map(str::trim).collect());
            let (mut text, uid_map) = if let Some(max) = limit {
                let result = commands::inspect::scroll_collect(&client, verbose, uid.as_deref(), role_filter.as_deref(), max).await?;
                (result.text, result.uid_map)
            } else {
                let s = commands::inspect::run(&client, verbose, max_depth, uid.as_deref(), role_filter.as_deref()).await?;
                (s.text, s.uid_map)
            };
            if urls {
                text = commands::inspect::resolve_urls(&client, &text, &uid_map).await;
            }
            if let Some(browser_s) = store.browsers.get_mut(&cli.browser) {
                let page = session::ensure_page(browser_s, &cli.page, &target_id);
                page.uid_map = uid_map;
                page.last_snapshot = Some(text.clone());
            }
            if json_mode {
                json_output(&json!({"ok": true, "snapshot": text}));
            } else {
                println!("{text}");
            }
        }

        Command::Diff => {
            let old_snapshot = store
                .browsers
                .get(&cli.browser)
                .and_then(|b| b.pages.get(&cli.page))
                .and_then(|p| p.last_snapshot.clone());
            let old_text = old_snapshot.ok_or("No previous snapshot. Run 'chrome-agent inspect' first.")?;
            let snapshot = commands::inspect::run(&client, false, None, None, None).await?;
            let diff = commands::diff::diff_snapshots(&old_text, &snapshot.text);
            let stats = commands::diff::diff_stats(&diff);
            if let Some(browser_s) = store.browsers.get_mut(&cli.browser) {
                let page = session::ensure_page(browser_s, &cli.page, &target_id);
                page.last_snapshot = Some(snapshot.text);
                page.uid_map = snapshot.uid_map;
            }
            if json_mode {
                json_output(&json!({
                    "ok": true,
                    "added": stats.added,
                    "removed": stats.removed,
                    "changed": stats.changed,
                    "diff": diff.trim_end(),
                }));
            } else {
                print!("{diff}");
            }
        }

        Command::Screenshot { filename } => {
            let path = commands::screenshot::run(&client, filename.as_deref()).await?;
            if json_mode {
                json_output(&json!({"ok": true, "path": path}));
            } else {
                println!("{path}");
            }
        }

        Command::Extract { selector, limit, scroll, a11y } => {
            let result = if a11y {
                commands::extract::run_a11y(&client, limit, scroll).await?
            } else {
                if scroll {
                    commands::extract::scroll_to_load(&client).await?;
                }
                commands::extract::run(&client, selector.as_deref(), limit).await?
            };
            if json_mode {
                json_output(&commands::extract::to_json(&result));
            } else {
                print!("{}", commands::extract::format_text(&result));
            }
        }

        Command::Eval { expression, selector } => {
            let expr = if let Some(ref sel) = selector {
                let escaped = serde_json::to_string(sel).unwrap_or_default();
                format!("((el) => {{ if (!el) throw new Error('No element matches selector ' + {escaped}); return {expression} }})(document.querySelector({escaped}))")
            } else {
                expression
            };
            if json_mode {
                let raw = commands::eval::run_raw(&client, &expr).await?;
                json_output(&json!({"ok": true, "result": raw}));
            } else {
                let result = commands::eval::run(&client, &expr).await?;
                println!("{result}");
            }
        }

        Command::Wait { what, pattern, timeout } => {
            let msg = commands::wait::run(&client, &what, &pattern, timeout).await?;
            if json_mode {
                json_output(&json!({"ok": true, "message": msg}));
            } else {
                println!("{msg}");
            }
        }

        Command::Type { text, selector } => {
            if let Some(ref sel) = selector {
                crate::element::focus_selector(&client, sel).await?;
            }
            crate::element::type_text(&client, &text).await?;
            let msg = if let Some(sel) = &selector {
                format!("Typed {} chars into selector '{sel}'", text.len())
            } else {
                format!("Typed {} chars", text.len())
            };
            if json_mode {
                json_output(&json!({"ok": true, "message": msg}));
            } else {
                println!("{msg}");
            }
        }

        Command::Press { key } => {
            crate::element::press_key(&client, &key).await?;
            let msg = format!("Pressed {key}");
            if json_mode {
                json_output(&json!({"ok": true, "message": msg}));
            } else {
                println!("{msg}");
            }
        }

        Command::Scroll { target, px } => {
            let msg = match target.as_str() {
                "down" => {
                    let _: serde_json::Value = client
                        .call("Runtime.evaluate", json!({
                            "expression": format!("window.scrollBy(0, {px})"),
                            "returnByValue": true,
                        }))
                        .await?;
                    format!("Scrolled down {px}px")
                }
                "up" => {
                    let _: serde_json::Value = client
                        .call("Runtime.evaluate", json!({
                            "expression": format!("window.scrollBy(0, -{px})"),
                            "returnByValue": true,
                        }))
                        .await?;
                    format!("Scrolled up {px}px")
                }
                uid => {
                    let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
                    let element_ref = uid_map.get(uid).ok_or_else(|| {
                        format!("Element uid={uid} not found. Run 'chrome-agent inspect' to get fresh uids.")
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
                                object_group: Some("chrome-agent".into()),
                                execution_context_id: None,
                            },
                        )
                        .await?;
                    let object_id = resolve_result.object.object_id.ok_or_else(|| {
                        format!("Element uid={uid} could not be resolved to a JS object.")
                    })?;
                    let _: serde_json::Value = client
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
            if json_mode {
                json_output(&json!({"ok": true, "message": msg}));
            } else {
                println!("{msg}");
            }
        }

        Command::Hover { uid } => {
            let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
            crate::element::hover(&client, &uid_map, &uid).await?;
            let msg = format!("Hovered uid={uid}");
            if json_mode {
                json_output(&json!({"ok": true, "message": msg}));
            } else {
                println!("{msg}");
            }
        }

        Command::Network { filter, body, live, limit, abort } => {
            if let Some(ref pattern) = abort {
                let timeout_secs = live.unwrap_or(30);
                let blocked = commands::network::run_route_abort(&client, pattern, timeout_secs).await?;
                if json_mode {
                    json_output(&json!({"ok": true, "blocked": blocked.len(), "urls": blocked}));
                } else {
                    println!("Blocking requests matching \"{pattern}\" for {timeout_secs}s...");
                    for url in &blocked {
                        println!("  blocked: {url}");
                    }
                    println!("Blocked {} request(s)", blocked.len());
                }
            } else {
                let entries = if let Some(secs) = live {
                    if cli.stealth { eprintln!("warning: --live enables Network domain (detectable)"); }
                    commands::network::run_live(&client, filter.as_deref(), body, limit, secs).await?
                } else {
                    commands::network::run_retroactive(&client, filter.as_deref(), limit).await?
                };
                if json_mode {
                    json_output(&json!({"ok": true, "requests": entries}));
                } else {
                    println!("{}", commands::network::format_text(&entries));
                }
            }
        }

        Command::Console { level, clear, limit } => {
            let entries = commands::console::run(&client, level.as_deref(), clear, limit).await?;
            if json_mode {
                let messages: Vec<serde_json::Value> = entries
                    .iter()
                    .map(|e| json!({"level": e.level, "message": e.message, "timestamp": e.timestamp}))
                    .collect();
                json_output(&json!({"ok": true, "messages": messages}));
            } else {
                println!("{}", commands::console::format_text(&entries));
            }
        }

        Command::Tabs => {
            if json_mode {
                let tabs = commands::tabs::run_structured(&browser_client, &store).await?;
                json_output(&json!({"ok": true, "tabs": tabs}));
            } else {
                let output = commands::tabs::run(&browser_client, &store).await?;
                print!("{output}");
            }
        }

        Command::Frame { target } => {
            let msg = commands::frame::run(&client, &target).await?;
            if json_mode {
                json_output(&json!({"ok": true, "message": msg}));
            } else {
                println!("{msg}");
            }
        }

        Command::Batch => {
            let input = {
                use std::io::Read as _;
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf)?;
                buf
            };
            let cmds = commands::batch::parse_commands(&input)?;
            for cmd in &cmds {
                let response = crate::pipe_dispatch::dispatch_single(
                    &client, &browser_client, &mut store,
                    &cli.browser, &cli.page, &target_id,
                    cli.timeout, cli.max_depth, cmd,
                ).await;
                let line = serde_json::to_string(&response).unwrap_or_default();
                println!("{line}");
            }
        }

        // Already handled above
        Command::Daemon { .. } | Command::Status | Command::Stop | Command::Close { .. }
        | Command::Pipe | Command::Replay { .. } | Command::History { .. } => {
            unreachable!()
        }
    }

    session::save_session(&mut store)?;

    Ok(())
}
