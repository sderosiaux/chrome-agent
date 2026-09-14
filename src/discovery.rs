//! Persistent experiments proposed by a calling agent. Observations never certify a recipe.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::BoxError;
use crate::macros::{Guards, Macro, Param, Step};
use crate::pipe_command::PipeCommand;

pub const MAX_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_RESPONSE: usize = 64 * 1024;
pub const PROFILE: &str = "local_observation_v1";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub id: String,
    pub revision: u64,
    pub reason: String,
    pub command: Value,
    #[serde(default)]
    pub checks: Vec<Value>,
    #[serde(default)]
    pub hypotheses: Vec<String>,
    #[serde(default)]
    pub unknowns: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Experiment {
    pub proposal: Proposal,
    pub commands: Vec<Value>,
    pub started_ms: u64,
    pub state: String,
    pub results: Vec<Value>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Discovery {
    pub schema: u32,
    pub profile: String,
    pub goal: String,
    pub entry_url: String,
    pub origin: String,
    pub inputs: BTreeMap<String, String>,
    pub browser: String,
    pub page: String,
    pub revision: u64,
    pub created_ms: u64,
    pub deadline_ms: u64,
    pub max_commands: u32,
    pub reserved_commands: u32,
    pub experiments: Vec<Experiment>,
}

pub fn now_ms() -> Result<u64, BoxError> {
    Ok(u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )?)
}

pub fn origin(url: &str) -> Result<String, BoxError> {
    text(url, "URL")?;
    let uri: tokio_tungstenite::tungstenite::http::Uri =
        url.split('#').next().unwrap_or(url).parse()?;
    let scheme = uri.scheme_str().ok_or("Expected an absolute HTTP(S) URL")?;
    if !matches!(scheme, "http" | "https")
        || uri.authority().is_none_or(|a| a.as_str().contains('@'))
    {
        return Err("Discovery URLs must use HTTP(S) without embedded credentials".into());
    }
    let authority = uri.authority().ok_or("URL has no authority")?;
    let raw_host = uri
        .host()
        .filter(|h| !h.is_empty())
        .ok_or("URL has no host")?;
    if authority.as_str() != raw_host && uri.port_u16().is_none() {
        return Err("URL port must be a valid unsigned 16-bit integer".into());
    }
    let host = raw_host.to_ascii_lowercase();
    let port = uri
        .port_u16()
        .unwrap_or(if scheme == "https" { 443 } else { 80 });
    Ok(format!("{scheme}://{host}:{port}"))
}

pub fn identifier(value: &str) -> Result<(), BoxError> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err("Identifiers must contain 1-64 ASCII letters, digits, '_' or '-'".into());
    }
    Ok(())
}

fn text(value: &str, label: &str) -> Result<(), BoxError> {
    if value.trim().is_empty() || value.len() > 4096 {
        return Err(format!("{label} must contain 1-4096 bytes of nonblank text").into());
    }
    Ok(())
}

pub fn params(inputs: &BTreeMap<String, String>) -> BTreeMap<String, Param> {
    inputs
        .keys()
        .map(|key| {
            (
                key.clone(),
                Param {
                    required: true,
                    secret: false,
                },
            )
        })
        .collect()
}

pub fn step(action: Value) -> Step {
    Step {
        action,
        expect: Guards::default(),
        unguarded: None,
    }
}

impl Discovery {
    pub fn validate(&self) -> Result<(), BoxError> {
        if self.schema != 1 || self.profile != PROFILE {
            return Err("Unsupported discovery schema or execution profile".into());
        }
        text(&self.goal, "goal")?;
        text(&self.browser, "browser")?;
        crate::browser::validate_browser_name(&self.browser)?;
        text(&self.page, "page")?;
        if self.origin != origin(&self.entry_url)?
            || self.inputs.len() > 32
            || serde_json::to_vec(&self.inputs)?.len() > 16384
            || !(1..=100).contains(&self.max_commands)
            || self.reserved_commands > self.max_commands
            || self.deadline_ms <= self.created_ms
            || self.deadline_ms - self.created_ms > 86_400_000
            || self.experiments.len() > 100
        {
            return Err("Invalid discovery scope, inputs or budget".into());
        }
        for key in self.inputs.keys() {
            identifier(key)?;
        }
        self.validate_experiments()?;
        Ok(())
    }

    fn validate_experiments(&self) -> Result<(), BoxError> {
        let mut ids = std::collections::BTreeSet::new();
        let mut revision = 0;
        let mut reserved = 0;
        let mut started = self.created_ms;
        for (index, experiment) in self.experiments.iter().enumerate() {
            if !ids.insert(&experiment.proposal.id)
                || experiment.proposal.revision != revision
                || experiment.started_ms < started
                || experiment.started_ms >= self.deadline_ms
                || experiment.commands != self.prepare(&experiment.proposal)?
                || experiment.results.len() > experiment.commands.len()
                || (index == 0
                    && (experiment.commands[0]["cmd"] != "goto"
                        || experiment.commands[0]["url"] != self.entry_url))
            {
                return Err("Inconsistent discovery experiment history".into());
            }
            for (result, command) in experiment.results.iter().zip(&experiment.commands) {
                if result["command"] != *command
                    || !result["response"]["ok"].is_boolean()
                    || serde_json::to_vec(&result["response"])?.len() > MAX_RESPONSE
                {
                    return Err("Invalid recorded observation".into());
                }
            }
            let valid = match experiment.state.as_str() {
                "pending" => experiment.results.is_empty() && experiment.error.is_none(),
                "observed" => {
                    experiment.results.len() == experiment.commands.len()
                        && experiment
                            .results
                            .iter()
                            .all(|r| r["response"]["ok"] == true)
                        && experiment.error.is_none()
                }
                "not_held" => {
                    experiment.error.is_some()
                        && experiment.results.last().is_some_and(|r| {
                            r["response"]["ok"] == false
                                && r["response"]["assertion"]["held"] == false
                        })
                }
                "error" | "uncertain" => experiment.error.is_some(),
                _ => false,
            };
            if !valid {
                return Err("Invalid discovery experiment outcome".into());
            }
            revision += if experiment.state == "pending" { 1 } else { 2 };
            reserved += u32::try_from(experiment.commands.len())?;
            started = experiment.started_ms;
        }
        if revision != self.revision || reserved != self.reserved_commands {
            return Err("Discovery revision or budget disagrees with its history".into());
        }
        Ok(())
    }

    /// Expand through the same binding and command validation as macro replay, then narrow
    /// the command profile. This limits explicit commands, not JavaScript run by the site.
    pub fn prepare(&self, proposal: &Proposal) -> Result<Vec<Value>, BoxError> {
        identifier(&proposal.id)?;
        text(&proposal.reason, "reason")?;
        if proposal.checks.len() > 8
            || proposal.hypotheses.len() > 16
            || proposal.unknowns.len() > 16
            || serde_json::to_vec(proposal)?.len() > 32768
        {
            return Err("Experiment exceeds its size or check limit".into());
        }
        for value in proposal.hypotheses.iter().chain(&proposal.unknowns) {
            text(value, "note")?;
        }
        let template = Macro {
            name: "discovery".into(),
            site: None,
            recorded_at: None,
            params: params(&self.inputs),
            steps: std::iter::once(&proposal.command)
                .chain(&proposal.checks)
                .cloned()
                .map(step)
                .collect(),
        };
        let commands: Vec<Value> = template
            .prepare(&self.inputs)?
            .into_iter()
            .map(|s| s.action)
            .collect();
        for (i, command) in commands.iter().enumerate() {
            let parsed = crate::pipe_command::parse(command)?;
            if i > 0 && !matches!(parsed, PipeCommand::Assert(_)) {
                return Err("Experiment checks must be assert commands".into());
            }
            match parsed {
                PipeCommand::Goto(args) if origin(&args.url)? == self.origin => {}
                PipeCommand::Inspect(_)
                | PipeCommand::Read(_)
                | PipeCommand::Text(_)
                | PipeCommand::Extract(_)
                | PipeCommand::Assert(_)
                | PipeCommand::Wait(_) => {}
                _ => {
                    return Err(format!(
                        "{} is outside the local observation command profile",
                        parsed.name()
                    )
                    .into());
                }
            }
        }
        if self.experiments.is_empty()
            && (commands[0]["cmd"] != "goto" || commands[0]["url"] != self.entry_url)
        {
            return Err("The first experiment must navigate to the discovery entry_url".into());
        }
        Ok(commands)
    }

    /// The reservation must be durable before any browser connection or command starts.
    pub fn reserve(&mut self, proposal: Proposal, at: u64) -> Result<usize, BoxError> {
        if self
            .experiments
            .iter()
            .any(|e| e.proposal.id == proposal.id)
        {
            return Err("Experiment ID already exists".into());
        }
        if proposal.revision != self.revision {
            return Err(format!(
                "Stale discovery revision: expected {}, received {}. Read discover show.",
                self.revision, proposal.revision
            )
            .into());
        }
        if at
            < self
                .experiments
                .last()
                .map_or(self.created_ms, |e| e.started_ms)
            || at >= self.deadline_ms
        {
            return Err("Discovery deadline expired or system clock moved backwards".into());
        }
        let commands = self.prepare(&proposal)?;
        let cost = u32::try_from(commands.len())?;
        if self.reserved_commands + cost > self.max_commands {
            return Err(
                "Discovery command budget exhausted; checks are charged with their experiment"
                    .into(),
            );
        }
        self.reserved_commands += cost;
        self.revision += 1;
        self.experiments.push(Experiment {
            proposal,
            commands,
            started_ms: at,
            state: "pending".into(),
            results: Vec::new(),
            error: None,
        });
        Ok(self.experiments.len() - 1)
    }

    pub fn view(&self, at: u64) -> Value {
        json!({"ok":true, "discovery":self,
            "remaining_commands":self.max_commands - self.reserved_commands,
            "remaining_ms":self.deadline_ms.saturating_sub(at),
            "model_usage":null,
            "unresolved_experiments":self.experiments.iter().filter(|e| matches!(e.state.as_str(), "pending" | "uncertain")).map(|e| &e.proposal.id).collect::<Vec<_>>(),
            "scope":"Local caller-owned observations. Hypotheses are agent claims; no recipe is independently validated. This command profile is not a browser or code sandbox."})
    }
}

/// Preserve failure evidence without returning an oversized success that could not be stored.
pub fn bounded_response(response: Value) -> Result<Value, BoxError> {
    if serde_json::to_vec(&response)?.len() > MAX_RESPONSE {
        return Err(
            "Observation exceeds 64 KiB; result not retained. Narrow the next read.".into(),
        );
    }
    Ok(response)
}

#[derive(Debug)]
pub struct Stopped(pub Value);

impl Stopped {
    pub fn report(&self) -> i32 {
        out_line!("{}", self.0);
        if self.0["outcome"] == "not_held" {
            2
        } else {
            1
        }
    }
}

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Discovery experiment did not complete")
    }
}
impl std::error::Error for Stopped {}
