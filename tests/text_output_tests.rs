//! What the DEFAULT output tells a person, on pages where the tool already knows better.
//!
//! Every measurement asserted here was already in `--json` before this suite existed. The text
//! branch printed the command's own message, the delta and `verdict: <word> (<reason>)`, and
//! dropped the rest — so the surface where a human judges the tool (the README quickstart, the
//! first try, the demo) reported a clean success on a page that had silently discarded the
//! write. These tests pin the lines that fixed that, and pin that no ANSI escape reaches a pipe.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;

mod common;
use common::TestBrowser;

fn binary() -> String {
    let mut path = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
    path.push("chrome-agent");
    path.to_string_lossy().into_owned()
}

/// Run the CLI with stdout captured — which is to say, NOT a terminal. Every assertion in this
/// file therefore also asserts the no-colour path.
fn run_cli(args: &[&str]) -> (String, i32) {
    let output = Command::new(binary()).args(args).output().expect("Failed to run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

/// Open a fixture and establish the baseline the change report compares against.
fn open_with_baseline(browser: &str, fixture: &str) -> bool {
    if !common::browser_ready() {
        return false;
    }
    let (_, code) = run_cli(&["--browser", browser, "goto", &common::fixture_url(fixture)]);
    if code != 0 {
        return common::unavailable(&format!("goto {fixture} failed"));
    }
    let (_, code) = run_cli(&["--browser", browser, "inspect"]);
    assert_eq!(code, 0, "inspect should establish the baseline");
    true
}

/// No escape sequence may reach a pipe. Asserted on every captured run in this file, not just
/// the dedicated test: colour that leaks into CI logs and `| grep` is the failure mode.
fn assert_no_ansi(stdout: &str) {
    assert!(
        !stdout.contains('\x1b'),
        "an ANSI escape reached a pipe: {}",
        stdout.escape_debug()
    );
}

/// The measured bug. `fill` reported "Filled selector '#micro'" and a verdict, over a field the
/// tool had already read back as empty.
#[test]
fn a_fill_the_page_reverted_says_so_in_text_mode() {
    let b = TestBrowser::new("text-out-revert");
    if !open_with_baseline(b.name(), "form_value_microtask_revert.html") {
        return;
    }
    let (stdout, code) = run_cli(&["--browser", b.name(), "fill", "--selector", "#micro", "SAVE20"]);
    assert_eq!(code, 0, "the write was delivered; only its result is bad news: {stdout}");
    assert_no_ansi(&stdout);
    assert!(stdout.contains("value: NOT KEPT"), "{stdout}");
    assert!(stdout.contains("wrote \"SAVE20\""), "what was written: {stdout}");
    assert!(stdout.contains("page holds \"\""), "and what the page holds: {stdout}");
    assert!(stdout.contains("read 60 ms later"), "as of a stated moment: {stdout}");
    // The word alone is jargon; the gloss is what a reader who has not read the taxonomy gets.
    assert!(
        stdout.contains("verdict: not_kept (value_reverted) — "),
        "the verdict keeps its shape and gains a gloss: {stdout}"
    );
    assert!(stdout.contains("next: stop"), "and a repeat is forbidden: {stdout}");
    // The advice is one or two lines here and a paragraph in JSON. The full text buried the
    // three lines above it, which are the ones carrying the news.
    let hint = stdout
        .lines()
        .find(|l| l.starts_with("hint: "))
        .unwrap_or_else(|| panic!("no hint line: {stdout}"));
    assert!(hint.len() < 200, "the terminal hint is a paragraph again ({}): {hint}", hint.len());
    assert!(hint.contains("Do not fill it again"), "shortening dropped the prohibition: {hint}");
    for line in stdout.lines() {
        assert!(line.len() < 300, "a line nobody reads: {line}");
    }

    // …and the JSON keeps the full text, which bench and the other tests read.
    let (out, code) = run_cli(&[
        "--browser", b.name(), "--json", "fill", "--selector", "#micro", "SAVE20",
    ]);
    assert_eq!(code, 0, "{out}");
    let v: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("not JSON ({e}): {out}"));
    let full = v["verdict_hint"].as_str().unwrap_or_default();
    assert!(full.len() > 300, "the JSON hint must stay complete: {full}");
    assert!(full.contains("value.actual"), "{full}");
}

/// The other half of the same rule: a fill the page kept adds no line. A tool that narrates its
/// successes teaches the reader to skip its output.
#[test]
fn a_fill_the_page_kept_stays_terse() {
    let b = TestBrowser::new("text-out-clean");
    if !open_with_baseline(b.name(), "form_value_reset_on_submit.html") {
        return;
    }
    let (stdout, code) =
        run_cli(&["--browser", b.name(), "fill", "--selector", "#email", "ada@example.com"]);
    assert_eq!(code, 0, "{stdout}");
    assert_no_ansi(&stdout);
    assert!(!stdout.contains("value:"), "a kept value is not news: {stdout}");
    assert!(!stdout.contains("NOT KEPT"), "{stdout}");
    assert!(stdout.contains("verdict: changed"), "{stdout}");
    assert!(stdout.contains("next: proceed"), "{stdout}");
    // No hint either: there is nothing for the reader to resolve.
    assert!(!stdout.contains("hint:"), "{stdout}");
}

/// S3, the archetype: the submit worked AND cleared the field the agent had just filled. Both
/// facts belong to the reader, and the second was JSON-only.
#[test]
fn a_value_this_action_destroyed_is_named_in_text_mode() {
    let b = TestBrowser::new("text-out-lost");
    if !open_with_baseline(b.name(), "form_value_reset_on_submit.html") {
        return;
    }
    let (out, code) =
        run_cli(&["--browser", b.name(), "fill", "--selector", "#email", "ada@example.com"]);
    assert_eq!(code, 0, "{out}");
    let (stdout, code) = run_cli(&["--browser", b.name(), "click", "--selector", "#submit"]);
    assert_eq!(code, 0, "{stdout}");
    assert_no_ansi(&stdout);
    assert!(stdout.contains("values lost: 1 field"), "{stdout}");
    assert!(stdout.contains("held \"ada@example.com\""), "and what it held: {stdout}");
    assert!(stdout.contains("textbox"), "and which field: {stdout}");
    // Not `proceed`: a form that submitted and cleared itself and one that threw the input
    // away look identical from here.
    assert!(stdout.contains("next: confirm"), "{stdout}");
}

/// `intercepted` without the receiver is a verdict a person cannot act on, and text mode is
/// what a person reads.
#[test]
fn an_intercepted_click_names_the_receiver_in_text_mode() {
    let b = TestBrowser::new("text-out-intercept");
    if !open_with_baseline(b.name(), "click_overlay.html") {
        return;
    }
    let (stdout, code) = run_cli(&["--browser", b.name(), "click", "--selector", "#target"]);
    assert_eq!(code, 0, "{stdout}");
    assert_no_ansi(&stdout);
    assert!(stdout.contains("received by: div#scrim"), "{stdout}");
    assert!(stdout.contains("verdict: intercepted (hit_test_receiver) — "), "{stdout}");
    assert!(stdout.contains("next: dismiss"), "{stdout}");
    // The hint the action wrote itself names the element; the generic one only says one exists.
    assert!(stdout.contains("hint: "), "{stdout}");
    assert!(stdout.contains("div#scrim"), "{stdout}");
}

/// `unchanged` is the verdict most easily over-read as "nothing happened", which the taxonomy
/// forbids as a conclusion. In text mode the gloss is the only thing that says so.
#[test]
fn the_unchanged_verdict_carries_its_limit_on_the_line() {
    let b = TestBrowser::new("text-out-unchanged");
    if !open_with_baseline(b.name(), "verdict_states.html") {
        return;
    }
    // Twice: the first press moves focus onto the body, which is a `changed / focus_only`.
    let (out, code) = run_cli(&["--browser", b.name(), "press", "ArrowDown"]);
    assert_eq!(code, 0, "{out}");
    let (stdout, code) = run_cli(&["--browser", b.name(), "press", "ArrowDown"]);
    assert_eq!(code, 0, "{stdout}");
    assert_no_ansi(&stdout);
    let line = stdout
        .lines()
        .find(|l| l.starts_with("verdict: unchanged"))
        .unwrap_or_else(|| panic!("no unchanged verdict: {stdout}"));
    assert!(line.contains("identical_tree"), "{line}");
    assert!(line.contains(" — "), "the gloss is not optional: {line}");
    assert!(line.contains("not the same as"), "and it states the limit: {line}");
    assert!(stdout.contains("next: confirm"), "never `retry`: {stdout}");
}

/// The token, in all three modes, from the one place a verdict is settled.
#[test]
fn the_next_token_rides_on_every_mode() {
    let b = TestBrowser::new("text-out-next-json");
    if !open_with_baseline(b.name(), "click_overlay.html") {
        return;
    }
    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "click", "--selector", "#target"]);
    assert_eq!(code, 0, "{stdout}");
    let v: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}"));
    assert_eq!(v["next"], "dismiss", "{v}");
    // Nothing was renamed or removed to make room for it.
    for field in ["ok", "message", "verdict", "verdict_reason", "verdict_hint", "intercepted_by"] {
        assert!(!v[field].is_null(), "{field} must survive: {v}");
    }

    let mut child = Command::new(binary())
        .args(["--browser", b.name(), "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pipe");
    let script = format!(
        "{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": common::fixture_url("click_overlay.html")}),
        serde_json::json!({"cmd": "click", "selector": "#target"}),
    );
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("pipe output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last = stdout.lines().rfind(|l| l.contains("\"verdict\"")).unwrap_or_default();
    let v: Value = serde_json::from_str(last).unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}"));
    assert!(
        ["proceed", "inspect", "retry", "confirm", "dismiss", "stop"]
            .contains(&v["next"].as_str().unwrap_or_default()),
        "pipe mode must answer with the same closed vocabulary: {v}"
    );
}

/// The whole rule in one assertion: whatever the page does, the default output of a mutating
/// action never sends the caller off to repeat it blind.
#[test]
fn no_page_makes_the_text_output_advise_a_blind_retry() {
    let b = TestBrowser::new("text-out-no-retry");
    if !common::browser_ready() {
        return;
    }
    for (fixture, args) in [
        ("form_value_microtask_revert.html", vec!["fill", "--selector", "#micro", "x"]),
        ("click_overlay.html", vec!["click", "--selector", "#target"]),
        ("form_value_reset_on_submit.html", vec!["click", "--selector", "#submit"]),
        ("verdict_states.html", vec!["press", "ArrowDown"]),
    ] {
        if !open_with_baseline(b.name(), fixture) {
            return;
        }
        let mut argv = vec!["--browser", b.name()];
        argv.extend(args);
        let (stdout, _) = run_cli(&argv);
        assert_no_ansi(&stdout);
        assert!(
            !stdout.contains("next: retry"),
            "{fixture} advised a blind retry — a repeat here is a second real action: {stdout}"
        );
        assert!(
            stdout.lines().any(|l| l.starts_with("next: ")),
            "{fixture} left the reader without a next step: {stdout}"
        );
    }
}
