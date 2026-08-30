//! `click <uid>` and `click --selector` are the same verb: both dispatch real CDP mouse
//! events, so a covered element hands the click to whatever covers it.

use std::process::Command;

use serde_json::Value;

mod common;
use common::TestBrowser;

fn run_cli(args: &[&str]) -> (String, i32) {
    let output = Command::new(common::binary())
        .args(args)
        .output()
        .expect("Failed to run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

fn open(browser: &str, fixture: &str) -> bool {
    if !common::browser_ready() {
        return false;
    }
    let url = common::fixture_url(fixture);
    let (_, code) = run_cli(&["--browser", browser, "goto", &url]);
    if code != 0 {
        return common::unavailable(&format!("goto {fixture} failed"));
    }
    true
}

fn eval(browser: &str, expression: &str) -> Value {
    let (stdout, code) = run_cli(&["--browser", browser, "--json", "eval", expression]);
    assert_eq!(code, 0, "eval failed: {stdout}");
    let v: Value = serde_json::from_str(&stdout).expect("JSON eval response");
    v["result"].clone()
}

/// A covering scrim receives the selector click, as a real pointer would.
#[test]
fn a_selector_click_lands_where_a_real_pointer_would() {
    let b = TestBrowser::new("click-parity-overlay");
    if !open(b.name(), "click_overlay.html") {
        return;
    }
    let (stdout, code) = run_cli(&[
        "--browser",
        b.name(),
        "--json",
        "click",
        "--selector",
        "#target",
    ]);
    assert_eq!(code, 0, "the click itself still succeeds: {stdout}");

    assert_eq!(
        eval(b.name(), "window.receiver"),
        Value::String("scrim".into()),
        "the scrim covers the button, so the scrim is what a pointer hits"
    );
}

#[test]
fn the_uid_path_and_the_selector_path_agree_on_who_receives_the_click() {
    let b = TestBrowser::new("click-parity-uid");
    if !open(b.name(), "click_overlay.html") {
        return;
    }
    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "inspect"]);
    assert_eq!(code, 0, "{stdout}");
    let snapshot: Value = serde_json::from_str(&stdout).expect("JSON inspect");
    let text = snapshot["snapshot"].as_str().unwrap_or_default();
    let uid = text
        .lines()
        .find(|l| l.contains("button") && l.contains("Underneath"))
        .and_then(|l| l.trim_start().strip_prefix("uid="))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no uid for the button in: {text}"))
        .to_string();

    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "click", &uid]);
    assert_eq!(code, 0, "{stdout}");
    let by_uid = eval(b.name(), "window.receiver");

    let (_, code) = run_cli(&["--browser", b.name(), "eval", "window.receiver = null; 1"]);
    assert_eq!(code, 0);
    let (stdout, code) = run_cli(&[
        "--browser",
        b.name(),
        "--json",
        "click",
        "--selector",
        "#target",
    ]);
    assert_eq!(code, 0, "{stdout}");
    let by_selector = eval(b.name(), "window.receiver");

    assert_eq!(
        by_uid, by_selector,
        "two spellings of `click` must not do different things"
    );
}

#[test]
fn an_uncovered_element_is_still_clicked_by_selector() {
    let b = TestBrowser::new("click-parity-plain");
    if !open(b.name(), "verdict_states.html") {
        return;
    }
    let (stdout, code) = run_cli(&[
        "--browser",
        b.name(),
        "--json",
        "click",
        "--selector",
        "#add",
    ]);
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(
        eval(b.name(), "!!document.querySelector('h4')"),
        Value::Bool(true),
        "the handler must have run"
    );
}

#[test]
fn an_element_with_no_layout_box_still_gets_its_handler() {
    let b = TestBrowser::new("click-parity-zerosize");
    if !open(b.name(), "verdict_states.html") {
        return;
    }
    let setup = "document.body.insertAdjacentHTML('beforeend', \
                 '<button id=zero style=\"width:0;height:0;padding:0;border:0\" \
                 onclick=\"window.zeroClicked = true\"></button>'); 1";
    let (_, code) = run_cli(&["--browser", b.name(), "eval", setup]);
    assert_eq!(code, 0);

    let (stdout, code) = run_cli(&[
        "--browser",
        b.name(),
        "--json",
        "click",
        "--selector",
        "#zero",
    ]);
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(
        eval(b.name(), "window.zeroClicked === true"),
        Value::Bool(true),
        "a zero-size element falls back to the JS click rather than aiming at nothing"
    );
}

#[test]
fn a_selector_that_matches_nothing_is_an_error() {
    let b = TestBrowser::new("click-parity-missing");
    if !open(b.name(), "verdict_states.html") {
        return;
    }
    let (stdout, code) = run_cli(&[
        "--browser",
        b.name(),
        "--json",
        "click",
        "--selector",
        "#nope",
    ]);
    assert_ne!(code, 0, "a missing element is a failure: {stdout}");
    assert!(stdout.contains("No element matches selector"), "{stdout}");
}

/// Under `scroll-behavior: smooth` the scroll is an animation, so the aim point must settle
/// before dispatch or the event lands at pre-scroll coordinates.
#[test]
fn a_smooth_scrolling_page_still_gets_its_click() {
    let b = TestBrowser::new("click-parity-smooth");
    if !open(b.name(), "smooth_scroll_click.html") {
        return;
    }
    let (stdout, code) = run_cli(&[
        "--browser",
        b.name(),
        "--json",
        "click",
        "--selector",
        "#target",
    ]);
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(
        eval(b.name(), "document.title"),
        Value::String("clicked".into()),
        "the click must land on the target after the scroll settles, not at its pre-scroll coordinates"
    );
}
