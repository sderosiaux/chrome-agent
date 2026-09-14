use clap::Parser as _;
use serde_json::json;

use crate::discovery::{Discovery, Proposal, origin};

fn state() -> Discovery {
    serde_json::from_value(json!({
        "schema":1,"profile":"local_observation_v1","goal":"Read headlines",
        "entry_url":"https://example.com/","origin":"https://example.com:443",
        "inputs":{"url":"https://example.com/","title":"A\"\n,{{literal}}"},
        "browser":"test","page":"default","revision":0,"created_ms":1000,
        "deadline_ms":2000,"max_commands":10,"reserved_commands":0,"experiments":[]
    }))
    .unwrap()
}

fn proposal() -> Proposal {
    serde_json::from_value(json!({"id":"open","revision":0,"reason":"Read the entry page",
        "command":{"cmd":"goto","url":"{{url}}"},"checks":[{"cmd":"assert","what":"text","contains":"{{title}}"}]})).unwrap()
}

#[test]
fn origins_normalize_ports_case_and_fragments_without_accepting_credentials() {
    for url in ["https://Example.COM", "https://example.com:443/news#today"] {
        assert_eq!(origin(url).unwrap(), "https://example.com:443");
    }
    assert_eq!(origin("http://[::1]:8080/a").unwrap(), "http://[::1]:8080");
    for url in [
        "/relative",
        "file:///tmp/page",
        "javascript:alert(1)",
        "https://user@example.com",
        "https://example.com:bad/",
        "https://example.com:65536/",
        "https://example.com:/",
    ] {
        assert!(origin(url).is_err(), "{url}");
    }
}

#[test]
fn reservations_bind_once_and_corrupt_history_cannot_be_resumed_or_exported() {
    let mut state = state();
    assert!(state.reserve(proposal(), 999).is_err());
    assert!(state.reserve(proposal(), 2000).is_err());
    let index = state.reserve(proposal(), 1000).unwrap();
    assert_eq!(
        state.experiments[index].commands[1]["contains"],
        "A\"\n,{{literal}}"
    );
    state.validate().unwrap();
    assert!(state.reserve(proposal(), 1001).is_err());
    let pending = serde_json::to_value(&state).unwrap();
    for (key, value) in [
        ("state", json!("observed")),
        ("commands", json!([])),
        ("started_ms", json!(2000)),
    ] {
        let mut changed = pending.clone();
        changed["experiments"][0][key] = value;
        assert!(
            serde_json::from_value::<Discovery>(changed)
                .unwrap()
                .validate()
                .is_err()
        );
    }
    let experiment = &mut state.experiments[index];
    experiment.state = "observed".into();
    experiment.results = experiment
        .commands
        .iter()
        .map(|c| json!({"command":c,"response":{"ok":true}}))
        .collect();
    state.revision += 1;
    state.validate().unwrap();
    let observed = serde_json::to_value(&state).unwrap();
    for (key, value) in [("revision", json!(1)), ("reserved_commands", json!(1))] {
        let mut changed = observed.clone();
        changed[key] = value;
        assert!(
            serde_json::from_value::<Discovery>(changed)
                .unwrap()
                .validate()
                .is_err()
        );
    }
    state.experiments[index].results[0]["response"]["ok"] = json!(false);
    assert!(state.validate().is_err());
}

#[test]
fn only_executing_discovery_can_own_a_browser_on_interrupt() {
    for args in [
        vec![
            "discover",
            "start",
            "state.json",
            "--goal",
            "Read",
            "--url",
            "https://example.com",
        ],
        vec!["discover", "show", "state.json"],
        vec![
            "discover",
            "export",
            "state.json",
            "--name",
            "candidate",
            "--steps",
            "open",
        ],
        vec!["discover", "step", "state.json", "--proposal", "{}"],
    ] {
        let cli = crate::cli::Cli::try_parse_from(
            std::iter::once("chrome-agent").chain(args.iter().copied()),
        )
        .unwrap();
        assert_eq!(
            crate::run_helpers::interrupt_owns_browser(&cli.command),
            args[1] == "step"
        );
    }
}
