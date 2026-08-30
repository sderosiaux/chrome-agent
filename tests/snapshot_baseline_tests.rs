//! A display flag narrows what is PRINTED, never what is stored as the diff baseline.
//!
//! `inspect --filter "heading,button,link"` used to persist the filtered rendering as
//! `last_snapshot`. The next `diff` then compared the whole page against that amputated copy
//! and reported every node the filter had dropped as an ADDITION. Measured on
//! `https://webmcp-coffee.jilles.fyi`: `added=22` of nodes that never moved, where a plain
//! `inspect` in the same sequence answered `added=0 removed=30 changed=3`.
//!
//! CLAUDE.md already documented this class for `--max-depth` on the ACTION path — "the
//! baseline snapshot is always taken at full depth" — and the fix had been applied there and
//! nowhere else. Every path below stores a snapshot after rendering a reduced one, and each
//! is measured against the same ground truth: one button is injected into a page of thirteen
//! nodes, so the honest answer is `added=1, removed=0, changed=0`.
//!
//! Before the fix, on `snapshot_filter_baseline.html`:
//!
//! | path                                        | added | changed |
//! |---------------------------------------------|-------|---------|
//! | `inspect` (no flag, the control)            | 1     | 0       |
//! | `inspect --filter button`                   | 13    | 0       |
//! | `inspect --max-depth 1`                     | 10    | 0       |
//! | `inspect --uid <main>`                      | 5     | 0       |
//! | `inspect --urls`                            | 1     | 1       |
//! | `inspect --limit 20 --filter button`        | 13    | 0       |
//! | `goto --inspect --max-depth 1` (CLI + pipe) | 10    | 0       |
//! | pipe action `inspect`+`max_depth`, report off | 10  | 0       |
//!
//! The last test here comes at the same bug from the other end, and it is the worse half. A
//! poisoned baseline is not only a wrong count on `diff` — it reaches the VERDICT of the next
//! action, which is an instruction rather than a datum. Measured independently on Wikipedia:
//! the same click on the same element answered `changed: 2656` / `next: proceed` after an
//! `inspect --urls` and `changed: 0` / `next: confirm` after a plain `inspect`. `proceed` and
//! `confirm` are opposite branches of the closed set of six, so an agent did two different
//! things because of a display flag it had used several steps earlier.

use std::process::Command;

mod common;
use common::TestBrowser;

/// The page holds thirteen accessibility nodes and an empty `#slot` to inject into.
const FIXTURE: &str = "snapshot_filter_baseline.html";

/// Adds exactly one node to the tree, inside `#slot`, which no filter or depth limit above
/// reaches from a stored baseline.
const INJECT: &str = "document.getElementById('slot').insertAdjacentHTML('beforeend', \
                      '<button id=\"fresh\">Freshly added</button>'); 1";

fn binary() -> String {
    let mut path = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
    path.push("chrome-agent");
    path.to_string_lossy().into_owned()
}

fn run_cli(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(binary()).args(args).output().expect("Failed to run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

/// Feed a sequence of JSON commands to one `pipe` session, returning the response lines.
fn run_pipe(args: &[&str], commands: &[String]) -> Vec<serde_json::Value> {
    use std::io::Write;
    let mut child = Command::new(binary())
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
    assert_eq!(code, 0, "injecting the one known node must succeed: {stderr}");
}

fn diff_json(browser: &str) -> serde_json::Value {
    let (stdout, _, _) = run_cli(&["--browser", browser, "--json", "diff"]);
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("diff should emit JSON: {e}\n{stdout}"))
}

/// The whole contract in one place: one node was added and nothing else moved.
///
/// Asserted on both counts, because the paths fail differently — a narrowed rendering
/// inflates `added`, while `--urls` annotates lines the next read renders bare and inflates
/// `changed` instead.
fn assert_only_the_injected_node_moved(json: &serde_json::Value, path: &str) {
    assert_eq!(
        json["added"], 1,
        "{path}: exactly one node was injected; anything more is the baseline missing nodes the page always had.\n{json}"
    );
    assert_eq!(json["removed"], 0, "{path}: nothing was removed from the page.\n{json}");
    assert_eq!(
        json["changed"], 0,
        "{path}: no node was rewritten; a non-zero count means the baseline rendered them differently.\n{json}"
    );
    assert!(
        json["diff"].as_str().unwrap_or_default().contains("Freshly added"),
        "{path}: the diff must name the node that really appeared.\n{json}"
    );
}

/// The uid of the first node with the given role, read off a plain `inspect`.
///
/// `backendNodeId`s are assigned per browser session, so they cannot be hardcoded.
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
    assert!(stdout.contains("button \"Go\""), "the filtered view is what is printed:\n{stdout}");
    assert!(!stdout.contains("heading"), "the filter still narrows the output:\n{stdout}");

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
    assert!(stdout.contains("main"), "depth 1 still prints the top level:\n{stdout}");
    assert!(!stdout.contains("combobox"), "depth 1 still cuts below it:\n{stdout}");

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
    assert!(stdout.contains("combobox"), "the subtree is still what is printed:\n{stdout}");
    assert!(!stdout.contains("contentinfo"), "the subtree still excludes the footer:\n{stdout}");

    inject(b.name());
    assert_only_the_injected_node_moved(&diff_json(b.name()), "inspect --uid <main>");
}

#[test]
fn urls_does_not_reach_the_baseline() {
    if !common::browser_ready() {
        return;
    }
    // `--urls` appends `url="…"` to every link line. Stored, that made the next diff — which
    // renders no url token — read every link on the page as CHANGED. It is the one path here
    // that inflated `changed` rather than `added`.
    let b = TestBrowser::new("baseline-urls");
    if !goto(b.name()) {
        return;
    }
    let (stdout, _, code) = run_cli(&["--browser", b.name(), "inspect", "--urls"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("url=\""), "--urls still annotates what it prints:\n{stdout}");

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
    let (stdout, _, code) =
        run_cli(&["--browser", b.name(), "inspect", "--limit", "20", "--filter", "button"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("button \"Go\""), "the collected view is still printed:\n{stdout}");
    assert!(!stdout.contains("heading"), "the filter still applies to it:\n{stdout}");

    inject(b.name());
    assert_only_the_injected_node_moved(&diff_json(b.name()), "inspect --limit 20 --filter button");
}

#[test]
fn paging_still_windows_the_printed_view() {
    if !common::browser_ready() {
        return;
    }
    // `--max-chars`/`--offset` cut the PRINTED text and must keep doing so: they were the one
    // reduction already excluded from the baseline, and the fix must not fold them in.
    let b = TestBrowser::new("baseline-paging");
    if !goto(b.name()) {
        return;
    }
    let (stdout, _, code) =
        run_cli(&["--browser", b.name(), "inspect", "--filter", "button", "--max-chars", "12"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("chars truncated"), "paging still truncates:\n{stdout}");
    assert!(stdout.contains("--offset 12"), "paging still says how to continue:\n{stdout}");

    inject(b.name());
    assert_only_the_injected_node_moved(&diff_json(b.name()), "inspect --filter --max-chars");
}

// ─── the uid map is part of the baseline ───

#[test]
fn a_display_flag_does_not_narrow_the_uid_map() {
    if !common::browser_ready() {
        return;
    }
    // The stored map used to be the reduced rendering's too, so `--max-depth 1` left a map of
    // four uids on a page of thirteen and the button below the limit was unreachable — a
    // display flag deciding which nodes the next command may act on.
    let b = TestBrowser::new("baseline-uidmap");
    if !goto(b.name()) {
        return;
    }
    let button_uid = uid_of_role(b.name(), "button");
    let (_, _, code) = run_cli(&["--browser", b.name(), "inspect", "--max-depth", "1"]);
    assert_eq!(code, 0);

    let (stdout, _, _) = run_cli(&["--browser", b.name(), "--json", "click", &button_uid]);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("click should emit JSON: {e}\n{stdout}"));
    assert_eq!(
        json["ok"], true,
        "a uid deeper than --max-depth is still a node on the page, got {json}"
    );
}

// ─── the verdict of the NEXT action, which is an instruction and not a datum ───

/// The click response for `#go`, after `inspect` was run with `flags`.
///
/// `#go` is a button with no handler: the honest reading of clicking it is that the page did
/// not move, whatever `inspect` printed beforehand.
fn verdict_of_inert_click(label: &str, flags: &[&str]) -> Option<serde_json::Value> {
    let b = TestBrowser::new(label);
    if !goto(b.name()) {
        return None;
    }
    let mut args = vec!["--browser", b.name(), "inspect"];
    args.extend_from_slice(flags);
    let (_, _, code) = run_cli(&args);
    assert_eq!(code, 0, "inspect {flags:?} should succeed");

    let (stdout, _, _) =
        run_cli(&["--browser", b.name(), "--json", "click", "--selector", "#go"]);
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("click should emit JSON: {e}\n{stdout}"));
    assert_eq!(json["ok"], true, "the click itself should succeed, got {json}");
    Some(json)
}

#[test]
fn a_display_flag_does_not_flip_the_verdict_of_the_next_action() {
    if !common::browser_ready() {
        return;
    }
    // Two sessions, one page, one identical click on one inert button. The ONLY difference is
    // a display flag used on the `inspect` BEFORE the click. Measured on this fixture before
    // the fix, which is the Wikipedia shape in miniature:
    //
    //   after `inspect --urls` : changed / tree_delta        next: proceed   (changed: 1)
    //   after `inspect`        : no_effect / delivered_no_change  next: confirm   (changed: 0)
    //
    // `--urls` had stored `url="…"` on the link's line; the post-click read renders it bare,
    // so the link came back as a rewritten node and the delta was not empty. The cause is the
    // same poisoned baseline the tests above measure through `diff` — but here it lands on
    // `next`, which an agent BRANCHES on, and the two branches are opposites.
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

    // Agreement alone would also be satisfied by both sides being wrong. `proceed` is the
    // specific false instruction the Wikipedia measurement found: carry on, the page moved —
    // about a click on a button with no handler.
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
    let (stdout, stderr, code) =
        run_cli(&["--browser", b.name(), "goto", &url, "--inspect", "--max-depth", "1"]);
    if code != 0 {
        common::unavailable(&format!("goto --inspect failed: {stderr}"));
        return;
    }
    assert!(!stdout.contains("combobox"), "--max-depth still cuts the printed tree:\n{stdout}");

    inject(b.name());
    assert_only_the_injected_node_moved(&diff_json(b.name()), "goto --inspect --max-depth 1");
}

#[test]
fn pipe_goto_inspect_max_depth_does_not_reach_the_baseline() {
    if !common::browser_ready() {
        return;
    }
    // `goto` is deliberately outside `mutates_page`, so nothing overwrites what it stored:
    // this is where `attach_snapshot`'s truncated baseline actually surfaced.
    let b = TestBrowser::new("baseline-pipegoto");
    let url = common::fixture_url(FIXTURE);
    let responses = run_pipe(
        &["--browser", b.name()],
        &[
            format!(r#"{{"cmd":"goto","url":"{url}","inspect":true,"max_depth":1}}"#),
            format!(r#"{{"cmd":"eval","expression":{}}}"#, serde_json::json!(INJECT)),
            r#"{"cmd":"diff"}"#.to_string(),
        ],
    );
    let Some(diff) = responses.last() else {
        common::unavailable("pipe produced no response");
        return;
    };
    assert_eq!(responses[0]["ok"], true, "goto should succeed: {}", responses[0]);
    assert!(
        !responses[0]["snapshot"].as_str().unwrap_or_default().contains("combobox"),
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
    // With `--verdict off` no change report runs, so the truncated snapshot an action's
    // `inspect: true` stored was the one the next `diff` compared against. With the report on
    // it was overwritten by a full read, which is why this only ever failed here.
    let b = TestBrowser::new("baseline-pipeoff");
    let url = common::fixture_url(FIXTURE);
    let responses = run_pipe(
        &["--browser", b.name(), "--verdict", "off"],
        &[
            format!(r#"{{"cmd":"goto","url":"{url}"}}"#),
            r##"{"cmd":"click","selector":"#go","inspect":true,"max_depth":1}"##.to_string(),
            format!(r#"{{"cmd":"eval","expression":{}}}"#, serde_json::json!(INJECT)),
            r#"{"cmd":"diff"}"#.to_string(),
        ],
    );
    let Some(diff) = responses.last() else {
        common::unavailable("pipe produced no response");
        return;
    };
    assert_eq!(responses[1]["ok"], true, "click should succeed: {}", responses[1]);
    assert_only_the_injected_node_moved(diff, "pipe click inspect+max_depth, report off");
}
