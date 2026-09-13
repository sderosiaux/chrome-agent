//! Task-level regressions: preflight must precede effects; replay must preserve evidence.

use serde_json::{Value, json};

mod common;
#[path = "common/macros.rs"]
mod macros;
use common::TestBrowser;
use macros::{TestMacro, run_cli, run_pipe};

struct TempFile(std::path::PathBuf);
impl TempFile {
    fn new(label: &str, extension: &str) -> Self {
        Self(common::temp_path(label, extension))
    }
}
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn record(browser: &TestBrowser, file: &TestMacro, commands: Vec<Value>) -> Value {
    let recording = TempFile::new("task-recording", "jsonl");
    let commands: Vec<Value> = commands
        .into_iter()
        .map(|mut cmd| {
            cmd["_record"] = json!(recording.0);
            cmd
        })
        .collect();
    let responses = run_pipe(browser.name(), &commands);
    assert_eq!(responses.len(), commands.len(), "{responses:?}");
    assert!(responses.iter().all(|v| v["ok"] == true), "{responses:?}");
    let (stdout, stderr, code) = run_cli(&[
        "--json",
        "macro",
        "record",
        file.name(),
        "--from",
        "0",
        "--from-recording",
        &recording.0.to_string_lossy(),
    ]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    serde_json::from_str(&stdout).unwrap()
}

fn run(browser: &TestBrowser, file: &TestMacro, vars: &[&str]) -> (Value, i32) {
    let mut args = vec![
        "--json",
        "--browser",
        browser.name(),
        "macro",
        "run",
        file.name(),
    ];
    for var in vars {
        args.extend(["--var", var]);
    }
    let (stdout, stderr, code) = run_cli(&args);
    let value = serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{e}: {stdout}{stderr}"));
    (value, code)
}

#[test]
fn every_invalid_later_step_is_rejected_before_the_first_fill() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("macro-preflight");
    let file = TestMacro::new("macro-preflight");
    let initial = run_pipe(
        browser.name(),
        &[
            json!({"cmd":"goto", "url":common::fixture_url("form_value_plain_input.html")}),
            json!({"cmd":"fill", "selector":"#plain", "value":"untouched"}),
        ],
    );
    assert_eq!(initial[1]["value"]["verbatim"], true);
    for later in [
        json!({"do":{"cmd":"typo_command"}}),
        json!({"do":{"cmd":"fill", "selector":"#plain", "value":"{{missing}}"}}),
        json!({"do":{"cmd":"text"}, "expect":{"url_matches":"["}}),
        json!({"do":{"cmd":"assert", "what":"text", "matches":"["}}),
        json!({"do":{"cmd":"assert", "what":"text", "contains":"Saved", "within":0}}),
        json!({"do":{"cmd":"wait", "what":"text"}}),
        json!({"do":{"cmd":"batch", "commands":[{"cmd":"typo_command"}]}}),
    ] {
        file.write(json!({"steps":[{"do":{"cmd":"fill", "selector":"#plain", "value":"should-never-happen"}}, later]}));
        let (report, code) = run(&browser, &file, &[]);
        assert_eq!(code, 1, "{report}");
        assert!(
            report["error"].as_str().unwrap().contains("step 1"),
            "{report}"
        );
        let check = run_pipe(
            browser.name(),
            &[json!({"cmd":"assert", "what":"value", "selector":"#plain", "equals":"untouched"})],
        );
        assert_eq!(check[0]["ok"], true, "{check:?}");
    }
}

#[test]
fn check_is_offline_and_parameter_values_survive_the_cli_and_replay() {
    let file = TestMacro::new("macro-literal-input");
    let text = "ends-in-quote\"\n中文,é\\{{another}}";
    file.write(json!({"params":{"input":{"secret":true}}, "steps":[
        {"do":{"cmd":"goto", "url":common::fixture_url("macro_tasks.html")}},
        {"do":{"cmd":"fill", "selector":"#note", "value":"{{input}}"}, "expect":{"verbatim":true}},
        {"do":{"cmd":"assert", "what":"value", "selector":"#note", "equals":"{{input}}"}}
    ]}));
    let binding = format!("input={text}");
    let browser = TestBrowser::new("macro-literal-input");
    let (stdout, stderr, code) = run_cli(&[
        "--json",
        "--browser",
        browser.name(),
        "macro",
        "check",
        file.name(),
        "--var",
        &binding,
    ]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let checked: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(checked["browser_opened"], false);
    let sessions = file
        .path()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("sessions.json");
    assert!(
        !std::fs::read_to_string(sessions)
            .unwrap_or_default()
            .contains(browser.name())
    );
    if !common::browser_ready() {
        return;
    }
    let (report, code) = run(&browser, &file, &[&binding]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["steps"][2]["result"]["assertion"]["held"], true);
    assert!(
        !report.to_string().contains("中文"),
        "secret leaked: {report}"
    );
}

#[test]
fn a_failed_observation_never_overwrites_a_working_macro() {
    let file = TestMacro::new("macro-incomplete");
    file.write(json!({"steps":[{"do":{"cmd":"text"}}]}));
    let original = std::fs::read(file.path()).unwrap();
    for (command, response) in [
        (
            json!({"cmd":"fill", "selector":"#ctrl", "value":"requested"}),
            json!({"ok":true,"verdict":"not_kept","value":{"verbatim":false}}),
        ),
        (
            json!({"cmd":"assert","what":"exists","selector":"#done"}),
            json!({"ok":false,"assertion":{"held":false}}),
        ),
        (
            json!({"cmd":"download","selector":"#export"}),
            json!({"ok":true,"downloaded":false}),
        ),
        (
            json!({"cmd":"wait","selector":"#done"}),
            json!({"ok":false,"error":"timeout"}),
        ),
        (
            json!({"cmd":"click","xy":[20,30]}),
            json!({"ok":true,"delivery":"target_hit"}),
        ),
        (json!({"cmd":"scroll","target":"n17"}), json!({"ok":true})),
    ] {
        let recording = TempFile::new("incomplete-recording", "jsonl");
        std::fs::write(
            &recording.0,
            format!(
                "{}\n{}\n",
                json!({"cmd":{"cmd":"goto","url":"https://example.com"},"response":{"ok":true}}),
                json!({"cmd":command,"response":response})
            ),
        )
        .unwrap();
        let (stdout, stderr, code) = run_cli(&[
            "--json",
            "macro",
            "record",
            file.name(),
            "--from-recording",
            &recording.0.to_string_lossy(),
        ]);
        assert_eq!(code, 1, "{stdout}{stderr}");
        let report: Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(report["refused"][0]["index"], 1, "{report}");
        assert!(report["path"].is_null());
        assert_eq!(std::fs::read(file.path()).unwrap(), original);
    }
}

#[test]
fn cart_replay_waits_and_checks_identity_variant_quantity_and_preexisting_items() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("macro-cart");
    let file = TestMacro::new("macro-cart");
    let url = common::fixture_url("macro_tasks.html");
    let report = record(
        &browser,
        &file,
        vec![
            json!({"cmd":"goto", "url":url}),
            json!({"cmd":"assert", "what":"exists", "selector":"#cart article", "count":0}),
            json!({"cmd":"select", "selector":"#size", "value":"M"}),
            json!({"cmd":"click", "selector":"#add"}),
            json!({"cmd":"wait", "selector":"#cart article", "timeout":3}),
            json!({"cmd":"assert", "what":"exists", "selector":"#cart article[data-sku='linen-shirt'][data-size='M'][data-quantity='1']", "count":1}),
            json!({"cmd":"text", "selector":"#cart"}),
        ],
    );
    assert_eq!(report["steps"], 7);
    let mut body: Value = serde_json::from_slice(&std::fs::read(file.path()).unwrap()).unwrap();
    body["params"] = json!({"start_url":{}});
    body["steps"][0]["do"]["url"] = json!("{{start_url}}");
    file.write(body);
    let (result, code) = run(&browser, &file, &[&format!("start_url={url}")]);
    assert_eq!(code, 0, "{result}");
    assert!(
        result["steps"][6]["result"]
            .to_string()
            .contains("quantity 1"),
        "{result}"
    );

    let (existing, code) = run(
        &browser,
        &file,
        &[&format!("start_url={url}?case=existing#existing")],
    );
    assert_eq!(code, 2, "{existing}");
    assert_eq!(existing["stopped_at"], 1);
    assert_eq!(existing["guard"], "assertion");
    let state = run_pipe(
        browser.name(),
        &[
            json!({"cmd":"eval", "expression":"({calls:window.addCalls||0,quantity:document.querySelector('#cart article').dataset.quantity})"}),
        ],
    );
    assert_eq!(state[0]["result"], json!({"calls":0,"quantity":"1"}));

    let (wrong, code) = run(
        &browser,
        &file,
        &[&format!("start_url={url}?case=wrong#wrong")],
    );
    assert_eq!(code, 2, "{wrong}");
    assert_eq!(wrong["stopped_at"], 5);
    assert_eq!(wrong["result"]["assertion"]["actual"], 0);
    assert_eq!(wrong["steps"].as_array().unwrap().len(), 5);
}

#[test]
fn news_replay_returns_titles_links_dates_and_download_evidence() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("macro-news");
    let file = TestMacro::new("macro-news");
    let download = TempFile::new("macro-news-export", "csv");
    let read_news = r"(() => {
        const homepage = [...document.querySelectorAll('#news article:not([data-sponsored])')].map(el => ({
            title: el.querySelector('a').textContent, url: el.querySelector('a').href,
            published: el.querySelector('time').dateTime
        }));
        return {homepage, latest: [...homepage].sort((a,b) => b.published.localeCompare(a.published))};
    })()";
    let report = record(
        &browser,
        &file,
        vec![
            json!({"cmd":"goto", "url":common::fixture_url("macro_tasks.html")}),
            json!({"cmd":"wait", "selector":"#news article", "timeout":2}),
            json!({"cmd":"assert", "what":"exists", "selector":"#news article:not([data-sponsored])", "count":2}),
            json!({"cmd":"eval", "expression":read_news}),
            json!({"cmd":"download", "url":"data:text/csv,title%2Cdate%0ALatest%20report%2C2026-09-13", "out":download.0}),
        ],
    );
    assert_eq!(report["steps"], 5);
    std::fs::remove_file(&download.0).unwrap();
    let (result, code) = run(&browser, &file, &[]);
    assert_eq!(code, 0, "{result}");
    let news = &result["steps"][3]["result"]["result"];
    assert_eq!(news["homepage"][0]["title"], "Editor's pick");
    assert_eq!(
        news["latest"][0],
        json!({"title":"Latest report", "url":"https://example.com/latest", "published":"2026-09-13T10:00:00Z"})
    );
    assert_eq!(news["homepage"].as_array().unwrap().len(), 2);
    assert!(!news.to_string().contains("Sponsored"));
    assert_eq!(result["steps"][4]["result"]["downloaded"], true);
    assert_eq!(result["steps"][4]["result"]["path"], json!(download.0));
    assert_eq!(
        std::fs::read_to_string(&download.0).unwrap(),
        "title,date\nLatest report,2026-09-13"
    );
}

#[test]
fn download_without_a_file_stops_with_evidence_and_never_repeats_the_click() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("macro-download-no-file");
    let file = TestMacro::new("macro-download-no-file");
    file.write(json!({"steps":[
        {"do":{"cmd":"goto", "url":common::fixture_url("macro_tasks.html")}},
        {"do":{"cmd":"text", "selector":"#status"}},
        {"do":{"cmd":"download", "selector":"#add", "timeout":1}, "expect":{"downloaded":true}},
        {"do":{"cmd":"eval", "expression":"window.afterDownload=true"}}
    ]}));
    let (report, code) = run(&browser, &file, &[]);
    assert_eq!(code, 2, "{report}");
    assert_eq!(report["stopped_at"], 2);
    assert_eq!(report["guard"], "downloaded");
    assert_eq!(report["result"]["downloaded"], false);
    assert_eq!(report["result"]["delivery"], "target_hit");
    assert_ne!(report["result"]["dispatched"], false);
    assert_eq!(report["steps"][1]["result"]["text"], "Ready");
    let state = run_pipe(
        browser.name(),
        &[
            json!({"cmd":"eval", "expression":"({calls:window.addCalls,after:window.afterDownload||false})"}),
        ],
    );
    assert_eq!(state[0]["result"], json!({"calls":1, "after":false}));
}

#[test]
fn a_real_controlled_revert_cannot_be_recorded_as_success() {
    if !common::browser_ready() {
        return;
    }
    let browser = TestBrowser::new("macro-reverted-value");
    let file = TestMacro::new("macro-reverted-value");
    let recording = TempFile::new("macro-reverted-value", "jsonl");
    let responses = run_pipe(
        browser.name(),
        &[
            json!({"_record":recording.0, "cmd":"goto", "url":common::fixture_url("form_value_controlled_revert.html")}),
            json!({"_record":recording.0, "cmd":"fill", "selector":"#ctrl", "value":"requested-value"}),
            json!({"_record":recording.0, "cmd":"text", "selector":"#marker"}),
        ],
    );
    assert_eq!(responses[1]["ok"], true);
    assert_eq!(responses[1]["verdict"], "not_kept");
    assert_eq!(responses[1]["value"]["verbatim"], false);
    let (stdout, stderr, code) = run_cli(&[
        "--json",
        "macro",
        "record",
        file.name(),
        "--from-recording",
        &recording.0.to_string_lossy(),
    ]);
    assert_eq!(code, 1, "{stdout}{stderr}");
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["refused"][0]["index"], 1);
    assert!(!file.path().exists());
}
