//! What a click can claim about where it landed.
//!
//! Each test asserts both what the response reports and what the page actually recorded.

use std::process::Command;

use serde_json::Value;

mod common;
use common::TestBrowser;

fn run_cli(args: &[&str]) -> (String, i32) {
    let output = Command::new(common::binary()).args(args).output().expect("Failed to run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

/// Open a fixture and take the first snapshot, so every click below has a verdict baseline.
fn open(browser: &str, fixture: &str) -> bool {
    if !common::browser_ready() {
        return false;
    }
    let url = common::fixture_url(fixture);
    let (_, code) = run_cli(&["--browser", browser, "goto", &url]);
    if code != 0 {
        return common::unavailable(&format!("goto {fixture} failed"));
    }
    let (_, code) = run_cli(&["--browser", browser, "--json", "inspect"]);
    if code != 0 {
        return common::unavailable(&format!("inspect {fixture} failed"));
    }
    true
}

fn eval(browser: &str, expression: &str) -> Value {
    let (stdout, code) = run_cli(&["--browser", browser, "--json", "eval", expression]);
    assert_eq!(code, 0, "eval failed: {stdout}");
    let v: Value = serde_json::from_str(&stdout).expect("JSON eval response");
    v["result"].clone()
}

/// The uid the current snapshot gives the node whose line contains every needle.
fn uid_for(browser: &str, needles: &[&str]) -> String {
    let (stdout, code) = run_cli(&["--browser", browser, "--json", "inspect"]);
    assert_eq!(code, 0, "{stdout}");
    let snapshot: Value = serde_json::from_str(&stdout).expect("JSON inspect");
    let text = snapshot["snapshot"].as_str().unwrap_or_default().to_string();
    text.lines()
        .find(|line| needles.iter().all(|n| line.contains(n)))
        .and_then(|line| line.trim_start().strip_prefix("uid="))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no uid matching {needles:?} in:\n{text}"))
        .to_string()
}

fn click(browser: &str, args: &[&str]) -> Value {
    let mut argv = vec!["--browser", browser, "--json", "click"];
    argv.extend_from_slice(args);
    let (stdout, code) = run_cli(&argv);
    assert_eq!(code, 0, "click failed: {stdout}");
    serde_json::from_str(&stdout).expect("JSON click response")
}

/// Reset focus, so two clicks are compared from the same starting state.
fn blur(browser: &str) {
    let _ = eval(browser, "document.activeElement && document.activeElement.blur(); 1");
}

// --- The overlay -----------------------------------------------------------

/// Both spellings of `click` must name the scrim, and the page must agree it got the event.
#[test]
fn a_covered_button_reports_the_element_that_took_the_click() {
    let b = TestBrowser::new("hit-overlay");
    if !open(b.name(), "click_overlay.html") {
        return;
    }
    let uid = uid_for(b.name(), &["button", "Underneath"]);

    for aim in [vec![uid.as_str()], vec!["--selector", "#target"]] {
        let _ = eval(b.name(), "window.receiver = null; 1");
        let response = click(b.name(), &aim);

        assert_eq!(response["ok"], Value::Bool(true), "the click is still delivered: {response}");
        assert_eq!(response["verdict"], "intercepted", "aimed via {aim:?}: {response}");
        assert_eq!(response["verdict_reason"], "hit_test_receiver");
        assert_eq!(response["delivery"], "intercepted");
        assert_eq!(
            response["intercepted_by"]["id"], "scrim",
            "the receiver must be named, not merely implied: {response}"
        );
        assert_eq!(response["intercepted_by"]["tag"], "DIV");
        assert_eq!(
            response["uid"], uid,
            "the response still names the node that was aimed at: {response}"
        );
        assert!(
            response["verdict_hint"].as_str().unwrap_or_default().contains("div#scrim"),
            "the hint has to say which element to deal with: {response}"
        );
        assert_eq!(
            eval(b.name(), "window.receiver"),
            Value::String("scrim".into()),
            "the verdict claims the scrim received it, so the page must say so too"
        );
    }
}

/// `--on-intercept refuse` sends no event at all.
#[test]
fn refusing_an_interception_dispatches_nothing() {
    let b = TestBrowser::new("hit-overlay-refuse");
    if !open(b.name(), "click_overlay.html") {
        return;
    }
    let _ = eval(b.name(), "window.receiver = null; 1");
    let (stdout, code) = run_cli(&[
        "--browser", b.name(), "--json", "--on-intercept", "refuse",
        "click", "--selector", "#target",
    ]);
    assert_ne!(code, 0, "a refusal is a failure the caller has to handle: {stdout}");
    let response: Value = serde_json::from_str(&stdout).expect("JSON error response");
    assert_eq!(response["ok"], Value::Bool(false));
    let error = response["error"].as_str().unwrap_or_default();
    assert!(error.contains("div#scrim"), "the refusal names the receiver: {error}");
    assert_eq!(
        eval(b.name(), "window.receiver"),
        Value::Null,
        "refuse means nothing was dispatched — not even to the overlay"
    );
}

#[test]
fn a_smooth_scrolling_page_lands_or_says_it_could_not_aim() {
    let b = TestBrowser::new("hit-smooth");
    if !open(b.name(), "smooth_scroll_click.html") {
        return;
    }
    let uid = uid_for(b.name(), &["button", "Click me"]);
    let response = click(b.name(), &[uid.as_str()]);
    let landed = eval(b.name(), "document.title") == Value::String("clicked".into());

    if landed {
        assert_eq!(response["delivery"], "target_hit", "{response}");
    } else {
        assert_eq!(
            response["verdict_reason"], "scroll_not_settled",
            "a click that did not land must say why, not report the focus it moved: {response}"
        );
        assert_eq!(response["delivery"], "not_settled");
        assert!(
            response["message"].as_str().unwrap_or_default().starts_with("Did not click"),
            "an action that dispatched nothing must not answer \"Clicked\": {response}"
        );
    }
    assert_ne!(
        response["verdict_reason"], "focus_only",
        "focus churn may not stand in for a click that never arrived: {response}"
    );
}

// --- Shapes a naive hit test gets wrong in the other direction ---------------

/// The visible box is a sibling span, so the hit retargets via `closest('label').control`.
#[test]
fn a_label_that_forwards_its_click_is_not_an_interception() {
    let b = TestBrowser::new("hit-label");
    if !open(b.name(), "intercept_label_span_forwards_click.html") {
        return;
    }
    let uid = uid_for(b.name(), &["checkbox"]);
    let response = click(b.name(), &[uid.as_str()]);
    assert_ne!(response["verdict"], "intercepted", "{response}");
    assert_eq!(response["delivery"], "target_hit", "{response}");
    assert_eq!(
        eval(b.name(), "document.querySelector('#agree').checked"),
        Value::Bool(true),
        "the control really was toggled"
    );
}

/// `elementFromPoint` returns the host, so the probe descends into the shadow root.
#[test]
fn a_button_inside_an_open_shadow_root_is_hit_not_intercepted() {
    let b = TestBrowser::new("hit-shadow");
    if !open(b.name(), "intercept_shadow_host_open.html") {
        return;
    }
    let uid = uid_for(b.name(), &["button", "Buy"]);
    let response = click(b.name(), &[uid.as_str()]);
    assert_eq!(response["delivery"], "target_hit", "{response}");
    assert_ne!(response["verdict"], "intercepted");
    assert_eq!(eval(b.name(), "document.title"), Value::String("inner clicked".into()));
}

/// A modal gets the `modal_dialog` reason, not `blocked`: the recovery is to close it.
#[test]
fn a_modal_dialog_is_named_as_the_receiver_it_is() {
    let b = TestBrowser::new("hit-modal");
    if !open(b.name(), "intercept_modal_backdrop.html") {
        return;
    }
    // The dialog hides the shell from the a11y tree, so the button behind it has no uid.
    let response = click(b.name(), &["--selector", "#behind"]);
    assert_eq!(response["verdict"], "intercepted", "{response}");
    assert_eq!(response["verdict_reason"], "modal_dialog", "{response}");
    assert_eq!(response["intercepted_by"]["tag"], "DIALOG");
    assert_eq!(response["intercepted_by"]["id"], "terms");
    assert_eq!(response["intercepted_by"]["modal"], Value::Bool(true));
    assert_eq!(
        eval(b.name(), "window.receiver"),
        Value::Null,
        "the button behind a modal receives nothing"
    );
}

/// The bounding-box centre falls on the paragraph in the gap between the two line boxes.
#[test]
fn an_inline_link_across_two_lines_is_aimed_at_its_largest_box() {
    let b = TestBrowser::new("hit-wrapped");
    if !open(b.name(), "intercept_wrapped_inline_link.html") {
        return;
    }
    let uid = uid_for(b.name(), &[" link \"a link"]);
    let response = click(b.name(), &[uid.as_str()]);
    assert_ne!(
        response["verdict"], "intercepted",
        "the paragraph that contains the link is not an interceptor: {response}"
    );
    assert_eq!(
        eval(b.name(), "document.title"),
        Value::String("link clicked".into()),
        "the link's own handler ran"
    );
}

/// A zero-size element has no point to aim at, so the click is synthetic and no hit test ran.
#[test]
fn a_synthetic_click_reports_that_it_was_synthetic() {
    let b = TestBrowser::new("hit-js");
    if !open(b.name(), "intercept_js_click_fallback.html") {
        return;
    }
    let response = click(b.name(), &["--selector", "#hidden"]);
    assert_eq!(response["delivery"], "js", "{response}");
    assert_ne!(
        response["verdict"], "intercepted",
        "a JS click performs no hit test, so interception is inapplicable, not undetected"
    );
    assert_ne!(
        response["verdict"], "no_effect",
        "and it cannot prove delivery either: {response}"
    );
    assert_eq!(eval(b.name(), "document.title"), Value::String("js clicked".into()));
}

// --- `no_effect` and the limits on it ---------------------------------------

/// `no_effect` means delivery proven and observation window quiet, by either spelling.
#[test]
fn an_uncovered_listenerless_button_reports_no_effect_by_either_route() {
    let b = TestBrowser::new("hit-inert");
    if !open(b.name(), "verdict_inert_no_listener.html") {
        return;
    }
    let uid = uid_for(b.name(), &["button", "Does nothing"]);

    let mut verdicts = Vec::new();
    for aim in [vec![uid.as_str()], vec!["--selector", "#inert-btn"]] {
        // Focus is state and a click moves it: both aims start from none.
        blur(b.name());
        let response = click(b.name(), &aim);
        assert_eq!(response["delivery"], "target_hit", "aimed via {aim:?}: {response}");
        assert_eq!(response["verdict"], "no_effect", "aimed via {aim:?}: {response}");
        assert!(
            response["observed_after_ms"].as_u64().is_some(),
            "`no_effect` is a claim about a window and must carry it: {response}"
        );
        let hint = response["verdict_hint"].as_str().unwrap_or_default();
        for blind_spot in ["canvas", "CSS-only", "after the window"] {
            assert!(hint.contains(blind_spot), "the hint omits {blind_spot}: {hint}");
        }
        verdicts.push(response["verdict"].clone());
    }
    assert_eq!(verdicts[0], verdicts[1], "one verb, one verdict");
}

/// An overlay in the parent covering the frame is invisible from the frame's own document.
#[test]
fn a_target_inside_an_iframe_is_clicked_but_not_judged() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("hit-iframe");
    let url = common::fixture_url("intercept_iframe_contains_target.html");
    // One pipe session: the frame binding and the uid map live on the connection.
    let mut session = PipeSession::start(b.name());
    session.send(&serde_json::json!({"cmd": "goto", "url": url}));
    session.send(&serde_json::json!({"cmd": "frame", "target": "#shop"}));
    let snapshot = session.send(&serde_json::json!({"cmd": "inspect"}));
    let text = snapshot["snapshot"].as_str().unwrap_or_default().to_string();
    let uid = text
        .lines()
        .find(|line| line.contains("button"))
        .and_then(|line| line.trim_start().strip_prefix("uid="))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no button uid inside the frame:\n{text}"))
        .to_string();

    let clicked = session.send(&serde_json::json!({"cmd": "click", "uid": uid}));
    assert_eq!(
        clicked["delivery"], "not_probed",
        "no claim may be made about a document we cannot hit-test: {clicked}"
    );
    assert_ne!(clicked["verdict"], "intercepted", "{clicked}");
    let effect = session.send(&serde_json::json!({
        "cmd": "eval",
        "expression": "document.querySelector('#buy').textContent"
    }));
    assert_eq!(
        effect["result"],
        Value::String("bought".into()),
        "the aim point still has to be mapped through the frame's own offset"
    );
}

/// Across every fixture: no JS dispatch claims an interception, and none goes unnamed.
#[test]
fn no_js_dispatch_anywhere_claims_an_interception() {
    let b = TestBrowser::new("hit-mechanical");
    if !common::browser_ready() {
        return;
    }
    let fixtures = [
        ("click_overlay.html", "#target"),
        ("intercept_js_click_fallback.html", "#hidden"),
        ("intercept_modal_backdrop.html", "#behind"),
        ("verdict_inert_no_listener.html", "#inert-btn"),
        ("intercept_wrapped_inline_link.html", "#wrapped"),
    ];
    for (fixture, selector) in fixtures {
        if !open(b.name(), fixture) {
            return;
        }
        let response = click(b.name(), &["--selector", selector]);
        if response["delivery"] == "js" {
            assert_ne!(
                response["verdict"], "intercepted",
                "{fixture} claims an interception on a synthetic click: {response}"
            );
        }
        if response["verdict"] == "intercepted" {
            assert_eq!(response["delivery"], "intercepted", "{fixture}: {response}");
            assert!(
                response["intercepted_by"].is_object(),
                "{fixture} claims an interception without naming the receiver: {response}"
            );
        }
    }
}

/// A live pipe session: one command in, one response out, connection state preserved.
struct PipeSession {
    child: std::process::Child,
    responses: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
}

impl PipeSession {
    fn start(browser: &str) -> Self {
        use std::io::BufRead as _;
        use std::process::Stdio;
        let mut child = Command::new(common::binary())
            .args(["--browser", browser, "pipe"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn pipe");
        let stdout = child.stdout.take().expect("pipe stdout");
        Self { child, responses: std::io::BufReader::new(stdout).lines() }
    }

    fn send(&mut self, cmd: &Value) -> Value {
        use std::io::Write as _;
        let stdin = self.child.stdin.as_mut().expect("pipe stdin");
        writeln!(stdin, "{cmd}").expect("write command");
        stdin.flush().expect("flush command");
        let line = self
            .responses
            .next()
            .expect("a response per command")
            .expect("readable response");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad pipe line {line}: {e}"))
    }
}

impl Drop for PipeSession {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.wait();
    }
}
