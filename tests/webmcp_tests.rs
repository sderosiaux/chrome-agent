//! `webmcp list`/`webmcp call` end-to-end, against `webmcp_honest_liar_partial.html` — three
//! tools that return the SAME string byte-for-byte, only one of which moves the page.
//!
//! A declared result proves nothing, so every assertion reads the accessibility-tree delta the
//! shared `mutates_page` hook attaches, not the declared string.

use std::io::Write as _;
use std::process::{Command, Stdio};

use serde_json::Value;

mod common;
use common::TestBrowser;

/// Feed JSON command lines to `chrome-agent pipe`, one parsed `Value` per output line.
fn run_pipe(browser: &str, commands: &[Value]) -> Vec<Value> {
    let mut child = Command::new(common::binary())
        .args(["--browser", browser, "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn chrome-agent pipe");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for cmd in commands {
            writeln!(stdin, "{}", serde_json::to_string(cmd).unwrap()).unwrap();
        }
    }
    let output = child.wait_with_output().expect("wait pipe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).unwrap_or_else(|_| Value::String(l.to_string())))
        .collect()
}


// --- list ---

#[test]
fn list_reports_all_three_tools_with_no_output_schema() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-webmcp-list");
    let browser = guard.name();
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("webmcp_honest_liar_partial.html")}),
            serde_json::json!({"cmd": "webmcp_list"}),
        ],
    );

    assert_eq!(responses[1]["ok"], Value::Bool(true), "{:?}", responses[1]);
    assert_eq!(responses[1]["frame_scoped"], Value::Bool(false));
    let tools = responses[1]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 3, "{tools:?}");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"add_to_cart"));
    assert!(names.contains(&"add_to_cart_broken"));
    assert!(names.contains(&"add_to_cart_partial"));
    // The protocol has no return-value counterpart to inputSchema, so `output_schema` is null.
    for tool in tools {
        assert_eq!(tool["output_schema"], Value::Null, "{tool:?}");
        assert!(tool["input_schema"]["type"] == "object", "{tool:?}");
    }
}

#[test]
fn list_on_a_page_with_no_model_context_is_refused_with_a_chrome_arg_hint() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-webmcp-no-context");
    let browser = guard.name();
    // Any ordinary fixture with no WebMCP polyfill.
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("assert_page.html")}),
            serde_json::json!({"cmd": "webmcp_list"}),
        ],
    );

    assert_eq!(responses[1]["ok"], Value::Bool(false), "{:?}", responses[1]);
    let hint = responses[1]["hint"].as_str().unwrap_or("");
    assert!(hint.contains("--chrome-arg"), "{hint}");
    assert!(hint.contains("--enable-features=WebMCP"), "{hint}");
}

// --- call: the honest/liar/partial demonstration ---

/// The one string all three tools return, byte-identical. Pinned so a fixture change is caught.
const IDENTICAL_RETURN: &str = "{\"success\":true,\"item\":\"Espresso Blend\",\"price\":\"$18.00\"}";

#[test]
fn an_honest_tool_reports_the_tree_delta_that_backs_its_declared_success() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-webmcp-honest");
    let browser = guard.name();
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("webmcp_honest_liar_partial.html")}),
            serde_json::json!({"cmd": "inspect"}),
            serde_json::json!({"cmd": "webmcp_call", "name": "add_to_cart", "args": {"item": "Espresso Blend"}}),
        ],
    );

    let call = &responses[2];
    assert_eq!(call["ok"], Value::Bool(true), "{call:?}");
    assert_eq!(call["declared_result"], IDENTICAL_RETURN);
    assert_eq!(call["verdict"], "changed", "{call:?}");
    assert_eq!(call["verdict_reason"], "tree_delta", "{call:?}");
    assert_eq!(call["next"], "proceed");
    assert!(call["changed"]["added"].as_u64().unwrap_or(0) > 0, "{call:?}");
}

#[test]
fn a_liar_tool_reports_an_identical_tree_and_names_it_unproven_not_absent() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-webmcp-liar");
    let browser = guard.name();
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("webmcp_honest_liar_partial.html")}),
            serde_json::json!({"cmd": "inspect"}),
            serde_json::json!({"cmd": "webmcp_call", "name": "add_to_cart_broken", "args": {"item": "Espresso Blend"}}),
        ],
    );

    let call = &responses[2];
    assert_eq!(call["ok"], Value::Bool(true), "{call:?}");
    assert_eq!(call["declared_result"], IDENTICAL_RETURN);
    assert_eq!(call["verdict"], "unchanged", "{call:?}");
    assert_eq!(call["verdict_reason"], "identical_tree", "{call:?}");
    assert_eq!(call["changed"]["added"], 0);
    assert_eq!(call["changed"]["changed"], 0);
    // Never claim the action had no effect, only that the tree was quiet while watched.
    let hint = call["verdict_hint"].as_str().unwrap_or("");
    assert!(hint.contains("not the same as the action having no effect"), "{hint}");
}

#[test]
fn a_partial_tool_is_distinguished_from_the_liar_by_degree_of_change() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-webmcp-partial");
    let browser = guard.name();
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("webmcp_honest_liar_partial.html")}),
            serde_json::json!({"cmd": "inspect"}),
            serde_json::json!({"cmd": "webmcp_call", "name": "add_to_cart_partial", "args": {"item": "Espresso Blend"}}),
        ],
    );

    let call = &responses[2];
    assert_eq!(call["ok"], Value::Bool(true), "{call:?}");
    assert_eq!(call["declared_result"], IDENTICAL_RETURN);
    // It moved the heading text, unlike the liar, but added and removed nothing, unlike the
    // honest tool. Degree of change is the only thing telling the three apart.
    assert_eq!(call["verdict"], "changed", "{call:?}");
    assert_eq!(call["verdict_reason"], "tree_delta", "{call:?}");
    assert_eq!(call["changed"]["added"], 0, "{call:?}");
    assert_eq!(call["changed"]["removed"], 0, "{call:?}");
    assert!(call["changed"]["changed"].as_u64().unwrap_or(0) > 0, "{call:?}");
}

// --- the spec's own traps, caught before they reach the page ---

#[test]
fn an_unknown_tool_name_is_refused_with_the_known_names_and_a_hint() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-webmcp-unknown");
    let browser = guard.name();
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("webmcp_honest_liar_partial.html")}),
            serde_json::json!({"cmd": "webmcp_call", "name": "not_a_real_tool", "args": {}}),
        ],
    );

    let call = &responses[1];
    assert_eq!(call["ok"], Value::Bool(false), "{call:?}");
    let error = call["error"].as_str().unwrap_or("");
    assert!(error.contains("add_to_cart"), "known tools should be named: {error}");
    let hint = call["hint"].as_str().unwrap_or("");
    assert!(hint.contains("webmcp list"), "{hint}");
}

/// Run one CLI command in `--json` mode, for the CLI-mode tests below.
fn cli_run(browser: &str, args: &[&str]) -> Value {
    let mut full = vec!["--browser", browser];
    full.extend_from_slice(args);
    full.push("--json");
    let output = Command::new(common::binary()).args(&full).output().expect("run chrome-agent");
    serde_json::from_str(&String::from_utf8_lossy(&output.stdout))
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&output.stdout).into_owned()))
}

#[test]
fn cli_args_that_are_not_valid_json_text_are_refused_before_touching_the_page() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-webmcp-cli-bad-args");
    let browser = guard.name();
    let url = common::fixture_url("webmcp_honest_liar_partial.html");
    let _ = cli_run(browser, &["goto", &url]);
    let response = cli_run(browser, &["webmcp", "call", "add_to_cart", "--args", "not json at all"]);

    assert_eq!(response["ok"], Value::Bool(false), "{response:?}");
    let error = response["error"].as_str().unwrap_or("");
    assert!(error.contains("not valid JSON"), "{error}");
}

#[test]
fn cli_list_and_call_agree_with_pipe_mode() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-webmcp-cli");
    let browser = guard.name();
    let url = common::fixture_url("webmcp_honest_liar_partial.html");
    let _ = cli_run(browser, &["goto", &url]);
    let list = cli_run(browser, &["webmcp", "list"]);
    assert_eq!(list["ok"], Value::Bool(true), "{list:?}");
    assert_eq!(list["tools"].as_array().unwrap().len(), 3);

    let _ = cli_run(browser, &["inspect"]);
    let call = cli_run(
        browser,
        &["webmcp", "call", "add_to_cart_broken", "--args", "{\"item\":\"Espresso Blend\"}"],
    );
    assert_eq!(call["ok"], Value::Bool(true), "{call:?}");
    assert_eq!(call["declared_result"], IDENTICAL_RETURN);
    assert_eq!(call["verdict"], "unchanged", "{call:?}");
}

// --- frame scoping: measured, not assumed (see the commands::webmcp module doc) ---

#[test]
fn a_frame_scoped_list_reports_undefined_and_says_it_is_unproven() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-webmcp-frame");
    let browser = guard.name();
    let responses = run_pipe(
        browser,
        &[
            serde_json::json!({"cmd": "goto", "url": common::fixture_url("webmcp_iframe_host.html")}),
            serde_json::json!({"cmd": "frame", "target": "#cart-frame"}),
            serde_json::json!({"cmd": "webmcp_list"}),
        ],
    );

    assert_eq!(responses[1]["ok"], Value::Bool(true), "frame switch: {:?}", responses[1]);
    let call = &responses[2];
    assert_eq!(call["ok"], Value::Bool(false), "{call:?}");
    let hint = call["hint"].as_str().unwrap_or("");
    assert!(hint.contains("bound frame's isolated world"), "{hint}");
    assert!(hint.contains("NOT proof"), "{hint}");
    // The iframe really does register tools; the isolated world cannot see them.
}
