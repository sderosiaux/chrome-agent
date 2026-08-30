//! What `focus` proves, and what it does not.
//!
//! `focus_only` is the verdict the classifier falls to when nothing but focus moved, and its
//! whole claim is that the action ARRIVED somewhere — on a path with no hit test (`--xy`, a
//! JS click, a target inside a frame) it is the only delivery evidence available.
//!
//! Chrome marks the `RootWebArea` node `focused` whenever the document — `<body>` in DOM
//! terms — holds focus. That is what a click on something non-focusable leaves behind, and it
//! is also what the FIRST click anywhere in a freshly loaded page leaves behind, including
//! one that hit nothing at all. Measured before the fix, on the fixture below:
//!
//! ```text
//! click --xy on an inert paragraph
//!   verdict : changed / focus_only
//!   next    : proceed
//!   focus   : {"from": null, "to": "n1"}      <- n1 is the RootWebArea
//! ```
//!
//! `proceed`, on a click that reached nothing. Reproduced identically on `en.wikipedia.org`,
//! where the same shape reads `focus: none -> n27` while the page's own `document.activeElement`
//! answers `BODY`.
//!
//! The reading was never wrong — the browser really did move focus to the document — so the
//! `focus` field is untouched. What changed is that it no longer licenses the word.

use std::process::{Command, Output, Stdio};
use std::io::Write as _;
use std::time::{Duration, Instant};

use serde_json::Value;

mod common;
use common::TestBrowser;

fn binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
    path.push("chrome-agent");
    path
}

fn run_pipe(browser: &str, script: &str) -> Vec<Value> {
    let mut child = Command::new(binary())
        .args(["--browser", browser, "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn chrome-agent");
    child.stdin.take().expect("stdin").write_all(script.as_bytes()).expect("write");
    let deadline = Instant::now() + Duration::from_mins(1);
    loop {
        if child.try_wait().expect("poll").is_some() {
            let out: Output = child.wait_with_output().expect("collect");
            return String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| serde_json::from_str(l).expect("JSON line"))
                .collect();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("pipe timed out");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The centre of an element, so the `--xy` path can be aimed without a hit test.
fn centre_of(browser: &str, url: &str, id: &str) -> (i64, i64) {
    let script = format!(
        "{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": url}),
        serde_json::json!({"cmd": "eval", "expression": format!(
            "(()=>{{const r=document.getElementById('{id}').getBoundingClientRect();\
              return [Math.round(r.x+r.width/2), Math.round(r.y+r.height/2)]}})()"
        )}),
    );
    let lines = run_pipe(browser, &script);
    let xy = &lines[1]["result"];
    (xy[0].as_i64().expect("x"), xy[1].as_i64().expect("y"))
}

/// The defect, end to end: a click that reached nothing focusable must not answer `proceed`.
///
/// `--xy` is the path that matters — it names no element, so there is no hit test and
/// `focus_only` is the only word that could have claimed delivery.
#[test]
fn focus_landing_on_the_document_does_not_prove_a_click_arrived() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("focus-evidence-xy");
    let url = common::fixture_url("focus_after_click.html");
    let (x, y) = centre_of(guard.name(), &url, "log");

    let script = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": url}),
        serde_json::json!({"cmd": "inspect"}),
        serde_json::json!({"cmd": "click", "xy": [x, y]}),
    );
    let clicked = &run_pipe(guard.name(), &script)[2];

    assert_eq!(clicked["ok"], true, "the click itself is not a failure: {clicked}");
    assert_eq!(
        clicked["verdict"], "unchanged",
        "the document taking focus is not proof the click reached an element: {clicked}"
    );
    assert_eq!(clicked["verdict_reason"], "identical_tree", "{clicked}");
    assert_ne!(clicked["next"], "proceed", "an agent must not carry on from this: {clicked}");

    // The reading was always true and stays on the response: the fix is about what the word
    // may claim, not about hiding the measurement. The root's uid is a `backendNodeId` and
    // differs between documents — asserting a literal `n1` pinned an accident of one run —
    // so what is checked is that the destination is reported and that the delta names it.
    let focused = clicked["focus"]["to"]
        .as_str()
        .unwrap_or_else(|| panic!("the focus move must still be reported: {clicked}"));
    assert!(
        clicked["delta"]
            .as_str()
            .is_some_and(|d| d.contains(&format!("focus: none -> {focused}"))),
        "the delta line is unchanged: {clicked}"
    );
}

/// The control, and the reason the fix judges the destination rather than dropping focus
/// altogether: focus landing on a real element is still evidence, and still reported.
#[test]
fn focus_landing_on_a_real_element_is_still_evidence() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("focus-evidence-real");
    let url = common::fixture_url("focus_after_click.html");
    let script = format!(
        "{}\n{}\n{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": url}),
        serde_json::json!({"cmd": "inspect"}),
        serde_json::json!({"cmd": "click", "uid": "n8"}),
        serde_json::json!({"cmd": "click", "uid": "n10"}),
    );
    let lines = run_pipe(guard.name(), &script);

    let button = &lines[2];
    assert_eq!(button["focus"]["to"], "n8", "the button itself took focus: {button}");
    assert_eq!(button["verdict"], "changed", "{button}");

    // A span inside a link: the browser focuses the ANCESTOR link, which is correct and is
    // why `focus.to` must not be read as "the element you clicked".
    let span = &lines[3];
    assert_eq!(span["uid"], "n10", "the click was aimed at the span: {span}");
    assert_eq!(span["focus"]["to"], "n9", "focus went to the link that wraps it: {span}");
    assert_eq!(span["verdict"], "changed", "{span}");
}
