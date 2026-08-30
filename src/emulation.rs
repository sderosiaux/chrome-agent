use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cdp::client::{CdpClient, CdpClientError};
use crate::cdp::types::EvaluateResult;
use crate::session::{PageSession, SessionStore};

// CDP's upper bound for the `screenWidth` and `screenHeight` fields in `override_params()`.
const MAX_CDP_SCREEN_DIMENSION: u32 = 10_000_000;

/// Explicit metrics persisted for one named page. No preset identifier: CDP accepts concrete
/// metrics, and the device catalog belongs to the `DevTools` frontend, not the protocol.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEmulation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub width: u32,
    pub height: u32,
    #[serde(rename = "dpr", alias = "deviceScaleFactor")]
    pub device_scale_factor: f64,
    pub mobile: bool,
    pub touch: bool,
    pub orientation: DeviceOrientation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum DeviceOrientation {
    Portrait,
    Landscape,
}

impl DeviceOrientation {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "portrait" => Ok(Self::Portrait),
            "landscape" => Ok(Self::Landscape),
            _ => Err(format!(
                "unknown orientation {value:?}; use \"portrait\" or \"landscape\""
            )),
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Portrait => "portrait",
            Self::Landscape => "landscape",
        }
    }
}

impl DeviceEmulation {
    pub fn new(
        label: Option<String>,
        width: u32,
        height: u32,
        device_scale_factor: f64,
        mobile: bool,
        touch: bool,
        orientation: Option<DeviceOrientation>,
    ) -> Result<Self, String> {
        if width == 0 || width > MAX_CDP_SCREEN_DIMENSION {
            return Err(format!(
                "width must be between 1 and {MAX_CDP_SCREEN_DIMENSION}"
            ));
        }
        if height == 0 || height > MAX_CDP_SCREEN_DIMENSION {
            return Err(format!(
                "height must be between 1 and {MAX_CDP_SCREEN_DIMENSION}"
            ));
        }
        if !device_scale_factor.is_finite() || device_scale_factor <= 0.0 {
            return Err("dpr must be a finite number greater than 0".into());
        }
        let label = label
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let orientation = orientation.unwrap_or(if width <= height {
            DeviceOrientation::Portrait
        } else {
            DeviceOrientation::Landscape
        });
        if matches!(orientation, DeviceOrientation::Portrait) && width > height {
            return Err("portrait orientation requires width no greater than height".into());
        }
        if matches!(orientation, DeviceOrientation::Landscape) && height >= width {
            return Err("landscape orientation requires width greater than height".into());
        }
        Ok(Self {
            label,
            width,
            height,
            device_scale_factor,
            mobile,
            touch,
            orientation,
        })
    }

    #[must_use]
    pub fn text_line(&self) -> String {
        let label = self
            .label
            .as_deref()
            .map_or_else(String::new, |label| format!(" label={label:?}"));
        format!(
            "Device emulation:{label} viewport={}x{} dpr={} mobile={} touch={} orientation={}",
            self.width,
            self.height,
            self.device_scale_factor,
            self.mobile,
            self.touch,
            self.orientation.as_str()
        )
    }

    fn override_params(&self) -> Value {
        let (orientation_type, angle) = match self.orientation {
            DeviceOrientation::Portrait => ("portraitPrimary", 0),
            DeviceOrientation::Landscape => ("landscapePrimary", 90),
        };
        // `screenWidth`/`screenHeight`/`screenOrientation` are experimental CDP fields. The
        // basic viewport override does not update `window.screen` or the Screen Orientation
        // API, so omitting them exposes contradictory dimensions to the page.
        json!({
            "width": self.width,
            "height": self.height,
            "deviceScaleFactor": self.device_scale_factor,
            "mobile": self.mobile,
            "screenWidth": self.width,
            "screenHeight": self.height,
            "screenOrientation": {
                "type": orientation_type,
                "angle": angle,
            },
        })
    }
}

/// Apply the complete override set to one CDP target. The three commands are one logical
/// operation: on any failure [`clear_overrides`] runs before the original error is returned.
async fn apply_overrides(
    client: &CdpClient,
    config: &DeviceEmulation,
) -> Result<(), CdpClientError> {
    if let Err(error) = client
        .send(
            "Emulation.setDeviceMetricsOverride",
            config.override_params(),
        )
        .await
    {
        let _ = clear_overrides(client).await;
        return Err(error);
    }

    // The option promises touch capability, not a finger count, so enabled uses CDP's default
    // of one. Disabled omits `maxTouchPoints`; Chromium rejects an explicit zero.
    let touch_params = if config.touch {
        json!({"enabled": true, "maxTouchPoints": 1})
    } else {
        json!({"enabled": false})
    };
    if let Err(error) = client
        .send("Emulation.setTouchEmulationEnabled", touch_params)
        .await
    {
        let _ = clear_overrides(client).await;
        return Err(error);
    }

    // Enabling this experimental switch can leave `Input.dispatchMouseEvent` unanswered
    // forever. Keep the browser-side conversion off; the input dispatcher emits explicit
    // touch events instead.
    if let Err(error) = client
        .send(
            "Emulation.setEmitTouchEventsForMouse",
            json!({"enabled": false, "configuration": "desktop"}),
        )
        .await
    {
        let _ = clear_overrides(client).await;
        return Err(error);
    }

    client.set_touch_emulation(config.touch);
    Ok(())
}

/// Clear every override set by [`apply_overrides`]. Each command is attempted even if an
/// earlier one fails; the first error is returned only once all three have run.
pub async fn clear_overrides(client: &CdpClient) -> Result<(), CdpClientError> {
    client.set_touch_emulation(false);
    let emit = client
        .send(
            "Emulation.setEmitTouchEventsForMouse",
            json!({"enabled": false, "configuration": "desktop"}),
        )
        .await;
    let touch = client
        .send(
            "Emulation.setTouchEmulationEnabled",
            json!({"enabled": false}),
        )
        .await;
    let metrics = client
        .send("Emulation.clearDeviceMetricsOverride", json!({}))
        .await;

    emit.and(touch).and(metrics)
}

/// Make the emulated page Chrome's active target before touching its overrides.
///
/// Required, not cosmetic: a background target's Screen Orientation API reports the ACTIVE
/// target's orientation, so `emulate status` on a mobile page reads "landscape" off a sibling
/// created just before it (e2e-pinned). Cost: one target per browser is foreground, so every
/// command on an emulated page backgrounds its siblings, throttling their rAF and timers.
async fn activate_target(client: &CdpClient, target_id: &str) -> Result<(), CdpClientError> {
    client
        .send("Target.activateTarget", json!({"targetId": target_id}))
        .await
}

/// Apply and persist a configuration for one named page. The session update is the commit
/// point: it happens only after CDP accepts the full override set and the page reports its
/// effective values. Any failure before that restores the configuration active at entry.
pub async fn apply_and_store(
    client: &CdpClient,
    store: &mut SessionStore,
    browser_name: &str,
    page_name: &str,
    config: DeviceEmulation,
) -> Result<Value, crate::BoxError> {
    let (previous, target_id) = match page_session(store, browser_name, page_name) {
        Ok(page) => (page.device_emulation.clone(), page.target_id.clone()),
        Err(error) => return Err(error),
    };
    activate_target(client, &target_id).await?;
    if let Err(error) = apply_overrides(client, &config).await {
        let message = error.to_string();
        restore_previous(client, previous.as_ref(), &message).await?;
        return Err(message.into());
    }
    let observed = match read_effective_metrics(client)
        .await
        .map_err(|error| error.to_string())
    {
        Ok(observed) => observed,
        Err(message) => {
            let _ = clear_overrides(client).await;
            restore_previous(client, previous.as_ref(), &message).await?;
            return Err(message.into());
        }
    };
    page_session_mut(store, browser_name, page_name)?.device_emulation = Some(config.clone());
    Ok(json!({"ok": true, "emulation": config, "effective": observed}))
}

/// Report both the requested configuration and the values currently observable by the page.
pub async fn status(
    client: &CdpClient,
    store: &SessionStore,
    browser_name: &str,
    page_name: &str,
) -> Result<Value, crate::BoxError> {
    let (config, target_id) = match page_session(store, browser_name, page_name) {
        Ok(page) => (page.device_emulation.clone(), page.target_id.clone()),
        Err(error) => return Err(error),
    };
    if let Some(config) = config {
        activate_target(client, &target_id).await?;
        let observed = read_effective_metrics(client).await?;
        Ok(json!({"ok": true, "emulation": config, "effective": observed}))
    } else {
        Ok(json!({"ok": true, "emulation": null}))
    }
}

/// Clear the target overrides before removing this page's persisted configuration.
pub async fn clear(
    client: &CdpClient,
    store: &mut SessionStore,
    browser_name: &str,
    page_name: &str,
) -> Result<Value, crate::BoxError> {
    let target_id = page_session(store, browser_name, page_name)?
        .target_id
        .clone();
    activate_target(client, &target_id).await?;
    clear_overrides(client).await?;
    page_session_mut(store, browser_name, page_name)?.device_emulation = None;
    Ok(json!({"ok": true, "emulation": null}))
}

/// Reapply the configuration attached to this browser and named-page pair. Chrome drops every
/// `Emulation.*` override when the CDP session that set it detaches, while the named page
/// survives across invocations, so each new connection reapplies the stored values first.
pub async fn reapply(
    client: &CdpClient,
    store: &SessionStore,
    browser_name: &str,
    page_name: &str,
) -> Result<(), crate::BoxError> {
    let (config, target_id) = match page_session(store, browser_name, page_name) {
        Ok(page) => (page.device_emulation.clone(), page.target_id.clone()),
        Err(error) => return Err(error),
    };
    if let Some(config) = config {
        activate_target(client, &target_id).await?;
        apply_overrides(client, &config).await?;
    }
    Ok(())
}

async fn restore_previous(
    client: &CdpClient,
    previous: Option<&DeviceEmulation>,
    original_error: &str,
) -> Result<(), crate::BoxError> {
    let Some(previous) = previous else {
        return Ok(());
    };
    apply_overrides(client, previous).await.map_err(|restore_error| {
        format!(
            "{original_error}; restoring the previous device configuration also failed: {restore_error}"
        )
        .into()
    })
}

fn page_session<'a>(
    store: &'a SessionStore,
    browser_name: &str,
    page_name: &str,
) -> Result<&'a PageSession, crate::BoxError> {
    store
        .browsers
        .get(browser_name)
        .and_then(|browser| browser.pages.get(page_name))
        .ok_or_else(|| "Current page disappeared from the session store".into())
}

fn page_session_mut<'a>(
    store: &'a mut SessionStore,
    browser_name: &str,
    page_name: &str,
) -> Result<&'a mut PageSession, crate::BoxError> {
    store
        .browsers
        .get_mut(browser_name)
        .and_then(|browser| browser.pages.get_mut(page_name))
        .ok_or_else(|| "Current page disappeared from the session store".into())
}

/// Format values returned by [`read_effective_metrics`] for human-readable status output.
#[must_use]
pub fn format_effective_metrics(value: &Value) -> String {
    format!(
        "Effective: viewport={}x{} screen={}x{} dpr={} touch_points={} coarse_pointer={} orientation={}",
        value["layoutViewport"]["width"],
        value["layoutViewport"]["height"],
        value["screen"]["width"],
        value["screen"]["height"],
        value["deviceScaleFactor"],
        value["touchPoints"],
        value["coarsePointer"],
        value["orientation"].as_str().unwrap_or("unknown"),
    )
}

/// Read the values page script observes after Chromium has applied the overrides. Measured
/// rather than copied from the request, so status shows Chromium's normalization.
async fn read_effective_metrics(client: &CdpClient) -> Result<Value, crate::BoxError> {
    // No `contextId`: emulation belongs to the page target, while `frame` binds ordinary eval
    // to an iframe. Status describes the top document without clearing that binding.
    let result: EvaluateResult = client
        .call(
            "Runtime.evaluate",
            json!({
                "expression": r"({
                    layoutViewport: {width: innerWidth, height: innerHeight},
                    screen: {width: screen.width, height: screen.height},
                    deviceScaleFactor: devicePixelRatio,
                    touchPoints: navigator.maxTouchPoints,
                    coarsePointer: matchMedia('(pointer: coarse)').matches,
                    orientation: screen.orientation.type.startsWith('portrait')
                        ? 'portrait'
                        : 'landscape'
                })",
                "returnByValue": true,
                "awaitPromise": true,
            }),
        )
        .await?;

    if let Some(exception) = &result.exception_details {
        return Err(format!(
            "Evaluation error: {}",
            exception
                .exception
                .as_ref()
                .and_then(|error| error.description.as_deref())
                .unwrap_or(&exception.text)
        )
        .into());
    }

    Ok(result.result.value.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientation_parser_matches_cli_casing() {
        assert_eq!(
            DeviceOrientation::parse("portrait"),
            Ok(DeviceOrientation::Portrait)
        );
        assert_eq!(
            DeviceOrientation::parse("landscape"),
            Ok(DeviceOrientation::Landscape)
        );
        assert!(DeviceOrientation::parse("Portrait").is_err());
    }

    #[test]
    fn orientation_defaults_from_dimensions() {
        let portrait = DeviceEmulation::new(None, 412, 915, 2.625, true, true, None).unwrap();
        let landscape = DeviceEmulation::new(None, 915, 412, 2.625, true, true, None).unwrap();
        assert_eq!(portrait.orientation, DeviceOrientation::Portrait);
        assert_eq!(landscape.orientation, DeviceOrientation::Landscape);
    }

    #[test]
    fn invalid_metrics_are_refused_before_cdp() {
        assert!(DeviceEmulation::new(None, 0, 915, 1.0, true, true, None).is_err());
        assert!(DeviceEmulation::new(None, 412, 0, 1.0, true, true, None).is_err());
        assert!(DeviceEmulation::new(None, 412, 915, 0.0, true, true, None).is_err());
        assert!(DeviceEmulation::new(None, 412, 915, f64::NAN, true, true, None).is_err());
        assert!(DeviceEmulation::new(None, 412, 915, f64::INFINITY, true, true, None).is_err());
        assert!(DeviceEmulation::new(None, 412, 915, 11.0, true, true, None).is_ok());
        assert!(
            DeviceEmulation::new(
                None,
                500,
                500,
                1.0,
                false,
                false,
                Some(DeviceOrientation::Landscape),
            )
            .is_err()
        );
    }

    #[test]
    fn requested_json_uses_the_same_dpr_name_as_pipe_input() {
        let config = DeviceEmulation::new(None, 390, 844, 3.0, true, true, None).unwrap();
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(value["dpr"], 3.0);
        assert!(value.get("deviceScaleFactor").is_none());

        let legacy: DeviceEmulation = serde_json::from_value(json!({
            "width": 390,
            "height": 844,
            "deviceScaleFactor": 2.0,
            "mobile": true,
            "touch": true,
            "orientation": "portrait"
        }))
        .unwrap();
        assert!((legacy.device_scale_factor - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn effective_values_have_a_compact_human_form() {
        let text = format_effective_metrics(&json!({
            "layoutViewport": {"width": 412, "height": 915},
            "screen": {"width": 412, "height": 915},
            "deviceScaleFactor": 2.625,
            "touchPoints": 1,
            "coarsePointer": true,
            "orientation": "portrait",
        }));
        assert_eq!(
            text,
            "Effective: viewport=412x915 screen=412x915 dpr=2.625 touch_points=1 coarse_pointer=true orientation=portrait"
        );
    }

    #[test]
    fn labels_are_trimmed_and_empty_labels_are_omitted() {
        let named = DeviceEmulation::new(
            Some("  checkout phone  ".into()),
            412,
            915,
            1.0,
            true,
            true,
            None,
        )
        .unwrap();
        let unnamed =
            DeviceEmulation::new(Some("  ".into()), 412, 915, 1.0, true, true, None).unwrap();
        assert_eq!(named.label.as_deref(), Some("checkout phone"));
        assert_eq!(unnamed.label, None);
    }
}
