use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn binary() -> String {
    let mut path = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
    path.push("chrome-agent");
    path.to_string_lossy().into_owned()
}

fn fixture_url(name: &str) -> String {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures");
    path.push(name);
    format!("file://{}", path.display())
}

fn run_cli(args: &[&str]) -> (String, i32) {
    let output = Command::new(binary()).args(args).output().expect("Failed to run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

fn chrome_available() -> bool {
    let candidates = if cfg!(target_os = "macos") {
        vec!["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"]
    } else {
        vec!["google-chrome", "chromium"]
    };
    for candidate in candidates {
        if std::path::Path::new(candidate).exists() {
            return true;
        }
        if Command::new("which").arg(candidate).output().is_ok_and(|o| o.status.success()) {
            return true;
        }
    }
    false
}

struct TestBrowser(&'static str);
impl Drop for TestBrowser {
    fn drop(&mut self) {
        let _ = run_cli(&["--browser", self.0, "close", "--purge"]);
    }
}

/// Load a fixture and fill `selector` with `value`. Returns the parsed response.
fn fill_on(browser: &str, fixture: &str, selector: &str, value: &str) -> Option<(Value, i32)> {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return None;
    }
    let (_, code) = run_cli(&["--browser", browser, "goto", &fixture_url(fixture)]);
    if code != 0 {
        eprintln!("SKIP: goto failed");
        return None;
    }
    let (out, code) = run_cli(&[
        "--browser", browser, "--verdict", "off", "--json", "fill", "--selector", selector, value,
    ]);
    Some((serde_json::from_str(&out).unwrap_or(Value::Null), code))
}

/// A control inside `<fieldset disabled>` is disabled, but `el.disabled` on the *input*
/// reads false: the IDL property reflects the element's own attribute, not the state it
/// inherits. So the value is set on a control the user could never have typed into, the
/// read-back agrees with the request, and every naive signal reports success.
#[test]
fn filling_a_control_disabled_by_its_fieldset_is_refused() {
    let b = TestBrowser("fill-fieldset");
    let Some((v, code)) = fill_on(b.0, "form_value_disabled_input.html", "#f", "1234") else {
        return;
    };
    assert_ne!(code, 0, "a disabled control cannot be filled: {v}");
    assert_eq!(v["ok"], false, "{v}");
}

/// A readonly input refuses the value too, and for a reason we can read before acting.
#[test]
fn filling_a_readonly_input_is_refused() {
    let b = TestBrowser("fill-readonly");
    let Some((v, code)) = fill_on(b.0, "form_value_readonly_input.html", "#f", "1234") else {
        return;
    };
    assert_ne!(code, 0, "a readonly input cannot be filled: {v}");
}

/// The one that matters most. A mask rewrites the value, and the request is neither
/// refused nor honoured. Reporting plain success hides it; reporting failure is wrong too.
/// The response has to carry what was asked for and what the page actually holds.
#[test]
fn a_mask_that_rewrites_the_value_reports_both_sides() {
    let b = TestBrowser("fill-mask");
    let Some((v, code)) = fill_on(b.0, "form_value_phone_mask.html", "#phone", "5551234567") else {
        return;
    };
    assert_eq!(code, 0, "the fill did land, it was reformatted: {v}");
    assert_eq!(v["value"]["requested"], "5551234567", "{v}");
    let actual = v["value"]["actual"].as_str().unwrap_or_default();
    assert_ne!(actual, "5551234567", "the page rewrote it: {v}");
    assert!(actual.contains("555"), "and this is what it holds now: {v}");
    assert_eq!(v["value"]["verbatim"], false, "so the caller is told it is not verbatim: {v}");
}

/// A plain input must stay simple: the value went in exactly as asked.
#[test]
fn a_plain_input_reports_the_value_went_in_verbatim() {
    let b = TestBrowser("fill-plain");
    let Some((v, code)) = fill_on(b.0, "form_value_plain_input.html", "input", "hello@example.com")
    else {
        return;
    };
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["value"]["verbatim"], true, "{v}");
    assert_eq!(v["value"]["actual"], "hello@example.com", "{v}");
}

/// `maxlength` constrains the editing pipeline, not the value setter, so a programmatic
/// fill walks straight past it. The value does land verbatim — and it is a value no person
/// could have typed, which the form will reject on submit. Saying only "filled" hides that.
#[test]
fn filling_past_maxlength_lands_verbatim_but_says_so() {
    let b = TestBrowser("fill-maxlen");
    let Some((v, code)) = fill_on(b.0, "form_value_maxlength_divergence.html", "#ml", "abcdefghijklmnop")
    else {
        return;
    };
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["value"]["verbatim"], true, "the setter is not bound by maxlength: {v}");
    let caveat = v["value"]["caveat"].as_str().unwrap_or_default();
    assert!(caveat.contains("maxlength=5"), "the cap that was bypassed must be named: {v}");
}
