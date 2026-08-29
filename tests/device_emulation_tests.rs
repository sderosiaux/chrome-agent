use std::io::{BufRead, BufReader, Read as _, Write as _};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

mod common;

fn binary() -> String {
    let mut path = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    path.push("chrome-agent");
    path.to_string_lossy().into_owned()
}

fn run(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(binary())
        .args(args)
        .output()
        .expect("run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

fn run_with_stdin(args: &[&str], input: &str) -> (String, String, i32) {
    let mut child = Command::new(binary())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run chrome-agent with stdin");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

fn run_json(args: &[&str]) -> Value {
    let (stdout, stderr, code) = run(args);
    assert_eq!(
        code, 0,
        "command failed: {args:?}\nstdout: {stdout}\nstderr: {stderr}"
    );
    serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("invalid JSON for {args:?}: {error}\nstdout: {stdout}"))
}

struct TestBrowser(String);

static NEXT_BROWSER: AtomicUsize = AtomicUsize::new(1);

impl TestBrowser {
    fn new() -> Self {
        Self(format!(
            "device-emulation-{}-{}",
            std::process::id(),
            NEXT_BROWSER.fetch_add(1, Ordering::Relaxed),
        ))
    }

    fn name(&self) -> &str {
        &self.0
    }
}

impl Drop for TestBrowser {
    fn drop(&mut self) {
        let _ = run(&["--browser", &self.0, "close", "--purge"]);
    }
}

struct TempRecording(std::path::PathBuf);

impl TempRecording {
    fn new(name: &str, contents: &str) -> Self {
        let path = std::env::temp_dir().join(format!("chrome-agent-{name}.jsonl"));
        std::fs::write(&path, contents).unwrap();
        Self(path)
    }

    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for TempRecording {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn read_page_metrics(browser: &str, page: &str) -> Value {
    run_json(&[
        "--browser",
        browser,
        "--page",
        page,
        "--json",
        "eval",
        r"({screen:[screen.width,screen.height],dpr:devicePixelRatio,touch:navigator.maxTouchPoints,coarse:matchMedia('(pointer: coarse)').matches})",
    ])["result"]
        .clone()
}

#[test]
fn emulation_stays_on_its_page_reapplies_and_cleans_up() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new();
    let probe = common::fixture_url("emulation_probe.html");
    let other = common::fixture_url("extract_cards.html");

    let response = run_json(&[
        "--browser",
        browser.name(),
        "--page",
        "mobile",
        "--json",
        "goto",
        &probe,
    ]);
    assert_eq!(response["ok"], true);

    let applied = run_json(&[
        "--browser",
        browser.name(),
        "--page",
        "mobile",
        "--json",
        "emulate",
        "device",
        "--label",
        "checkout phone",
        "--width",
        "412",
        "--height",
        "915",
        "--dpr",
        "2.625",
        "--mobile",
        "--touch",
        "--orientation",
        "portrait",
    ]);
    assert_eq!(applied["emulation"]["label"], "checkout phone");
    assert_eq!(applied["emulation"]["dpr"], 2.625);
    assert!(applied["emulation"].get("deviceScaleFactor").is_none());
    assert_eq!(applied["emulation"]["orientation"], "portrait");
    assert_eq!(applied["effective"]["screen"]["width"], 412);
    assert_eq!(applied["effective"]["screen"]["height"], 915);
    assert_eq!(applied["effective"]["layoutViewport"]["width"], 412);
    assert_eq!(applied["effective"]["deviceScaleFactor"], 2.625);
    assert_eq!(applied["effective"]["touchPoints"], 1);
    assert_eq!(applied["effective"]["coarsePointer"], true);
    assert_eq!(applied["effective"]["orientation"], "portrait");

    // A fresh CLI invocation must reapply `--touch` without making CDP mouse dispatch hang.
    // chrome-agent synthesizes a tap, and Chromium follows it with compatibility mouse events.
    assert_eq!(
        run_json(&[
            "--browser",
            browser.name(),
            "--page",
            "mobile",
            "--json",
            "eval",
            "(() => { window.__inputEvents = []; const el = document.querySelector('#marker'); for (const name of ['touchstart', 'touchend', 'mousedown', 'mouseup', 'click']) el.addEventListener(name, () => window.__inputEvents.push(name)); return true; })()",
        ])["result"],
        true
    );
    assert_eq!(
        run_json(&[
            "--browser",
            browser.name(),
            "--page",
            "mobile",
            "--json",
            "click",
            "--selector",
            "#marker",
        ])["ok"],
        true
    );
    assert_eq!(
        run_json(&[
            "--browser",
            browser.name(),
            "--page",
            "mobile",
            "--json",
            "eval",
            "window.__inputEvents",
        ])["result"],
        serde_json::json!(["touchstart", "touchend", "mousedown", "mouseup", "click"])
    );

    let (apply_text, stderr, code) = run(&[
        "--browser",
        browser.name(),
        "--page",
        "mobile",
        "emulate",
        "device",
        "--width",
        "412",
        "--height",
        "915",
        "--dpr",
        "2.625",
        "--mobile",
        "--touch",
    ]);
    assert_eq!(code, 0, "text apply failed: {stderr}");
    assert!(apply_text.contains("Device emulation:"), "{apply_text}");
    assert!(
        apply_text.contains("Effective: viewport=412x915"),
        "{apply_text}"
    );

    let (status_text, stderr, code) = run(&[
        "--browser",
        browser.name(),
        "--page",
        "mobile",
        "emulate",
        "status",
    ]);
    assert_eq!(code, 0, "text status failed: {stderr}");
    assert!(status_text.contains("Device emulation:"), "{status_text}");
    assert!(
        status_text.contains(
            "Effective: viewport=412x915 screen=412x915 dpr=2.625 touch_points=1 coarse_pointer=true orientation=portrait"
        ),
        "{status_text}"
    );

    // Every CLI call opens a new CDP connection. The same observed values therefore prove that
    // the named page's persisted configuration was reapplied rather than retained in this process.
    let mobile = read_page_metrics(browser.name(), "mobile");
    assert_eq!(mobile["screen"], serde_json::json!([412, 915]));
    assert_eq!(mobile["dpr"], 2.625);
    assert_eq!(mobile["touch"], 1);
    assert_eq!(mobile["coarse"], true);

    // The override is target-scoped: a sibling created afterwards must retain Chrome's defaults.
    let desktop_goto = run_json(&[
        "--browser",
        browser.name(),
        "--page",
        "desktop",
        "--json",
        "goto",
        &probe,
    ]);
    assert_eq!(desktop_goto["ok"], true);
    let desktop_status = run_json(&[
        "--browser",
        browser.name(),
        "--page",
        "desktop",
        "--json",
        "emulate",
        "status",
    ]);
    assert!(desktop_status["emulation"].is_null());
    let desktop = read_page_metrics(browser.name(), "desktop");
    assert_eq!(desktop["dpr"], 1.0);
    assert_eq!(desktop["touch"], 0);
    assert_eq!(desktop["coarse"], false);

    // Creating a sibling makes it Chrome's active target. Reconnecting to the emulated page must
    // reactivate that target so Screen Orientation does not silently fall back to the sibling's.
    let mobile_after_sibling = run_json(&[
        "--browser",
        browser.name(),
        "--page",
        "mobile",
        "--json",
        "emulate",
        "status",
    ]);
    assert_eq!(mobile_after_sibling["effective"]["orientation"], "portrait");

    // Navigation replaces the document but not the target, so the named page keeps its metrics.
    let navigated = run_json(&[
        "--browser",
        browser.name(),
        "--page",
        "mobile",
        "--json",
        "goto",
        &other,
    ]);
    assert_eq!(navigated["ok"], true);
    let after_navigation = read_page_metrics(browser.name(), "mobile");
    assert_eq!(after_navigation["screen"], serde_json::json!([412, 915]));
    assert_eq!(after_navigation["dpr"], 2.625);
    assert_eq!(after_navigation["touch"], 1);

    let reset = run_json(&[
        "--browser",
        browser.name(),
        "--page",
        "mobile",
        "--json",
        "emulate",
        "reset",
    ]);
    assert!(reset["emulation"].is_null());
    let clean = read_page_metrics(browser.name(), "mobile");
    assert_eq!(clean["dpr"], 1.0);
    assert_eq!(clean["touch"], 0);
    assert_eq!(clean["coarse"], false);

    // Chromium rejects `maxTouchPoints: 0`; a non-touch configuration must omit that field while
    // clearing touch capability and coarse-pointer detection.
    let non_touch = run_json(&[
        "--browser",
        browser.name(),
        "--page",
        "mobile",
        "--json",
        "emulate",
        "device",
        "--width",
        "1024",
        "--height",
        "768",
    ]);
    assert_eq!(non_touch["ok"], true);
    assert_eq!(non_touch["emulation"]["touch"], false);
    assert_eq!(non_touch["effective"]["touchPoints"], 0);
    assert_eq!(non_touch["effective"]["coarsePointer"], false);
}

#[test]
fn pipe_and_batch_share_the_same_page_scoped_state() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new();
    let probe = common::fixture_url("emulation_probe.html");
    assert_eq!(
        run_json(&[
            "--browser",
            browser.name(),
            "--page",
            "mobile",
            "--json",
            "goto",
            &probe,
        ])["ok"],
        true
    );

    let commands = concat!(
        "{\"cmd\":\"emulate\",\"action\":\"device\",\"width\":390,\"height\":844,\"dpr\":\"bad\"}\n",
        "{\"cmd\":\"emulate\",\"action\":\"status\"}\n",
        "{\"cmd\":\"emulate\",\"action\":\"device\",\"label\":\"pipe phone\",",
        "\"width\":390,\"height\":844,\"dpr\":3,\"mobile\":true,\"touch\":true}\n",
        "{\"cmd\":\"emulate\",\"action\":\"status\"}\n",
    );
    let (stdout, stderr, code) = run_with_stdin(
        &["--browser", browser.name(), "--page", "mobile", "pipe"],
        commands,
    );
    assert_eq!(code, 0, "pipe failed: {stderr}");
    let responses: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 4, "pipe responses: {responses:?}");
    assert_eq!(responses[0]["ok"], false);
    assert!(responses[0]["error"].as_str().unwrap().contains("dpr"));
    assert!(responses[1]["emulation"].is_null());
    assert_eq!(responses[3]["emulation"]["label"], "pipe phone");
    assert_eq!(responses[3]["effective"]["screen"]["width"], 390);

    // The pipe persisted the configuration, so a separate CLI process can reconnect and observe it.
    let status = run_json(&[
        "--browser",
        browser.name(),
        "--page",
        "mobile",
        "--json",
        "emulate",
        "status",
    ]);
    assert_eq!(status["effective"]["deviceScaleFactor"], 3.0);
    assert_eq!(status["effective"]["touchPoints"], 1);

    let (stdout, stderr, code) = run_with_stdin(
        &[
            "--browser",
            browser.name(),
            "--page",
            "mobile",
            "--json",
            "batch",
        ],
        r#"[{"cmd":"emulate","action":"reset"},{"cmd":"emulate","action":"status"}]"#,
    );
    assert_eq!(code, 0, "batch failed: {stderr}");
    let batch: Value = serde_json::from_str(&stdout).unwrap();
    assert!(batch["results"][0]["emulation"].is_null());
    assert!(batch["results"][1]["emulation"].is_null());
}

#[test]
fn status_stays_page_scoped_while_pipe_eval_stays_in_its_frame() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new();
    let parent = common::fixture_url("frame_parent.html");
    assert_eq!(
        run_json(&[
            "--browser",
            browser.name(),
            "--page",
            "mobile",
            "--json",
            "goto",
            &parent,
        ])["ok"],
        true
    );

    let commands = concat!(
        "{\"cmd\":\"emulate\",\"action\":\"device\",\"width\":390,\"height\":844}\n",
        "{\"cmd\":\"frame\",\"target\":\"#the-frame\"}\n",
        "{\"cmd\":\"emulate\",\"action\":\"status\"}\n",
        "{\"cmd\":\"eval\",\"expression\":\"document.title\"}\n",
        "{\"cmd\":\"emulate\",\"action\":\"reset\"}\n",
    );
    let (stdout, stderr, code) = run_with_stdin(
        &["--browser", browser.name(), "--page", "mobile", "pipe"],
        commands,
    );
    assert_eq!(code, 0, "pipe failed: {stderr}");
    let responses: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 5, "pipe responses: {responses:?}");

    let status = &responses[2];
    assert_eq!(status["effective"]["layoutViewport"]["width"], 390);
    assert_eq!(status["effective"]["layoutViewport"]["height"], 844);
    assert_eq!(status["effective"]["orientation"], "portrait");
    assert_eq!(responses[3]["result"], "CHILD FRAME TITLE");
    assert_eq!(responses[4]["ok"], true);
}

#[test]
fn an_open_pipe_publishes_emulation_changes_immediately() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new();
    let probe = common::fixture_url("emulation_probe.html");
    assert_eq!(
        run_json(&[
            "--browser",
            browser.name(),
            "--page",
            "mobile",
            "--json",
            "goto",
            &probe,
        ])["ok"],
        true
    );

    let mut child = Command::new(binary())
        .args(["--browser", browser.name(), "--page", "mobile", "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start pipe");
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());

    writeln!(
        input,
        r#"{{"cmd":"emulate","action":"device","width":390,"height":844,"dpr":3,"mobile":true,"touch":true}}"#
    )
    .unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let applied: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(applied["ok"], true);

    // The pipe still owns stdin and has not reached its final save. Visibility here therefore
    // proves that a successful device command commits its session state immediately — a pipe
    // that only persisted on exit would leave a crashed session's emulation unrecorded.
    let concurrent_status = run_json(&[
        "--browser",
        browser.name(),
        "--page",
        "mobile",
        "--json",
        "emulate",
        "status",
    ]);
    assert_eq!(concurrent_status["emulation"]["width"], 390);
    assert_eq!(concurrent_status["effective"]["deviceScaleFactor"], 3.0);

    // Reset through the pipe itself (via batch, which shares the recovery state), then confirm
    // a fresh CLI process reads the cleared configuration. Concurrent WRITES from a second
    // process are deliberately not exercised: one writer per --browser is the store's contract.
    writeln!(
        input,
        r#"{{"cmd":"batch","commands":[{{"cmd":"emulate","action":"reset"}}]}}"#
    )
    .unwrap();
    input.flush().unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    let reset: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reset["ok"], true);
    assert_eq!(reset["results"][0]["ok"], true);

    let concurrent_reset = run_json(&[
        "--browser",
        browser.name(),
        "--page",
        "mobile",
        "--json",
        "emulate",
        "status",
    ]);
    assert!(concurrent_reset["emulation"].is_null());

    drop(input);
    let status = child.wait().unwrap();
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(status.success(), "pipe failed: {stderr}");
}

#[test]
fn replay_applies_reports_and_resets_page_emulation() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new();
    let probe = common::fixture_url("emulation_probe.html");
    assert_eq!(
        run_json(&[
            "--browser",
            browser.name(),
            "--page",
            "mobile",
            "--json",
            "goto",
            &probe,
        ])["ok"],
        true
    );

    let recording = TempRecording::new(
        browser.name(),
        concat!(
            "{\"cmd\":\"emulate\",\"action\":\"device\",\"width\":390,\"height\":844,\"dpr\":3,\"mobile\":true,\"touch\":true}\n",
            "{\"cmd\":\"emulate\",\"action\":\"status\"}\n",
            "{\"cmd\":\"emulate\",\"action\":\"reset\"}\n",
            "{\"cmd\":\"emulate\",\"action\":\"status\"}\n",
        ),
    );
    let (stdout, stderr, code) = run(&[
        "--browser",
        browser.name(),
        "--page",
        "mobile",
        "replay",
        recording.path(),
    ]);
    assert_eq!(code, 0, "replay failed: {stderr}");
    let responses: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 4, "replay responses: {responses:?}");
    assert_eq!(responses[0]["effective"]["deviceScaleFactor"], 3.0);
    assert_eq!(responses[1]["emulation"]["width"], 390);
    assert!(responses[2]["emulation"].is_null());
    assert!(responses[3]["emulation"].is_null());

    let status = run_json(&[
        "--browser",
        browser.name(),
        "--page",
        "mobile",
        "--json",
        "emulate",
        "status",
    ]);
    assert!(status["emulation"].is_null());
}

#[test]
fn browser_restart_discards_page_emulation() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new();
    let probe = common::fixture_url("emulation_probe.html");

    assert_eq!(
        run_json(&[
            "--browser",
            browser.name(),
            "--page",
            "mobile",
            "--json",
            "goto",
            &probe,
        ])["ok"],
        true
    );
    assert_eq!(
        run_json(&[
            "--browser",
            browser.name(),
            "--page",
            "mobile",
            "--json",
            "emulate",
            "device",
            "--width",
            "412",
            "--height",
            "915",
            "--dpr",
            "2.625",
            "--mobile",
            "--touch",
        ])["ok"],
        true
    );

    let (_, stderr, code) = run(&["--browser", browser.name(), "close"]);
    assert_eq!(code, 0, "close failed: {stderr}");

    assert_eq!(
        run_json(&[
            "--browser",
            browser.name(),
            "--page",
            "mobile",
            "--json",
            "goto",
            &probe,
        ])["ok"],
        true
    );
    let status = run_json(&[
        "--browser",
        browser.name(),
        "--page",
        "mobile",
        "--json",
        "emulate",
        "status",
    ]);
    assert!(
        status["emulation"].is_null(),
        "restart retained stale state: {status}"
    );
}
