use serde_json::json;

use crate::BoxError;
use crate::cli::{Cli, Command, DaemonAction, EmulateAction};
use crate::run_helpers::{ReportPolicy, check_report, cmd_close, cmd_purge_orphans, cmd_status, cmd_stop, get_uid_map, json_output, output_action, output_action_with, output_goto};
use crate::{commands, pipe, session};

/// The biggest awaited futures in this function are `Box::pin`ned before being awaited.
///
/// Not style: `clippy::large_stack_frames` (nursery, denied in CI) sums the sizes of every MIR
/// local in a body, and each `.await` here contributes its callee's whole future as a separate
/// local. One `match cli.command` with ~40 arms therefore adds up to a number no single frame
/// ever holds — measured at 527,450 bytes against a 512,000 limit, while the real state machine
/// for this function is 8,608 bytes (`-Zprint-type-sizes`). Boxing turns the largest of those
/// locals into an 8-byte pointer for the cost of one allocation per process run, on paths that
/// are about to do network I/O anyway. Un-boxing any of them re-trips the lint.
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

        Command::Close { purge, purge_orphans, orphans } => {
            // Processes before profiles: a profile whose browser is still running is not
            // removable, so sweeping the disk first would skip exactly the directories
            // this pair is meant to reclaim.
            if orphans {
                crate::orphans::cmd_close_orphans(cli.json)?;
            }
            if purge_orphans {
                return cmd_purge_orphans(cli.json);
            }
            if orphans {
                return Ok(());
            }
            return cmd_close(&cli.browser, purge, cli.json);
        }

        Command::Pipe => {
            return Box::pin(pipe::run_pipe(&cli)).await;
        }

        Command::Replay { ref file, ref vars } => {
            return Box::pin(pipe::run_replay(&cli, file, vars.as_deref())).await;
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

    // All other commands need a browser connection + CDP client. Loading the store,
    // connecting to (or launching) the named browser, and resolving its page live in
    // `connect_cli::resolve_cli_connection` — split out for the repo's 1000-line file cap.
    let (mut store, browser_client, client, target_id) =
        Box::pin(crate::connect_cli::resolve_cli_connection(&cli)).await?;
    // The caller's own answer to "how long am I willing to wait" also bounds every CDP
    // response, so a page promise that never settles fails instead of hanging forever.
    client.set_call_timeout(std::time::Duration::from_secs(cli.timeout));
    let dialog_policy = crate::setup::DialogPolicy::parse(&cli.dialog)?;
    client.spawn_dialog_handler(dialog_policy, cli.dialog_text.clone());
    // Reapplying before `device` or `reset` would let an invalid stored configuration prevent the
    // command that repairs it. Batch defers the same decision to `run_batch`, where command order
    // determines which entries remain blocked and when recovery takes effect.
    let defers_emulation_reapply = matches!(
        &cli.command,
        Command::Emulate {
            action: EmulateAction::Device { .. } | EmulateAction::Reset,
        } | Command::Batch { .. }
    );
    if !defers_emulation_reapply
        && let Err(error) =
            crate::emulation::reapply(&client, &store, &cli.browser, &cli.page).await
    {
        // Same contract as the pipe's EmulationRecovery: the failure names the one command
        // that repairs it, with the real values filled in, and the command was NOT run —
        // acting on a page whose stored metrics silently failed to apply would report
        // results measured under the wrong viewport.
        return Err(format!(
            "Could not reapply this page's stored device configuration: {error}. \
             Clear it: chrome-agent --browser {} --page {} emulate reset",
            cli.browser, cli.page
        )
        .into());
    }

    let json_mode = cli.json;
    let policy = ReportPolicy {
        changes: cli.verdict == "auto",
        budget: cli.budget,
        on_intercept: crate::hit_test::OnIntercept::parse(&cli.on_intercept)?,
    };
    match cli.command {
        Command::Goto { url, inspect, max_depth, wait_for, headers } => {
            let depth = max_depth.or(cli.max_depth);
            let parsed_headers = headers
                .iter()
                .map(|h| commands::goto::parse_header(h))
                .collect::<Result<Vec<_>, _>>()?;
            let result = commands::goto::run(&client, &url, cli.timeout, &parsed_headers).await?;
            if let Some(ref selector) = wait_for {
                commands::wait::run(&client, "selector", selector, cli.timeout, 500).await?;
            }
            let _ = commands::history::append(&result.url, &result.title, &cli.page);
            output_goto(&client, &mut store, &cli.browser, &cli.page, &target_id, &result.url, &result.title, Some(&result.landed), inspect, depth, json_mode).await?;
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

            // The selector path resolves and reports its own node, from the same handle it
            // probes and clicks; only --xy has no element to name.
            let (msg, details) = if let Some(ref sel) = selector {
                let outcome = crate::element::click_selector(&client, sel, policy.on_intercept).await?;
                let target = format!("selector '{sel}'");
                (
                    outcome.refusal_message("click", &target).unwrap_or_else(|| format!("Clicked {target}")),
                    Some(outcome.report()),
                )
            } else if let Some(ref coords) = xy {
                if coords.len() != 2 {
                    return Err("--xy requires exactly 2 values: x,y".into());
                }
                crate::element::click_at_coords(&client, coords[0], coords[1]).await?;
                (format!("Clicked at ({}, {})", coords[0], coords[1]), None)
            } else {
                let uid = uid.as_ref().unwrap();
                let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
                let (msg, outcome) = commands::click::run(&client, &uid_map, uid, policy.on_intercept).await?;
                (msg, Some(outcome.report()))
            };

            output_action_with(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, &policy.for_action(inspect, depth), json_mode, details).await?;
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

            let target = crate::run_helpers::target_details(&client, selector.as_deref(), uid.as_deref()).await;
            let (msg, outcome) = if let Some(ref sel) = selector {
                let outcome = crate::element::fill_selector(&client, sel, &value).await?;
                (format!("Filled selector '{sel}'"), outcome)
            } else {
                let uid = uid.as_ref().unwrap();
                let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
                commands::fill::run(&client, &uid_map, uid, &value).await?
            };

            output_action_with(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, &policy.for_action(inspect, depth), json_mode, crate::run_helpers::merge_details(target, Some(json!({"value": crate::run_helpers::fill_value_report(&outcome)})))).await?;
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
            let (msg, outcomes) = commands::fill::run_form(&client, &uid_map, &parsed).await?;
            let details = Some(json!({"values": crate::run_helpers::bulk_fill_report("uid", &outcomes)}));
            output_action_with(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, &policy.for_action(inspect, depth), json_mode, details).await?;
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
            // Same as goto: the old document is gone, so every stored uid now points at a
            // node that no longer exists, and backendNodeId counters overlap between
            // documents so a stale one can silently resolve to an unrelated element.
            if let Some(browser_s) = store.browsers.get_mut(&cli.browser) {
                session::ensure_page(browser_s, &cli.page, &target_id).uid_map.clear();
            }
            if json_mode {
                json_output(&json!({"ok": true, "title": title_str}));
            } else {
                println!("Navigated back — {title_str}");
            }
        }

        Command::Forward { inspect, max_depth } => {
            let depth = max_depth.or(cli.max_depth);
            let history: serde_json::Value = client
                .call("Page.getNavigationHistory", json!({}))
                .await?;
            let current_index = history.get("currentIndex").and_then(serde_json::Value::as_i64).unwrap_or(0);
            let entries = history.get("entries").and_then(serde_json::Value::as_array);
            let entry_count = entries.map_or(0, Vec::len) as i64;
            if current_index >= entry_count - 1 {
                let msg = "Already at last history entry".to_string();
                if json_mode {
                    json_output(&json!({"ok": true, "title": "", "message": msg}));
                } else {
                    println!("{msg}");
                }
            } else {
                let next_entry_id = entries
                    .and_then(|e| e.get(usize::try_from(current_index + 1).unwrap_or(0)))
                    .and_then(|e| e.get("id"))
                    .and_then(serde_json::Value::as_i64)
                    .ok_or("Could not find next history entry")?;
                client.send("Page.navigateToHistoryEntry", json!({"entryId": next_entry_id})).await?;
                let _ = client.wait_for_event("Page.loadEventFired", std::time::Duration::from_secs(5)).await;
                let dest: crate::cdp::types::EvaluateResult = client
                    .call("Runtime.evaluate", json!({"expression": "({title: document.title, url: location.href})", "returnByValue": true}))
                    .await?;
                let title_str = dest.result.value.as_ref().and_then(|v| v.get("title")).and_then(|v| v.as_str()).unwrap_or("");
                let url_str = dest.result.value.as_ref().and_then(|v| v.get("url")).and_then(|v| v.as_str()).unwrap_or("");
                // A navigation, so it answers like `goto`: no change report (the caller
                // navigated on purpose, and pipe/batch never attach one), stale uids
                // dropped, `--inspect` refills them from the destination.
                //
                // No `landed`: the caller asked for the next history entry, not for a URL,
                // so there is no requested URL to have been redirected away from.
                output_goto(&client, &mut store, &cli.browser, &cli.page, &target_id, url_str, title_str, None, inspect, depth, json_mode).await?;
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

            let (msg, details) = if let Some(ref sel) = selector {
                let outcome = crate::element::dblclick_selector(&client, sel, policy.on_intercept).await?;
                let target = format!("selector '{sel}'");
                (
                    outcome
                        .refusal_message("double-click", &target)
                        .unwrap_or_else(|| format!("Double-clicked {target}")),
                    Some(outcome.report()),
                )
            } else if let Some(ref coords) = xy {
                if coords.len() != 2 {
                    return Err("--xy requires exactly 2 values: x,y".into());
                }
                crate::element::dblclick_at_coords(&client, coords[0], coords[1]).await?;
                (format!("Double-clicked at ({}, {})", coords[0], coords[1]), None)
            } else {
                let uid = uid.as_ref().unwrap();
                let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
                let (msg, outcome) = commands::dblclick::run(&client, &uid_map, uid, policy.on_intercept).await?;
                (msg, Some(outcome.report()))
            };

            output_action_with(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, &policy.for_action(inspect, depth), json_mode, details).await?;
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

            let target = crate::run_helpers::target_details(&client, selector.as_deref(), uid.as_deref()).await;
            let (msg, outcome) = if let Some(ref sel) = selector {
                let outcome = crate::element::select_option_selector(&client, sel, &value).await?;
                (format!("Selected \"{}\" on selector '{sel}'", outcome.label()), outcome)
            } else {
                let uid = uid.as_ref().unwrap();
                let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
                let outcome = commands::select::run(&client, &uid_map, uid, &value).await?;
                (format!("Selected \"{}\" on uid={uid}", outcome.label()), outcome)
            };
            let details = Some(crate::run_helpers::select_report(&outcome));

            output_action_with(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, &policy.for_action(inspect, depth), json_mode, crate::run_helpers::merge_details(target, details)).await?;
        }

        Command::Check { uid, selector, inspect, max_depth } => {
            let depth = max_depth.or(cli.max_depth);
            if uid.is_none() && selector.is_none() {
                return Err("Provide a uid or --selector.".into());
            }
            let target = crate::run_helpers::target_details(&client, selector.as_deref(), uid.as_deref()).await;
            let outcome = if let Some(ref sel) = selector {
                crate::element::set_checked_selector(&client, sel, true).await?
            } else {
                let uid = uid.as_ref().unwrap();
                let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
                commands::check::run(&client, &uid_map, uid, true, policy.on_intercept).await?
            };
            let (msg, details) = check_report(outcome);
            output_action_with(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, &policy.for_action(inspect, depth), json_mode, crate::run_helpers::merge_details(target, details)).await?;
        }

        Command::Uncheck { uid, selector, inspect, max_depth } => {
            let depth = max_depth.or(cli.max_depth);
            if uid.is_none() && selector.is_none() {
                return Err("Provide a uid or --selector.".into());
            }
            let target = crate::run_helpers::target_details(&client, selector.as_deref(), uid.as_deref()).await;
            let outcome = if let Some(ref sel) = selector {
                crate::element::set_checked_selector(&client, sel, false).await?
            } else {
                let uid = uid.as_ref().unwrap();
                let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
                commands::check::run(&client, &uid_map, uid, false, policy.on_intercept).await?
            };
            let (msg, details) = check_report(outcome);
            output_action_with(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, &policy.for_action(inspect, depth), json_mode, crate::run_helpers::merge_details(target, details)).await?;
        }

        Command::Upload { files, uid, selector, inspect, max_depth } => {
            let depth = max_depth.or(cli.max_depth);
            if uid.is_none() && selector.is_none() {
                return Err("Provide --uid or --selector to identify the file input.".into());
            }
            let target = crate::run_helpers::target_details(&client, selector.as_deref(), uid.as_deref()).await;
            let msg = if let Some(ref sel) = selector {
                crate::element::set_file_input_selector(&client, sel, &files).await?;
                format!("Uploaded {} file(s) to selector '{sel}'", files.len())
            } else {
                let uid = uid.as_ref().unwrap();
                let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
                commands::upload::run(&client, &uid_map, uid, &files).await?
            };
            output_action_with(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, &policy.for_action(inspect, depth), json_mode, target).await?;
        }

        Command::Drag { from, to, inspect, max_depth } => {
            let depth = max_depth.or(cli.max_depth);
            let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
            let msg = commands::drag::run(&client, &uid_map, &from, &to).await?;
            output_action(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, &policy.for_action(inspect, depth), json_mode).await?;
        }

        Command::Inspect { verbose, max_depth, uid, filter, scroll, limit, urls, max_chars, offset } => {
            if scroll {
                commands::extract::scroll_to_load(&client).await?;
            }
            let role_filter: Option<Vec<&str>> = filter.as_deref().map(|f| f.split(',').map(str::trim).collect());
            let (mut text, uid_map, doc_identity) = if let Some(max) = limit {
                let result = commands::inspect::scroll_collect(&client, verbose, uid.as_deref(), role_filter.as_deref(), max).await?;
                (result.text, result.uid_map, result.identity)
            } else {
                let s = commands::inspect::run(&client, verbose, max_depth, uid.as_deref(), role_filter.as_deref()).await?;
                (s.text, s.uid_map, s.identity)
            };
            if urls {
                text = commands::inspect::resolve_urls(&client, &text, &uid_map).await;
            }
            // Persist the FULL snapshot so diff and uid lookups stay complete;
            // paging only affects what we print/return.
            if let Some(browser_s) = store.browsers.get_mut(&cli.browser) {
                let page = session::ensure_page(browser_s, &cli.page, &target_id);
                page.uid_map = uid_map;
                page.last_snapshot = Some(text.clone());
                let (f, l) = doc_identity.map_or((None, None), |(f, l)| (Some(f), Some(l)));
                page.last_snapshot_frame = f;
                page.last_snapshot_loader = l;
            }
            let paged = commands::inspect::paginate(&text, offset, max_chars);
            if json_mode {
                json_output(&json!({
                    "ok": true,
                    "snapshot": paged.text,
                    "total_chars": paged.total_chars,
                    "truncated": paged.truncated,
                    "next_offset": paged.next_offset,
                }));
            } else {
                println!("{}", paged.text);
            }
        }

        Command::Diff => {
            let page_state = store
                .browsers
                .get(&cli.browser)
                .and_then(|b| b.pages.get(&cli.page));
            let old_text = page_state
                .and_then(|p| p.last_snapshot.clone())
                .ok_or("No previous snapshot. Run 'chrome-agent inspect' first.")?;
            let stored = page_state.and_then(|p| {
                p.last_snapshot_frame.clone().zip(p.last_snapshot_loader.clone())
            });
            let snapshot = commands::inspect::run(&client, false, None, None, None).await?;
            let identity = commands::diff::Identity::from_loader(
                stored.as_ref().map(|(f, l)| (f.as_str(), l.as_str())),
                snapshot.identity.as_ref().map(|(f, l)| (f.as_str(), l.as_str())),
            );
            let result = commands::diff::compare(identity, &old_text, &snapshot.text);
            if let Some(browser_s) = store.browsers.get_mut(&cli.browser) {
                let page = session::ensure_page(browser_s, &cli.page, &target_id);
                page.last_snapshot = Some(snapshot.text);
            let (f, l) = snapshot.identity.map_or((None, None), |(f, l)| (Some(f), Some(l)));
            page.last_snapshot_frame = f;
            page.last_snapshot_loader = l;
                page.uid_map = snapshot.uid_map;
            }
            if json_mode {
                let mut obj = json!({
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
                    obj["focus"] = json!({"from": result.focus_from, "to": result.focus_to});
                }
                if let Some(hint) = result.hint {
                    obj["hint"] = json!(hint);
                }
                json_output(&obj);
            } else {
                if result.document_changed {
                    println!("Page navigated — previous uids are gone. New page:");
                }
                print!("{}", result.text);
            }
        }

        Command::Screenshot { filename, format, quality, max_width, uid, selector } => {
            if uid.is_some() && selector.is_some() {
                return Err("Provide only one of uid or --selector for an element screenshot.".into());
            }
            let clip = if let Some(ref u) = uid {
                let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
                Some(crate::geometry::clip_for_uid(&client, &uid_map, u).await?)
            } else if let Some(ref sel) = selector {
                Some(crate::geometry::clip_for_selector(&client, sel).await?)
            } else {
                None
            };
            let opts = commands::screenshot::ScreenshotOpts {
                filename: filename.as_deref(),
                format: commands::screenshot::ImgFormat::parse(&format)?,
                quality,
                max_width,
                clip,
            };
            let path = commands::screenshot::run(&client, &opts).await?;
            if json_mode {
                json_output(&json!({"ok": true, "path": path}));
            } else {
                println!("{path}");
            }
        }

        Command::Download { url, out, timeout, max_bytes } => {
            let result = commands::download::run(&client, &url, out.as_deref(), timeout, max_bytes).await?;
            if json_mode {
                json_output(&json!({
                    "ok": true,
                    "path": result.path,
                    "bytes": result.bytes,
                    "mime": result.mime,
                }));
            } else {
                println!("{} ({} bytes, {})", result.path, result.bytes, result.mime);
            }
        }

        Command::Pdf { filename, landscape, background } => {
            let opts = commands::pdf::PdfOpts {
                filename: filename.as_deref(),
                landscape,
                background,
            };
            let path = commands::pdf::run(&client, &opts).await?;
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

        Command::Wait { what, pattern, timeout, idle_ms } => {
            let msg = commands::wait::run(&client, &what, &pattern, timeout, idle_ms).await?;
            // Not in `mutates_page`, so pipe/batch answer plainly — the CLI must too.
            if json_mode {
                json_output(&json!({"ok": true, "message": msg}));
            } else {
                println!("{msg}");
            }
        }

        Command::Assert { ref what } => {
            // A read, so no change report and no verdict: nothing moved. `run_cli` returns
            // `commands::assert::NotHeld` when the claim did not hold, which `main` turns
            // into exit 2 before its generic error path (which would say 1).
            let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
            commands::assert::run_cli(&client, &uid_map, what, json_mode).await?;
        }

        Command::Type { text, selector } => {
            if let Some(ref sel) = selector {
                crate::element::focus_selector(&client, sel).await?;
            }
            crate::element::require_editable_focus(&client).await?;
            crate::element::type_text(&client, &text).await?;
            let msg = if let Some(sel) = &selector {
                format!("Typed {} chars into selector '{sel}'", text.len())
            } else {
                format!("Typed {} chars", text.len())
            };
            output_action(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, &policy.for_action(false, None), json_mode).await?;
        }

        Command::Press { key } => {
            crate::element::press_key(&client, &key).await?;
            let msg = format!("Pressed {key}");
            output_action(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, &policy.for_action(false, None), json_mode).await?;
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
            output_action(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, &policy.for_action(false, None), json_mode).await?;
        }

        Command::Hover { uid } => {
            let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
            crate::element::hover(&client, &uid_map, &uid).await?;
            let msg = format!("Hovered uid={uid}");
            output_action(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, &policy.for_action(false, None), json_mode).await?;
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
            // Not in `mutates_page`, so pipe/batch answer plainly — the CLI must too.
            if json_mode {
                json_output(&json!({"ok": true, "message": msg}));
            } else {
                println!("{msg}");
            }
        }

        Command::Emulate { action } => {
            match action {
                EmulateAction::Device { label, width, height, dpr, mobile, touch, orientation } => {
                    let config = crate::emulation::DeviceEmulation::new(
                        label, width, height, dpr, mobile, touch, orientation,
                    )?;
                    let response = crate::emulation::apply_and_store(
                        &client, &mut store, &cli.browser, &cli.page, config.clone(),
                    ).await?;
                    session::save_session(&mut store)?;
                    if json_mode {
                        json_output(&response);
                    } else {
                        println!("{}", config.text_line());
                        println!(
                            "{}",
                            crate::emulation::format_effective_metrics(&response["effective"])
                        );
                    }
                }
                EmulateAction::Status => {
                    let requested_line = store.browsers.get(&cli.browser)
                        .and_then(|browser| browser.pages.get(&cli.page))
                        .and_then(|page| page.device_emulation.as_ref())
                        .map(crate::emulation::DeviceEmulation::text_line);
                    let response = crate::emulation::status(
                        &client, &store, &cli.browser, &cli.page,
                    ).await?;
                    if json_mode {
                        json_output(&response);
                    } else if let Some(requested_line) = requested_line {
                        println!("{requested_line}");
                        println!("{}", crate::emulation::format_effective_metrics(&response["effective"]));
                    } else {
                        println!("No device emulation on page={:?}.", cli.page);
                    }
                }
                EmulateAction::Reset => {
                    let response = crate::emulation::clear(
                        &client, &mut store, &cli.browser, &cli.page,
                    ).await?;
                    session::save_session(&mut store)?;
                    if json_mode {
                        json_output(&response);
                    } else {
                        println!("Cleared device emulation from page={:?}.", cli.page);
                    }
                }
            }
        }

        Command::Batch { stop_on_error } => {
            let input = {
                use std::io::Read as _;
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf)?;
                buf
            };
            let cmds = commands::batch::parse_commands(&input)?;
            // The same loop pipe mode's `{"cmd":"batch"}` runs, so `--stop-on-error` and
            // `"stop_on_error"` cannot drift apart. One recovery state is shared by every entry;
            // a reset can repair later commands without reapplying before each one.
            let mut emulation_recovery = crate::pipe_dispatch::EmulationRecovery::new(
                &client, &store, &cli.browser, &cli.page,
            ).await;
            let out = crate::pipe_dispatch::run_batch(
                &client, &browser_client, &mut store,
                &cli.browser, &cli.page, &target_id,
                cli.timeout, cli.max_depth, policy, &cmds, stop_on_error,
                &mut emulation_recovery,
            ).await;
            json_output(&out);
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
