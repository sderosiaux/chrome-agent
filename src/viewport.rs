//! Exact desktop viewport, owned by a named page rather than its OS window.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{BoxError, cdp::client::CdpClient, page_ctx::PageCtx, session::PageSession};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub dpr: f64,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            width: 1440,
            height: 900,
            dpr: 1.0,
        }
    }
}

impl fmt::Display for Viewport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}@{}", self.width, self.height, self.dpr)
    }
}

impl FromStr for Viewport {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let (size, dpr) = raw.split_once('@').unwrap_or((raw, "1"));
        let (width, height) = size
            .split_once('x')
            .ok_or("expected WxH[@dpr], e.g. 1440x900 or 390x844@2")?;
        let dimension = |value: &str| {
            if value.is_empty() || !value.bytes().all(|c| c.is_ascii_digit()) {
                return Err("viewport dimensions must be positive integers".to_string());
            }
            value
                .parse::<u32>()
                .map_err(|_| "viewport dimension is out of range".to_string())
        };
        let viewport = Self {
            width: dimension(width)?,
            height: dimension(height)?,
            dpr: dpr
                .parse()
                .map_err(|_| "dpr must be a finite number greater than zero")?,
        };
        viewport.validate()?;
        Ok(viewport)
    }
}

impl Viewport {
    fn validate(self) -> Result<(), String> {
        if !(1..=10_000_000).contains(&self.width) || !(1..=10_000_000).contains(&self.height) {
            return Err("viewport width and height must be between 1 and 10000000".into());
        }
        if !self.dpr.is_finite() || self.dpr <= 0.0 {
            return Err("dpr must be a finite number greater than zero".into());
        }
        Ok(())
    }
}

/// Device emulation, when explicitly enabled, temporarily supersedes the desktop viewport.
pub fn requested(page: &PageSession) -> Viewport {
    page.device_emulation
        .as_ref()
        .map_or(page.viewport, |device| Viewport {
            width: device.width,
            height: device.height,
            dpr: device.device_scale_factor,
        })
}

pub async fn apply(client: &CdpClient, viewport: Viewport) -> Result<(), BoxError> {
    viewport.validate()?;
    client
        .send(
            "Emulation.setDeviceMetricsOverride",
            json!({
                "width": viewport.width,
                "height": viewport.height,
                "deviceScaleFactor": viewport.dpr,
                "mobile": false,
                "screenWidth": viewport.width,
                "screenHeight": viewport.height,
                // Resize the rendered surface as well, even if the headed OS window is smaller.
                "dontSetVisibleSize": false,
            }),
        )
        .await?;
    Ok(())
}

/// Always measure the top document; a prior `frame` command must not turn this into an iframe
/// measurement. Missing readings are errors, never zeros or values copied from the request.
pub async fn measure(client: &CdpClient) -> Result<Value, BoxError> {
    let result: crate::cdp::types::EvaluateResult = client.call("Runtime.evaluate", json!({
        "expression": "({innerWidth,innerHeight,outerWidth,outerHeight,screen:{width:screen.width,height:screen.height},devicePixelRatio,min1024:matchMedia('(min-width: 1024px)').matches})",
        "returnByValue": true,
    })).await?;
    if let Some(exception) = result.exception_details {
        return Err(format!("Viewport measurement failed: {}", exception.text).into());
    }
    let value = result
        .result
        .value
        .ok_or("Viewport measurement returned no value")?;
    for key in ["innerWidth", "innerHeight", "devicePixelRatio"] {
        if value[key].as_f64().is_none() {
            return Err(format!("Viewport measurement missing {key}").into());
        }
    }
    Ok(value)
}

pub fn report(requested: Viewport, reading: Result<Value, String>) -> Value {
    match reading {
        Ok(effective) => {
            let delta = json!({
                "width": effective["innerWidth"].as_f64().unwrap_or(f64::NAN) - f64::from(requested.width),
                "height": effective["innerHeight"].as_f64().unwrap_or(f64::NAN) - f64::from(requested.height),
                "dpr": effective["devicePixelRatio"].as_f64().unwrap_or(f64::NAN) - requested.dpr,
            });
            let exact = ["width", "height", "dpr"]
                .iter()
                .all(|key| delta[key].as_f64() == Some(0.0));
            json!({"ok": exact, "verdict": if exact {"exact"} else {"mismatch"},
                "requested": requested, "effective": effective, "delta": delta})
        }
        Err(error) => json!({"ok": false, "verdict": "unknown", "requested": requested,
            "effective": null, "error": error}),
    }
}

pub async fn get(ctx: &PageCtx<'_>) -> Result<Value, BoxError> {
    let page = ctx
        .page_state()
        .ok_or("Current page disappeared from the session store")?;
    Ok(report(
        requested(page),
        measure(ctx.client).await.map_err(|e| e.to_string()),
    ))
}

/// Commit the requested state only after CDP accepts it. Read-back remains mandatory even
/// with `--verdict off`; a mismatch is persisted as a request, never represented as success.
pub async fn set(ctx: &mut PageCtx<'_>, viewport: Viewport) -> Result<Value, BoxError> {
    viewport.validate()?;
    let had_device = ctx
        .page_state()
        .ok_or("Current page disappeared from the session store")?
        .device_emulation
        .is_some();
    let applying = async {
        if had_device {
            crate::emulation::clear_overrides(ctx.client).await?;
        }
        apply(ctx.client, viewport).await
    }
    .await
    .map_err(|error| error.to_string());
    if let Err(error) = applying {
        crate::emulation::reapply(ctx.client, ctx.store, ctx.browser, ctx.page)
            .await
            .map_err(|restore| format!("{error}; restoring previous metrics failed: {restore}"))?;
        return Err(error.into());
    }
    let page = ctx
        .store
        .browsers
        .get_mut(ctx.browser)
        .and_then(|b| b.pages.get_mut(ctx.page))
        .ok_or("Current page disappeared from the session store")?;
    page.viewport = viewport;
    page.device_emulation = None;
    crate::session::save_session(ctx.store)?;
    get(ctx).await
}

pub fn text_report(value: &Value) -> String {
    let mut text = format!(
        "Viewport: requested={}x{}@{} measured={}x{}@{} verdict={} delta={}",
        value["requested"]["width"],
        value["requested"]["height"],
        value["requested"]["dpr"],
        value["effective"]["innerWidth"],
        value["effective"]["innerHeight"],
        value["effective"]["devicePixelRatio"],
        value["verdict"].as_str().unwrap_or("unknown"),
        value["delta"]
    );
    if let Some(error) = value["error"].as_str() {
        text.push_str(&format!(" error={error}"));
    }
    text
}

/// Preserve the measured failure in CLI JSON and exit nonzero without flattening it to a
/// generic error string. Pipe responses already carry this same object.
#[derive(Debug)]
pub struct NotExact(pub Value);

impl fmt::Display for NotExact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for NotExact {}

pub fn require_exact(response: Value) -> Result<Value, BoxError> {
    if response["ok"] == true {
        Ok(response)
    } else {
        Err(Box::new(NotExact(response)))
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ViewportArgs {
    Set { size: String },
    Get {},
}

pub async fn dispatch(ctx: &mut PageCtx<'_>, args: &ViewportArgs) -> Result<Value, BoxError> {
    match args {
        ViewportArgs::Set { size } => set(ctx, size.parse()?).await,
        ViewportArgs::Get {} => get(ctx).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_rejects_unknown_fields_and_invalid_sizes_before_dispatch() {
        for command in [
            json!({"cmd":"viewport","action":"get","size":"1024x768"}),
            json!({"cmd":"viewport","action":"set","size":"1024x768","width":12}),
            json!({"cmd":"viewport","action":"set","size":"0x768"}),
            json!({"cmd":"viewport","action":"set","size":1024}),
            json!({"cmd":"viewport","action":"unknown"}),
        ] {
            assert!(
                crate::pipe_command::parse(&command).is_err(),
                "accepted {command}"
            );
        }
    }

    #[test]
    fn parses_css_dimensions_and_optional_dpr() {
        assert_eq!("1440x900".parse::<Viewport>().unwrap(), Viewport::default());
        assert_eq!(
            "390x844@2".parse::<Viewport>().unwrap(),
            Viewport {
                width: 390,
                height: 844,
                dpr: 2.0
            }
        );
        assert!(("1024x768@1.25".parse::<Viewport>().unwrap().dpr - 1.25).abs() < f64::EPSILON);
    }

    #[test]
    fn rejects_malformed_zero_nonfinite_and_out_of_range() {
        for value in [
            "",
            "1440",
            "1X2",
            "1x2x3",
            "1.5x2",
            "-1x2",
            "+1x2",
            "0x2",
            "1x0",
            "10000001x2",
            "1x4294967296",
            "1x2@",
            "1x2@2@3",
            "1x2@0",
            "1x2@-1",
            "1x2@NaN",
            "1x2@inf",
            "1x2@1e999",
            " 1x2",
        ] {
            assert!(value.parse::<Viewport>().is_err(), "accepted {value}");
        }
    }

    #[test]
    fn verdict_is_based_on_reading_including_dpr_and_failure() {
        let requested = Viewport::default();
        let exact = report(
            requested,
            Ok(json!({"innerWidth":1440,"innerHeight":900,"devicePixelRatio":1})),
        );
        assert_eq!(exact["verdict"], "exact");
        for measured in [
            json!({"innerWidth":756,"innerHeight":469,"devicePixelRatio":1}),
            json!({"innerWidth":1440,"innerHeight":900,"devicePixelRatio":2}),
        ] {
            let mismatch = report(requested, Ok(measured));
            assert_eq!(mismatch["ok"], false);
            assert_eq!(mismatch["verdict"], "mismatch");
        }
        assert_eq!(
            report(requested, Err("unreadable".into()))["verdict"],
            "unknown"
        );
    }
}
