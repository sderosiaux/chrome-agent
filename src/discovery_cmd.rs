//! CLI continuation protocol. Only `step` opens Chrome; all records are private local files.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::BoxError;
use crate::cli::Cli;
use crate::discovery::{self, Discovery, Proposal, Stopped, now_ms};
use crate::discovery_store::Store;

#[derive(Subcommand)]
pub enum DiscoveryAction {
    /// Start a private discovery record without opening Chrome (parent directory must exist)
    Start {
        file: PathBuf,
        #[arg(long)]
        goal: String,
        /// Entry URL; the first experiment must navigate here
        #[arg(long)]
        url: String,
        /// JSON object of string parameters; stored privately, never inferred from page content
        #[arg(long, default_value = "{}")]
        inputs: String,
        /// Total dispatch budget, including assertion checks and failed attempts
        #[arg(long, default_value = "50", value_parser = clap::value_parser!(u32).range(1..=100))]
        max_commands: u32,
        /// Wall-clock lifetime of this discovery, in seconds, including time between invocations
        #[arg(long, default_value = "1800", value_parser = clap::value_parser!(u64).range(1..=86400))]
        within: u64,
    },
    /// Read the objective, revision, experiments and unknowns without opening Chrome
    Show { file: PathBuf },
    /// Execute one revision-bound experiment, or retrieve the result of an identical request
    Step {
        file: PathBuf,
        /// JSON: id, revision, reason, command, optional checks/hypotheses/unknowns
        #[arg(long)]
        proposal: String,
    },
    /// Install a local candidate macro from an explicitly selected, successful path
    Export {
        file: PathBuf,
        /// New macro name; an existing macro is never replaced
        #[arg(long)]
        name: String,
        /// Experiment IDs in recorded order; unsuccessful or unresolved steps are refused
        #[arg(long, value_delimiter = ',', required = true)]
        steps: Vec<String>,
    },
}

pub async fn run_cli(cli: &Cli, action: &DiscoveryAction) -> Result<(), BoxError> {
    let report = match action {
        DiscoveryAction::Start {
            file,
            goal,
            url,
            inputs,
            max_commands,
            within,
        } => {
            if inputs.len() > 16384 {
                return Err("Inputs exceed 16 KiB".into());
            }
            let at = now_ms()?;
            let state = Discovery {
                schema: 1,
                profile: discovery::PROFILE.into(),
                goal: goal.clone(),
                entry_url: url.clone(),
                origin: discovery::origin(url)?,
                inputs: serde_json::from_str(inputs)?,
                browser: cli.browser.clone(),
                page: cli.page.clone(),
                revision: 0,
                created_ms: at,
                deadline_ms: at + within * 1000,
                max_commands: *max_commands,
                reserved_commands: 0,
                experiments: Vec::new(),
            };
            state.validate()?;
            Store::lock(file)?.create(&state)?;
            state.view(at)
        }
        DiscoveryAction::Show { file } => crate::discovery_store::read(file)?.view(now_ms()?),
        DiscoveryAction::Step { file, proposal } => {
            if proposal.len() > 32768 {
                return Err("Proposal exceeds 32 KiB".into());
            }
            let proposal: Proposal = serde_json::from_str(proposal)?;
            Box::pin(experiment(cli, file, proposal)).await?
        }
        DiscoveryAction::Export { file, name, steps } => export(file, name, steps)?,
    };
    if report["ok"] == false {
        return Err(Box::new(Stopped(report)));
    }
    out_line!("{report}");
    Ok(())
}

fn result(state: &Discovery, index: usize, replayed: bool) -> Value {
    let experiment = &state.experiments[index];
    json!({"ok":experiment.state == "observed", "revision":state.revision,
        "outcome":if experiment.state == "pending" { "uncertain" } else { &experiment.state },
        "replayed":replayed, "experiment":experiment,
        "remaining_commands":state.max_commands-state.reserved_commands,
        "next":if experiment.state == "pending" || experiment.state == "uncertain" {
            "Inspect current state with a new observation; this experiment will not be dispatched again."
        } else { "Use this revision for the next experiment. Assertions check declared facts, not arbitrary task completion." }})
}

async fn experiment(cli: &Cli, path: &Path, proposal: Proposal) -> Result<Value, BoxError> {
    let store = Store::lock(path)?;
    let mut state = store.load()?;
    if let Some(index) = state
        .experiments
        .iter()
        .position(|e| e.proposal.id == proposal.id)
    {
        if state.experiments[index].proposal != proposal {
            return Err(
                "Experiment ID already belongs to a different proposal; nothing was dispatched"
                    .into(),
            );
        }
        return Ok(result(&state, index, true));
    }
    if cli.browser != state.browser || cli.page != state.page {
        return Err(format!(
            "Use --browser {} --page {} for this discovery; nothing was dispatched",
            state.browser, state.page
        )
        .into());
    }
    let at = now_ms()?;
    let index = state.reserve(proposal, at)?;
    store.save(&state)?;
    let remaining = Duration::from_millis(state.deadline_ms.saturating_sub(now_ms()?));
    let error = if remaining.is_zero() {
        Some("Discovery deadline expired after reservation, before opening the browser".into())
    } else {
        match tokio::time::timeout(
            remaining,
            execute(cli, &mut state.experiments[index], &state.origin),
        )
        .await
        {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(error.to_string()),
            Err(_) => Some(
                "Discovery deadline expired during an experiment; a command may have run".into(),
            ),
        }
    };
    if let Some(error) = error {
        state.experiments[index].state = "uncertain".into();
        state.experiments[index].error = Some(error);
    }
    state.revision += 1;
    // If this write fails, the durable reservation remains pending. Never print a success
    // whose evidence could not be persisted for the next agent.
    store.save(&state)?;
    Ok(result(&state, index, false))
}

async fn page_url(session: &crate::pipe::Session) -> Result<String, BoxError> {
    let result: Value = session
        .client
        .call(
            "Runtime.evaluate",
            json!({"expression":"location.href", "returnByValue":true}),
        )
        .await?;
    result["result"]["value"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "Could not observe the page URL".into())
}

async fn execute(
    cli: &Cli,
    experiment: &mut discovery::Experiment,
    origin: &str,
) -> Result<(), BoxError> {
    let mut session = crate::pipe::open_session(cli).await?;
    let mut recovery = crate::pipe_emulation::EmulationRecovery::new(
        &session.client,
        &session.store,
        &cli.browser,
        &cli.page,
    )
    .await;
    for command in &experiment.commands {
        // Navigation is the one way to return to scope from an unrelated or redirected page.
        // These observations are not a network firewall: site scripts may still send requests.
        let before = page_url(&session).await?;
        if command["cmd"] != "goto" && !discovery::origin(&before).is_ok_and(|url| url == origin) {
            experiment.state = "error".into();
            experiment.error =
                Some("Current page is outside the discovery origin; command not dispatched".into());
            break;
        }
        let response = crate::pipe::dispatch_on(&mut session, cli, command, &mut recovery).await;
        let response = discovery::bounded_response(response)?;
        let ok = response["ok"] == true;
        let not_held = response["assertion"]["held"] == false;
        experiment
            .results
            .push(json!({"command":command, "response":response, "url_before":before}));
        if !ok {
            experiment.state = if not_held { "not_held" } else { "error" }.into();
            experiment.error =
                Some("Command or declared assertion did not succeed; see results".into());
            break;
        }
        let after = page_url(&session).await?;
        if let Some(result) = experiment.results.last_mut() {
            result["url_after"] = json!(after);
        }
        if !discovery::origin(&after).is_ok_and(|url| url == origin) {
            experiment.state = "error".into();
            experiment.error =
                Some("Page left the discovery origin; no further command dispatched".into());
            break;
        }
    }
    crate::session::save_session(&mut session.store)?;
    if experiment.state == "pending" {
        experiment.state = "observed".into();
    }
    Ok(())
}

fn export(path: &Path, name: &str, ids: &[String]) -> Result<Value, BoxError> {
    crate::macros::check_name(name)?;
    let store = Store::lock(path)?;
    let state = store.load()?;
    let mut previous = None;
    let mut actions = Vec::new();
    for id in ids {
        let (index, experiment) = state
            .experiments
            .iter()
            .enumerate()
            .find(|(_, e)| e.proposal.id == *id)
            .ok_or_else(|| format!("Unknown experiment '{id}'"))?;
        if previous.is_some_and(|p| index <= p) {
            return Err("Select distinct experiments in their recorded order".into());
        }
        previous = Some(index);
        if experiment.state != "observed" {
            return Err(format!(
                "Experiment '{id}' did not finish successfully; candidate not exported"
            )
            .into());
        }
        actions.push(experiment.proposal.command.clone());
        actions.extend(experiment.proposal.checks.clone());
    }
    if actions.first().is_none_or(|a| a["cmd"] != "goto")
        || actions.last().is_none_or(|a| a["cmd"] != "assert")
    {
        return Err(
            "A candidate path must start with goto and end with an explicit assertion".into(),
        );
    }
    if actions
        .iter()
        .any(|a| a.get("uid").is_some_and(|uid| !uid.is_null()))
    {
        return Err(
            "Candidate paths cannot retain document-specific uids; observe again using selectors"
                .into(),
        );
    }
    // Keep only declared bindings actually referenced by this path, with no input defaults.
    let template = serde_json::to_string(&actions)?;
    let params = discovery::params(&state.inputs)
        .into_iter()
        .filter(|(key, _)| template.contains(&format!("{{{{{key}}}}}")))
        .collect();
    let candidate = crate::macros::Macro {
        name: name.into(),
        site: Some(state.origin.clone()),
        recorded_at: Some(now_ms()?.to_string()),
        params,
        steps: actions.into_iter().map(discovery::step).collect(),
    };
    let vars = state
        .inputs
        .iter()
        .filter(|(key, _)| candidate.params.contains_key(*key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    candidate.prepare(&vars)?;
    crate::secure_fs::create_private_dir_all(&crate::macros::store_dir())?;
    let destination = crate::macros::path_of(name);
    Store::lock(&destination)?.create(&candidate)?;
    Ok(
        json!({"ok":true, "status":"candidate", "macro":name, "path":destination,
        "source_revision":state.revision, "experiments":ids, "params":candidate.params,
        "scope":"Successful selected path with caller-declared checks. Local trusted macro; not independently validated, sandboxed or published."}),
    )
}
