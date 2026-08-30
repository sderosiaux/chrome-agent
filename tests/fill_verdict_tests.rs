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

/// Load a fixture and fill `selector` with `value`. Returns the parsed response.
fn fill_on(browser: &str, fixture: &str, selector: &str, value: &str) -> Option<(Value, i32)> {
    if !common::browser_ready() {
        return None;
    }
    let (_, code) = run_cli(&["--browser", browser, "goto", &common::fixture_url(fixture)]);
    if code != 0 {
        common::unavailable(&format!("goto {fixture} failed"));
        return None;
    }
    let (out, code) = run_cli(&[
        "--browser", browser, "--verdict", "off", "--json", "fill", "--selector", selector, value,
    ]);
    Some((serde_json::from_str(&out).unwrap_or(Value::Null), code))
}

/// Load a fixture, establish a baseline, then fill with the change report ON, so the verdict
/// is decided against a real tree comparison.
fn fill_with_verdict(browser: &str, fixture: &str, selector: &str, value: &str) -> Option<Value> {
    if !common::browser_ready() {
        return None;
    }
    let (_, code) = run_cli(&["--browser", browser, "goto", &common::fixture_url(fixture)]);
    if code != 0 {
        common::unavailable(&format!("goto {fixture} failed"));
        return None;
    }
    let (_, code) = run_cli(&["--browser", browser, "inspect"]);
    assert_eq!(code, 0, "inspect should establish the baseline");
    let (out, _) = run_cli(&["--browser", browser, "--json", "fill", "--selector", selector, value]);
    Some(serde_json::from_str(&out).unwrap_or_else(|e| panic!("not JSON ({e}): {out}")))
}

/// A value the page emptied is `not_kept / value_reverted`: the failed read-back outranks the
/// focus move the fill itself caused.
#[test]
fn a_value_the_page_emptied_is_not_reported_as_a_change() {
    let b = TestBrowser::new("fill-verdict-micro");
    let Some(v) = fill_with_verdict(b.name(), "form_value_microtask_revert.html", "#micro", "hello@example.com")
    else {
        return;
    };
    assert_eq!(v["value"]["verbatim"], false, "the page did empty it: {v}");
    assert_eq!(v["verdict"], "not_kept", "{v}");
    assert_eq!(v["verdict_reason"], "value_reverted", "{v}");
    // The delta stays on the response.
    assert!(v["changed"].is_object(), "the change report is still there to read: {v}");
    let hint = v["verdict_hint"].as_str().unwrap_or_default();
    assert!(hint.contains("value.actual"), "the hint names the field to read: {v}");
    assert!(hint.contains("Do not fill it again"), "and forbids the reflex: {v}");
}

/// The other revert shape: this fixture rewrites the value inside the dispatched `input` event.
#[test]
fn a_controlled_component_that_takes_the_value_back_says_not_kept() {
    let b = TestBrowser::new("fill-verdict-controlled");
    let Some(v) = fill_with_verdict(b.name(), "form_value_controlled_revert.html", "input", "typed by the agent")
    else {
        return;
    };
    assert_eq!(v["verdict"], "not_kept", "{v}");
    assert_eq!(v["value"]["verbatim"], false, "{v}");
}

/// A mask is `not_kept / value_rewritten`: the write landed, in the page's own shape.
#[test]
fn a_mask_that_reformats_the_value_says_rewritten_not_reverted() {
    let b = TestBrowser::new("fill-verdict-mask");
    let Some(v) = fill_with_verdict(b.name(), "form_value_phone_mask.html", "#phone", "5551234567") else {
        return;
    };
    assert_eq!(v["verdict"], "not_kept", "{v}");
    assert_eq!(v["verdict_reason"], "value_rewritten", "{v}");
    assert!(
        v["value"]["actual"].as_str().unwrap_or_default().contains("555"),
        "and both strings are still on the response: {v}"
    );
}

/// A fill the page kept reports `tree_delta`; `value_kept` is only for a write the tree
/// cannot show.
#[test]
fn a_fill_the_page_kept_still_reports_the_change() {
    let b = TestBrowser::new("fill-verdict-plain");
    let Some(v) = fill_with_verdict(b.name(), "form_value_plain_input.html", "input", "hello@example.com")
    else {
        return;
    };
    assert_eq!(v["value"]["verbatim"], true, "{v}");
    assert_eq!(v["verdict"], "changed", "{v}");
    assert_eq!(v["verdict_reason"], "tree_delta", "the delta shows the value, so it wins: {v}");
    assert!(
        v["delta"].as_str().unwrap_or_default().contains("hello@example.com"),
        "and it names what changed and where: {v}"
    );
}

/// A secret renders as a fixed marker, so refilling one produces no diffable change and the
/// read-back is the only evidence: `changed / value_kept`.
#[test]
fn a_secret_refill_the_tree_cannot_show_is_not_reported_as_focus_alone() {
    let b = TestBrowser::new("fill-verdict-secret-refill");
    // The fixture's fields are pre-filled, so this is a refill: marker before, marker after.
    let Some(v) = fill_with_verdict(b.name(), "snapshot_secret_values.html", "#card", "4242424242424242")
    else {
        return;
    };
    assert_eq!(v["value"]["redacted"], true, "{v}");
    assert_eq!(v["value"]["verbatim"], true, "the write was read back and held: {v}");
    assert_eq!(v["changed"]["changed"], 0, "and the tree could not show it: {v}");
    assert_eq!(v["verdict"], "changed", "{v}");
    assert_eq!(v["verdict_reason"], "value_kept", "{v}");
    assert_ne!(v["verdict_reason"], "focus_only", "{v}");
    assert_eq!(v["next"], "proceed", "{v}");
    assert!(
        !serde_json::to_string(&v).unwrap_or_default().contains("4242424242424242"),
        "and the verdict did not put the card number on stdout: {v}"
    );
}

/// The boundary: a FIRST fill of a secret field still reports `tree_delta`, because the
/// marker appearing where the tree showed no value is visible. Only a refill is invisible.
#[test]
fn filling_an_empty_secret_field_still_reports_the_tree_delta() {
    let b = TestBrowser::new("fill-verdict-secret-first");
    let Some(v) = fill_with_verdict(b.name(), "form_value_password.html", "#p", "topsecret123") else {
        return;
    };
    assert_eq!(v["value"]["verbatim"], true, "{v}");
    assert_eq!(v["verdict_reason"], "tree_delta", "{v}");
    assert!(
        v["delta"].as_str().unwrap_or_default().contains("<redacted>"),
        "the marker is what appeared, not the value: {v}"
    );
    assert!(
        !serde_json::to_string(&v).unwrap_or_default().contains("topsecret123"),
        "{v}"
    );
}

/// A bulk fill is judged on its worst field through the same rung: a form of pre-filled
/// secret fields reports `verbatim: true` per field while the tree shows nothing.
#[test]
fn a_bulk_fill_of_secret_fields_reports_the_write_it_confirmed() {
    let b = TestBrowser::new("fill-verdict-bulk-secret");
    if !common::browser_ready() {
        return;
    }
    let url = common::fixture_url("snapshot_secret_values.html");
    let (_, code) = run_cli(&["--browser", b.name(), "goto", &url]);
    if code != 0 {
        common::unavailable("goto snapshot_secret_values.html failed");
        return;
    }
    let (snapshot, code) = run_cli(&["--browser", b.name(), "inspect"]);
    assert_eq!(code, 0, "inspect should establish the baseline");
    // uids, not selectors: `fill-form` takes `uid=value` pairs.
    let uid_of = |label: &str| -> String {
        snapshot
            .lines()
            .find(|l| l.contains(label) && l.contains("textbox"))
            .and_then(|l| l.split_whitespace().find(|w| w.starts_with("uid=")))
            .map_or_else(
                || panic!("no textbox for {label}: {snapshot}"),
                |w| w.trim_start_matches("uid=").to_string(),
            )
    };
    let card = format!("{}=4242424242424242", uid_of("Card number"));
    let pw = format!("{}=hunter3secret", uid_of("Password"));
    let (out, _) = run_cli(&["--browser", b.name(), "--json", "fill-form", &card, &pw]);
    let v: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("not JSON ({e}): {out}"));
    for field in v["values"].as_array().expect("per-field reports") {
        assert_eq!(field["value"]["verbatim"], true, "{v}");
        assert_eq!(field["value"]["redacted"], true, "{v}");
    }
    assert_eq!(v["verdict"], "changed", "{v}");
    assert_eq!(v["verdict_reason"], "value_kept", "{v}");
    let printed = serde_json::to_string(&v).unwrap_or_default();
    assert!(!printed.contains("4242424242424242") && !printed.contains("hunter3secret"), "{v}");
}

/// A secret is redacted to `verbatim` and two lengths, which is enough to classify it.
#[test]
fn a_password_the_page_discards_is_classified_without_being_printed() {
    let b = TestBrowser::new("fill-verdict-secret");
    let Some(v) = fill_with_verdict(b.name(), "form_value_password.html", "#p", "topsecret123") else {
        return;
    };
    // This fixture keeps the value, so the postcondition holds.
    assert_eq!(v["value"]["redacted"], true, "{v}");
    assert_eq!(v["value"]["verbatim"], true, "{v}");
    assert_ne!(v["verdict"], "not_kept", "the page kept it: {v}");
    assert!(
        !serde_json::to_string(&v).unwrap_or_default().contains("topsecret123"),
        "and nothing about the verdict put it on stdout: {v}"
    );
}

/// `--verdict off` skips the page read, but the read-back happens inside the fill, so a
/// reverted value is still reported.
#[test]
fn a_reverted_value_is_reported_even_with_the_change_report_off() {
    let b = TestBrowser::new("fill-verdict-off");
    let Some((v, _)) = fill_on(b.name(), "form_value_microtask_revert.html", "#micro", "hello@example.com")
    else {
        return;
    };
    assert_eq!(v["verdict"], "not_kept", "{v}");
    assert_eq!(v["verdict_reason"], "value_reverted", "{v}");
    assert!(v["changed"].is_null(), "off still means no page read: {v}");
}

/// Pipe settles its verdict in `pipe_report`, so it is checked separately from the CLI.
#[test]
fn pipe_says_not_kept_the_way_the_cli_does() {
    if !common::browser_ready() {
        return;
    }
    let url = common::fixture_url("form_value_microtask_revert.html");
    let script = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": url}),
        serde_json::json!({"cmd": "inspect"}),
        serde_json::json!({"cmd": "fill", "selector": "#micro", "value": "hello@example.com"}),
    );
    let guard = TestBrowser::new("fill-verdict-pipe");
    let browser = guard.name().to_string();
    let mut child = Command::new(common::binary())
        .args(["--browser", &browser, "pipe"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn pipe");
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("pipe output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last: Value = stdout
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
        .expect("a fill response");
    assert_eq!(last["verdict"], "not_kept", "{last}");
    assert_eq!(last["verdict_reason"], "value_reverted", "{last}");
}

/// The `value_kept` rung through the pipe, which classifies in `pipe_report` not `run_helpers`.
#[test]
fn pipe_says_value_kept_the_way_the_cli_does() {
    if !common::browser_ready() {
        return;
    }
    let url = common::fixture_url("snapshot_secret_values.html");
    let script = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": url}),
        serde_json::json!({"cmd": "inspect"}),
        serde_json::json!({"cmd": "fill", "selector": "#card", "value": "4242424242424242"}),
    );
    let guard = TestBrowser::new("fill-verdict-pipe-kept");
    let browser = guard.name().to_string();
    let mut child = Command::new(common::binary())
        .args(["--browser", &browser, "pipe"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn pipe");
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("pipe output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last: Value = stdout
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
        .expect("a fill response");
    assert_eq!(last["value"]["verbatim"], true, "{last}");
    assert_eq!(last["verdict"], "changed", "{last}");
    assert_eq!(last["verdict_reason"], "value_kept", "{last}");
    assert!(!stdout.contains("4242424242424242"), "and nothing leaked on the way: {stdout}");
}

/// A control inside `<fieldset disabled>` is refused. `el.disabled` on the input reads false:
/// the IDL property reflects the element's own attribute, not the inherited state.
#[test]
fn filling_a_control_disabled_by_its_fieldset_is_refused() {
    let b = TestBrowser::new("fill-fieldset");
    let Some((v, code)) = fill_on(b.name(), "form_value_disabled_input.html", "#dis", "1234") else {
        return;
    };
    assert_ne!(code, 0, "a disabled control cannot be filled: {v}");
    assert_eq!(v["ok"], false, "{v}");
    assert!(
        v["error"].as_str().unwrap_or_default().contains("disabled"),
        "and the reason must be the disabled state, not some other failure: {v}"
    );
}

/// A readonly input is refused, with the reason named and no JS stack trace in the error.
#[test]
fn filling_a_readonly_input_is_refused() {
    let b = TestBrowser::new("fill-readonly");
    let Some((v, code)) = fill_on(b.name(), "form_value_readonly_input.html", "#ro", "1234") else {
        return;
    };
    assert_ne!(code, 0, "a readonly input cannot be filled: {v}");
    assert!(
        v["error"].as_str().unwrap_or_default().contains("readonly"),
        "and the reason must name it: {v}"
    );
    assert!(
        !v["error"].as_str().unwrap_or_default().contains("    at "),
        "a JS stack trace is noise in an agent's error field: {v}"
    );
}

/// A mask leaves the request neither refused nor honoured, so both strings are reported.
#[test]
fn a_mask_that_rewrites_the_value_reports_both_sides() {
    let b = TestBrowser::new("fill-mask");
    let Some((v, code)) = fill_on(b.name(), "form_value_phone_mask.html", "#phone", "5551234567") else {
        return;
    };
    assert_eq!(code, 0, "the fill did land, it was reformatted: {v}");
    assert_eq!(v["value"]["requested"], "5551234567", "{v}");
    let actual = v["value"]["actual"].as_str().unwrap_or_default();
    assert_ne!(actual, "5551234567", "the page rewrote it: {v}");
    assert!(actual.contains("555"), "and this is what it holds now: {v}");
    assert_eq!(v["value"]["verbatim"], false, "so the caller is told it is not verbatim: {v}");
}

#[test]
fn a_plain_input_reports_the_value_went_in_verbatim() {
    let b = TestBrowser::new("fill-plain");
    let Some((v, code)) = fill_on(b.name(), "form_value_plain_input.html", "input", "hello@example.com")
    else {
        return;
    };
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["value"]["verbatim"], true, "{v}");
    assert_eq!(v["value"]["actual"], "hello@example.com", "{v}");
}

/// `maxlength` constrains the editing pipeline, not the value setter, so the fill lands
/// verbatim and a caveat names the cap the form will reject on.
#[test]
fn filling_past_maxlength_lands_verbatim_but_says_so() {
    let b = TestBrowser::new("fill-maxlen");
    let Some((v, code)) = fill_on(b.name(), "form_value_maxlength_divergence.html", "#ml", "abcdefghijklmnop")
    else {
        return;
    };
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["value"]["verbatim"], true, "the setter is not bound by maxlength: {v}");
    let caveat = v["value"]["caveat"].as_str().unwrap_or_default();
    assert!(caveat.contains("maxlength=5"), "the cap that was bypassed must be named: {v}");
}

/// A password is never echoed back; `verbatim` and the lengths survive redaction.
#[test]
fn a_password_field_is_never_echoed_back() {
    let b = TestBrowser::new("fill-secret");
    let Some((v, code)) = fill_on(b.name(), "form_value_password.html", "#p", "topsecret123") else {
        return;
    };
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["value"]["redacted"], true, "{v}");
    assert_eq!(v["value"]["verbatim"], true, "the useful part survives: {v}");
    assert_eq!(v["value"]["requested_length"], 12, "{v}");
    assert!(
        !serde_json::to_string(&v).unwrap_or_default().contains("topsecret123"),
        "the secret must not appear anywhere in the response: {v}"
    );
}
