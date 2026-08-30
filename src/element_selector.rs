//! CSS-selector-based element actions (click, double-click, fill, focus). Re-exported from
//! `element`.

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::element::{ElementError, PointerVerb, js_exception};

/// Bind `el` to what the selector matches, in the words the rest of the tool uses.
///
/// Two failures, two messages, and they were one: `document.querySelector("[")` throws a
/// `SyntaxError`, which reached the caller as `SyntaxError: Failed to execute 'querySelector' on
/// 'Document': …` — true, and silent about the fact that the SELECTOR is what could not be read.
/// `hit_test::resolve_selector` says `Selector '[' could not be used: …` on the pointer paths;
/// this is the same sentence for the paths that resolve in page instead of taking a handle.
pub fn bind_element_js(selector: &str) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    let unusable = serde_json::to_string(&format!("Selector '{selector}' could not be used: "))
        .unwrap_or_default();
    format!(
        r"let el;
            try {{ el = document.querySelector({sel}); }}
            catch (e) {{ throw new Error({unusable} + e); }}
            if (!el) throw new Error('No element matches selector: ' + {sel});"
    )
}

/// Single-click an element matched by a CSS selector.
///
/// Resolves ONE handle and does everything through it — aim probe, dispatch, uid on the response
/// — so a re-render between round trips cannot bind three different nodes. Dispatching real input
/// rather than `el.click()` means a covered element hands the click to whatever covers it, and
/// the response names it. Falls back to a JS `click()` only when there is no layout box.
pub async fn click_selector(
    client: &CdpClient,
    selector: &str,
    on_intercept: crate::hit_test::OnIntercept,
) -> Result<crate::hit_test::Dispatched, ElementError> {
    pointer_selector(client, PointerVerb::Click, selector, on_intercept).await
}

/// Double-click an element matched by a CSS selector.
///
/// Same single-handle path as `click_selector`, dispatching a genuine native double-click.
pub async fn dblclick_selector(
    client: &CdpClient,
    selector: &str,
    on_intercept: crate::hit_test::OnIntercept,
) -> Result<crate::hit_test::Dispatched, ElementError> {
    pointer_selector(client, PointerVerb::DblClick, selector, on_intercept).await
}

/// Aim `verb` at the one handle the selector resolved to, and name that node on whatever comes
/// back. `named` runs only on the `Ok` path, and a refusal needs the node too.
async fn pointer_selector(
    client: &CdpClient,
    verb: PointerVerb,
    selector: &str,
    on_intercept: crate::hit_test::OnIntercept,
) -> Result<crate::hit_test::Dispatched, ElementError> {
    let handle = crate::hit_test::resolve_selector(client, selector).await?;
    let outcome = crate::element::aim_and_dispatch(
        client,
        verb,
        &handle.object_id,
        None,
        on_intercept,
        &format!("selector '{selector}'"),
    )
    .await
    .map_err(|e| e.naming(handle.uid.clone(), handle.role.clone(), handle.name.clone()))?;
    Ok(outcome.named(handle.uid, handle.role, handle.name))
}

/// Fill an element matched by a CSS selector, and report what the page holds afterwards. Probe,
/// write and read-back happen in one evaluation, so all three bind the same node.
///
/// `asserted_secret` is the caller's own claim about the value; it only ever ADDS redaction to
/// what `element::SECRET_FIELD` read off the element. See `element::fill_with`.
pub async fn fill_selector_with(
    client: &CdpClient,
    selector: &str,
    value: &str,
    asserted_secret: bool,
) -> Result<crate::element::FillOutcome, ElementError> {
    let handle = crate::hit_test::resolve_selector(client, selector).await?;
    fill_selector_handle(client, &handle, value, asserted_secret).await
}

/// Fill the exact selector handle the response will name.
pub async fn fill_selector_handle(
    client: &CdpClient,
    handle: &crate::hit_test::SelectorHandle,
    value: &str,
    asserted_secret: bool,
) -> Result<crate::element::FillOutcome, ElementError> {
    crate::element::fill_object_with(client, &handle.object_id, value, asserted_secret).await
}

/// Focus an element matched by a CSS selector via `Runtime.evaluate`.
pub async fn focus_selector(client: &CdpClient, selector: &str) -> Result<(), ElementError> {
    let js = format!(
        r"(() => {{
            {bind}
            el.focus();
        }})()",
        bind = bind_element_js(selector)
    );
    let result: serde_json::Value = client
        .call(
            "Runtime.evaluate",
            json!({ "expression": js, "returnByValue": true }),
        )
        .await
        .map_err(|e| ElementError::Action(format!("focus_selector failed: {e}")))?;

    // A throw here is a selector that named nothing (or could not be read), which is `NotFound`
    // rather than a failed action — the same reading, a different variant.
    if let Some(thrown) = js_exception(&result) {
        return Err(ElementError::NotFound(thrown));
    }

    Ok(())
}
