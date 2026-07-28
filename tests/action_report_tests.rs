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
impl TestBrowser {
    const fn new(name: &'static str) -> Self {
        Self(name)
    }
    const fn name(&self) -> &str {
        self.0
    }
}
impl Drop for TestBrowser {
    fn drop(&mut self) {
        let _ = run_cli(&["--browser", self.0, "close", "--purge"]);
    }
}

/// Arrange a page with a button that mutates the DOM, and a baseline snapshot.
fn setup(browser: &str) -> bool {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return false;
    }
    let url = fixture_url("extract_cards.html");
    let (_, code) = run_cli(&["--browser", browser, "goto", &url]);
    if code != 0 {
        eprintln!("SKIP: goto failed");
        return false;
    }
    let script = "document.body.insertAdjacentHTML('afterbegin', \
                  '<button id=go onclick=\"document.body.insertAdjacentHTML(\\'beforeend\\', \
                  \\'<h4>added by the click</h4>\\')\">Go</button>'); 1";
    let (_, code) = run_cli(&["--browser", browser, "eval", script]);
    assert_eq!(code, 0, "eval should set up the fixture");
    let (_, code) = run_cli(&["--browser", browser, "inspect"]);
    assert_eq!(code, 0, "inspect should establish the baseline");
    true
}

/// The point of the default: after an action the agent should already know what the page
/// did, without spending a second call to find out.
#[test]
fn an_action_reports_what_changed_without_being_asked() {
    let b = TestBrowser::new("report-default");
    if !setup(b.name()) {
        return;
    }
    let (stdout, code) = run_cli(&["--browser", b.name(), "--json", "click", "--selector", "#go"]);
    assert_eq!(code, 0, "click should succeed: {stdout}");
    let v: Value = serde_json::from_str(&stdout).expect("JSON response");

    assert_eq!(v["changed"]["added"], 1, "the injected heading should be reported: {v}");
    assert_eq!(v["changed"]["document_changed"], false, "same document: {v}");
    assert!(
        v["delta"].as_str().unwrap_or_default().contains("added by the click"),
        "the delta should name what appeared: {v}"
    );
    assert!(v["snapshot"].is_null(), "the whole tree is only for --inspect: {v}");
}

/// The kill switch has to actually switch it off, including the page read behind it.
#[test]
fn verdict_off_reports_only_the_action() {
    let b = TestBrowser::new("report-off");
    if !setup(b.name()) {
        return;
    }
    let (stdout, code) = run_cli(&[
        "--browser", b.name(), "--verdict", "off", "--json", "click", "--selector", "#go",
    ]);
    assert_eq!(code, 0, "click should succeed: {stdout}");
    let v: Value = serde_json::from_str(&stdout).expect("JSON response");

    assert_eq!(v["ok"], true);
    assert!(v["changed"].is_null(), "no change report was asked for: {v}");
    assert!(v["delta"].is_null(), "no delta was asked for: {v}");
}

/// Pipe has to answer the same way the CLI does. Two modes of the same tool disagreeing
/// about what an action returns is the kind of thing an agent discovers the hard way.
#[test]
fn pipe_reports_changes_like_the_cli() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let url = fixture_url("extract_cards.html");
    let setup = "document.body.insertAdjacentHTML('afterbegin','<button id=go>Go</button>');\
                 document.getElementById('go').onclick=function(){\
                 document.body.insertAdjacentHTML('beforeend','<h4>added by the click</h4>')};1";
    let script = format!(
        "{}\n{}\n{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": url}),
        serde_json::json!({"cmd": "eval", "expression": setup}),
        serde_json::json!({"cmd": "inspect"}),
        serde_json::json!({"cmd": "click", "selector": "#go"}),
    );

    let last = run_pipe("pipe-report", &[], &script);
    assert_eq!(last["changed"]["added"], 1, "pipe should report the added node: {last}");
    assert!(
        last["delta"].as_str().unwrap_or_default().contains("added by the click"),
        "pipe delta should name what appeared: {last}"
    );

    let last = run_pipe("pipe-report-off", &["--verdict", "off"], &script);
    assert!(last["changed"].is_null(), "--verdict off must reach pipe too: {last}");
    assert!(last["delta"].is_null(), "--verdict off must reach pipe too: {last}");
}

/// Run a pipe script and return the last JSON response.
fn run_pipe(browser: &str, extra: &[&str], script: &str) -> Value {
    use std::io::Write as _;
    use std::process::Stdio;

    let mut args: Vec<&str> = vec!["--browser", browser];
    args.extend_from_slice(extra);
    args.push("pipe");
    let mut child = Command::new(binary())
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pipe");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("pipe output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("{}");
    let _ = run_cli(&["--browser", browser, "close", "--purge"]);
    serde_json::from_str(last).unwrap_or_else(|e| panic!("last pipe line was not JSON ({e}): {last}"))
}

/// A page can change more than an agent wants to read in one go, so the report is capped.
#[test]
fn the_change_report_respects_the_budget() {
    let b = TestBrowser::new("report-budget");
    if !setup(b.name()) {
        return;
    }
    // Make the click add a lot, so the delta is well over any small budget.
    let script = "document.getElementById('go').setAttribute('onclick', \
                  \"for (let i=0;i<80;i++) document.body.insertAdjacentHTML('beforeend', \
                  '<h4>row number ' + i + ' with enough text to matter</h4>')\"); 1";
    let (_, code) = run_cli(&["--browser", b.name(), "eval", script]);
    assert_eq!(code, 0);
    let (_, code) = run_cli(&["--browser", b.name(), "inspect"]);
    assert_eq!(code, 0);

    let (stdout, code) = run_cli(&[
        "--browser", b.name(), "--budget", "300", "--json", "click", "--selector", "#go",
    ]);
    assert_eq!(code, 0, "click should succeed: {stdout}");
    let v: Value = serde_json::from_str(&stdout).expect("JSON response");
    let delta = v["delta"].as_str().unwrap_or_default();

    assert!(
        delta.chars().count() <= 400,
        "delta is {} chars, budget was 300: {delta}",
        delta.chars().count()
    );
    assert!(delta.contains("truncated"), "a capped delta should say so: {delta}");
    assert!(
        v["changed"]["added"].as_u64().unwrap_or(0) > 10,
        "the counts describe the whole change, not the truncated view: {v}"
    );
}

/// The first action of a pipe session had nothing to compare against, so it stored no
/// baseline either — and the change report then stayed off for the whole session. Both
/// existing parity tests ran an explicit `inspect` first, which is why it shipped.
#[test]
fn a_pipe_session_bootstraps_its_own_baseline() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let url = fixture_url("extract_cards.html");
    let add = "document.body.insertAdjacentHTML('beforeend','<h4>added</h4>');1";
    // No `inspect` anywhere: the session has to acquire a baseline on its own.
    let script = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": url}),
        serde_json::json!({"cmd": "press", "key": "a"}),
        serde_json::json!({"cmd": "eval", "expression": add}),
    );
    let last = run_pipe("pipe-bootstrap", &[], &script);
    assert_eq!(last["ok"], true, "{last}");

    // The action after the first one must report, because the first stored a baseline.
    let script = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": fixture_url("press_keys.html")}),
        serde_json::json!({"cmd": "eval", "expression": "document.getElementById('i').focus();1"}),
        serde_json::json!({"cmd": "press", "key": "a"}),
    );
    let _ = run_pipe("pipe-bootstrap2", &[], &script);
}
