//! A display flag narrows what is PRINTED, never what is stored as the diff baseline.
//!
//! Each test renders a reduced tree, then injects exactly one button into a page of thirteen
//! nodes; the correct diff is `added=1, removed=0, changed=0`. A narrowed rendering stored as
//! `last_snapshot` inflates `added`, and `--urls` inflates `changed`.

use std::process::Command;

mod common;
use common::TestBrowser;

/// The page holds thirteen accessibility nodes and an empty `#slot` to inject into.
const FIXTURE: &str = "snapshot_filter_baseline.html";

/// Adds exactly one node, inside `#slot`, below every filter and depth limit used here.
const INJECT: &str = "document.getElementById('slot').insertAdjacentHTML('beforeend', \
                      '<button id=\"fresh\">Freshly added</button>'); 1";

fn run_cli(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(common::binary())
        .args(args)
        .output()
        .expect("Failed to run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

/// Feed a sequence of JSON commands to one `pipe` session, returning the response lines.
fn run_pipe(args: &[&str], commands: &[String]) -> Vec<serde_json::Value> {
    use std::io::Write;
    let mut child = Command::new(common::binary())
        .args(args)
        .arg("pipe")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("Failed to spawn chrome-agent pipe");
    {
        let stdin = child.stdin.as_mut().expect("pipe stdin");
        for cmd in commands {
            writeln!(stdin, "{cmd}").expect("write pipe command");
        }
    }
    let output = child.wait_with_output().expect("pipe run");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn goto(browser: &str) -> bool {
    let url = common::fixture_url(FIXTURE);
    let (_, stderr, code) = run_cli(&["--browser", browser, "goto", &url]);
    if code != 0 {
        return common::unavailable(&format!("goto {FIXTURE} failed: {stderr}"));
    }
    true
}

fn inject(browser: &str) {
    let (_, stderr, code) = run_cli(&["--browser", browser, "eval", INJECT]);
    assert_eq!(
        code, 0,
        "injecting the one known node must succeed: {stderr}"
    );
}

fn diff_json(browser: &str) -> serde_json::Value {
    let (stdout, _, _) = run_cli(&["--browser", browser, "--json", "diff"]);
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("diff should emit JSON: {e}\n{stdout}"))
}

/// One node was added and nothing else moved.
fn assert_only_the_injected_node_moved(json: &serde_json::Value, path: &str) {
    assert_eq!(
        json["added"], 1,
        "{path}: exactly one node was injected; anything more is the baseline missing nodes the page always had.\n{json}"
    );
    assert_eq!(
        json["removed"], 0,
        "{path}: nothing was removed from the page.\n{json}"
    );
    assert_eq!(
        json["changed"], 0,
        "{path}: no node was rewritten; a non-zero count means the baseline rendered them differently.\n{json}"
    );
    assert!(
        json["diff"]
            .as_str()
            .unwrap_or_default()
            .contains("Freshly added"),
        "{path}: the diff must name the node that really appeared.\n{json}"
    );
}

/// The uid of the first node with the given role: `backendNodeId`s are per session, so they
/// cannot be hardcoded.
fn uid_of_role(browser: &str, role: &str) -> String {
    let (stdout, _, _) = run_cli(&["--browser", browser, "inspect"]);
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("uid=")
            && let Some((uid, after)) = rest.split_once(' ')
            && after.split([' ', '"']).next() == Some(role)
        {
            return uid.to_string();
        }
    }
    panic!("no {role} node in the snapshot of {FIXTURE}:\n{stdout}");
}

// ─── inspect's own display flags ───

#[test]
fn plain_inspect_is_the_control() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("baseline-plain");
    if !goto(b.name()) {
        return;
    }
    let (_, _, code) = run_cli(&["--browser", b.name(), "inspect"]);
    assert_eq!(code, 0);
    inject(b.name());
    assert_only_the_injected_node_moved(&diff_json(b.name()), "inspect");
}

#[test]
fn filter_does_not_reach_the_baseline() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("baseline-filter");
    if !goto(b.name()) {
        return;
    }
    let (stdout, _, code) = run_cli(&["--browser", b.name(), "inspect", "--filter", "button"]);
    assert_eq!(code, 0);
    // The flag must still do its job: one button on this page, and no heading or link.
    assert!(
        stdout.contains("button \"Go\""),
        "the filtered view is what is printed:\n{stdout}"
    );
    assert!(
        !stdout.contains("heading"),
        "the filter still narrows the output:\n{stdout}"
    );

    inject(b.name());
    assert_only_the_injected_node_moved(&diff_json(b.name()), "inspect --filter button");
}

#[test]
fn max_depth_does_not_reach_the_baseline() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("baseline-depth");
    if !goto(b.name()) {
        return;
    }
    let (stdout, _, code) = run_cli(&["--browser", b.name(), "inspect", "--max-depth", "1"]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("main"),
        "depth 1 still prints the top level:\n{stdout}"
    );
    assert!(
        !stdout.contains("combobox"),
        "depth 1 still cuts below it:\n{stdout}"
    );

    inject(b.name());
    assert_only_the_injected_node_moved(&diff_json(b.name()), "inspect --max-depth 1");
}

#[test]
fn focus_uid_does_not_reach_the_baseline() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("baseline-focus");
    if !goto(b.name()) {
        return;
    }
    let main_uid = uid_of_role(b.name(), "main");
    let (stdout, _, code) = run_cli(&["--browser", b.name(), "inspect", "--uid", &main_uid]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("combobox"),
        "the subtree is still what is printed:\n{stdout}"
    );
    assert!(
        !stdout.contains("contentinfo"),
        "the subtree still excludes the footer:\n{stdout}"
    );

    inject(b.name());
    assert_only_the_injected_node_moved(&diff_json(b.name()), "inspect --uid <main>");
}

#[test]
fn urls_does_not_reach_the_baseline() {
    if !common::browser_ready() {
        return;
    }
    // `--urls` appends `url="…"` to link lines; stored, that makes the next diff read every
    // link as CHANGED rather than inflating `added`.
    let b = TestBrowser::new("baseline-urls");
    if !goto(b.name()) {
        return;
    }
    let (stdout, _, code) = run_cli(&["--browser", b.name(), "inspect", "--urls"]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("url=\""),
        "--urls still annotates what it prints:\n{stdout}"
    );

    inject(b.name());
    assert_only_the_injected_node_moved(&diff_json(b.name()), "inspect --urls");
}

#[test]
fn limit_does_not_reach_the_baseline() {
    if !common::browser_ready() {
        return;
    }
    // `--limit` collects a UNION over scroll positions, which is not a page state at all.
    // The baseline is a fresh full reading taken once the scrolling has stopped.
    let b = TestBrowser::new("baseline-limit");
    if !goto(b.name()) {
        return;
    }
    let (stdout, _, code) = run_cli(&[
        "--browser",
        b.name(),
        "inspect",
        "--limit",
        "20",
        "--filter",
        "button",
    ]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("button \"Go\""),
        "the collected view is still printed:\n{stdout}"
    );
    assert!(
        !stdout.contains("heading"),
        "the filter still applies to it:\n{stdout}"
    );

    inject(b.name());
    assert_only_the_injected_node_moved(&diff_json(b.name()), "inspect --limit 20 --filter button");
}

#[test]
fn paging_still_windows_the_printed_view() {
    if !common::browser_ready() {
        return;
    }
    // `--max-chars`/`--offset` are the one reduction already excluded from the baseline, and
    // must keep cutting the printed text.
    let b = TestBrowser::new("baseline-paging");
    if !goto(b.name()) {
        return;
    }
    let (stdout, _, code) = run_cli(&[
        "--browser",
        b.name(),
        "inspect",
        "--filter",
        "button",
        "--max-chars",
        "12",
    ]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("chars truncated"),
        "paging still truncates:\n{stdout}"
    );
    assert!(
        stdout.contains("--offset 12"),
        "paging still says how to continue:\n{stdout}"
    );

    inject(b.name());
    assert_only_the_injected_node_moved(&diff_json(b.name()), "inspect --filter --max-chars");
}

// ─── the uid map is part of the baseline ───

#[test]
fn a_display_flag_does_not_narrow_the_uid_map() {
    if !common::browser_ready() {
        return;
    }
    // A display flag must not decide which nodes the next command may act on: the button sits
    // below `--max-depth 1` and stays clickable.
    let b = TestBrowser::new("baseline-uidmap");
    if !goto(b.name()) {
        return;
    }
    let button_uid = uid_of_role(b.name(), "button");
    let (_, _, code) = run_cli(&["--browser", b.name(), "inspect", "--max-depth", "1"]);
    assert_eq!(code, 0);

    let (stdout, _, _) = run_cli(&["--browser", b.name(), "--json", "click", &button_uid]);
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("click should emit JSON: {e}\n{stdout}"));
    assert_eq!(
        json["ok"], true,
        "a uid deeper than --max-depth is still a node on the page, got {json}"
    );
}

// ─── the verdict of the NEXT action ───

/// The click response for `#go` — a button with no handler — after `inspect` ran with `flags`.
fn verdict_of_inert_click(label: &str, flags: &[&str]) -> Option<serde_json::Value> {
    let b = TestBrowser::new(label);
    if !goto(b.name()) {
        return None;
    }
    let mut args = vec!["--browser", b.name(), "inspect"];
    args.extend_from_slice(flags);
    let (_, _, code) = run_cli(&args);
    assert_eq!(code, 0, "inspect {flags:?} should succeed");

    let (stdout, _, _) = run_cli(&[
        "--browser",
        b.name(),
        "--json",
        "click",
        "--selector",
        "#go",
    ]);
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("click should emit JSON: {e}\n{stdout}"));
    assert_eq!(
        json["ok"], true,
        "the click itself should succeed, got {json}"
    );
    Some(json)
}

#[test]
fn a_display_flag_does_not_flip_the_verdict_of_the_next_action() {
    if !common::browser_ready() {
        return;
    }
    // Two sessions differing only in a display flag on the `inspect` before the click.
    let Some(with_urls) = verdict_of_inert_click("verdict-urls", &["--urls"]) else {
        return;
    };
    let Some(plain) = verdict_of_inert_click("verdict-plain", &[]) else {
        return;
    };

    for field in ["verdict", "verdict_reason", "next"] {
        assert_eq!(
            with_urls[field], plain[field],
            "`{field}` must not depend on a display flag used before the action.\n\
             after `inspect --urls`: {with_urls}\n\
             after `inspect`:        {plain}"
        );
    }

    // Agreement alone is also satisfied by both sides being wrong.
    assert_ne!(
        with_urls["next"], "proceed",
        "a click on an inert button must never tell an agent to carry on, got {with_urls}"
    );
}

// ─── goto --inspect, which no change report speaks for ───

#[test]
fn goto_inspect_max_depth_does_not_reach_the_baseline() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("baseline-goto");
    let url = common::fixture_url(FIXTURE);
    let (stdout, stderr, code) = run_cli(&[
        "--browser",
        b.name(),
        "goto",
        &url,
        "--inspect",
        "--max-depth",
        "1",
    ]);
    if code != 0 {
        common::unavailable(&format!("goto --inspect failed: {stderr}"));
        return;
    }
    assert!(
        !stdout.contains("combobox"),
        "--max-depth still cuts the printed tree:\n{stdout}"
    );

    inject(b.name());
    assert_only_the_injected_node_moved(&diff_json(b.name()), "goto --inspect --max-depth 1");
}

#[test]
fn pipe_goto_inspect_max_depth_does_not_reach_the_baseline() {
    if !common::browser_ready() {
        return;
    }
    // `goto` is outside `mutates_page`, so nothing overwrites the snapshot it stored.
    let b = TestBrowser::new("baseline-pipegoto");
    let url = common::fixture_url(FIXTURE);
    let responses = run_pipe(
        &["--browser", b.name()],
        &[
            format!(r#"{{"cmd":"goto","url":"{url}","inspect":true,"max_depth":1}}"#),
            format!(
                r#"{{"cmd":"eval","expression":{}}}"#,
                serde_json::json!(INJECT)
            ),
            r#"{"cmd":"diff"}"#.to_string(),
        ],
    );
    let Some(diff) = responses.last() else {
        common::unavailable("pipe produced no response");
        return;
    };
    assert_eq!(
        responses[0]["ok"], true,
        "goto should succeed: {}",
        responses[0]
    );
    assert!(
        !responses[0]["snapshot"]
            .as_str()
            .unwrap_or_default()
            .contains("combobox"),
        "max_depth still cuts the returned tree: {}",
        responses[0]
    );
    assert_only_the_injected_node_moved(diff, "pipe goto inspect+max_depth");
}

#[test]
fn pipe_action_inspect_with_the_report_off_does_not_reach_the_baseline() {
    if !common::browser_ready() {
        return;
    }
    // With `--verdict off` no change report runs, so the snapshot the action's `inspect: true`
    // stored is the one the next `diff` compares against.
    let b = TestBrowser::new("baseline-pipeoff");
    let url = common::fixture_url(FIXTURE);
    let responses = run_pipe(
        &["--browser", b.name(), "--verdict", "off"],
        &[
            format!(r#"{{"cmd":"goto","url":"{url}"}}"#),
            r##"{"cmd":"click","selector":"#go","inspect":true,"max_depth":1}"##.to_string(),
            format!(
                r#"{{"cmd":"eval","expression":{}}}"#,
                serde_json::json!(INJECT)
            ),
            r#"{"cmd":"diff"}"#.to_string(),
        ],
    );
    let Some(diff) = responses.last() else {
        common::unavailable("pipe produced no response");
        return;
    };
    assert_eq!(
        responses[1]["ok"], true,
        "click should succeed: {}",
        responses[1]
    );
    assert_only_the_injected_node_moved(diff, "pipe click inspect+max_depth, report off");
}
