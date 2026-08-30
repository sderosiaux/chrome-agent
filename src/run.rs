use serde_json::{Value, json};

use crate::BoxError;
use crate::cli::{Cli, Command, DaemonAction, EmulateAction, WebmcpAction};
use crate::page_ctx::PageCtx;
use crate::pipe_command::{
    EvalArgs, FillArgs, FillFormArgs, FrameArgs, HoverArgs, InspectArgs, PairArgs, PointerArgs,
    PressArgs, ReadArgs, ScreenshotArgs, ScrollArgs, TextArgs, TypeArgs, UploadArgs, ValueArgs,
    WaitArgs,
};
use crate::pipe_dispatch as dispatch;
use crate::run_helpers::{
    ActionReport, ReportPolicy, cmd_close, cmd_purge_orphans, cmd_status, cmd_stop, json_output,
    output_action_with, output_goto, print_batch_text,
};
use crate::{commands, pipe, session};

/// The CLI rendering of a dispatcher's response: `message` is the action's sentence and
/// everything else it observed rides as details through [`output_action_with`], which owns the
/// change report, `--inspect` and the text output.
///
/// The CLI never asks a dispatcher for `inspect`. `output_action_with` takes ONE reading and
/// renders it twice — the baseline at full depth, the display at `--max-depth` — where a
/// dispatcher's own `inspect` would take a second one.
async fn output_dispatched(
    ctx: &mut PageCtx<'_>,
    mut value: Value,
    report: &ActionReport,
    json_mode: bool,
) -> Result<(), BoxError> {
    let msg = value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let details = value
        .as_object_mut()
        .map(|map| {
            map.remove("ok");
            map.remove("message");
            std::mem::take(map)
        })
        .filter(|map| !map.is_empty())
        .map(Value::Object);
    output_action_with(ctx, msg, report, json_mode, details).await
}

/// A read verb's response: JSON as-is, or the one string text mode prints for it.
fn output_read(json_mode: bool, value: &Value, text: &str) {
    if json_mode {
        json_output(value);
    } else {
        println!("{text}");
    }
}

/// One step through the history stack, in the CLI's words. The response is
/// `dispatch::history_step`'s, so all three modes answer the same object at the boundary; text
/// mode reads `url`'s presence to tell a navigation from a stated non-event.
async fn output_history_step(
    ctx: &mut PageCtx<'_>,
    delta: i64,
    inspect: bool,
    max_depth: Option<usize>,
    json_mode: bool,
) -> Result<(), BoxError> {
    let mut out = Box::pin(dispatch::history_step(ctx, delta)).await?;
    // `--inspect` refills the map the step just cleared, as `goto --inspect` does.
    if inspect && out.get("url").is_some() {
        out["snapshot"] = json!(Box::pin(dispatch::attach_snapshot(ctx, max_depth)).await?);
    }
    if json_mode {
        json_output(&out);
        return Ok(());
    }
    let field = |key: &str| {
        out.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    match out.get("url") {
        Some(_) => println!("{} — {}", field("url"), field("title")),
        None => println!("{}", field("message")),
    }
    if let Some(snapshot) = out.get("snapshot").and_then(Value::as_str) {
        println!("{snapshot}");
    }
    Ok(())
}

/// CLI command dispatch.
///
/// Every arm that also exists in pipe mode calls the SAME `pipe_dispatch::dispatch_*`, builds
/// its typed args from clap and renders the response. What is left in each arm is the rendering
/// — the one thing the two front ends genuinely do not share, since `--json` is a CLI flag and
/// pipe mode is JSON by definition.
///
/// The biggest awaited futures here are `Box::pin`ned. Required, not style:
/// `clippy::large_stack_frames` (denied in CI) sums every MIR local, and one `.await` per
/// match arm summed past the 512,000-byte limit even though the real state machine is a
/// fraction of that. Un-boxing any of them re-trips the lint.
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
                        return Err(
                            "Daemon is not supported on Windows. Commands work without a daemon."
                                .into(),
                        );
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

        Command::Close {
            purge,
            purge_orphans,
            orphans,
        } => {
            // Processes before profiles: a profile whose browser still runs is not
            // removable, so sweeping the disk first would skip the directories this pair
            // exists to reclaim.
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

        // Handled before a browser is opened: list/show/record only touch files, and
        // launching a Chrome to read a directory would be absurd.
        Command::Macro { ref action } => {
            return Box::pin(crate::macros_cmd::run_cli(&cli, action)).await;
        }

        Command::Replay { ref file, ref vars } => {
            return Box::pin(pipe::run_replay(&cli, file, vars.as_deref())).await;
        }

        // Reads a file, not a page. One reading, two renderings — `to_json` is the shape the
        // pipe's `history` answers with too.
        Command::History { ref filter, limit } => {
            let entries = commands::history::run(filter.as_deref(), limit)?;
            if cli.json {
                json_output(&commands::history::to_json(&entries));
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

    // Every other command needs a browser and a CDP client.
    let (mut store, browser_client, client, target_id) =
        Box::pin(crate::connect_cli::resolve_cli_connection(&cli)).await?;
    // --timeout also bounds every CDP call, so a page promise that never settles fails
    // instead of hanging forever.
    client.set_call_timeout(std::time::Duration::from_secs(cli.timeout));
    let dialog_policy = crate::setup::DialogPolicy::parse(&cli.dialog)?;
    client.spawn_dialog_handler(dialog_policy, cli.dialog_text.clone());
    // Reapplying before `device` or `reset` would let an invalid stored configuration block
    // the command that repairs it. Batch defers the same decision to `run_batch`.
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
        // Same contract as the pipe's EmulationRecovery: name the repairing command with
        // real values, and do NOT run this one — its results would be measured under the
        // wrong viewport.
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
    // The page every arm below acts on. `store` lives in here from now on: reach it as
    // `ctx.store`.
    let mut ctx = PageCtx {
        client: &client,
        browser_client: &browser_client,
        store: &mut store,
        browser: &cli.browser,
        page: &cli.page,
        target_id: &target_id,
        timeout: cli.timeout,
        max_depth: cli.max_depth,
        report: policy,
    };
    match cli.command {
        // `goto` is the one navigation verb that does NOT route through its dispatcher: text
        // mode renders `landed` from the `Landing` struct (`text_line`), and the JSON the
        // dispatcher returns has already flattened it. `output_goto` owns both renderings.
        Command::Goto {
            url,
            inspect,
            max_depth,
            wait_for,
            headers,
        } => {
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
            output_goto(
                &mut ctx,
                &result.url,
                &result.title,
                Some(&result.landed),
                inspect,
                depth,
                json_mode,
            )
            .await?;
        }

        Command::Click {
            uid,
            selector,
            xy,
            inspect,
            max_depth,
        } => {
            let args = PointerArgs {
                uid,
                selector,
                xy,
                on_intercept: None,
                inspect: None,
                max_depth: None,
            };
            let out = Box::pin(dispatch::dispatch_click(&mut ctx, &args)).await?;
            let report = policy.for_action(inspect, max_depth.or(cli.max_depth));
            Box::pin(output_dispatched(&mut ctx, out, &report, json_mode)).await?;
        }

        Command::Dblclick {
            uid,
            selector,
            xy,
            inspect,
            max_depth,
        } => {
            let args = PointerArgs {
                uid,
                selector,
                xy,
                on_intercept: None,
                inspect: None,
                max_depth: None,
            };
            let out = Box::pin(dispatch::dispatch_dblclick(&mut ctx, &args)).await?;
            let report = policy.for_action(inspect, max_depth.or(cli.max_depth));
            Box::pin(output_dispatched(&mut ctx, out, &report, json_mode)).await?;
        }

        Command::Fill {
            value,
            uid,
            selector,
            secret,
            inspect,
            max_depth,
        } => {
            let args = FillArgs {
                value,
                uid,
                selector,
                secret: Some(secret),
                inspect: None,
                max_depth: None,
            };
            let out = Box::pin(dispatch::dispatch_fill(&mut ctx, &args)).await?;
            let report = policy.for_action(inspect, max_depth.or(cli.max_depth));
            Box::pin(output_dispatched(&mut ctx, out, &report, json_mode)).await?;
        }

        Command::FillForm {
            pairs,
            inspect,
            max_depth,
        } => {
            let parsed: Vec<PairArgs> = pairs
                .iter()
                .map(|p| {
                    let (uid, value) = p
                        .split_once('=')
                        .ok_or_else(|| format!("Invalid pair (expected uid=value): {p}"))?;
                    Ok::<_, BoxError>(PairArgs {
                        uid: Some(uid.to_string()),
                        value: Some(value.to_string()),
                    })
                })
                .collect::<Result<_, _>>()?;
            let args = FillFormArgs {
                pairs: Some(parsed),
                inspect: None,
                max_depth: None,
            };
            let out = Box::pin(dispatch::dispatch_fill_form(&mut ctx, &args)).await?;
            let report = policy.for_action(inspect, max_depth.or(cli.max_depth));
            Box::pin(output_dispatched(&mut ctx, out, &report, json_mode)).await?;
        }

        Command::Select {
            value,
            uid,
            selector,
            inspect,
            max_depth,
        } => {
            let args = ValueArgs {
                value,
                uid,
                selector,
                inspect: None,
                max_depth: None,
            };
            let out = Box::pin(dispatch::dispatch_select(&mut ctx, &args)).await?;
            let report = policy.for_action(inspect, max_depth.or(cli.max_depth));
            Box::pin(output_dispatched(&mut ctx, out, &report, json_mode)).await?;
        }

        Command::Check {
            uid,
            selector,
            inspect,
            max_depth,
        } => {
            let out = Box::pin(dispatch::dispatch_check(
                &ctx,
                true,
                uid.as_deref(),
                selector.as_deref(),
                None,
            ))
            .await?;
            let report = policy.for_action(inspect, max_depth.or(cli.max_depth));
            Box::pin(output_dispatched(&mut ctx, out, &report, json_mode)).await?;
        }

        Command::Uncheck {
            uid,
            selector,
            inspect,
            max_depth,
        } => {
            let out = Box::pin(dispatch::dispatch_check(
                &ctx,
                false,
                uid.as_deref(),
                selector.as_deref(),
                None,
            ))
            .await?;
            let report = policy.for_action(inspect, max_depth.or(cli.max_depth));
            Box::pin(output_dispatched(&mut ctx, out, &report, json_mode)).await?;
        }

        Command::Upload {
            files,
            uid,
            selector,
            inspect,
            max_depth,
        } => {
            let args = UploadArgs {
                files: Some(files),
                uid,
                selector,
            };
            let out = Box::pin(dispatch::dispatch_upload(&ctx, &args)).await?;
            let report = policy.for_action(inspect, max_depth.or(cli.max_depth));
            Box::pin(output_dispatched(&mut ctx, out, &report, json_mode)).await?;
        }

        Command::Drag {
            from,
            to,
            inspect,
            max_depth,
        } => {
            let args = crate::pipe_command::DragArgs {
                from: Some(from),
                to: Some(to),
            };
            let out = Box::pin(dispatch::dispatch_drag(&ctx, &args)).await?;
            let report = policy.for_action(inspect, max_depth.or(cli.max_depth));
            Box::pin(output_dispatched(&mut ctx, out, &report, json_mode)).await?;
        }

        Command::Hover { uid } => {
            let out = Box::pin(dispatch::dispatch_hover(
                &ctx,
                &HoverArgs { uid: Some(uid) },
            ))
            .await?;
            let report = policy.for_action(false, None);
            Box::pin(output_dispatched(&mut ctx, out, &report, json_mode)).await?;
        }

        Command::Type {
            text,
            selector,
            secret,
        } => {
            let args = TypeArgs {
                text,
                selector,
                secret: Some(secret),
            };
            let out = Box::pin(dispatch::dispatch_type(&client, &args)).await?;
            let report = policy.for_action(false, None);
            Box::pin(output_dispatched(&mut ctx, out, &report, json_mode)).await?;
        }

        Command::Press { key } => {
            let out = Box::pin(dispatch::dispatch_press(&client, &PressArgs { key })).await?;
            let report = policy.for_action(false, None);
            Box::pin(output_dispatched(&mut ctx, out, &report, json_mode)).await?;
        }

        Command::Scroll { target, px } => {
            let args = ScrollArgs {
                target,
                px: Some(px),
            };
            let out = Box::pin(dispatch::dispatch_scroll(&ctx, &args)).await?;
            let report = policy.for_action(false, None);
            Box::pin(output_dispatched(&mut ctx, out, &report, json_mode)).await?;
        }

        Command::Back { inspect, max_depth } => {
            let depth = max_depth.or(cli.max_depth);
            output_history_step(&mut ctx, -1, inspect, depth, json_mode).await?;
        }

        Command::Forward { inspect, max_depth } => {
            let depth = max_depth.or(cli.max_depth);
            output_history_step(&mut ctx, 1, inspect, depth, json_mode).await?;
        }

        Command::Text {
            uid,
            selector,
            truncate,
        } => {
            let args = TextArgs {
                uid,
                selector,
                truncate: truncate.map(|n| n as u64),
            };
            let out = dispatch::dispatch_text(&ctx, &args).await?;
            let text = out["text"].as_str().unwrap_or_default().to_string();
            output_read(json_mode, &out, &text);
        }

        Command::Read { html, truncate } => {
            let args = ReadArgs {
                html: Some(html),
                truncate: truncate.map(|n| n as u64),
            };
            let out = Box::pin(dispatch::dispatch_read(&client, &args)).await?;
            if json_mode {
                json_output(&out);
            } else {
                let field = |key: &str| out.get(key).and_then(Value::as_str).unwrap_or_default();
                let title = field("title");
                if !title.is_empty() {
                    println!("# {title}");
                    println!();
                }
                // `--html` prints nothing when Readability produced no content, as it always
                // did; the plain-text branch always has something to print.
                if html {
                    if let Some(content) = out.get("content").and_then(Value::as_str) {
                        println!("{content}");
                    }
                } else {
                    println!("{}", field("text"));
                }
            }
        }

        Command::Inspect {
            verbose,
            max_depth,
            uid,
            filter,
            scroll,
            limit,
            urls,
            max_chars,
            offset,
        } => {
            let args = InspectArgs {
                uid,
                filter,
                verbose: Some(verbose),
                scroll: Some(scroll),
                urls: Some(urls),
                limit: limit.map(|n| n as u64),
                max_depth: max_depth.map(|n| n as u64),
                offset: Some(offset as u64),
                max_chars: max_chars.map(|n| n as u64),
            };
            let out = Box::pin(dispatch::dispatch_inspect(&mut ctx, &args)).await?;
            let text = out["snapshot"].as_str().unwrap_or_default().to_string();
            output_read(json_mode, &out, &text);
        }

        Command::Diff => {
            let out = Box::pin(dispatch::dispatch_diff(&mut ctx)).await?;
            if json_mode {
                json_output(&out);
            } else {
                if out["document_changed"] == json!(true) {
                    println!("Page navigated — previous uids are gone. New page:");
                }
                let body = out["diff"].as_str().unwrap_or_default();
                if !body.is_empty() {
                    println!("{body}");
                }
            }
        }

        Command::Screenshot {
            filename,
            format,
            quality,
            max_width,
            uid,
            selector,
        } => {
            let args = ScreenshotArgs {
                filename,
                uid,
                selector,
                format: Some(format),
                quality: quality.map(u64::from),
                max_width: max_width.map(u64::from),
            };
            let out = Box::pin(dispatch::dispatch_screenshot(&ctx, &args)).await?;
            let path = out["path"].as_str().unwrap_or_default().to_string();
            output_read(json_mode, &out, &path);
        }

        Command::Download {
            url,
            uid,
            selector,
            out,
            timeout,
            max_bytes,
        } => {
            let target = commands::download::Target::parse(
                url.as_deref(),
                uid.as_deref(),
                selector.as_deref(),
            )?;
            let uid_map = ctx.uid_map();
            let req = commands::download::Request {
                target: &target,
                out: out.as_deref(),
                timeout_secs: timeout,
                max_bytes,
                on_intercept: policy.on_intercept,
                browser: &cli.browser,
            };
            let outcome = Box::pin(commands::download::dispatch(&client, &uid_map, &req)).await?;
            if json_mode {
                json_output(&outcome.to_json());
            } else {
                outcome.print_text();
            }
        }

        Command::Pdf {
            filename,
            landscape,
            background,
        } => {
            let args = crate::pipe_command::PdfArgs {
                filename,
                landscape: Some(landscape),
                background: Some(background),
            };
            let out = Box::pin(dispatch::dispatch_pdf(&client, &args)).await?;
            let path = out["path"].as_str().unwrap_or_default().to_string();
            output_read(json_mode, &out, &path);
        }

        // `collect` is shared with the dispatcher; only the two renderings differ, and
        // `format_text` reads the record structs the JSON has already flattened.
        Command::Extract {
            selector,
            limit,
            scroll,
            a11y,
        } => {
            let result = Box::pin(commands::extract::collect(
                &client,
                selector.as_deref(),
                limit,
                scroll,
                a11y,
            ))
            .await?;
            if json_mode {
                json_output(&commands::extract::to_json(&result));
            } else {
                print!("{}", commands::extract::format_text(&result));
            }
        }

        // Two readers, deliberately: `run_raw` answers the JSON and `run` the display string,
        // which differ on a result with no value (`undefined` rather than `null`). The
        // expression they evaluate is one function.
        Command::Eval {
            expression,
            selector,
        } => {
            if json_mode {
                let args = EvalArgs {
                    expression,
                    selector,
                };
                json_output(&Box::pin(dispatch::dispatch_eval(&client, &args)).await?);
            } else {
                let expr = commands::eval::scoped_expression(&expression, selector.as_deref());
                println!("{}", commands::eval::run(&client, &expr).await?);
            }
        }

        Command::Wait {
            what,
            pattern,
            timeout,
            idle_ms,
        } => {
            let args = WaitArgs {
                what: Some(what),
                pattern: Some(pattern),
                text: None,
                url: None,
                selector: None,
                timeout: Some(timeout),
                idle_ms: Some(idle_ms),
            };
            let out = Box::pin(dispatch::dispatch_wait(&client, &args)).await?;
            let msg = out["message"].as_str().unwrap_or_default().to_string();
            output_read(json_mode, &out, &msg);
        }

        Command::Assert { ref what } => {
            // A read: no change report, no verdict. `run_cli` returns `assert::NotHeld` when
            // the claim did not hold, which `main` turns into exit 2.
            let uid_map = ctx.uid_map();
            Box::pin(commands::assert::run_cli(
                &client, &uid_map, what, json_mode,
            ))
            .await?;
        }

        Command::Network {
            filter,
            body,
            live,
            limit,
            abort,
        } => {
            if abort.is_none() && live.is_some() && cli.stealth {
                eprintln!("warning: --live enables Network domain (detectable)");
            }
            let capture = Box::pin(commands::network::collect(
                &client,
                filter.as_deref(),
                body,
                live,
                limit,
                abort.as_deref(),
            ))
            .await?;
            if json_mode {
                json_output(&capture.to_json());
            } else {
                println!("{}", capture.text());
            }
        }

        // One reading, two renderings. `to_json` is the shape the dispatcher answers with too;
        // `format_text` reads the `ConsoleReading` the JSON has already flattened.
        Command::Console {
            level,
            clear,
            limit,
        } => {
            let reading = Box::pin(commands::console::run(
                &client,
                level.as_deref(),
                clear,
                limit,
            ))
            .await?;
            if json_mode {
                json_output(&commands::console::to_json(&reading));
            } else {
                println!("{}", commands::console::format_text(&reading));
            }
        }

        Command::Tabs => {
            if json_mode {
                json_output(&dispatch::dispatch_tabs(&ctx).await?);
            } else {
                print!(
                    "{}",
                    commands::tabs::run(ctx.browser_client, ctx.store).await?
                );
            }
        }

        Command::Frame { target } => {
            let out = dispatch::dispatch_frame(&client, &FrameArgs { target }).await?;
            let msg = out["message"].as_str().unwrap_or_default().to_string();
            output_read(json_mode, &out, &msg);
        }

        // Not routed through `dispatch_emulate`: clap has already parsed these into typed
        // fields, and the pipe's parser exists to turn untyped JSON into the same values.
        // Going through it would re-serialise a parse that already succeeded.
        Command::Emulate { action } => match action {
            EmulateAction::Device {
                label,
                width,
                height,
                dpr,
                mobile,
                touch,
                orientation,
            } => {
                let config = crate::emulation::DeviceEmulation::new(
                    label,
                    width,
                    height,
                    dpr,
                    mobile,
                    touch,
                    orientation,
                )?;
                let response = crate::emulation::apply_and_store(
                    &client,
                    ctx.store,
                    &cli.browser,
                    &cli.page,
                    config.clone(),
                )
                .await?;
                session::save_session(ctx.store)?;
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
                let requested_line = ctx
                    .store
                    .browsers
                    .get(&cli.browser)
                    .and_then(|browser| browser.pages.get(&cli.page))
                    .and_then(|page| page.device_emulation.as_ref())
                    .map(crate::emulation::DeviceEmulation::text_line);
                let response =
                    crate::emulation::status(&client, ctx.store, &cli.browser, &cli.page).await?;
                if json_mode {
                    json_output(&response);
                } else if let Some(requested_line) = requested_line {
                    println!("{requested_line}");
                    println!(
                        "{}",
                        crate::emulation::format_effective_metrics(&response["effective"])
                    );
                } else {
                    println!("No device emulation on page={:?}.", cli.page);
                }
            }
            EmulateAction::Reset => {
                let response =
                    crate::emulation::clear(&client, ctx.store, &cli.browser, &cli.page).await?;
                session::save_session(ctx.store)?;
                if json_mode {
                    json_output(&response);
                } else {
                    println!("Cleared device emulation from page={:?}.", cli.page);
                }
            }
        },

        Command::Webmcp { action } => {
            match action {
                WebmcpAction::List => {
                    let out = Box::pin(dispatch::dispatch_webmcp_list(&client)).await?;
                    if json_mode {
                        json_output(&out);
                    } else {
                        let tools = out["tools"].as_array().cloned().unwrap_or_default();
                        print!("{}", commands::webmcp::render_list_text(&tools));
                        if out["frame_scoped"] == json!(true) {
                            println!("{}", commands::webmcp::FRAME_SCOPED_LIST_NOTE);
                        }
                    }
                }
                // `--args` is a raw JSON string here and a parsed object in pipe mode, so this
                // reaches `call_tool` directly; `call_report` is the shared half.
                WebmcpAction::Call {
                    name,
                    args,
                    inspect,
                    max_depth,
                } => {
                    let outcome =
                        Box::pin(commands::webmcp::call_tool(&client, &name, &args)).await?;
                    let report = policy.for_action(inspect, max_depth.or(cli.max_depth));
                    Box::pin(output_dispatched(
                        &mut ctx,
                        commands::webmcp::call_report(&outcome),
                        &report,
                        json_mode,
                    ))
                    .await?;
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
            // `"stop_on_error"` cannot drift. One recovery state shared by every entry: a
            // reset repairs later commands without reapplying before each one.
            let mut emulation_recovery =
                dispatch::EmulationRecovery::new(&client, ctx.store, &cli.browser, &cli.page).await;
            // Boxed for the same stack-frame reason as the other arms (see `run`'s doc).
            let out = Box::pin(dispatch::run_batch(
                &mut ctx,
                &cmds,
                stop_on_error,
                &mut emulation_recovery,
            ))
            .await;
            // `stopped_at` is set by `run_batch` only when `stop_on_error` cut the run short,
            // so its presence IS "an entry failed and the rest never ran".
            let stopped_at = out.get("stopped_at").and_then(Value::as_u64);
            if json_mode {
                json_output(&out);
            } else {
                print_batch_text(&out, stopped_at);
            }
            // Saved here rather than at the end of `run`: the arm returns through the error
            // channel when it stopped, and a batch that ran commands owes the store its
            // uid_map and snapshot either way. `save_session` also disarms the reaper, so the
            // error path in `main` cannot mistake this browser for a leaked one.
            session::save_session(ctx.store)?;
            if stopped_at.is_some() {
                return Err(Box::new(crate::run_helpers::BatchStopped));
            }
            return Ok(());
        }

        // Already handled above
        Command::Daemon { .. }
        | Command::Status
        | Command::Stop
        | Command::Close { .. }
        | Command::Pipe
        | Command::Replay { .. }
        | Command::History { .. }
        | Command::Macro { .. } => {
            unreachable!()
        }
    }

    session::save_session(ctx.store)?;

    Ok(())
}
