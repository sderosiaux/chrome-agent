//! Composite and form-control dispatchers, plus `run_batch`. Re-exported from
//! `pipe_dispatch` so callers use a single path.

use serde_json::{Value, json};

use crate::cdp::client::CdpClient;
use crate::commands;
use crate::page_ctx::PageCtx;
use crate::pipe_command::{
    AssertArgs, DragArgs, FillAndSubmitArgs, FillFormArgs, HistoryArgs, HoverArgs,
    NavigateAndReadArgs, PointerArgs, UploadArgs, ValueArgs, WebmcpCallArgs,
};

use crate::pipe_dispatch::{as_usize, attach_snapshot, on_intercept};
use crate::run_helpers::merge_into;

// --- Composite dispatchers ---

pub async fn dispatch_navigate_and_read(
    ctx: &mut PageCtx<'_>,
    args: &NavigateAndReadArgs,
) -> Result<Value, crate::BoxError> {
    let goto_result = commands::goto::run(ctx.client, &args.url, ctx.timeout, &[]).await?;
    // Navigation already happened even if history or Readability fails below. Stale backend ids
    // can overlap the new document and resolve to an unrelated element.
    ctx.clear_uid_map();
    ctx.client.set_frame_context(None); // navigation invalidates any bound frame (issue #8)
    let _ = commands::history::append(&goto_result.url, &goto_result.title, ctx.page);
    let read_result = commands::read::run(ctx.client, false, args.truncate.map(as_usize)).await?;
    let mut out = json!({"ok": true, "url": goto_result.url, "title": goto_result.title, "content": read_result.text_content});
    // `landed` matters more here than on a bare `goto`: without it a login page's prose comes
    // back as if it were the article that was asked for.
    goto_result.landed.attach(&mut out, ctx.browser);
    Ok(out)
}

pub async fn dispatch_fill_and_submit(
    ctx: &PageCtx<'_>,
    args: &FillAndSubmitArgs,
) -> Result<Value, crate::BoxError> {
    let client = ctx.client;
    let timeout = ctx.timeout;
    let fields = args
        .fields
        .as_deref()
        .ok_or("fill_and_submit: missing \"fields\" array")?;
    let submit_selector = args
        .submit
        .as_deref()
        .ok_or("fill_and_submit: missing \"submit\" selector")?;
    let wait_for = args.wait_for.as_deref();
    let field_count = fields.len();
    let mut outcomes = Vec::new();
    for field in fields {
        let selector = field
            .selector
            .as_deref()
            .ok_or("fill_and_submit: each field needs \"selector\"")?;
        let value = field
            .value
            .as_deref()
            .ok_or("fill_and_submit: each field needs \"value\"")?;
        let outcome = crate::element::fill_selector_with(client, selector, value, false).await?;
        outcomes.push((selector.to_string(), outcome));
    }
    let intercept = on_intercept(
        args.on_intercept.as_deref(),
        crate::hit_test::OnIntercept::default(),
    )?;
    let submitted = crate::element::click_selector(client, submit_selector, intercept).await?;
    if let Some(pattern) = wait_for {
        let is_selector = pattern.contains('.')
            || pattern.contains('#')
            || pattern.contains('[')
            || pattern.contains('>');
        let wait_type = if is_selector { "selector" } else { "text" };
        commands::wait::run(client, wait_type, pattern, timeout, 500).await?;
    }
    // The read below is best effort: Readability rejects plenty of legitimate pages, and the
    // fill and submit already landed. Failing here would invite a second submit.
    let message = format!(
        "Filled {field_count} fields, submitted, waited for '{}'",
        wait_for.unwrap_or("none")
    );
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

pub fn dispatch_history(args: &HistoryArgs) -> Result<Value, crate::BoxError> {
    let entries = commands::history::run(args.filter.as_deref(), args.limit.map_or(20, as_usize))?;
    Ok(commands::history::to_json(&entries))
}

pub async fn dispatch_fill_form(
    ctx: &mut PageCtx<'_>,
    args: &FillFormArgs,
) -> Result<Value, crate::BoxError> {
    let pairs = args
        .pairs
        .as_deref()
        .ok_or("fill-form requires \"pairs\" array (e.g. [{\"uid\":\"n1\",\"value\":\"a\"}])")?;
    let parsed: Vec<(&str, &str)> = pairs
        .iter()
        .map(|pair| {
            let uid = pair.uid.as_deref().ok_or("Each pair needs \"uid\"")?;
            let value = pair.value.as_deref().ok_or("Each pair needs \"value\"")?;
            Ok::<_, crate::BoxError>((uid, value))
        })
        .collect::<Result<_, _>>()?;
    let uid_map = ctx.uid_map();
    // `commands::fill::run_form` rather than a second loop: the CLI arm already called it, and
    // the two messages had drifted ("Filled 3 fields: uid=n1, …" against "Filled 3 fields").
    let (msg, outcomes) = commands::fill::run_form(ctx.client, &uid_map, &parsed).await?;
    let mut obj = json!({"ok": true, "message": msg});
    obj["values"] = crate::run_helpers::bulk_fill_report("uid", &outcomes);
    if args.inspect.unwrap_or(false) {
        let max_depth = args.max_depth.map(as_usize).or(ctx.max_depth);
        obj["snapshot"] = json!(attach_snapshot(ctx, max_depth).await?);
    }
    Ok(obj)
}

pub async fn dispatch_hover(ctx: &PageCtx<'_>, args: &HoverArgs) -> Result<Value, crate::BoxError> {
    let uid = args.uid.as_deref().ok_or("hover requires \"uid\"")?;
    let uid_map = ctx.uid_map();
    crate::element::hover(ctx.client, &uid_map, uid).await?;
    Ok(json!({"ok": true, "message": format!("Hovered uid={uid}")}))
}

// --- New command dispatchers ---

pub async fn dispatch_dblclick(
    ctx: &mut PageCtx<'_>,
    args: &PointerArgs,
) -> Result<Value, crate::BoxError> {
    let max_depth = args.max_depth.map(as_usize).or(ctx.max_depth);
    let on_intercept = on_intercept(args.on_intercept.as_deref(), ctx.report.on_intercept)?;
    // The node is resolved before the action: afterwards it may be detached, and the answer
    // would describe a different page.
    let (msg, details) = if let Some(sel) = &args.selector {
        let outcome = crate::element::dblclick_selector(ctx.client, sel, on_intercept).await?;
        let target = format!("selector '{sel}'");
        (
            outcome
                .refusal_message("double-click", &target)
                .unwrap_or_else(|| format!("Double-clicked {target}")),
            Some(outcome.report()),
        )
    } else if let Some([x, y]) = args.xy {
        crate::element::dblclick_at_coords(ctx.client, x, y).await?;
        (format!("Double-clicked at ({x}, {y})"), None)
    } else if let Some(uid) = &args.uid {
        let uid_map = ctx.uid_map();
        let (msg, outcome) =
            commands::dblclick::run(ctx.client, &uid_map, uid, on_intercept).await?;
        (msg, Some(outcome.report()))
    } else {
        return Err("dblclick: provide \"uid\", \"selector\", or \"xy\"".into());
    };
    let mut obj = json!({"ok": true, "message": msg});
    merge_into(&mut obj, details.as_ref());
    if args.inspect.unwrap_or(false) {
        let snapshot = attach_snapshot(ctx, max_depth).await?;
        obj["snapshot"] = json!(snapshot);
    }
    Ok(obj)
}

pub async fn dispatch_select(
    ctx: &mut PageCtx<'_>,
    args: &ValueArgs,
) -> Result<Value, crate::BoxError> {
    let max_depth = args.max_depth.map(as_usize).or(ctx.max_depth);
    let (target, msg, outcome) = if let Some(sel) = &args.selector {
        let handle = crate::hit_test::resolve_selector(ctx.client, sel).await?;
        let target = handle.report();
        let outcome =
            crate::element::select_option_handle(ctx.client, &handle, &args.value).await?;
        (
            target,
            format!("Selected \"{}\" on selector '{sel}'", outcome.label()),
            outcome,
        )
    } else if let Some(uid) = &args.uid {
        let uid_map = ctx.uid_map();
        let outcome = crate::element::select_option(ctx.client, &uid_map, uid, &args.value).await?;
        (
            Some(json!({"uid": uid})),
            format!("Selected \"{}\" on uid={uid}", outcome.label()),
            outcome,
        )
    } else {
        return Err("select: provide \"uid\" or \"selector\"".into());
    };
    let mut obj = json!({"ok": true, "message": msg});
    merge_into(&mut obj, Some(&crate::run_helpers::select_report(&outcome)));
    merge_into(&mut obj, target.as_ref());
    if args.inspect.unwrap_or(false) {
        let snapshot = attach_snapshot(ctx, max_depth).await?;
        obj["snapshot"] = json!(snapshot);
    }
    Ok(obj)
}

/// `desired` comes from the caller, not from the command: the dispatcher already knows whether
/// the verb was `check` or `uncheck`, and `uncheck` does not declare the field at all.
pub async fn dispatch_check(
    ctx: &PageCtx<'_>,
    desired: bool,
    uid: Option<&str>,
    selector: Option<&str>,
    intercept: Option<&str>,
) -> Result<Value, crate::BoxError> {
    let (target, outcome) = if let Some(sel) = selector {
        let handle = crate::hit_test::resolve_selector(ctx.client, sel).await?;
        let target = handle.report();
        let outcome = crate::element::set_checked_handle(ctx.client, &handle, sel, desired).await?;
        (target, outcome)
    } else if let Some(uid) = uid {
        let uid_map = ctx.uid_map();
        let on_intercept = on_intercept(intercept, ctx.report.on_intercept)?;
        let outcome =
            crate::element::set_checked(ctx.client, &uid_map, uid, desired, on_intercept).await?;
        (Some(json!({"uid": uid})), outcome)
    } else {
        return Err("check: provide \"uid\" or \"selector\"".into());
    };
    let (message, details) = crate::run_helpers::check_report(outcome);
    let mut obj = json!({"ok": true, "message": message});
    // The whole `target`, not just its `uid`: the selector path also resolved `role` and `name`,
    // which the CLI arm has always reported and this one dropped.
    merge_into(&mut obj, target.as_ref());
    // `observed_after_ms` is absent when the element already held the state: nothing was
    // dispatched, so there is no post-action moment.
    merge_into(&mut obj, details.as_ref());
    Ok(obj)
}

pub async fn dispatch_upload(
    ctx: &PageCtx<'_>,
    args: &UploadArgs,
) -> Result<Value, crate::BoxError> {
    let files = args
        .files
        .clone()
        .ok_or("upload: missing \"files\" array")?;
    let (target, msg) = if let Some(uid) = &args.uid {
        let uid_map = ctx.uid_map();
        crate::element::set_file_input(ctx.client, &uid_map, uid, &files).await?;
        (
            Some(json!({"uid": uid})),
            format!("Uploaded {} file(s) to uid={uid}", files.len()),
        )
    } else if let Some(sel) = &args.selector {
        let handle = crate::hit_test::resolve_selector(ctx.client, sel).await?;
        let target = handle.report();
        crate::element::set_file_input_handle(ctx.client, &handle, &files).await?;
        (
            target,
            format!("Uploaded {} file(s) to selector '{sel}'", files.len()),
        )
    } else {
        return Err("upload: provide \"uid\" or \"selector\"".into());
    };
    let mut obj = json!({"ok": true, "message": msg});
    merge_into(&mut obj, target.as_ref());
    Ok(obj)
}

pub async fn dispatch_drag(ctx: &PageCtx<'_>, args: &DragArgs) -> Result<Value, crate::BoxError> {
    let from = args.from.as_deref().ok_or("drag: missing \"from\" uid")?;
    let to = args.to.as_deref().ok_or("drag: missing \"to\" uid")?;
    let uid_map = ctx.uid_map();
    crate::element::drag(ctx.client, &uid_map, from, to).await?;
    Ok(json!({"ok": true, "message": format!("Dragged uid={from} to uid={to}")}))
}

// --- Assert ---

/// `assert` for pipe and batch. No exit code here, so `held` rides on `ok`: a claim that did
/// not hold answers `{"ok":false,"assertion":{…}}`, one that could not be checked answers
/// `{"ok":false,"error":…}`, and the presence of `assertion` tells them apart. Not in
/// `mutates_page`: a read, so no change report and no verdict.
///
/// The keys were checked by `AssertArgs`; the values go back through
/// `commands::assert::from_json`, the one parser the CLI also uses.
pub async fn dispatch_assert(
    ctx: &PageCtx<'_>,
    args: &AssertArgs,
) -> Result<Value, crate::BoxError> {
    let assertion = commands::assert::from_json(&args.as_value())?;
    let uid_map = ctx.uid_map();
    let outcome = commands::assert::run(ctx.client, &uid_map, &assertion).await?;
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
///
/// `args` stays a raw `Value`: they are the page's own schema, not this protocol's.
pub async fn dispatch_webmcp_call(
    client: &CdpClient,
    args: &WebmcpCallArgs,
) -> Result<Value, crate::BoxError> {
    let tool_args = args.args.clone().unwrap_or_else(|| json!({}));
    let args_text = serde_json::to_string(&tool_args)?;
    let outcome = commands::webmcp::call_tool(client, &args.name, &args_text).await?;
    Ok(commands::webmcp::call_report(&outcome))
}

// --- Batch ---

/// Run commands through `dispatch_single`. The one loop behind both batch front ends: CLI
/// `batch` and `{"cmd":"batch",…}` in pipe mode.
///
/// The entries stay `Value`: each is itself a command, parsed by `dispatch_single` in turn.
///
/// `stop_on_error` is off by default — a batch also collects independent observations. When
/// on, the response carries `stopped_at` and `skipped`.
pub async fn run_batch(
    ctx: &mut PageCtx<'_>,
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
            crate::pipe_dispatch::dispatch_single(&mut *ctx, c, &mut *emulation_recovery).await
        };
        emulation_recovery.update_after(c, &r);
        let ok = r.get("ok").and_then(Value::as_bool).unwrap_or(false);
        results.push(r);
        if stop_on_error && !ok {
            stopped_at = Some(index);
            break;
        }
    }
    let all_ok = results
        .iter()
        .all(|r| r.get("ok").and_then(Value::as_bool).unwrap_or(false));
    let mut obj = json!({"ok": all_ok, "results": results});
    if let Some(index) = stopped_at {
        obj["stopped_at"] = json!(index);
        obj["skipped"] = json!(commands_list.len() - index - 1);
    }
    obj
}
