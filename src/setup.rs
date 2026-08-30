//! Page-level setup: console interceptor injection and stealth patches.

use serde_json::json;

use crate::cdp::client::CdpClient;

/// How to answer JavaScript dialogs (`alert`/`confirm`/`prompt`/`beforeunload`).
///
/// A native dialog blocks the page with no DOM signal, so an unanswered one hangs the next
/// command. `Accept`/`Dismiss` auto-answer; `Manual` leaves dialogs alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogPolicy {
    Accept,
    Dismiss,
    Manual,
}

impl DialogPolicy {
    /// Parse the `--dialog` flag value (case-insensitive).
    pub fn parse(s: &str) -> Result<Self, crate::BoxError> {
        match s.to_ascii_lowercase().as_str() {
            "accept" => Ok(Self::Accept),
            "dismiss" => Ok(Self::Dismiss),
            "manual" => Ok(Self::Manual),
            other => {
                Err(format!("Unknown --dialog {other:?}. Use \"accept\", \"dismiss\", or \"manual\".").into())
            }
        }
    }

    /// Whether a background handler should be installed for this policy.
    #[must_use]
    pub const fn auto_handles(self) -> bool {
        !matches!(self, Self::Manual)
    }
}

/// The concrete response to send to `Page.handleJavaScriptDialog`.
#[derive(Debug, PartialEq, Eq)]
pub struct DialogResponse {
    pub accept: bool,
    /// Text to submit for a `prompt()`; `None` for other dialog types.
    pub prompt_text: Option<String>,
}

/// Decide how to answer a dialog of `dialog_type` under `policy`.
///
/// `Accept` confirms every dialog (for `beforeunload`, "proceed") and supplies
/// `--dialog-text` to a `prompt`, empty if none. `Dismiss` and `Manual` cancel.
#[must_use]
pub fn dialog_decision(policy: DialogPolicy, dialog_type: &str, text: Option<&str>) -> DialogResponse {
    match policy {
        DialogPolicy::Accept => DialogResponse {
            accept: true,
            prompt_text: (dialog_type == "prompt").then(|| text.unwrap_or("").to_string()),
        },
        DialogPolicy::Dismiss | DialogPolicy::Manual => {
            DialogResponse { accept: false, prompt_text: None }
        }
    }
}

/// Apply stealth anti-detection patches. Must be called after `Page.enable`.
///
/// All four are attempted unconditionally and reported independently; a failure is named on
/// stderr (never stdout, so `--json` stays clean) and is not an error, since a browser
/// missing one patch still works. `Page.addScriptToEvaluateOnNewDocument` reports only that
/// Chrome accepted the script, not that it ran without throwing on the next document.
pub async fn apply_stealth(client: &CdpClient) {
    // Named per patch: which one failed decides which fingerprint is still exposed.
    if let Err(e) = client.enable("Network").await {
        warn_patch("Network.enable (required by the user-agent override)", &e.to_string());
    }

    // 1. navigator.webdriver + chrome.runtime, Permissions, WebGL/WebGL2 and the
    //    screenX/pageX input leak — injected before ANY page JS runs.
    if let Err(e) = client
        .send(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": STEALTH_PATCHES_JS }),
        )
        .await
    {
        warn_patch(
            "Page.addScriptToEvaluateOnNewDocument (all 7 fingerprint patches, every future document)",
            &e.to_string(),
        );
    }

    // 2. Patch the current page immediately (in case we connected mid-session).
    //
    // Guarded on reading the property first: `Object.defineProperty` makes a
    // non-configurable own property, so on a page step 1 already patched this throws
    // "TypeError: Cannot redefine property: webdriver" — the normal case for every
    // `--stealth` command after the first, which would warn on nearly every invocation.
    // A page that froze `navigator.webdriver` itself still reports
    // (`tests/fixtures/webdriver_locked.html`). The guard removes a throw, not a patch.
    let webdriver_now: Result<crate::cdp::types::EvaluateResult, _> = client
        .call(
            "Runtime.evaluate",
            json!({"expression": "if (navigator.webdriver !== undefined) { \
                Object.defineProperty(navigator, 'webdriver', { get: () => undefined }); \
            }"}),
        )
        .await;
    match webdriver_now {
        // An evaluation that throws answers `Ok` and reports it in `exceptionDetails`,
        // so the result is read rather than discarded.
        Ok(r) => {
            if let Some(exception) = &r.exception_details {
                // `text` alone is just "Uncaught"; the description carries the reason.
                // First line only — the rest is a stack trace inside a one-line
                // expression this file wrote.
                let reason = exception
                    .exception
                    .as_ref()
                    .and_then(|e| e.description.as_deref())
                    .unwrap_or(&exception.text);
                warn_patch(
                    "Runtime.evaluate navigator.webdriver (the already-loaded page)",
                    reason.lines().next().unwrap_or(reason),
                );
            }
        }
        Err(e) => warn_patch(
            "Runtime.evaluate navigator.webdriver (the already-loaded page)",
            &e.to_string(),
        ),
    }

    // 3. Override user-agent to remove "HeadlessChrome"
    if let Err(e) = client
        .send(
            "Network.setUserAgentOverride",
            json!({
                "userAgent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
                "acceptLanguage": "en-US,en;q=0.9",
                "platform": "MacIntel"
            }),
        )
        .await
    {
        warn_patch(
            "Network.setUserAgentOverride (the UA still says HeadlessChrome)",
            &e.to_string(),
        );
    }
}

/// Report one stealth patch that did not land, on stderr.
fn warn_patch(patch: &str, reason: &str) {
    eprintln!("warning: stealth patch not applied — {patch}: {reason}");
}

// --- JS source constants ---

const STEALTH_PATCHES_JS: &str = r#"
    Object.defineProperty(navigator, 'webdriver', { get: () => undefined });
    // Mask chrome.runtime (headless doesn't have it)
    if (!window.chrome) window.chrome = {};
    if (!window.chrome.runtime) window.chrome.runtime = { connect: () => {}, sendMessage: () => {} };
    // Mask Permissions API inconsistency (headless returns "prompt" for notifications)
    const perms = navigator.permissions;
    const origQuery = window.Permissions && Permissions.prototype.query;
    if (origQuery && perms) {
        Permissions.prototype.query = (params) => (
            params.name === 'notifications'
                ? Promise.resolve({ state: Notification.permission })
                : origQuery.call(perms, params)
        );
    }
    // Mask webGL vendor/renderer (headless gives "Google Inc." / "ANGLE")
    const getParam = WebGLRenderingContext.prototype.getParameter;
    WebGLRenderingContext.prototype.getParameter = function(param) {
        if (param === 37445) return 'Intel Inc.';
        if (param === 37446) return 'Intel Iris OpenGL Engine';
        return getParam.call(this, param);
    };
    // WebGL2 is a sibling interface (does not inherit from WebGLRenderingContext),
    // so its getParameter leaks the real headless vendor/renderer unless patched too.
    if (typeof WebGL2RenderingContext !== 'undefined') {
        const getParam2 = WebGL2RenderingContext.prototype.getParameter;
        WebGL2RenderingContext.prototype.getParameter = function(param) {
            if (param === 37445) return 'Intel Inc.';
            if (param === 37446) return 'Intel Iris OpenGL Engine';
            return getParam2.call(this, param);
        };
    }
    // Fix CDP input leak: screenX/screenY == pageX/pageY reveals automation.
    const __screenOffset = { x: Math.floor(Math.random() * 100) + 50, y: Math.floor(Math.random() * 100) + 80 };
    const origMouseEvent = MouseEvent;
    window.MouseEvent = class extends origMouseEvent {
        constructor(type, init = {}) {
            if (init.screenX !== undefined) init.screenX += __screenOffset.x;
            if (init.screenY !== undefined) init.screenY += __screenOffset.y;
            super(type, init);
        }
    };
"#;

#[cfg(test)]
mod tests {
    use super::{dialog_decision, DialogPolicy, DialogResponse, STEALTH_PATCHES_JS};

    #[test]
    fn policy_parse_is_case_insensitive() {
        assert_eq!(DialogPolicy::parse("Accept").unwrap(), DialogPolicy::Accept);
        assert_eq!(DialogPolicy::parse("DISMISS").unwrap(), DialogPolicy::Dismiss);
        assert_eq!(DialogPolicy::parse("manual").unwrap(), DialogPolicy::Manual);
        assert!(DialogPolicy::parse("nope").is_err());
    }

    #[test]
    fn only_manual_skips_handler() {
        assert!(DialogPolicy::Accept.auto_handles());
        assert!(DialogPolicy::Dismiss.auto_handles());
        assert!(!DialogPolicy::Manual.auto_handles());
    }

    #[test]
    fn accept_confirms_alert_and_confirm_without_prompt_text() {
        for t in ["alert", "confirm"] {
            assert_eq!(
                dialog_decision(DialogPolicy::Accept, t, Some("ignored")),
                DialogResponse { accept: true, prompt_text: None }
            );
        }
    }

    #[test]
    fn accept_supplies_prompt_text() {
        assert_eq!(
            dialog_decision(DialogPolicy::Accept, "prompt", Some("hello")),
            DialogResponse { accept: true, prompt_text: Some("hello".into()) }
        );
        // prompt with no --dialog-text defaults to empty string, still accepted.
        assert_eq!(
            dialog_decision(DialogPolicy::Accept, "prompt", None),
            DialogResponse { accept: true, prompt_text: Some(String::new()) }
        );
    }

    #[test]
    fn accept_proceeds_through_beforeunload() {
        // "proceed with navigation" == accept=true, no prompt text.
        assert_eq!(
            dialog_decision(DialogPolicy::Accept, "beforeunload", None),
            DialogResponse { accept: true, prompt_text: None }
        );
    }

    #[test]
    fn dismiss_cancels_every_type() {
        for t in ["alert", "confirm", "prompt", "beforeunload"] {
            assert_eq!(
                dialog_decision(DialogPolicy::Dismiss, t, Some("x")),
                DialogResponse { accept: false, prompt_text: None }
            );
        }
    }

    #[test]
    fn manual_falls_through_to_cancel() {
        // Handler is gated out for Manual, but the arm is live: pin it to "cancel".
        assert_eq!(
            dialog_decision(DialogPolicy::Manual, "confirm", Some("x")),
            DialogResponse { accept: false, prompt_text: None }
        );
    }

    #[test]
    fn permissions_patch_calls_query_on_the_instance_not_the_prototype() {
        // `origQuery.call(Permissions.prototype, ...)` throws "Illegal invocation":
        // query must run against a real navigator.permissions instance.
        assert!(
            STEALTH_PATCHES_JS.contains("const perms = navigator.permissions;"),
            "permissions instance must be captured"
        );
        assert!(
            STEALTH_PATCHES_JS.contains("origQuery.call(perms, params)"),
            "query must be invoked on the permissions instance"
        );
        assert!(
            !STEALTH_PATCHES_JS.contains("origQuery.call(Permissions.prototype"),
            "must not invoke query on the prototype (Illegal invocation)"
        );
    }

    #[test]
    fn webgl2_get_parameter_is_patched() {
        // WebGL2RenderingContext does not inherit from WebGLRenderingContext, so its
        // getParameter needs its own override or webgl2 leaks the headless vendor.
        assert!(
            STEALTH_PATCHES_JS.contains("WebGL2RenderingContext.prototype.getParameter"),
            "WebGL2 getParameter must be overridden"
        );
        // Existence-guarded so it does not throw where WebGL2 is unavailable.
        assert!(
            STEALTH_PATCHES_JS.contains("typeof WebGL2RenderingContext !== 'undefined'"),
            "WebGL2 override must be existence-guarded"
        );
    }
}
