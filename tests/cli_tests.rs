use std::process::Command;

mod common;
use common::TestBrowser;

/// Get the path to the built binary.
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

/// Run chrome-agent with args and return (stdout, stderr, `exit_code`).
fn run_cli(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(binary())
        .args(args)
        .output()
        .expect("Failed to run chrome-agent");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);

    (stdout, stderr, code)
}

/// Every verb `--help` lists, minus clap's own `help`.
///
/// Derived, not typed: this used to be nineteen names written by hand under an assertion that
/// promised "all subcommands", against the forty-two the CLI has. A verb added to `Command`
/// joined `--help` and no test noticed — the same defect as a documentation table nobody
/// re-reads, one directory away.
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
        assert_eq!(code, 0, "--help lists `{verb}`, which the parser does not accept: {stderr}");
    }
}

/// The documents an agent reads show every verb in a form it can copy.
///
/// A verb that exists only in `--help` is a verb an agent will not reach for: the guide is
/// what `--help` appends, and the README and SKILL.md are what a coding agent is handed on
/// install. Nothing checked that the three of them together covered the CLI, and five verbs
/// were missing from at least one.
///
/// Two forms count, and a plain word match is neither — `stop`, `status`, `type` and `macro`
/// are ordinary English, and `stop` is also a `next` token printed in the verdict table of
/// every one of these files. Measured: a substring search finds all four everywhere and turns
/// five genuinely undocumented verbs green.
#[test]
fn every_verb_appears_as_an_invocation_in_the_documents_an_agent_reads() {
    let verbs = subcommands();
    let docs = [
        ("llm-guide.txt", include_str!("../llm-guide.txt")),
        ("README.md", include_str!("../README.md")),
        ("skills/chrome-agent/SKILL.md", include_str!("../skills/chrome-agent/SKILL.md")),
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

/// A verb no document shows in invocation form, and why that is the right answer for it.
///
/// The marker carries its reason in line, on the model of `isolation-exempt:` in
/// `harness_tests.rs`: a bare list of names is a list nobody can ever shrink, because nothing
/// records what would have to become true to remove an entry. `"*"` means every document.
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

/// The document shows `chrome-agent … <verb>` somewhere.
///
/// The verb is the first KNOWN verb after the binary's name, not the first word: global flags
/// parse on either side of it, so `chrome-agent --page mobile emulate status` names `emulate`
/// and `chrome-agent --json extract` names `extract`. A flag's value is skipped because it is
/// not a verb — and where one would be (`--connect auto inspect`), it is not one either.
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

/// The document has a table row whose FIRST cell is a backticked command starting with the verb.
///
/// The first cell only. The verdict table's third column holds `` `stop` ``, which is the `next`
/// token and not the verb — accepting any cell would report `stop` as documented in all three
/// files on the strength of a table about something else entirely.
fn heads_a_command_row(doc: &str, verb: &str) -> bool {
    doc.lines()
        .map(str::trim)
        .filter(|line| line.starts_with('|'))
        .filter_map(|line| line.trim_start_matches('|').split('|').next())
        .any(|cell| {
            cell.trim().strip_prefix('`').and_then(|rest| rest.strip_prefix(verb)).is_some_and(
                |tail| tail.starts_with('`') || tail.starts_with(' ') || tail.starts_with('\\'),
            )
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

/// A global flag parses on either side of the subcommand.
///
/// `chrome-agent fill --selector "#micro" "x" --json` used to fail with a raw clap error and the
/// tip "to pass '--json' as a value, use '-- --json'" — advice for a different problem, on the
/// most natural way to reach for the flag, and on the caller's first attempt. `CHROME_AGENT_PARSE_ONLY`
/// returns the moment clap has spoken, so this is clap's verdict and no browser is launched.
#[test]
fn a_global_flag_is_accepted_on_either_side_of_the_verb() {
    let parses = |args: &[&str]| {
        let output = Command::new(binary())
            .args(args)
            .env("CHROME_AGENT_PARSE_ONLY", "1")
            .output()
            .expect("run chrome-agent");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stderr).lines().next().unwrap_or("").to_string(),
        )
    };
    let cases: &[(&[&str], &[&str])] = &[
        (&["--json", "fill", "--selector", "#micro", "x"], &["fill", "--selector", "#micro", "x", "--json"]),
        (&["--json", "click", "n1"], &["click", "n1", "--json"]),
        (&["--json", "inspect"], &["inspect", "--json"]),
        (&["--verdict", "off", "click", "n1"], &["click", "n1", "--verdict", "off"]),
        // isolation-exempt: CHROME_AGENT_PARSE_ONLY, so no browser is launched and the name
        // reaches nothing — the case under test is where the flag sits, not what it names.
        (&["--browser", "a7", "eval", "1"], &["eval", "1", "--browser", "a7"]),
    ];
    for (before, after) in cases {
        let (ok, err) = parses(before);
        assert!(ok, "the documented order stopped working: {before:?} -> {err}");
        let (ok, err) = parses(after);
        assert!(ok, "a global flag after the verb must parse: {after:?} -> {err}");
    }
}

/// The two flags that cannot be global, and why: `wait`/`download` declare their own
/// `--timeout`, and the twelve action commands their own `--max-depth`. A global arg propagates
/// into every subcommand, so sharing an id with one is a duplicate-argument panic at startup.
/// Both positions still parse, each meaning its own thing, and `run.rs` resolves them with
/// `local.or(global)`.
#[test]
fn the_two_locally_redeclared_flags_still_work_in_both_positions() {
    for args in [
        vec!["wait", "selector", ".x", "--timeout", "5"],
        vec!["--timeout", "5", "click", "n1"],
        vec!["click", "n1", "--max-depth", "2"],
        vec!["--max-depth", "2", "click", "n1"],
    ] {
        let output = Command::new(binary())
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

/// The two flags that must precede the verb now say so, in the caller's own words.
///
/// `chrome-agent click n1 --timeout 5` is rejected on purpose — `wait` and `download` declare
/// their own `--timeout` with their own defaults, so the global one cannot propagate into every
/// subcommand. The harm was never the rule; it was clap's answer to it, `tip: to pass
/// '--timeout' as a value, use '-- --timeout'`, which is advice for escaping a literal string
/// nobody meant to pass.
#[test]
fn a_flag_that_must_precede_the_verb_says_so_instead_of_offering_to_escape_it() {
    for (args, flag, moved) in [
        (vec!["click", "n1", "--timeout", "5"], "--timeout", "chrome-agent --timeout 5 click n1"),
        (vec!["text", "--max-depth", "2"], "--max-depth", "chrome-agent --max-depth 2 text"),
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

        // The strongest form of the hint contract: the command it hands back has to run. A hint
        // that names an invocation the parser also rejects is worse than no hint.
        let suggested: Vec<&str> = moved.split_whitespace().skip(1).collect();
        let output = Command::new(binary())
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

/// And an unrelated usage error keeps clap's own wording, tip included: this rewrite covers the
/// one error clap gets wrong, not its output in general.
#[test]
fn an_unrelated_usage_error_is_left_to_clap() {
    let (_, stderr, code) = run_cli(&["click", "n1", "--nonsense"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("unexpected argument '--nonsense'"), "{stderr}");
    assert!(stderr.contains("-- --nonsense"), "clap's tip should survive here: {stderr}");
    assert!(!stderr.contains("read before the verb"), "{stderr}");
}

/// An invocation whose arguments are wrong must be told so, whatever the machine holds.
///
/// `run.rs` resolves the store, the browser and the CDP client BEFORE its second `match`, so any
/// validation living inside an arm — or between the connection and the arm — answers "No browser
/// session 'default'" on a machine with no session. That is a true sentence about a problem the
/// caller does not have, and it sends them to launch a browser for an invocation that could never
/// run. Four were reachable that way; each is now refused by the parser, before anything is
/// resolved or launched. The `HOME` is empty on purpose: with a session present, all four
/// happened to answer correctly, which is why the download case reached `main` green and CI red.
#[test]
fn an_invalid_invocation_is_refused_before_a_browser_is_resolved() {
    let home = common::temp_path("argcheck", "home");
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();

    // Each pair is an invocation and a word its refusal has to contain — the argument the caller
    // got wrong, never the state of the machine.
    for (args, names) in [
        (&["download"][..], "--selector"),
        (&["download", "https://example.com/f.csv", "--uid", "n1"][..], "cannot be used with"),
        (&["screenshot", "--format", "webp"][..], "--format"),
        (&["--dialog", "nope", "inspect"][..], "--dialog"),
        (&["goto", "https://example.com", "--header", "nocolon"][..], "--header"),
    ] {
        let output = Command::new(binary())
            .args(args)
            .env("HOME", &home)
            .output()
            .expect("run chrome-agent");
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        assert_eq!(output.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(stderr.contains(names), "{args:?} never names {names}: {stderr}");
        assert!(
            !stderr.contains("browser session"),
            "{args:?} answered about a browser, not about its arguments: {stderr}"
        );
    }

    // Nothing was launched on the way: the refusals happen in `Cli::try_parse`, which runs
    // before the store is read. A profile directory here would mean one of them got as far as
    // `resolve_cli_connection`.
    assert!(
        !home.join(".chrome-agent").join("browsers").exists(),
        "a refused invocation still launched a browser"
    );
    std::fs::remove_dir_all(&home).ok();
}

/// The two value lists added above are checked case-insensitively, because the parsers behind
/// them (`setup::DialogPolicy::parse`, `screenshot::ImgFormat::parse`) accept and unit-test
/// those spellings. Moving the check to clap must not quietly narrow what the tool accepts.
/// `CHROME_AGENT_PARSE_ONLY` is the project's own way to reach clap's verdict without a browser.
#[test]
fn the_spellings_those_parsers_accept_still_parse() {
    for args in [
        &["--dialog", "DISMISS", "inspect"][..],
        &["--dialog", "Accept", "inspect"][..],
        &["screenshot", "--format", "JPG"][..],
        &["screenshot", "--format", "PNG"][..],
        &["goto", "https://example.com", "--header", "X-Trace: a:b"][..],
    ] {
        let output = Command::new(binary())
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
    // Should show either "No active browser sessions" or existing sessions
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
    // The one-liner is what a token-conscious agent reads instead of the 26 KB `--help`, so it
    // has to carry the guarantee: a click that reports success may still have been taken by
    // something stacked above the target, and the response is where that shows.
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

// Integration tests that require Chrome (skipped in CI without Chrome)
// These are guarded by a check for Chrome availability.

/// Two CLI invocations reach the same named browser: the second sees what the first navigated to.
///
/// This test used to answer three different questions with the same green. It navigated to
/// `https://example.com`, and on a non-zero exit it printed `goto failed (may be network
/// issue)` and returned — an `eprintln!` and a bare `return`, past `common::unavailable`,
/// so `CHROME_AGENT_REQUIRE_BROWSER` could not turn that silence into a failure the way it
/// does for every other skip in this suite. The `eval` half then asserted only `if code == 0`,
/// which is a test that cannot fail on the thing it is named after. Both branches are gone,
/// and so is the network: the page is a fixture on disk, so a failure here is this tool's.
#[test]
fn goto_then_eval_share_one_named_browser() {
    if !common::browser_ready() {
        return;
    }

    let b = TestBrowser::new("test-integration");
    let url = format!("file://{}", common::fixture_path("assert_page.html").display());

    let (stdout, stderr, code) = run_cli(&["--browser", b.name(), "goto", &url]);
    assert_eq!(code, 0, "goto {url} failed: {stderr}");
    assert!(stdout.contains("Assertable page"), "goto output: {stdout}");

    // Same browser name, a second process, a second connection: the page has to still be there.
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

    // Fixture page: the button counts `click` vs `dblclick` events separately.
    // Written to a temp file and loaded via file:// (avoids data:-URL encoding).
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

    // The whole point of the fix: a selector double-click must fire `dblclick`,
    // not just a single `click`. Pre-fix (click_selector → el.click()) left __d=0.
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
        assert!(stdout.contains("uid="), "inspect should contain uid=N: {stdout}");
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
