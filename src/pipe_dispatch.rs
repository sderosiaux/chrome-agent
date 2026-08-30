use serde_json::{Value, json};

use crate::cdp::client::CdpClient;
use crate::commands;
use crate::page_ctx::PageCtx;
// One definition each, in the module the CLI already reads them from: both used to exist here
// too, and `pipe_dispatch_actions` called BOTH spellings of `get_uid_map`.
use crate::pipe_command::{
    ConsoleArgs, DownloadArgs, EvalArgs, ExtractArgs, FillArgs, FrameArgs, GotoArgs, InspectArgs,
    NetworkArgs, PdfArgs, PipeCommand, PointerArgs, PressArgs, ReadArgs, ScreenshotArgs,
    ScrollArgs, TextArgs, TypeArgs, WaitArgs,
};
pub use crate::pipe_emulation::{EmulationRecovery, dispatch_emulate};
pub use crate::pipe_report::{attach_change_report, mutates_page};
use crate::run_helpers::merge_into;

pub use crate::pipe_dispatch_actions::{
    dispatch_assert, dispatch_check, dispatch_dblclick, dispatch_drag, dispatch_fill_and_submit,
    dispatch_fill_form, dispatch_history, dispatch_hover, dispatch_navigate_and_read,
    dispatch_select, dispatch_upload, dispatch_webmcp_call, dispatch_webmcp_list, run_batch,
};

// --- Per-command dispatchers ---

pub async fn dispatch_goto(
    ctx: &mut PageCtx<'_>,
    args: &GotoArgs,
) -> Result<Value, crate::BoxError> {
    let max_depth = args.max_depth.map(as_usize).or(ctx.max_depth);
    let parsed_headers = args
        .headers
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|h| commands::goto::parse_header(h.as_str()))
        .collect::<Result<Vec<_>, _>>()?;

    let result = commands::goto::run(ctx.client, &args.url, ctx.timeout, &parsed_headers).await?;
    // Mirror the CLI: after navigation, optionally wait for a CSS selector.
    if let Some(selector) = &args.wait_for {
        commands::wait::run(ctx.client, "selector", selector, ctx.timeout, 500).await?;
    }
    // Navigation destroys any bound frame's isolated world, so eval/inspect must fall back
    // to the freshly loaded top document (issue #8).
    ctx.client.set_frame_context(None);
    // `backendNodeId` counters overlap between documents, so a uid from the previous page can
    // resolve to an unrelated node on this one. `last_snapshot` deliberately survives, so `diff`
    // answers `document_changed` instead of erroring. The CLI arm had always done this and the
    // dispatcher never had, so pipe and batch carried stale uids across every `goto`.
    ctx.clear_uid_map();
    let _ = commands::history::append(&result.url, &result.title, ctx.page);

    let mut obj = json!({"ok": true, "url": result.url, "title": result.title});
    // `goto` is outside `mutates_page`, so nothing else speaks for it: `landed` rides on its
    // own response. The browser name goes with it so hints name a reachable session.
    result.landed.attach(&mut obj, ctx.browser);
    if args.inspect.unwrap_or(false) {
        let snapshot = attach_snapshot(ctx, max_depth).await?;
        obj["snapshot"] = json!(snapshot);
    }
    Ok(obj)
}

pub async fn dispatch_click(
    ctx: &mut PageCtx<'_>,
    args: &PointerArgs,
) -> Result<Value, crate::BoxError> {
    let max_depth = args.max_depth.map(as_usize).or(ctx.max_depth);
    let on_intercept = on_intercept(args.on_intercept.as_deref(), ctx.report.on_intercept)?;

    let (msg, details) = if let Some(sel) = &args.selector {
        let outcome = crate::element::click_selector(ctx.client, sel, on_intercept).await?;
        let target = format!("selector '{sel}'");
        (
            outcome
                .refusal_message("click", &target)
                .unwrap_or_else(|| format!("Clicked {target}")),
            Some(outcome.report()),
        )
    } else if let Some([x, y]) = args.xy {
        crate::element::click_at_coords(ctx.client, x, y).await?;
        (format!("Clicked at ({x}, {y})"), None)
    } else if let Some(uid) = &args.uid {
        let uid_map = ctx.uid_map();
        let (msg, outcome) = commands::click::run(ctx.client, &uid_map, uid, on_intercept).await?;
        (msg, Some(outcome.report()))
    } else {
        return Err("click: provide \"uid\", \"selector\", or \"xy\"".into());
    };

    let mut obj = json!({"ok": true, "message": msg});
    merge_into(&mut obj, details.as_ref());
    if args.inspect.unwrap_or(false) {
        let snapshot = attach_snapshot(ctx, max_depth).await?;
        obj["snapshot"] = json!(snapshot);
    }
    Ok(obj)
}

pub async fn dispatch_fill(
    ctx: &mut PageCtx<'_>,
    args: &FillArgs,
) -> Result<Value, crate::BoxError> {
    let max_depth = args.max_depth.map(as_usize).or(ctx.max_depth);
    let secret = args.secret.unwrap_or(false);

    let (target, msg, outcome) = if let Some(sel) = &args.selector {
        let handle = crate::hit_test::resolve_selector(ctx.client, sel).await?;
        let target = handle.report();
        let outcome =
            crate::element::fill_selector_handle(ctx.client, &handle, &args.value, secret).await?;
        (target, format!("Filled selector '{sel}'"), outcome)
    } else if let Some(uid) = &args.uid {
        let uid_map = ctx.uid_map();
        let (msg, outcome) =
            commands::fill::run(ctx.client, &uid_map, uid, &args.value, secret).await?;
        (Some(json!({"uid": uid})), msg, outcome)
    } else {
        return Err("fill: provide \"uid\" or \"selector\"".into());
    };

    let mut obj = json!({"ok": true, "message": msg});
    merge_into(&mut obj, target.as_ref());
    obj["value"] = crate::run_helpers::fill_value_report(&outcome);
    if args.inspect.unwrap_or(false) {
        let snapshot = attach_snapshot(ctx, max_depth).await?;
        obj["snapshot"] = json!(snapshot);
    }
    Ok(obj)
}

pub async fn dispatch_inspect(
    ctx: &mut PageCtx<'_>,
    args: &InspectArgs,
) -> Result<Value, crate::BoxError> {
    let verbose = args.verbose.unwrap_or(false);
    let max_depth = args.max_depth.map(as_usize);
    let uid = args.uid.as_deref();
    let role_filter: Option<Vec<&str>> = args
        .filter
        .as_deref()
        .map(|f| f.split(',').map(str::trim).collect());

    if args.scroll.unwrap_or(false) {
        commands::extract::scroll_to_load(ctx.client).await?;
    }
    let views = if let Some(max) = args.limit.map(as_usize) {
        commands::inspect::scroll_collect(ctx.client, verbose, uid, role_filter.as_deref(), max)
            .await?
    } else {
        commands::inspect::views(ctx.client, verbose, max_depth, uid, role_filter.as_deref())
            .await?
    };
    // `--urls` annotates only the lines it returns: applied to the baseline it would make
    // every link read as changed on the next diff, which reads no url= token.
    let shown = if args.urls.unwrap_or(false) {
        commands::inspect::resolve_urls(ctx.client, views.shown(), &views.full.uid_map).await
    } else {
        views.shown().to_string()
    };

    // Persist the FULL snapshot so diff and uid lookups stay complete: display flags and
    // paging affect only what is returned.
    ctx.store_snapshot(views.full);

    let offset = args.offset.map_or(0, as_usize);
    let paged = commands::inspect::paginate(&shown, offset, args.max_chars.map(as_usize));
    Ok(json!({
        "ok": true,
        "snapshot": paged.text,
        "total_chars": paged.total_chars,
        "truncated": paged.truncated,
        "next_offset": paged.next_offset,
    }))
}

pub async fn dispatch_diff(ctx: &mut PageCtx<'_>) -> Result<Value, crate::BoxError> {
    let page_state = ctx.page_state();
    let old_text = page_state
        .and_then(|p| p.last_snapshot.clone())
        .ok_or("No previous snapshot. Run inspect first.")?;
    let stored = page_state.and_then(|p| {
        p.last_snapshot_frame
            .clone()
            .zip(p.last_snapshot_loader.clone())
    });

    let snapshot = commands::inspect::run(ctx.client, false, None, None, None).await?;
    let identity = commands::diff::Identity::from_loader(
        stored.as_ref().map(|(f, l)| (f.as_str(), l.as_str())),
        snapshot
            .identity
            .as_ref()
            .map(|(f, l)| (f.as_str(), l.as_str())),
    );
    let result = commands::diff::compare(identity, &old_text, &snapshot.text);
    ctx.store_snapshot(snapshot);

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

pub async fn dispatch_eval(client: &CdpClient, args: &EvalArgs) -> Result<Value, crate::BoxError> {
    let expr = commands::eval::scoped_expression(&args.expression, args.selector.as_deref());
    let raw = commands::eval::run_raw(client, &expr).await?;
    Ok(json!({"ok": true, "result": raw}))
}

pub async fn dispatch_read(client: &CdpClient, args: &ReadArgs) -> Result<Value, crate::BoxError> {
    let result = commands::read::run(
        client,
        args.html.unwrap_or(false),
        args.truncate.map(as_usize),
    )
    .await?;
    let mut obj = json!({"ok": true, "title": result.title, "text": result.text_content});
    if let Some(excerpt) = &result.excerpt {
        obj["excerpt"] = json!(excerpt);
    }
    if let Some(byline) = &result.byline {
        obj["byline"] = json!(byline);
    }
    // pipe/batch is JSON-only, so this is the only place `--html` can be observed.
    if let Some(content) = &result.content {
        obj["content"] = json!(content);
    }
    Ok(obj)
}

pub async fn dispatch_text(ctx: &PageCtx<'_>, args: &TextArgs) -> Result<Value, crate::BoxError> {
    let uid_map = ctx.uid_map();
    let text = commands::text::run(
        ctx.client,
        args.uid.as_deref(),
        args.selector.as_deref(),
        &uid_map,
    )
    .await?;
    let full_length = text.chars().count();
    let (text, truncated) = if let Some(n) = args.truncate.map(as_usize) {
        if full_length > n {
            (
                crate::truncate::truncate_str(&text, n, "...").into_owned(),
                true,
            )
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

pub async fn dispatch_screenshot(
    ctx: &PageCtx<'_>,
    args: &ScreenshotArgs,
) -> Result<Value, crate::BoxError> {
    let format = commands::screenshot::ImgFormat::parse(args.format.as_deref().unwrap_or("png"))?;

    let clip = if let Some(u) = &args.uid {
        let uid_map = ctx.uid_map();
        Some(crate::geometry::clip_for_uid(ctx.client, &uid_map, u).await?)
    } else if let Some(sel) = &args.selector {
        Some(crate::geometry::clip_for_selector(ctx.client, sel).await?)
    } else {
        None
    };

    let opts = commands::screenshot::ScreenshotOpts {
        filename: args.filename.as_deref(),
        format,
        quality: args.quality,
        max_width: args.max_width,
        clip,
    };
    let path = commands::screenshot::run(ctx.client, &opts).await?;
    Ok(json!({"ok": true, "path": path}))
}

/// `download` in pipe and batch, through the CLI's entry point
/// (`commands::download::dispatch`) so the URL and click paths keep one response shape.
pub async fn dispatch_download(
    ctx: &PageCtx<'_>,
    args: &DownloadArgs,
) -> Result<Value, crate::BoxError> {
    let target = commands::download::Target::parse(
        args.url.as_deref(),
        args.uid.as_deref(),
        args.selector.as_deref(),
    )?;
    let uid_map = ctx.uid_map();
    let req = commands::download::Request {
        target: &target,
        out: args.out.as_deref(),
        timeout_secs: args.timeout.unwrap_or(ctx.timeout),
        max_bytes: download_max_bytes(args.max_bytes)?,
        on_intercept: on_intercept(args.on_intercept.as_deref(), ctx.report.on_intercept)?,
        browser: ctx.browser,
    };
    let outcome = commands::download::dispatch(ctx.client, &uid_map, &req).await?;
    Ok(outcome.to_json())
}

fn download_max_bytes(requested: Option<u64>) -> Result<usize, crate::BoxError> {
    let value = requested.unwrap_or(commands::download::DEFAULT_MAX_BYTES as u64);
    let value =
        usize::try_from(value).map_err(|_| "download: max_bytes exceeds platform limits")?;
    if value == 0 {
        return Err("download: max_bytes must be greater than zero".into());
    }
    Ok(value)
}

pub async fn dispatch_pdf(client: &CdpClient, args: &PdfArgs) -> Result<Value, crate::BoxError> {
    let opts = commands::pdf::PdfOpts {
        filename: args.filename.as_deref(),
        landscape: args.landscape.unwrap_or(false),
        background: args.background.unwrap_or(false),
    };
    let path = commands::pdf::run(client, &opts).await?;
    Ok(json!({"ok": true, "path": path}))
}

pub async fn dispatch_wait(client: &CdpClient, args: &WaitArgs) -> Result<Value, crate::BoxError> {
    let (what, pattern) = wait_condition(args)?;
    // The CLI `wait --timeout` default (10s), not the global 30s page-load timeout: a wait
    // is per-condition and does not inherit --timeout.
    let timeout = args.timeout.unwrap_or(WAIT_DEFAULT_TIMEOUT);
    let idle_ms = args.idle_ms.unwrap_or(500);
    let msg = commands::wait::run(client, &what, &pattern, timeout, idle_ms).await?;
    Ok(json!({"ok": true, "message": msg}))
}

/// One step through the history stack: `-1` is `back`, `+1` is `forward`.
///
/// The two verbs were the same twenty lines twice over, differing in the sign and in the
/// boundary test — which only `forward` had. `back` fired `history.back()` blind and answered
/// `{"ok":true,"title":…}` whether or not a document changed; at the start of the stack that
/// cost the full five-second wait for a `loadEventFired` that never comes, and read as a
/// successful navigation. The boundary is now read from `Page.getNavigationHistory` first, and
/// both verbs report the `url` they landed on, as `goto` does.
///
/// Takes the context rather than the client because a step that lands on another document owes
/// the store the same uid hygiene `goto` does — see the `clear_uid_map` call below.
pub async fn history_step(ctx: &mut PageCtx<'_>, delta: i64) -> Result<Value, crate::BoxError> {
    let client = ctx.client;
    let history: Value = client.call("Page.getNavigationHistory", json!({})).await?;
    let current_index = history
        .get("currentIndex")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let entries = history.get("entries").and_then(Value::as_array);
    let entry_count = entries.map_or(0, Vec::len) as i64;
    let target_index = current_index + delta;
    if target_index < 0 || target_index >= entry_count {
        let edge = if delta < 0 { "first" } else { "last" };
        return Ok(json!({
            "ok": true,
            "title": "",
            "message": format!("Already at {edge} history entry"),
        }));
    }
    let entry_id = entries
        .and_then(|e| e.get(usize::try_from(target_index).unwrap_or(0)))
        .and_then(|e| e.get("id"))
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            let which = if delta < 0 { "previous" } else { "next" };
            format!("Could not find {which} history entry")
        })?;
    // Subscribe BEFORE navigating: a cached history entry can fire Page.loadEventFired
    // before a late subscription exists, stalling until the timeout.
    let mut rx = client.page_events();
    client
        .send("Page.navigateToHistoryEntry", json!({"entryId": entry_id}))
        .await?;
    client.set_frame_context(None); // history navigation invalidates any bound frame
    // Same rule as `goto`, and the divergence this lift closed: the CLI arm cleared the map and
    // the dispatcher never did, so a uid from before the step still resolved in pipe and batch —
    // to whatever node the new document happens to give that `backendNodeId`.
    ctx.clear_uid_map();
    let _ = CdpClient::wait_for_event_on(
        &mut rx,
        "Page.loadEventFired",
        std::time::Duration::from_secs(5),
    )
    .await;
    let dest: crate::cdp::types::EvaluateResult = client
        .call("Runtime.evaluate", json!({"expression": "({title: document.title, url: location.href})", "returnByValue": true})).await?;
    let field = |key: &str| {
        dest.result
            .value
            .as_ref()
            .and_then(|v| v.get(key))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    Ok(json!({"ok": true, "title": field("title"), "url": field("url")}))
}

pub async fn dispatch_scroll(
    ctx: &PageCtx<'_>,
    args: &ScrollArgs,
) -> Result<Value, crate::BoxError> {
    let px = args.px.unwrap_or(500);
    let client = ctx.client;
    let msg = match args.target.as_str() {
        "down" => {
            let _: Value = client.call("Runtime.evaluate", json!({"expression": format!("window.scrollBy(0, {px})"), "returnByValue": true})).await?;
            format!("Scrolled down {px}px")
        }
        "up" => {
            let _: Value = client.call("Runtime.evaluate", json!({"expression": format!("window.scrollBy(0, -{px})"), "returnByValue": true})).await?;
            format!("Scrolled up {px}px")
        }
        uid => {
            let uid_map = ctx.uid_map();
            let element_ref = uid_map.get(uid).ok_or_else(|| {
                format!(
                    "Element uid={uid} not found. Run 'chrome-agent inspect' to get fresh uids."
                )
            })?;
            let backend_node_id = element_ref
                .backend_node_id()
                .ok_or_else(|| format!("Element uid={uid} has no resolvable backend node."))?;
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
            let _: Value = client.call("Runtime.callFunctionOn", json!({"objectId": object_id, "functionDeclaration": "function() { this.scrollIntoView({block: 'center'}); }", "returnByValue": true})).await?;
            format!("Scrolled uid={uid} into view")
        }
    };
    Ok(json!({"ok": true, "message": msg}))
}

/// `type`, in all three modes. `secret` only ever ADDS redaction — there is deliberately no way
/// to force a value to be printed.
///
/// The message is `type_text_with`'s, not a template built here: it is the one that withholds the
/// length when the focused element is a secret field. Both front ends used to discard it and
/// rebuild `Typed {text.len()} chars`, which made that redaction inert (and counted BYTES where
/// the element module counts characters).
pub async fn dispatch_type(client: &CdpClient, args: &TypeArgs) -> Result<Value, crate::BoxError> {
    if let Some(sel) = &args.selector {
        crate::element::focus_selector(client, sel).await?;
    }
    crate::element::require_editable_focus(client).await?;
    let typed =
        crate::element::type_text_with(client, &args.text, args.secret.unwrap_or(false)).await?;
    let msg = match &args.selector {
        Some(sel) => format!("{typed} into selector '{sel}'"),
        None => typed,
    };
    Ok(json!({"ok": true, "message": msg}))
}

pub async fn dispatch_press(
    client: &CdpClient,
    args: &PressArgs,
) -> Result<Value, crate::BoxError> {
    crate::element::press_key(client, &args.key).await?;
    Ok(json!({"ok": true, "message": format!("Pressed {}", args.key)}))
}

pub async fn dispatch_tabs(ctx: &PageCtx<'_>) -> Result<Value, crate::BoxError> {
    let tabs = commands::tabs::run_structured(ctx.browser_client, ctx.store).await?;
    Ok(json!({"ok": true, "tabs": tabs}))
}

pub async fn dispatch_network(
    client: &CdpClient,
    args: &NetworkArgs,
) -> Result<Value, crate::BoxError> {
    let capture = commands::network::collect(
        client,
        args.filter.as_deref(),
        args.body.unwrap_or(false),
        args.live,
        args.limit.map_or(50, as_usize),
        args.abort.as_deref(),
    )
    .await?;
    Ok(capture.to_json())
}

pub async fn dispatch_console(
    client: &CdpClient,
    args: &ConsoleArgs,
) -> Result<Value, crate::BoxError> {
    let reading = commands::console::run(
        client,
        args.level.as_deref(),
        args.clear.unwrap_or(false),
        args.limit.map_or(50, as_usize),
    )
    .await?;
    Ok(commands::console::to_json(&reading))
}

pub async fn dispatch_extract(
    client: &CdpClient,
    args: &ExtractArgs,
) -> Result<Value, crate::BoxError> {
    let result = commands::extract::collect(
        client,
        args.selector.as_deref(),
        args.limit.map_or(10, as_usize),
        args.scroll.unwrap_or(false),
        args.a11y.unwrap_or(false),
    )
    .await?;
    Ok(commands::extract::to_json(&result))
}

// --- Helpers ---

/// Attach the tree a command's `inspect: true` asked for, and store the baseline.
///
/// The pipe/batch half of the rule `run_helpers::output_action_with` states for the CLI:
/// `max_depth` decides what is RETURNED, never what is stored.
pub async fn attach_snapshot(
    ctx: &mut PageCtx<'_>,
    max_depth: Option<usize>,
) -> Result<String, crate::BoxError> {
    let views = commands::inspect::views(ctx.client, false, max_depth, None, None).await?;
    let shown = views.shown().to_string();
    ctx.store_snapshot(views.full);
    Ok(shown)
}

/// The protocol counts in `u64`; the readers below take `usize`. Used as `opt.map(as_usize)`.
pub const fn as_usize(value: u64) -> usize {
    value as usize
}

/// The per-command `on_intercept` override, falling back to the session's policy.
pub fn on_intercept(
    value: Option<&str>,
    fallback: crate::hit_test::OnIntercept,
) -> Result<crate::hit_test::OnIntercept, crate::BoxError> {
    value.map_or(Ok(fallback), |v| {
        crate::hit_test::OnIntercept::parse(v).map_err(Into::into)
    })
}

/// Default `wait` timeout in seconds — mirrors the CLI `wait --timeout` default.
const WAIT_DEFAULT_TIMEOUT: u64 = 10;

/// Resolve `wait`'s (what, pattern) from the several accepted shapes.
/// `network-idle` needs no pattern; every other condition requires one.
fn wait_condition(args: &WaitArgs) -> Result<(String, String), crate::BoxError> {
    if let Some(what) = &args.what {
        if what == "network-idle" {
            return Ok((what.clone(), String::new()));
        }
        let pattern = args
            .pattern
            .clone()
            .ok_or("wait: missing \"pattern\" (use {\"what\":\"text\",\"pattern\":\"...\"})")?;
        return Ok((what.clone(), pattern));
    }
    for (what, pattern) in [
        ("text", &args.text),
        ("url", &args.url),
        ("selector", &args.selector),
    ] {
        if let Some(pattern) = pattern {
            return Ok((what.to_string(), pattern.clone()));
        }
    }
    Err("wait: specify {\"what\":\"text\",\"pattern\":\"...\"} or {\"text\":\"...\"} or {\"url\":\"...\"} or {\"selector\":\"...\"} or {\"what\":\"network-idle\"}".into())
}

// --- Frame ---

pub async fn dispatch_frame(
    client: &CdpClient,
    args: &FrameArgs,
) -> Result<Value, crate::BoxError> {
    let msg = commands::frame::run(client, &args.target).await?;
    Ok(json!({"ok": true, "message": msg}))
}

// --- Batch ---

/// One dispatched command's future, type-erased.
///
/// `batch` is itself a command, so `dispatch_single` → `run_batch` → `dispatch_single` is a
/// cycle, and rustc cannot size a future whose type contains itself.
/// Erasing it here is what lets the two front ends share ONE dispatcher: `pipe.rs` used to keep
/// a second copy of the whole match purely to own the `batch` arm, and the two had already
/// drifted — a `batch` nested in a `batch` answered `Unknown command: batch`.
/// `Send` is part of the contract, not decoration: the clippy nursery enforces it across the
/// whole crate, and erasing it here would make `run` itself non-`Send`.
pub type Dispatched<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = Value> + Send + 'a>>;

/// Dispatch one command. The single dispatcher behind pipe, pipe `batch` and CLI `batch`.
pub fn dispatch_single<'a>(
    ctx: &'a mut PageCtx<'_>,
    cmd: &'a Value,
    emulation_recovery: &'a mut EmulationRecovery,
) -> Dispatched<'a> {
    Box::pin(dispatch_one(ctx, cmd, emulation_recovery))
}

/// What a command that never ran answers with. Shared by the parse refusal and by every
/// dispatcher's `Err`, so both clear the wait and both get the same hint treatment.
fn refused(ctx: &PageCtx<'_>, error: &crate::BoxError) -> Value {
    // The wait this command paid for dies with it. `take_settle_wait_ms` is only
    // reached from `attach_verdict_for`, which a failed command never gets to, so on a
    // shared connection the slot survived and the NEXT command reported the wait as
    // its own. `mark_dispatch` clears it going in; this clears it going out.
    let _ = ctx.client.take_settle_wait_ms();
    // A refusal carries what it measured (receiver, aim point, branch); its Display
    // alone would drop all of it on the one path where nothing was dispatched.
    if let Some(refusal) = crate::hit_test::refusal_in(error) {
        return refusal.to_json(ctx.browser);
    }
    let msg = error.to_string();
    let mut obj = json!({"ok": false, "error": msg});
    if let Some(h) = crate::run_helpers::error_hint(&msg, ctx.browser) {
        obj["hint"] = json!(h);
    }
    obj
}

async fn dispatch_one(
    ctx: &mut PageCtx<'_>,
    cmd: &Value,
    emulation_recovery: &mut EmulationRecovery,
) -> Value {
    // The protocol is decoded once, before anything is dispatched: an unknown key is an error
    // naming the key rather than a field silently dropped on the way in.
    let parsed = match crate::pipe_command::parse(cmd) {
        Ok(parsed) => parsed,
        Err(e) => return refused(ctx, &e),
    };
    // The canonical verb, so an alias cannot fall out of `mutates_page`.
    let cmd_name = parsed.name();
    let report = ctx.report;
    // Capture the baseline before dispatching: a command run with `inspect` refreshes it,
    // and comparing against the refreshed copy would report that nothing moved.
    let baseline = if report.changes && mutates_page(cmd_name) {
        ctx.store
            .browsers
            .get(ctx.browser)
            .and_then(|b| b.pages.get(ctx.page))
            .map(|p| {
                (
                    p.last_snapshot.clone(),
                    p.last_snapshot_frame
                        .clone()
                        .zip(p.last_snapshot_loader.clone()),
                )
            })
    } else {
        None
    };
    let mut value = {
        let result: Result<Value, crate::BoxError> = match &parsed {
            PipeCommand::Goto(a) => dispatch_goto(ctx, a).await,
            PipeCommand::Click(a) => dispatch_click(ctx, a).await,
            PipeCommand::Fill(a) => dispatch_fill(ctx, a).await,
            PipeCommand::Inspect(a) => dispatch_inspect(ctx, a).await,
            PipeCommand::Eval(a) => dispatch_eval(ctx.client, a).await,
            PipeCommand::Read(a) => dispatch_read(ctx.client, a).await,
            PipeCommand::Text(a) => dispatch_text(ctx, a).await,
            PipeCommand::Screenshot(a) => dispatch_screenshot(ctx, a).await,
            PipeCommand::Pdf(a) => dispatch_pdf(ctx.client, a).await,
            PipeCommand::Download(a) => dispatch_download(ctx, a).await,
            PipeCommand::Wait(a) => dispatch_wait(ctx.client, a).await,
            PipeCommand::Back(_) => history_step(ctx, -1).await,
            PipeCommand::Forward(_) => history_step(ctx, 1).await,
            PipeCommand::Scroll(a) => dispatch_scroll(ctx, a).await,
            PipeCommand::Type(a) => dispatch_type(ctx.client, a).await,
            PipeCommand::Press(a) => dispatch_press(ctx.client, a).await,
            PipeCommand::Dblclick(a) => dispatch_dblclick(ctx, a).await,
            PipeCommand::Select(a) => dispatch_select(ctx, a).await,
            // The verb decides the desired state; only `check` may be talked out of it by an
            // explicit `"desired"`, which is why `uncheck` does not declare the field at all.
            PipeCommand::Check(a) => {
                dispatch_check(
                    ctx,
                    a.desired.unwrap_or(true),
                    a.uid.as_deref(),
                    a.selector.as_deref(),
                    a.on_intercept.as_deref(),
                )
                .await
            }
            PipeCommand::Uncheck(a) => {
                dispatch_check(
                    ctx,
                    false,
                    a.uid.as_deref(),
                    a.selector.as_deref(),
                    a.on_intercept.as_deref(),
                )
                .await
            }
            PipeCommand::Upload(a) => dispatch_upload(ctx, a).await,
            PipeCommand::Drag(a) => dispatch_drag(ctx, a).await,
            PipeCommand::Hover(a) => dispatch_hover(ctx, a).await,
            PipeCommand::FillForm(a) => dispatch_fill_form(ctx, a).await,
            PipeCommand::Tabs(_) => dispatch_tabs(ctx).await,
            PipeCommand::Network(a) => dispatch_network(ctx.client, a).await,
            PipeCommand::Console(a) => dispatch_console(ctx.client, a).await,
            PipeCommand::Diff(_) => dispatch_diff(ctx).await,
            PipeCommand::Extract(a) => dispatch_extract(ctx.client, a).await,
            PipeCommand::NavigateAndRead(a) => dispatch_navigate_and_read(ctx, a).await,
            PipeCommand::FillAndSubmit(a) => dispatch_fill_and_submit(ctx, a).await,
            PipeCommand::History(a) => dispatch_history(a),
            PipeCommand::Frame(a) => dispatch_frame(ctx.client, a).await,
            PipeCommand::Emulate(a) => dispatch_emulate(ctx, a).await,
            PipeCommand::Assert(a) => dispatch_assert(ctx, a).await,
            PipeCommand::WebmcpList(_) => dispatch_webmcp_list(ctx.client).await,
            PipeCommand::WebmcpCall(a) => dispatch_webmcp_call(ctx.client, a).await,
            PipeCommand::Batch(a) => {
                let commands = a
                    .commands
                    .as_deref()
                    .ok_or("batch: missing \"commands\" array");
                match commands {
                    Ok(list) => Ok(run_batch(
                        ctx,
                        list,
                        a.stop_on_error.unwrap_or(false),
                        emulation_recovery,
                    )
                    .await),
                    Err(e) => Err(e.into()),
                }
            }
        };
        // `result` must not outlive this block: BoxError is not Send, and an await with it in
        // scope would make every caller's future non-Send.
        match result {
            Ok(v) => v,
            Err(e) => return refused(ctx, &e),
        }
    };
    // Same as pipe: switching the report off must not read like an empty page. The hit test
    // still ran, so an intercepted click still reports its receiver here.
    if !report.changes && mutates_page(cmd_name) {
        crate::pipe_report::attach_verdict_for(
            ctx.client,
            &mut value,
            crate::verdict::Observation::ReportingDisabled,
        );
    }
    if let Some((old_text, old_url)) = baseline {
        attach_change_report(ctx, old_text.as_deref(), old_url, &mut value).await;
    }
    value
}

// --- Tests: pure JSON → typed-args parsing, no live Chrome ---

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipe_command::parse;

    fn args<T>(cmd: &Value, pick: impl Fn(PipeCommand) -> Option<T>) -> T {
        pick(parse(cmd).expect("parses")).expect("the expected variant")
    }

    #[test]
    fn scroll_honors_px_and_defaults_to_500() {
        let a = args(
            &json!({"cmd": "scroll", "target": "down", "px": 1200}),
            |c| match c {
                PipeCommand::Scroll(a) => Some(a),
                _ => None,
            },
        );
        assert_eq!(a.target, "down");
        assert_eq!(a.px.unwrap_or(500), 1200);

        let a = args(&json!({"cmd": "scroll", "target": "up"}), |c| match c {
            PipeCommand::Scroll(a) => Some(a),
            _ => None,
        });
        assert_eq!(a.px.unwrap_or(500), 500);
    }

    #[test]
    fn text_maps_each_exclusive_target_and_truncate() {
        let a = args(
            &json!({"cmd": "text", "uid": "n47", "truncate": 80}),
            |c| match c {
                PipeCommand::Text(a) => Some(a),
                _ => None,
            },
        );
        assert_eq!(a.uid.as_deref(), Some("n47"));
        assert!(a.selector.is_none());
        assert_eq!(a.truncate.map(as_usize), Some(80));

        let a = args(&json!({"cmd": "text", "selector": "main"}), |c| match c {
            PipeCommand::Text(a) => Some(a),
            _ => None,
        });
        assert_eq!(a.selector.as_deref(), Some("main"));
        assert!(a.uid.is_none());

        assert!(parse(&json!({"cmd": "text", "uid": "n47", "selector": "main"})).is_err());

        let a = args(&json!({"cmd": "text"}), |c| match c {
            PipeCommand::Text(a) => Some(a),
            _ => None,
        });
        assert!(a.uid.is_none() && a.selector.is_none() && a.truncate.is_none());
    }

    fn wait_args(cmd: &Value) -> WaitArgs {
        args(cmd, |c| match c {
            PipeCommand::Wait(a) => Some(a),
            _ => None,
        })
    }

    #[test]
    fn wait_network_idle_needs_no_pattern() {
        let (what, pattern) =
            wait_condition(&wait_args(&json!({"cmd": "wait", "what": "network-idle"}))).unwrap();
        assert_eq!(what, "network-idle");
        assert!(pattern.is_empty());
    }

    #[test]
    fn wait_explicit_what_pattern_and_shorthands() {
        let (what, pattern) = wait_condition(&wait_args(
            &json!({"cmd": "wait", "what": "text", "pattern": "Welcome"}),
        ))
        .unwrap();
        assert_eq!((what.as_str(), pattern.as_str()), ("text", "Welcome"));

        let (what, pattern) =
            wait_condition(&wait_args(&json!({"cmd": "wait", "selector": ".done"}))).unwrap();
        assert_eq!((what.as_str(), pattern.as_str()), ("selector", ".done"));
    }

    #[test]
    fn wait_missing_pattern_and_empty_are_refused() {
        let err = wait_condition(&wait_args(&json!({"cmd": "wait", "what": "text"})))
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing \"pattern\""), "{err}");
        assert!(wait_condition(&wait_args(&json!({"cmd": "wait"}))).is_err());
    }

    #[test]
    fn wait_default_timeout_is_ten() {
        // pipe/batch `wait` defaults to the CLI's 10s, not the global 30s page-load timeout.
        assert_eq!(WAIT_DEFAULT_TIMEOUT, 10);
    }

    #[test]
    fn download_max_bytes_defaults_and_rejects_zero() {
        assert_eq!(download_max_bytes(None).unwrap(), 67_108_864);
        assert_eq!(download_max_bytes(Some(10)).unwrap(), 10);
        assert!(download_max_bytes(Some(0)).is_err());
        // A non-integer never reaches here: the protocol refuses it by name.
        let err = parse(&json!({"cmd": "download", "max_bytes": "10"}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("\"max_bytes\""), "{err}");
    }
}
