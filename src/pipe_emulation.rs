use serde_json::{Value, json};

use crate::cdp::client::CdpClient;
use crate::session::SessionStore;

/// Tracks recovery after a stored device configuration fails to reapply.
///
/// The pipe must stay alive to accept `emulate device` or `emulate reset`, or the invalid
/// configuration is unrepairable through it. Until repair succeeds, every command that could
/// observe or mutate the misconfigured page is answered with the original reapply error.
pub struct EmulationRecovery {
    reapply_error: Option<String>,
}

impl EmulationRecovery {
    pub async fn new(
        client: &CdpClient,
        store: &SessionStore,
        browser_name: &str,
        page_name: &str,
    ) -> Self {
        Self {
            reapply_error: try_reapply(client, store, browser_name, page_name).await,
        }
    }

    /// Return the stored failure when this command cannot repair it; otherwise admit the command.
    #[must_use]
    pub fn refusal_for(&self, cmd: &Value) -> Option<Value> {
        self.reapply_error
            .as_ref()
            .filter(|_| !handles_reapply_failure(cmd))
            .map(|error| {
                let message =
                    format!("Could not reapply this page's stored device configuration: {error}");
                json!({
                    "ok": false,
                    "error": message,
                    "hint": concat!(
                        "Send {\"cmd\":\"emulate\",\"action\":\"reset\"} ",
                        "to clear the invalid configuration."
                    ),
                })
            })
    }

    /// Clear an existing failure after a recovery command succeeds. It does not reapply:
    /// `emulate device` has already applied and persisted its replacement on `ok: true` and
    /// `emulate reset` has cleared both, so reapplying would issue the CDP commands twice.
    pub fn update_after(&mut self, cmd: &Value, response: &Value) {
        if self.reapply_error.is_some()
            && repairs_reapply_failure(cmd)
            && response.get("ok").and_then(Value::as_bool) == Some(true)
        {
            self.reapply_error = None;
        }
    }
}

/// Whether the outer dispatcher must defer reapply handling to this command. `device` and
/// `reset` replace the stored state directly. A batch is admitted as a container and its
/// nested commands are each evaluated in order, so those preceding the repair stay blocked.
fn handles_reapply_failure(cmd: &Value) -> bool {
    repairs_reapply_failure(cmd) || cmd.get("cmd").and_then(Value::as_str) == Some("batch")
}

fn repairs_reapply_failure(cmd: &Value) -> bool {
    cmd.get("cmd").and_then(Value::as_str) == Some("emulate")
        && matches!(
            cmd.get("action").and_then(Value::as_str),
            Some("device" | "reset")
        )
}

/// Attempt reapplication and retain only the error text needed for recovery.
async fn try_reapply(
    client: &CdpClient,
    store: &SessionStore,
    browser_name: &str,
    page_name: &str,
) -> Option<String> {
    crate::emulation::reapply(client, store, browser_name, page_name)
        .await
        .err()
        .map(|error| error.to_string())
}

/// Execute the JSON form of `emulate`, preserving the same defaults and validation as the CLI.
pub async fn dispatch_emulate(
    ctx: &mut crate::page_ctx::PageCtx<'_>,
    args: &crate::pipe_command::EmulateArgs,
) -> Result<Value, crate::BoxError> {
    let action = args
        .action
        .as_deref()
        .ok_or("emulate: missing \"action\" (device, status, or reset)")?;
    let (client, browser_name, page_name) = (ctx.client, ctx.browser, ctx.page);

    let response = match action {
        "device" => {
            let config = parse_device_config(args)?;
            crate::emulation::apply_and_store(client, ctx.store, browser_name, page_name, config)
                .await
        }
        "status" => crate::emulation::status(client, ctx.store, browser_name, page_name).await,
        "reset" => crate::emulation::clear(client, ctx.store, browser_name, page_name).await,
        other => Err(format!(
            "emulate: unknown action {other:?}; use \"device\", \"status\", or \"reset\""
        )
        .into()),
    }?;

    // Pipe state otherwise reaches disk only when stdin closes. Commit here so a concurrent
    // CLI invocation sees the change while this connection is still open.
    if matches!(action, "device" | "reset") {
        crate::session::save_session(ctx.store)?;
    }
    Ok(response)
}

/// Validate `emulate device`'s values without coercion. Missing and null mean "use the CLI
/// default"; a present value of the wrong type is a caller error, never a guess.
///
/// The KEYS were already checked by `pipe_command::EmulateArgs` (`deny_unknown_fields`, so a
/// typo is refused by name); the values stay `Value` here because these messages name the field
/// and the type it wanted, which serde's `invalid type` does not.
fn parse_device_config(
    args: &crate::pipe_command::EmulateArgs,
) -> Result<crate::emulation::DeviceEmulation, crate::BoxError> {
    let label = optional_string(args.label.as_ref(), "label")?;
    let width = required_u32(args.width.as_ref(), "width")?;
    let height = required_u32(args.height.as_ref(), "height")?;
    let dpr = optional_f64(args.dpr.as_ref(), "dpr")?.unwrap_or(1.0);
    let mobile = optional_bool(args.mobile.as_ref(), "mobile")?.unwrap_or(false);
    let touch = optional_bool(args.touch.as_ref(), "touch")?.unwrap_or(false);
    let orientation = optional_string(args.orientation.as_ref(), "orientation")?
        .as_deref()
        .map(crate::emulation::DeviceOrientation::parse)
        .transpose()?;
    crate::emulation::DeviceEmulation::new(label, width, height, dpr, mobile, touch, orientation)
        .map_err(Into::into)
}

fn optional_string(value: Option<&Value>, key: &str) -> Result<Option<String>, crate::BoxError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("emulate device: \"{key}\" must be a string").into()),
    }
}

fn optional_f64(value: Option<&Value>, key: &str) -> Result<Option<f64>, crate::BoxError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .map(Some)
            .ok_or_else(|| format!("emulate device: \"{key}\" must be a number").into()),
    }
}

fn optional_bool(value: Option<&Value>, key: &str) -> Result<Option<bool>, crate::BoxError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(format!("emulate device: \"{key}\" must be a boolean").into()),
    }
}

fn required_u32(value: Option<&Value>, key: &str) -> Result<u32, crate::BoxError> {
    let value = value
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("emulate device: missing or invalid \"{key}\""))?;
    u32::try_from(value).map_err(|_| format!("emulate device: \"{key}\" is too large").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_commands_and_batches_handle_reapply_errors() {
        assert!(handles_reapply_failure(
            &json!({"cmd": "emulate", "action": "reset"})
        ));
        assert!(handles_reapply_failure(&json!({
            "cmd": "batch",
            "commands": [{"cmd": "eval", "expression": "1"}]
        })));
        assert!(!handles_reapply_failure(
            &json!({"cmd": "emulate", "action": "status"})
        ));
    }

    #[test]
    fn refusal_names_one_concrete_recovery_command() {
        let recovery = EmulationRecovery {
            reapply_error: Some("invalid stored dimensions".into()),
        };
        let response = recovery
            .refusal_for(&json!({"cmd": "eval", "expression": "1"}))
            .unwrap();
        assert_eq!(
            response["hint"],
            concat!(
                "Send {\"cmd\":\"emulate\",\"action\":\"reset\"} ",
                "to clear the invalid configuration."
            )
        );
    }

    #[test]
    fn successful_repair_clears_the_stored_reapply_error() {
        let mut recovery = EmulationRecovery {
            reapply_error: Some("invalid stored dimensions".into()),
        };
        let reset = json!({"cmd": "emulate", "action": "reset"});

        recovery.update_after(&reset, &json!({"ok": true, "emulation": null}));

        assert!(
            recovery
                .refusal_for(&json!({"cmd": "eval", "expression": "1"}))
                .is_none()
        );
    }

    #[test]
    fn failed_repair_keeps_the_stored_reapply_error() {
        let mut recovery = EmulationRecovery {
            reapply_error: Some("invalid stored dimensions".into()),
        };
        let reset = json!({"cmd": "emulate", "action": "reset"});

        recovery.update_after(&reset, &json!({"ok": false, "error": "reset failed"}));

        assert!(
            recovery
                .refusal_for(&json!({"cmd": "eval", "expression": "1"}))
                .is_some()
        );
    }

    /// Through the protocol, so these also pin that `emulate` accepts exactly these keys.
    fn device_config(mut cmd: Value) -> Result<crate::emulation::DeviceEmulation, crate::BoxError> {
        cmd["cmd"] = json!("emulate");
        cmd["action"] = json!("device");
        match crate::pipe_command::parse(&cmd)? {
            crate::pipe_command::PipeCommand::Emulate(args) => parse_device_config(&args),
            other => panic!("not an emulate: {}", other.name()),
        }
    }

    #[test]
    fn defaults_only_absent_or_null_optional_fields() {
        let config = device_config(json!({
            "width": 1024,
            "height": 768,
            "label": null,
            "dpr": null,
            "mobile": null,
            "touch": null,
            "orientation": null
        }))
        .unwrap();
        assert_eq!(config.label, None);
        assert!((config.device_scale_factor - 1.0).abs() < f64::EPSILON);
        assert!(!config.mobile);
        assert!(!config.touch);
        assert_eq!(
            config.orientation,
            crate::emulation::DeviceOrientation::Landscape
        );
    }

    #[test]
    fn emitted_requested_configuration_round_trips_as_pipe_input() {
        let expected = crate::emulation::DeviceEmulation::new(
            Some("checkout phone".into()),
            390,
            844,
            3.0,
            true,
            true,
            None,
        )
        .unwrap();
        let command = serde_json::to_value(&expected).unwrap();

        assert_eq!(device_config(command).unwrap(), expected);
    }

    #[test]
    fn rejects_wrong_optional_field_types() {
        for (field, value, expected) in [
            ("label", json!(42), "must be a string"),
            ("dpr", json!("3"), "must be a number"),
            ("mobile", json!("true"), "must be a boolean"),
            ("touch", json!(1), "must be a boolean"),
            ("orientation", json!(false), "must be a string"),
        ] {
            let mut cmd = json!({"width": 390, "height": 844});
            cmd[field] = value;
            let error = device_config(cmd).unwrap_err().to_string();
            assert!(
                error.contains(field) && error.contains(expected),
                "unexpected error for {field}: {error}"
            );
        }
    }

    /// A key `emulate` does not take is refused by name, not silently defaulted.
    #[test]
    fn an_unknown_emulate_key_is_refused_by_name() {
        let error = device_config(json!({"width": 390, "height": 844, "deviceScaleFactor": 3}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("deviceScaleFactor"), "{error}");
    }
}
