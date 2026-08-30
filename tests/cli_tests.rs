use std::process::Command;

mod common;
use common::TestBrowser;

/// Run chrome-agent with args and return (stdout, stderr, `exit_code`).
fn run_cli(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(common::binary())
        .args(args)
        .output()
        .expect("Failed to run chrome-agent");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);

    (stdout, stderr, code)
}

/// Every verb `--help` lists, minus clap's own `help`. Derived, so a later verb is covered.
fn subcommands() -> Vec<String> {
    let (stdout, _, code) = run_cli(&["--help"]);
    assert_eq!(code, 0);
    let listing = stdout
        .split_once("\nCommands:\n")
        .expect("--help lists its commands")
        .1
        .split_once("\n\nOptions:")
        .expect("the command listing ends where the options begin")
        .0;
    let verbs: Vec<String> = listing
        .lines()
        // A wrapped description line is indented deeper than the verb it belongs to.
        .filter_map(|line| line.strip_prefix("  "))
        .filter(|line| !line.starts_with(' '))
        .filter_map(|line| line.split_whitespace().next())
        .filter(|verb| *verb != "help")
        .map(str::to_string)
        .collect();
    assert!(
        verbs.len() >= 40,
        "only {} verb(s) parsed out of --help — did the listing's shape change? {verbs:?}",
        verbs.len()
    );
    verbs
}

#[test]
fn help_shows_all_subcommands() {
    let verbs = subcommands();
    for verb in &verbs {
        let (_, stderr, code) = run_cli(&[verb, "--help"]);
        assert_eq!(
            code, 0,
            "--help lists `{verb}`, which the parser does not accept: {stderr}"
        );
    }
}

/// The guide, README and SKILL.md each show every verb in invocation form. A plain word match
/// does not count: `stop`, `status`, `type` and `macro` are also ordinary English.
#[test]
fn every_verb_appears_as_an_invocation_in_the_documents_an_agent_reads() {
    let verbs = subcommands();
    let docs = [
        ("llm-guide.txt", include_str!("../llm-guide.txt")),
        ("README.md", include_str!("../README.md")),
        (
            "skills/chrome-agent/SKILL.md",
            include_str!("../skills/chrome-agent/SKILL.md"),
        ),
    ];
    let mut missing = Vec::new();
    for (name, doc) in docs {
        for verb in &verbs {
            if exempt(verb, name).is_some() {
                continue;
            }
            if !invoked(doc, verb, &verbs) && !heads_a_command_row(doc, verb) {
                missing.push(format!("{name} never shows `chrome-agent {verb}`"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "{} verb(s) exist but are not documented in invocation form:\n{}\n\
         Document them, or add the pair to UNDOCUMENTED_ON_PURPOSE with its reason.",
        missing.len(),
        missing.join("\n")
    );
}

/// A verb no document shows in invocation form, with its reason inline. `"*"` means every one.
const UNDOCUMENTED_ON_PURPOSE: &[(&str, &str, &str)] = &[
    (
        "daemon",
        "*",
        "the optional micro-daemon: its one subcommand, `daemon start`, is marked \
         \"used internally\" in its own help because chrome-agent spawns it. An agent \
         that types it has already gone wrong.",
    ),
    (
        "stop",
        "*",
        "stops that daemon, so it is reachable only by someone who started one by hand. \
         `close` is the verb for a browser, and it is documented everywhere.",
    ),
    (
        "frame",
        "skills/chrome-agent/SKILL.md",
        "SKILL.md documents `frame` as the pipe command it has to be, and says why in the \
         same block: separate CLI calls do not work, because each is a fresh connection and \
         the binding lives on the connection. A CLI invocation form here would show something \
         that cannot do what it looks like it does.",
    ),
];

fn exempt(verb: &str, doc: &str) -> Option<&'static str> {
    UNDOCUMENTED_ON_PURPOSE
        .iter()
        .find(|(name, scope, _)| *name == verb && (*scope == "*" || *scope == doc))
        .map(|(_, _, reason)| *reason)
}

/// The verb is the first KNOWN verb after the binary name, not the first word: global flags
/// may precede it.
fn invoked(doc: &str, verb: &str, verbs: &[String]) -> bool {
    doc.match_indices("chrome-agent ").any(|(at, _)| {
        let rest = &doc[at..];
        let line = rest.split('\n').next().unwrap_or(rest);
        line["chrome-agent ".len()..]
            .split(|c: char| c.is_whitespace() || c == '|' || c == '`')
            .find(|word| verbs.iter().any(|known| known == word))
            .is_some_and(|word| word == verb)
    })
}

/// A table row whose FIRST cell is a backticked command starting with the verb. First cell
/// only: the verdict table's third column holds `stop` as a `next` token.
fn heads_a_command_row(doc: &str, verb: &str) -> bool {
    doc.lines()
        .map(str::trim)
        .filter(|line| line.starts_with('|'))
        .filter_map(|line| line.trim_start_matches('|').split('|').next())
        .any(|cell| {
            cell.trim()
                .strip_prefix('`')
                .and_then(|rest| rest.strip_prefix(verb))
                .is_some_and(|tail| {
                    tail.starts_with('`') || tail.starts_with(' ') || tail.starts_with('\\')
                })
        })
}

#[test]
fn help_includes_llm_guide() {
    let (stdout, _, code) = run_cli(&["--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("LLM USAGE GUIDE"));
    assert!(stdout.contains("inspect -> read uids -> act"));
    assert!(stdout.contains("--inspect"));
}

#[test]
fn help_shows_global_flags() {
    let (stdout, _, code) = run_cli(&["--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("--browser"));
    assert!(stdout.contains("--connect"));
    assert!(stdout.contains("--proxy-server"));
    assert!(stdout.contains("--headed"));
    assert!(stdout.contains("--timeout"));
    assert!(stdout.contains("--ignore-https-errors"));
    assert!(stdout.contains("--page"));
}

/// A global flag parses on either side of the subcommand. `CHROME_AGENT_PARSE_ONLY` returns
/// as soon as clap has spoken.
#[test]
fn a_global_flag_is_accepted_on_either_side_of_the_verb() {
    let parses = |args: &[&str]| {
        let output = Command::new(common::binary())
            .args(args)
            .env("CHROME_AGENT_PARSE_ONLY", "1")
            .output()
            .expect("run chrome-agent");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .next()
                .unwrap_or("")
                .to_string(),
        )
    };
    let cases: &[(&[&str], &[&str])] = &[
        (
            &["--json", "fill", "--selector", "#micro", "x"],
            &["fill", "--selector", "#micro", "x", "--json"],
        ),
        (&["--json", "click", "n1"], &["click", "n1", "--json"]),
        (&["--json", "inspect"], &["inspect", "--json"]),
        (
            &["--verdict", "off", "click", "n1"],
            &["click", "n1", "--verdict", "off"],
        ),
        (
            // isolation-exempt: PARSE_ONLY, no browser launched; the name reaches nothing.
            &["--browser", "a7", "eval", "1"],
            // isolation-exempt: same, and the marker is repeated because rustfmt decides how
            // far these two lines sit from a comment above the pair.
            &["eval", "1", "--browser", "a7"],
        ),
    ];
    for (before, after) in cases {
        let (ok, err) = parses(before);
        assert!(
            ok,
            "the documented order stopped working: {before:?} -> {err}"
        );
        let (ok, err) = parses(after);
        assert!(
            ok,
            "a global flag after the verb must parse: {after:?} -> {err}"
        );
    }
}

/// `--timeout` and `--max-depth` cannot be global, because subcommands redeclare them. Both
/// positions still parse, and `run.rs` resolves them with `local.or(global)`.
#[test]
fn the_two_locally_redeclared_flags_still_work_in_both_positions() {
    for args in [
        vec!["wait", "selector", ".x", "--timeout", "5"],
        vec!["--timeout", "5", "click", "n1"],
        vec!["click", "n1", "--max-depth", "2"],
        vec!["--max-depth", "2", "click", "n1"],
    ] {
        let output = Command::new(common::binary())
            .args(&args)
            .env("CHROME_AGENT_PARSE_ONLY", "1")
            .output()
            .expect("run chrome-agent");
        assert!(
            output.status.success(),
            "{args:?} -> {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// A flag that must precede the verb says so and hands back a working invocation.
#[test]
fn a_flag_that_must_precede_the_verb_says_so_instead_of_offering_to_escape_it() {
    for (args, flag, moved) in [
        (
            vec!["click", "n1", "--timeout", "5"],
            "--timeout",
            "chrome-agent --timeout 5 click n1",
        ),
        (
            vec!["text", "--max-depth", "2"],
            "--max-depth",
            "chrome-agent --max-depth 2 text",
        ),
    ] {
        let (_, stderr, code) = run_cli(&args);
        assert_eq!(code, 1, "{args:?} should still be a usage error: {stderr}");
        assert!(
            stderr.contains(&format!("hint: {flag} is read before the verb")),
            "{args:?} did not state the rule: {stderr}"
        );
        assert!(
            stderr.contains(&format!("`{moved}`")),
            "{args:?} did not name the working invocation: {stderr}"
        );
        assert!(
            !stderr.contains(&format!("-- {flag}")),
            "clap's escape-it tip is back for {args:?}: {stderr}"
        );
        assert!(
            !stderr.contains("as a value"),
            "clap's escape-it tip is back for {args:?}: {stderr}"
        );

        // The invocation the hint hands back must itself parse.
        let suggested: Vec<&str> = moved.split_whitespace().skip(1).collect();
        let output = Command::new(common::binary())
            .args(&suggested)
            .env("CHROME_AGENT_PARSE_ONLY", "1")
            .output()
            .expect("run chrome-agent");
        assert!(
            output.status.success(),
            "the hint for {args:?} names an invocation that does not parse: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// An unrelated usage error keeps clap's own wording, tip included.
#[test]
fn an_unrelated_usage_error_is_left_to_clap() {
    let (_, stderr, code) = run_cli(&["click", "n1", "--nonsense"]);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("unexpected argument '--nonsense'"),
        "{stderr}"
    );
    assert!(
        stderr.contains("-- --nonsense"),
        "clap's tip should survive here: {stderr}"
    );
    assert!(!stderr.contains("read before the verb"), "{stderr}");
}

/// A bad invocation is refused by the parser, naming the argument rather than the machine.
/// Run under an empty `HOME`: with a session present these pass whatever the message says.
#[test]
fn an_invalid_invocation_is_refused_before_a_browser_is_resolved() {
    let home = common::temp_path("argcheck", "home");
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();

    // Each pair is an invocation and a word its refusal must contain.
    for (args, names) in [
        (&["download"][..], "--selector"),
        (
            &["download", "https://example.com/f.csv", "--uid", "n1"][..],
            "cannot be used with",
        ),
        (&["screenshot", "--format", "webp"][..], "--format"),
        (&["--dialog", "nope", "inspect"][..], "--dialog"),
        (
            &["goto", "https://example.com", "--header", "nocolon"][..],
            "--header",
        ),
        // Two ways to name one target, on every verb that takes more than one. These used to
        // live in `run::run`'s second match, i.e. after the browser was resolved, so
        // `click --selector a --xy 1,2` launched a Chrome before saying no.
        (
            &["click", "n1", "--selector", "#a"][..],
            "cannot be used with",
        ),
        (
            &["click", "--selector", "#a", "--xy", "1,2"][..],
            "cannot be used with",
        ),
        (
            &["dblclick", "n1", "--xy", "1,2"][..],
            "cannot be used with",
        ),
        (
            &["fill", "x", "--uid", "n1", "--selector", "#a"][..],
            "cannot be used with",
        ),
        (
            &["select", "x", "--uid", "n1", "--selector", "#a"][..],
            "cannot be used with",
        ),
        (
            &["check", "n1", "--selector", "#a"][..],
            "cannot be used with",
        ),
        (
            &["uncheck", "n1", "--selector", "#a"][..],
            "cannot be used with",
        ),
        (
            &["upload", "f.txt", "--uid", "n1", "--selector", "#a"][..],
            "cannot be used with",
        ),
        (
            &["text", "n1", "--selector", "#a"][..],
            "cannot be used with",
        ),
        (
            &["screenshot", "--uid", "n1", "--selector", "#a"][..],
            "cannot be used with",
        ),
        // And no way at all, on the seven where a target is mandatory.
        (&["click"][..], "required"),
        (&["dblclick"][..], "required"),
        (&["fill", "x"][..], "required"),
        (&["select", "x"][..], "required"),
        (&["check"][..], "required"),
        (&["uncheck"][..], "required"),
        (&["upload", "f.txt"][..], "required"),
        // `--xy` is a pair, judged by its own value parser: the two numbers arrive as one
        // comma-separated token, which `num_args` cannot count.
        (&["click", "--xy", "1,2,3"][..], "exactly 2 values"),
        (&["click", "--xy", "1"][..], "exactly 2 values"),
    ] {
        let output = Command::new(common::binary())
            .args(args)
            .env("HOME", &home)
            .output()
            .expect("run chrome-agent");
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        assert_eq!(output.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(
            stderr.contains(names),
            "{args:?} never names {names}: {stderr}"
        );
        assert!(
            stdout.trim().is_empty(),
            "{args:?} put a usage error on stdout: {stdout}"
        );
        assert!(
            !stderr.contains("browser session"),
            "{args:?} answered about a browser, not about its arguments: {stderr}"
        );
    }

    // `text` and `screenshot` take no target at all — the group is exclusive, not required.
    for args in [&["text"][..], &["screenshot"][..]] {
        let output = Command::new(common::binary())
            .args(args)
            .env("HOME", &home)
            .env("CHROME_AGENT_PARSE_ONLY", "1")
            .output()
            .expect("run chrome-agent");
        assert_eq!(
            output.status.code(),
            Some(0),
            "a whole-page {args:?} is not a missing target: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // A profile directory here would mean a refusal got as far as resolving a connection.
    assert!(
        !home.join(".chrome-agent").join("browsers").exists(),
        "a refused invocation still launched a browser"
    );
    std::fs::remove_dir_all(&home).ok();
}

/// Clap's value lists are case-insensitive, so the spellings `DialogPolicy::parse` and
/// `ImgFormat::parse` accept still parse.
#[test]
fn the_spellings_those_parsers_accept_still_parse() {
    for args in [
        &["--dialog", "DISMISS", "inspect"][..],
        &["--dialog", "Accept", "inspect"][..],
        &["screenshot", "--format", "JPG"][..],
        &["screenshot", "--format", "PNG"][..],
        &["goto", "https://example.com", "--header", "X-Trace: a:b"][..],
    ] {
        let output = Command::new(common::binary())
            .args(args)
            .env("CHROME_AGENT_PARSE_ONLY", "1")
            .output()
            .expect("run chrome-agent");
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn version_flag() {
    let (stdout, _, code) = run_cli(&["--version"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("chrome-agent"));
}

#[test]
fn status_works_without_browser() {
    let (stdout, _, code) = run_cli(&["status"]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("No active browser sessions") || stdout.contains("browser="),
        "Unexpected status output: {stdout}"
    );
}

#[test]
fn stop_when_no_daemon() {
    let (stdout, _, code) = run_cli(&["stop"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("not running") || stdout.contains("stopped"));
}

#[test]
fn goto_subcommand_help() {
    let (stdout, _, code) = run_cli(&["goto", "--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Navigate to a URL"));
    assert!(stdout.contains("--inspect"));
}

#[test]
fn click_subcommand_help() {
    let (stdout, _, code) = run_cli(&["click", "--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Click an element"));
    // The one-liner has to state that the response names who received the event.
    assert!(stdout.contains("who received the event"), "{stdout}");
    assert!(stdout.contains("--inspect"));
}

#[test]
fn fill_subcommand_help() {
    let (stdout, _, code) = run_cli(&["fill", "--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Fill an input"));
    assert!(stdout.contains("--inspect"));
}

#[test]
fn inspect_subcommand_help() {
    let (stdout, _, code) = run_cli(&["inspect", "--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("accessibility tree inspection"));
    assert!(stdout.contains("--verbose"));
}

#[test]
fn eval_subcommand_help() {
    let (stdout, _, code) = run_cli(&["eval", "--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Evaluate JavaScript"));
}

// The tests below need Chrome; each is guarded by `common::browser_ready`.

/// Two CLI invocations reach the same named browser. A local fixture, so a failure here is
/// this tool's and not the network's.
#[test]
fn goto_then_eval_share_one_named_browser() {
    if !common::browser_ready() {
        return;
    }

    let b = TestBrowser::new("test-integration");
    let url = format!(
        "file://{}",
        common::fixture_path("assert_page.html").display()
    );

    let (stdout, stderr, code) = run_cli(&["--browser", b.name(), "goto", &url]);
    assert_eq!(code, 0, "goto {url} failed: {stderr}");
    assert!(stdout.contains("Assertable page"), "goto output: {stdout}");

    // Same browser name, a second process and connection: the page is still there.
    let (stdout, stderr, code) = run_cli(&["--browser", b.name(), "eval", "document.title"]);
    assert_eq!(code, 0, "eval failed: {stderr}");
    assert!(stdout.contains("Assertable page"), "eval output: {stdout}");
}

#[test]
fn dblclick_selector_fires_real_double_click() {
    if !common::browser_ready() {
        return;
    }

    let b = TestBrowser::new("test-dblclick-selector");

    // The button counts `click` and `dblclick` separately. Loaded over file:// rather than
    // data: to avoid URL encoding.
    let html = "<!doctype html><html><body><button id=\"b\" \
        onclick=\"window.__c=(window.__c||0)+1\" \
        ondblclick=\"window.__d=(window.__d||0)+1\">x</button></body></html>";
    let path = common::temp_path("dblclick-selector-test", "html");
    std::fs::write(&path, html).expect("write fixture");
    let url = format!("file://{}", path.display());

    let (_, stderr, code) = run_cli(&["--browser", b.name(), "goto", &url]);
    if code != 0 {
        let _ = std::fs::remove_file(&path);
        common::unavailable(&format!("goto dblclick fixture failed: {stderr}"));
        return;
    }

    let (_, _, code) = run_cli(&["--browser", b.name(), "dblclick", "--selector", "#b"]);
    assert_eq!(code, 0, "dblclick --selector should succeed");

    // A selector double-click must fire `dblclick`, not just a single `click`.
    let (stdout, _, code) = run_cli(&["--browser", b.name(), "eval", "String(window.__d||0)"]);
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 0, "eval should succeed");
    assert!(
        stdout.contains('1'),
        "dblclick event must have fired once (window.__d), got: {stdout}"
    );
}

#[test]
fn headed_inspect_returns_uids() {
    if !common::browser_ready() {
        return;
    }

    let b = TestBrowser::new("test-inspect");

    let (_, _, code) = run_cli(&["--browser", b.name(), "goto", "https://example.com"]);

    if code != 0 {
        common::unavailable("goto https://example.com failed");
        return;
    }

    let (stdout, _, code) = run_cli(&["--browser", b.name(), "inspect"]);

    if code == 0 {
        assert!(
            stdout.contains("uid="),
            "inspect should contain uid=N: {stdout}"
        );
    }
}

#[test]
fn headed_screenshot_returns_path() {
    if !common::browser_ready() {
        return;
    }

    let b = TestBrowser::new("test-screenshot");

    let (_, _, code) = run_cli(&["--browser", b.name(), "goto", "https://example.com"]);

    if code != 0 {
        common::unavailable("goto https://example.com failed");
        return;
    }

    let (stdout, _, code) = run_cli(&["--browser", b.name(), "screenshot"]);

    if code == 0 {
        assert!(
            stdout.contains(".png") && stdout.contains(".chrome-agent/tmp/"),
            "screenshot should return a file path: {stdout}"
        );
        let path = stdout.trim();
        assert!(
            std::path::Path::new(path).exists(),
            "Screenshot file should exist at {path}"
        );
    }
}

#[test]
fn headed_tabs_lists_pages() {
    if !common::browser_ready() {
        return;
    }

    let b = TestBrowser::new("test-tabs");

    let (_, _, code) = run_cli(&["--browser", b.name(), "goto", "https://example.com"]);

    if code != 0 {
        common::unavailable("goto https://example.com failed");
        return;
    }

    let (stdout, _, code) = run_cli(&["--browser", b.name(), "tabs"]);

    if code == 0 {
        assert!(
            stdout.contains("TARGET_ID") || stdout.contains("example.com"),
            "tabs output: {stdout}"
        );
    }
}
