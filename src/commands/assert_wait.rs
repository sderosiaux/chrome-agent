//! Bounded observation of an existing assertion. Only reads are repeated, never actions.

use std::collections::HashMap;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use tokio::time::{Instant, sleep_until, timeout_at};

use super::assert::{Assertion, Outcome, read_once};
use crate::cdp::client::CdpClient;
use crate::element_ref::ElementRef;

const POLL_INTERVAL: Duration = Duration::from_millis(200);
const INVALID_WINDOW: &str =
    "assert: within must be a positive whole number of seconds that fits an observation deadline";

fn validate_seconds(seconds: u64) -> Result<u64, String> {
    if seconds == 0
        || seconds.checked_mul(1000).is_none()
        || std::time::Instant::now()
            .checked_add(Duration::from_secs(seconds))
            .is_none()
    {
        Err(INVALID_WINDOW.into())
    } else {
        Ok(seconds)
    }
}

/// Shared CLI and JSON range validation, before opening a browser or replaying a macro.
pub fn parse_seconds(value: &str) -> Result<u64, String> {
    validate_seconds(value.parse().map_err(|_| INVALID_WINDOW.to_string())?)
}

pub fn from_json(value: Option<&Value>) -> Result<Option<u64>, crate::BoxError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => Ok(Some(validate_seconds(
            value.as_u64().ok_or(INVALID_WINDOW)?,
        )?)),
    }
}

/// Timing belongs to this check; it is not an action's `waited_ms` or a promise of stability.
#[derive(Debug, Clone, Serialize)]
pub struct Observation {
    pub within_ms: u128,
    pub elapsed_ms: u128,
    pub observations: u64,
    pub timed_out: bool,
}

impl Observation {
    fn new(start: Instant, window: Duration, observations: u64, timed_out: bool) -> Self {
        Self {
            within_ms: window.as_millis(),
            elapsed_ms: start.elapsed().as_millis(),
            observations,
            timed_out,
        }
    }
}

/// An unreadable page is still exit 1, even after earlier observations contradicted the claim.
/// The earlier reading is evidence, not the current assertion result.
#[derive(Debug)]
pub struct ReadFailed {
    source: crate::BoxError,
    wait: Observation,
    last: Option<Outcome>,
}

impl ReadFailed {
    pub fn print_text(&self) {
        eprintln!("error: {self}");
        if let Some(last) = &self.last {
            eprintln!("last completed observation: {}", last.message());
        }
    }

    pub fn to_json(&self) -> Value {
        let mut report = json!({"ok": false, "error": self.to_string(), "wait": self.wait});
        if let Some(last) = &self.last {
            report["last_observation"] = last.assertion_json();
        }
        report
    }
}

impl std::fmt::Display for ReadFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "assert: observation failed after {} ms (limit {} ms, {} completed observations): {}",
            self.wait.elapsed_ms, self.wait.within_ms, self.wait.observations, self.source
        )
    }
}

impl std::error::Error for ReadFailed {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

pub async fn run(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    assertion: &Assertion,
    seconds: u64,
) -> Result<Outcome, crate::BoxError> {
    let window = Duration::from_secs(validate_seconds(seconds)?);
    let start = Instant::now();
    let deadline = start.checked_add(window).ok_or(INVALID_WINDOW)?;
    let mut observations = 0;
    let mut last: Option<Outcome> = None;
    loop {
        // Bound the whole read, including uid resolution and target metadata. Cancellation
        // drops its pending CDP request; it does not stop or replay any page-side action.
        let mut outcome = {
            let reading = timeout_at(deadline, read_once(client, uid_map, assertion)).await;
            match reading {
                Ok(Ok(outcome)) => outcome,
                failed => {
                    let timed_out = failed.is_err();
                    let source = match failed {
                        Ok(Err(error)) => error,
                        Err(_) => "observation deadline reached while reading the page".into(),
                        Ok(Ok(_)) => unreachable!(),
                    };
                    return Err(Box::new(ReadFailed {
                        source,
                        wait: Observation::new(start, window, observations, timed_out),
                        last,
                    }));
                }
            }
        };
        observations += 1;
        if outcome.held {
            outcome.wait = Some(Observation::new(start, window, observations, false));
            return Ok(outcome);
        }
        last = Some(outcome);
        // Do not start another read at the deadline and mislabel normal expiry as an
        // unreadable page. Keep the last completed comparison when time runs out in sleep.
        sleep_until((Instant::now() + POLL_INTERVAL).min(deadline)).await;
        if Instant::now() >= deadline {
            let mut outcome = last.expect("a completed comparison precedes every sleep");
            outcome.wait = Some(Observation::new(start, window, observations, true));
            return Ok(outcome);
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::*;
    use crate::cli::{Cli, Command};

    #[tokio::test]
    async fn connection_loss_stops_observation_and_preserves_the_last_read() {
        use futures_util::{SinkExt as _, StreamExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let request = socket.next().await.unwrap().unwrap();
            let request: Value = serde_json::from_str(request.to_text().unwrap()).unwrap();
            socket.send(tokio_tungstenite::tungstenite::Message::Text(json!({
                "id":request["id"], "result":{"result":{"type":"string", "value":"about:blank"}}
            }).to_string().into())).await.unwrap();
            socket.next().await.unwrap().unwrap();
            socket.close(None).await.unwrap();
        });
        let client = CdpClient::connect(&format!("ws://{address}"))
            .await
            .unwrap();
        let assertion = super::super::assert::from_json(
            &json!({"what":"url", "equals":"https://example.com", "within":5}),
        )
        .unwrap();
        let error = super::super::assert::run(&client, &HashMap::new(), &assertion)
            .await
            .unwrap_err();
        let failed = error.downcast_ref::<ReadFailed>().unwrap();
        let report = failed.to_json();
        assert_eq!(report["wait"]["timed_out"], false);
        assert_eq!(report["wait"]["observations"], 1);
        assert!(
            report["wait"]["elapsed_ms"].as_u64().unwrap() < 2000,
            "{report}"
        );
        assert_eq!(report["last_observation"]["actual"], "about:blank");
        assert!(report.get("assertion").is_none());
        server.await.unwrap();
    }

    #[test]
    fn within_has_the_same_range_in_cli_pipe_and_macro_preparation() {
        for value in ["0", "-1", "1.5", "no", "18446744073709551615"] {
            assert!(
                Cli::try_parse_from([
                    "chrome-agent",
                    "assert",
                    "text",
                    "--contains",
                    "Saved",
                    "--within",
                    value
                ])
                .is_err(),
                "{value}"
            );
            let parsed: Value = serde_json::from_str(value).unwrap_or_else(|_| json!(value));
            let command =
                json!({"cmd":"assert", "what":"text", "contains":"Saved", "within":parsed});
            assert!(crate::pipe_command::parse(&command).is_err(), "{command}");
            let file = crate::macros::Macro::parse(
                &json!({"name":"wait-check", "steps":[{"do":command}]}).to_string(),
            )
            .unwrap();
            assert!(file.prepare(&std::collections::BTreeMap::new()).is_err());
        }
        for parsed in [json!(true), json!([]), json!({}), json!("5")] {
            assert!(from_json(Some(&parsed)).is_err());
        }
        for args in [
            vec!["assert", "text", "--contains", "Saved", "--within", "5"],
            vec!["assert", "--within", "5", "text", "--contains", "Saved"],
        ] {
            let cli = Cli::try_parse_from(std::iter::once("chrome-agent").chain(args)).unwrap();
            let Command::Assert { within, .. } = cli.command else {
                panic!("assert")
            };
            assert_eq!(within, Some(5));
        }
        assert_eq!(from_json(None).unwrap(), None);
        assert_eq!(from_json(Some(&Value::Null)).unwrap(), None);
        assert_eq!(from_json(Some(&json!(5))).unwrap(), Some(5));
    }
}
