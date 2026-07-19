use crate::cdp::client::{CdpClient, FrameContext};

/// Switch execution context to an iframe or back to main frame.
///
/// On success this binds subsequent `eval`/`inspect` on the same connection to
/// the target frame (via [`CdpClient::set_frame_context`]). Switching to
/// `main` clears the binding, restoring the top document.
pub async fn run(client: &CdpClient, target: &str) -> Result<String, crate::BoxError> {
    if target == "main" {
        // Clear any frame binding — eval/inspect fall back to the top document.
        client.set_frame_context(None);
        Ok("Switched to main frame".into())
    } else {
        // Resolve the iframe element by CSS selector, validating it exists and
        // is an <iframe>. Return the element itself (returnByValue:false) so we
        // get an objectId to map to its owner frame via DOM.describeNode — this
        // targets the *specific* iframe matched, not merely the first child frame.
        let js = format!(
            r"(() => {{
                const el = document.querySelector({sel});
                if (!el) throw new Error('No element matches selector: ' + {sel});
                if (el.tagName !== 'IFRAME') throw new Error('Element is not an <iframe>');
                return el;
            }})()",
            sel = serde_json::to_string(target).unwrap_or_default()
        );
        let result: serde_json::Value = client
            .call("Runtime.evaluate", serde_json::json!({"expression": js}))
            .await?;

        if let Some(exc) = result.get("exceptionDetails") {
            let msg = exc.get("exception")
                .and_then(|ex| ex.get("description"))
                .and_then(|d| d.as_str())
                .or_else(|| exc.get("text").and_then(|t| t.as_str()))
                .unwrap_or("unknown error");
            return Err(msg.into());
        }

        let object_id = result.get("result")
            .and_then(|r| r.get("objectId"))
            .and_then(|id| id.as_str())
            .ok_or("Could not resolve iframe element")?;

        // Map the iframe element to the frameId of the document it hosts.
        let described: serde_json::Value = client
            .call("DOM.describeNode", serde_json::json!({"objectId": object_id}))
            .await?;
        let frame_id = described.get("node")
            .and_then(|n| n.get("frameId"))
            .and_then(|id| id.as_str())
            .ok_or("Could not determine the iframe's frameId")?
            .to_string();

        // Create an isolated world in the target frame and bind subsequent
        // eval/inspect on this connection to it.
        let world: serde_json::Value = client
            .call("Page.createIsolatedWorld", serde_json::json!({
                "frameId": frame_id,
            }))
            .await?;
        let context_id = world.get("executionContextId")
            .and_then(serde_json::Value::as_i64)
            .ok_or("Could not get execution context for iframe")?;

        client.set_frame_context(Some(FrameContext {
            frame_id: frame_id.clone(),
            context_id,
        }));

        Ok(format!("Switched to iframe '{target}' (frameId={frame_id})"))
    }
}
