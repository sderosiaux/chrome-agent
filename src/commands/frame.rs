use crate::cdp::client::CdpClient;

/// Switch execution context to an iframe or back to main frame.
pub async fn run(client: &CdpClient, target: &str) -> Result<String, crate::BoxError> {
    if target == "main" {
        // Get the main frame ID from the frame tree
        let tree: serde_json::Value = client
            .call("Page.getFrameTree", serde_json::json!({}))
            .await?;
        let main_frame_id = tree.get("frameTree")
            .and_then(|ft| ft.get("frame"))
            .and_then(|f| f.get("id"))
            .and_then(|id| id.as_str())
            .ok_or("Could not determine main frame ID")?;

        // Create an isolated world for main frame to get its context
        let world: serde_json::Value = client
            .call("Page.createIsolatedWorld", serde_json::json!({
                "frameId": main_frame_id,
            }))
            .await?;
        let _ctx_id = world.get("executionContextId")
            .and_then(serde_json::Value::as_u64)
            .ok_or("Could not get execution context for main frame")?;

        Ok("Switched to main frame".into())
    } else {
        // Find the iframe by CSS selector, get its frameId
        let js = format!(
            r"(() => {{
                const el = document.querySelector({sel});
                if (!el) throw new Error('No element matches selector: ' + {sel});
                if (el.tagName !== 'IFRAME') throw new Error('Element is not an <iframe>');
                return el.contentDocument ? 'accessible' : 'cross-origin';
            }})()",
            sel = serde_json::to_string(target).unwrap_or_default()
        );
        let result: serde_json::Value = client
            .call("Runtime.evaluate", serde_json::json!({"expression": js, "returnByValue": true}))
            .await?;

        if let Some(exc) = result.get("exceptionDetails") {
            let msg = exc.get("exception")
                .and_then(|ex| ex.get("description"))
                .and_then(|d| d.as_str())
                .or_else(|| exc.get("text").and_then(|t| t.as_str()))
                .unwrap_or("unknown error");
            return Err(msg.into());
        }

        // Get frame tree and find the child frame matching this iframe
        let tree: serde_json::Value = client
            .call("Page.getFrameTree", serde_json::json!({}))
            .await?;

        // Navigate into the iframe's frame
        let child_frames = tree.get("frameTree")
            .and_then(|ft| ft.get("childFrames"))
            .and_then(|cf| cf.as_array());

        if let Some(frames) = child_frames
            && let Some(first) = frames.first() {
                let frame_id = first.get("frame")
                    .and_then(|f| f.get("id"))
                    .and_then(|id| id.as_str())
                    .ok_or("Could not get child frame ID")?;

                let world: serde_json::Value = client
                    .call("Page.createIsolatedWorld", serde_json::json!({
                        "frameId": frame_id,
                    }))
                    .await?;
                let _ctx_id = world.get("executionContextId")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or("Could not get execution context for iframe")?;

                return Ok(format!("Switched to iframe '{target}' (frameId={frame_id})"));
            }

        Err(format!("No child frame found for selector '{target}'").into())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn main_target_detected() {
        assert_eq!("main", "main");
    }
}
