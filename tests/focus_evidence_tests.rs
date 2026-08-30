//! What `focus` proves, and what it does not.
//!
//! `focus_only` claims the action arrived somewhere, and on a path with no hit test (`--xy`, a
//! JS click, a target inside a frame) it is the only delivery evidence there is. But Chrome
//! marks the `RootWebArea` focused whenever the document holds focus, which is what a click on
//! anything non-focusable leaves behind — so the destination is judged, not the move itself.
//! The `focus` field is unchanged; it just no longer licenses the verdict.

use std::process::{Command, Output, Stdio};
use std::io::Write as _;
use std::time::{Duration, Instant};

use serde_json::Value;

mod common;
use common::TestBrowser;

fn run_pipe(browser: &str, script: &str) -> Vec<Value> {
    let mut child = Command::new(common::binary())
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

/// A `--xy` click that reached nothing focusable must not answer `proceed`. `--xy` names no
/// element, so there is no hit test and `focus_only` is the only word that could claim delivery.
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

    // The reading stays on the response. The root's uid is a `backendNodeId` and differs
    // between documents, so this checks that a destination is reported, not which one.
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

    // A span inside a link: the browser focuses the ANCESTOR link, so `focus.to` must not be
    // read as "the element you clicked".
    let span = &lines[3];
    assert_eq!(span["uid"], "n10", "the click was aimed at the span: {span}");
    assert_eq!(span["focus"]["to"], "n9", "focus went to the link that wraps it: {span}");
    assert_eq!(span["verdict"], "changed", "{span}");
}
