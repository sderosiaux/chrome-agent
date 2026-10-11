use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

mod common;
use common::TestBrowser;
// An older installed binary may rewrite ~/.chrome-agent/sessions.json concurrently and
// strip fields it cannot deserialize. All browsers in this suite share ONE isolated store,
// so parallel-browser tests still exercise the real read/merge/write isolation.
fn command() -> Command {
    let mut command = Command::new(common::binary());
    command.env("CHROME_AGENT_SESSION_DIR", session_dir());
    command
}

fn session_dir() -> &'static std::path::Path {
    static DIRECTORY: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    DIRECTORY
        .get_or_init(|| common::temp_path("viewport-sessions", "dir"))
        .as_path()
}

fn test_browser(label: &str) -> TestBrowser {
    TestBrowser::in_session(label, session_dir())
}

fn run(browser: &TestBrowser, page: &str, args: &[&str]) -> Value {
    let output = command()
        .args(["--browser", browser.name(), "--page", page, "--json"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn assert_exact(value: &Value, width: u32, height: u32, dpr: f64) {
    assert_eq!(value["ok"], true, "{value}");
    assert_eq!(value["verdict"], "exact", "{value}");
    assert_eq!(value["effective"]["innerWidth"], width);
    assert_eq!(value["effective"]["innerHeight"], height);
    assert_eq!(value["effective"]["devicePixelRatio"], dpr);
    assert_eq!(value["effective"]["min1024"], width >= 1024);
}

fn assert_png(browser: &TestBrowser, width: u32, height: u32) {
    let capture = run(browser, "probe", &["screenshot"]);
    let path = capture["path"].as_str().unwrap();
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), width);
    assert_eq!(
        u32::from_be_bytes(bytes[20..24].try_into().unwrap()),
        height
    );
    // The acceptance check uses the OS tool as well as the PNG header.
    let file = Command::new("file").arg(path).output().unwrap();
    assert!(file.status.success());
    let description = String::from_utf8_lossy(&file.stdout);
    assert!(
        description.contains(&format!("{width} x {height}")),
        "{description}"
    );
    println!("{}", description.trim());
    std::fs::remove_file(path).unwrap();
}

fn matrix(headed: bool) {
    let browser = test_browser("viewport-matrix");
    let fixture = common::fixture_url("viewport_probe.html");
    let mut args = vec!["goto", &fixture];
    if headed {
        args.push("--headed");
    }
    let landed = run(&browser, "probe", &args);
    assert_eq!(
        landed["landed"]["viewport"],
        json!({"width":1440,"height":900,"dpr":1.0})
    );
    for (size, width, height, dpr, pixel_width, pixel_height) in [
        ("1440x900", 1440, 900, 1.0, 1440, 900),
        ("1024x768", 1024, 768, 1.0, 1024, 768),
        ("390x844@2", 390, 844, 2.0, 780, 1688),
    ] {
        let applied = run(
            &browser,
            "probe",
            &["--verdict", "off", "viewport", "set", size],
        );
        println!("viewport set {size} (headed={headed}): {applied}");
        assert_exact(&applied, width, height, dpr);
        // Every run is a separate process and CDP connection.
        assert_eq!(
            run(&browser, "probe", &["eval", "innerWidth"])["result"],
            width
        );
        assert_exact(
            &run(&browser, "probe", &["viewport", "get"]),
            width,
            height,
            dpr,
        );
        let navigated = run(&browser, "probe", &["goto", &fixture]);
        assert_eq!(
            navigated["landed"]["viewport"],
            json!({"width":width,"height":height,"dpr":dpr})
        );
        let metrics = run(&browser, "probe", &["eval", "measure()"]);
        assert_eq!(metrics["result"]["innerHeight"], height);
        assert_eq!(metrics["result"]["min1024"], width >= 1024);
        assert_png(&browser, pixel_width, pixel_height);
    }
    let status = run(&browser, "probe", &["status"]);
    assert_eq!(
        status["defaultViewport"],
        json!({"width":1440,"height":900,"dpr":1.0})
    );
    let entry = status["browsers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["name"] == browser.name())
        .unwrap();
    assert_eq!(
        entry["viewports"]["probe"],
        json!({"width":390,"height":844,"dpr":2.0})
    );
}

#[test]
fn headless_sizes_media_queries_png_and_persistence() {
    if common::browser_ready() {
        matrix(false);
    }
}

#[test]
#[ignore = "requires a graphical desktop; run explicitly with --ignored"]
fn headed_sizes_media_queries_png_and_persistence() {
    assert!(common::browser_ready());
    matrix(true);
}

#[test]
fn global_flag_pages_and_parallel_browsers_are_isolated() {
    if !common::browser_ready() {
        return;
    }
    std::thread::scope(|scope| {
        let handles: Vec<_> = [("1024x768", 1024), ("390x844@2", 390)]
            .into_iter()
            .map(|(size, width)| {
                scope.spawn(move || {
                    let browser = test_browser("viewport-isolation");
                    let fixture = common::fixture_url("viewport_probe.html");
                    let landed = run(&browser, "sized", &["goto", &fixture, "--viewport", size]);
                    assert_eq!(landed["landed"]["viewport"]["width"], width);
                    assert_eq!(
                        run(&browser, "other", &["eval", "innerWidth"])["result"],
                        1440
                    );
                    assert_eq!(
                        run(&browser, "sized", &["eval", "innerWidth"])["result"],
                        width
                    );
                    let later = run(&browser, "sized", &["goto", &fixture]);
                    assert_eq!(later["landed"]["viewport"]["width"], width);
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
    });
}

#[test]
fn pipe_and_nested_batch_share_the_viewport_and_persist_it() {
    if !common::browser_ready() {
        return;
    }
    let browser = test_browser("viewport-pipe");
    let mut child = command()
        .args([
            "--browser",
            browser.name(),
            "--page",
            "probe",
            "--viewport",
            "1024x768",
            "pipe",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(concat!(
        "{\"cmd\":\"viewport\",\"action\":\"get\"}\n",
        "{\"cmd\":\"batch\",\"commands\":[{\"cmd\":\"viewport\",\"action\":\"set\",\"size\":\"390x844@2\"},{\"cmd\":\"viewport\",\"action\":\"get\"}]}\n",
        "{\"cmd\":\"viewport\",\"action\":\"set\",\"size\":\"0x900\"}\n",
        "{\"cmd\":\"viewport\",\"action\":\"get\"}\n",
    ).as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let values: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_exact(&values[0], 1024, 768, 1.0);
    assert_eq!(values[1]["ok"], true, "{}", values[1]);
    assert_eq!(values[2]["ok"], false);
    assert_exact(&values[3], 390, 844, 2.0);
    assert_exact(&run(&browser, "probe", &["viewport", "get"]), 390, 844, 2.0);
}

#[test]
fn device_reset_restores_desktop_and_viewport_set_leaves_device_mode() {
    if !common::browser_ready() {
        return;
    }
    let browser = test_browser("viewport-device");
    run(&browser, "probe", &["viewport", "set", "1024x768"]);
    run(
        &browser,
        "probe",
        &[
            "emulate", "device", "--width", "390", "--height", "844", "--dpr", "2", "--mobile",
            "--touch",
        ],
    );
    run(&browser, "probe", &["emulate", "reset"]);
    assert_exact(
        &run(&browser, "probe", &["viewport", "get"]),
        1024,
        768,
        1.0,
    );
    run(
        &browser,
        "probe",
        &[
            "emulate", "device", "--width", "390", "--height", "844", "--mobile", "--touch",
        ],
    );
    assert_exact(
        &run(&browser, "probe", &["viewport", "set", "1440x900"]),
        1440,
        900,
        1.0,
    );
    assert_eq!(
        run(&browser, "probe", &["eval", "navigator.maxTouchPoints"])["result"],
        0
    );
    assert!(run(&browser, "probe", &["emulate", "status"])["emulation"].is_null());
}

#[test]
fn invalid_cli_sizes_fail_before_launch_and_help_explains_default() {
    for args in [
        vec!["--viewport", "0x900", "eval", "innerWidth"],
        vec!["viewport", "set", "390x844@NaN"],
    ] {
        let output = command().args(args).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("invalid value"));
    }
    let output = command().arg("--help").output().unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("1440x900@1"));
}

#[test]
fn viewport_reads_the_top_document_and_reports_a_mismatch_without_success() {
    if !common::browser_ready() {
        return;
    }
    let browser = test_browser("viewport-observation");
    let mut child = command()
        .args(["--browser", browser.name(), "--page", "probe", "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let commands = [
        json!({"cmd":"eval","expression":"document.body.innerHTML='<iframe id=child style=width:100px;height:100px srcdoc=hello></iframe>'"}),
        json!({"cmd":"frame","target":"#child"}),
        json!({"cmd":"viewport","action":"set","size":"1024x768"}),
        json!({"cmd":"viewport","action":"get"}),
        json!({"cmd":"frame","target":"main"}),
        // Deliberately contradict the browser metrics to exercise the measured failure path.
        json!({"cmd":"eval","expression":"Object.defineProperty(window,'innerWidth',{value:123,configurable:true}); true"}),
        json!({"cmd":"viewport","action":"set","size":"390x844@2"}),
    ];
    let input = commands
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let values: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(values[1]["ok"], true, "{}", values[1]);
    assert_exact(&values[2], 1024, 768, 1.0);
    assert_exact(&values[3], 1024, 768, 1.0);
    assert_eq!(values[6]["verdict"], "mismatch", "{}", values[6]);
    assert_eq!(values[6]["ok"], false);
    assert_eq!(values[6]["effective"]["innerWidth"], 123);
    assert_eq!(values[6]["delta"]["width"], -267.0);

    let output = command()
        .args([
            "--browser",
            browser.name(),
            "--page",
            "probe",
            "--json",
            "viewport",
            "set",
            "1024x768",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["verdict"], "mismatch");
    assert_eq!(response["effective"]["innerWidth"], 123);

    let output = command()
        .args([
            "--browser",
            browser.name(),
            "--page",
            "probe",
            "--json",
            "--viewport",
            "1024x768",
            "eval",
            "window.shouldNotRun = true",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        run(&browser, "probe", &["eval", "typeof window.shouldNotRun"])["result"],
        "undefined"
    );

    run(
        &browser,
        "probe",
        &[
            "eval",
            "Object.defineProperty(window,'innerWidth',{get(){throw new Error('probe failure')},configurable:true}); true",
        ],
    );
    let output = command()
        .args([
            "--browser",
            browser.name(),
            "--page",
            "probe",
            "--json",
            "viewport",
            "get",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["verdict"], "unknown");
    assert_eq!(response["ok"], false);
    assert!(response["effective"].is_null());
}
