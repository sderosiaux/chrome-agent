//! What to do about an error, in the words the next action needs.
//!
//! Every hint holds to three rules, enforced by the tests below:
//!
//! 1. One fact about what is known — never a question, never a hedge about something measured.
//! 2. Exactly one imperative command with real values substituted (uid from the message,
//!    `--browser` from the invocation). Where two routes exist, state the criterion that chooses.
//! 3. When a retry would be dangerous, forbid it in words: a lost transport may already have
//!    delivered the click.
//!
//! Hints are owned `String`s, not `&'static str`, because rule 2 needs per-invocation values.
//! [`navigation`] holds the failures that happen before a document exists; [`element`] the
//! `error_hint` chain and the hints a successful click carries.

mod element;
mod navigation;

pub use element::{
    download_cancelled_hint, download_cap_hint, download_unfinished_hint, error_hint,
    intercepted_refusal_hint, no_download_hint, undispatched_download_hint,
};

/// The invocation prefix that reaches THIS session's browser.
///
/// Dropping `--browser` when the caller passed one points the reader at another agent's browser.
fn invocation(browser: &str) -> String {
    if browser == "default" {
        "chrome-agent".to_string()
    } else {
        format!("chrome-agent --browser {browser}")
    }
}

/// The uid an error names, when it names one (`uid=n47 …` → `n47`).
fn uid_in(msg: &str) -> Option<&str> {
    let rest = msg.split("uid=").nth(1)?;
    let end = rest.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// Rewrite a clap usage error that is really about flag position. Clap's `tip: … use
/// '-- --timeout'` is advice for escaping a literal string; it is replaced by the caller's own
/// line with the flag moved before the verb. Any other clap output is returned untouched.
#[must_use]
pub fn usage_error(rendered: &str, argv: &[String]) -> String {
    // Both conditions matter: `wait --timeout 5` with no pattern also names `--timeout`.
    let Some((flag, why)) = crate::cli::BEFORE_VERB_ONLY
        .iter()
        .copied()
        .find(|(flag, _)| rendered.contains(&format!("unexpected argument '{flag}'")))
    else {
        return rendered.to_string();
    };
    let mut out = String::new();
    let mut skip_blank = false;
    for line in rendered.lines() {
        if line.trim_start().starts_with(&format!("tip: to pass '{flag}'")) {
            // Skip the blank line after it too, or the output grows a double gap.
            skip_blank = true;
            continue;
        }
        if skip_blank && line.trim().is_empty() {
            skip_blank = false;
            continue;
        }
        skip_blank = false;
        out.push_str(line);
        out.push('\n');
    }
    let command = before_verb_form(argv, flag)
        .map_or_else(|| format!("chrome-agent {flag} <value> …"), |line| format!("`{line}`"));
    out.push_str(&format!(
        "hint: {flag} is read before the verb: {command}. Same values, same command — only the \
         flag moves. ({why}.)\n"
    ));
    out
}

/// This invocation, with `flag` and its value moved ahead of everything else.
///
/// Reordering rather than reconstructing keeps every other flag the caller passed (rule 2).
fn before_verb_form(argv: &[String], flag: &str) -> Option<String> {
    let with_equals = format!("{flag}=");
    let position = argv.iter().position(|arg| arg == flag || arg.starts_with(&with_equals))?;
    // `--timeout=5` is one argv entry and carries its own value; `--timeout 5` is two.
    let taken = if argv[position].starts_with(&with_equals) { 1 } else { 2 };
    let moved: Vec<&str> = argv[position..]
        .iter()
        .take(taken)
        .map(String::as_str)
        .collect();
    let rest = argv
        .iter()
        .enumerate()
        // Skip argv[0]: every command in a hint reads `chrome-agent …`, not the resolved path.
        .filter(|(i, _)| *i != 0 && (*i < position || *i >= position + taken))
        .map(|(_, arg)| arg.as_str());
    Some(
        std::iter::once("chrome-agent")
            .chain(moved)
            .chain(rest)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every error message this module claims to recognise, as the code spells it. Drives the
    /// contract tests, and a branch missing from here fails the scan below.
    const RECOGNISED: &[&str] = &[
        "Failed to connect to page after 8 attempts: Connection refused",
        "DevToolsActivePort file doesn't exist",
        "Connection refused",
        "No such file or directory",
        "Element uid=n5 not found. Run 'chrome-agent inspect' to get fresh uids.",
        // Same message, uid the snapshot printed but never stored: different recovery.
        "Element uid=e12 not found. Run 'chrome-agent inspect' to get fresh uids.",
        "Navigation failed for https://akamai.net: net::ERR_NAME_NOT_RESOLVED",
        "Navigation failed for https://localhost:3000/a: net::ERR_CONNECTION_REFUSED",
        "Navigation failed for https://wrong.host.test/: net::ERR_CERT_COMMON_NAME_INVALID",
        "Navigation failed for https://x.test/a: net::ERR_CONNECTION_RESET",
        "Navigation failed for https://x.test/a: net::ERR_HTTP_RESPONSE_CODE_FAILURE",
        "Navigation failed for http://127.0.0.1:9/x: net::ERR_UNSAFE_PORT",
        "Navigation failed for https://x.test/a: net::ERR_ABORTED",
        "Navigation failed for https://x.test/a: net::ERR_SOCKS_CONNECTION_FAILED",
        "Navigation failed for https://x.test/a: something with no code",
        "Navigation failed",
        "No snapshot stored for this page",
        "uid_map is empty",
        "Timeout waiting for selector",
        // `CdpClientError::Timeout` prefixes a lowercase `timeout: ` and carries no capital
        // `Timeout`, so the lowercase spelling needs its own entry.
        "timeout: Runtime.evaluate did not answer within 30s. An in-page promise that never settles (awaitPromise) is the usual cause; raise --timeout if the page is merely slow.",
        "Input.dispatchMouseEvent was dispatched and Chrome did not acknowledge it within 8s, so what the page did with it is unknown. The event may already have reached the page.",
        "Element uid=n47 has no visible box model.",
        "Refused to click uid=n47: not interactable",
        "not interactable",
        "No element matches selector: .missing",
        "Element uid=e12 has no resolvable backend node.",
        "response parse: invalid type",
        // Same failure with a serde-quoted CDP payload: `timeout` here is Chrome's word, a
        // decoy for the timeout branch.
        "response parse: invalid type: string \"timeout\", expected u64 at line 1 column 42",
        "Page may not have an article structure",
        "Readability failed",
        "Provide a uid, --selector, or --xy to identify the click target.",
        // Second spelling of the same branch, from `commands::assert` and `run.rs`.
        "assert value: Provide --uid or --selector to identify the element.",
        "Evaluation error: TypeError: foo",
        "dispatcher task exited",
        "transport closed",
        "Element is not an <iframe>",
        "No child frame found for selector",
        "Element is not a <select>",
        "No option matching: foo",
        "File not found: /tmp/nope",
        "assert: invalid regular expression",
        "batch: expected a JSON array",
        "Evaluation error: Error: chrome-agent: document.modelContext is undefined on this page.",
        "Evaluation error: Error: chrome-agent: document.modelContext is undefined in the bound frame's isolated world — this does not prove the frame has no tools, since a polyfill the frame's own main-world script installs is invisible here.",
        "Evaluation error: Error: chrome-agent: no WebMCP tool named \"foo\". Known tools: bar, baz.",
        "Evaluation error: TypeError: The provided value is not of type 'RegisteredTool'.",
        "Evaluation error: SyntaxError: \"[object Object]\" is not valid JSON\n    at JSON.parse (<anonymous>)\n    at Object.executeTool (file:///x.html:1:1)",
        // Native WebMCP's complaint about a non-string second argument (a polyfill spells it as
        // the `JSON.parse` failure above). Carries only the two fragments recognition keys on.
        "Evaluation error: Failed to parse input arguments (executeTool)",
    ];

    /// Spellings [`error_hint`] tests for that nothing can produce — guards, each with its
    /// reason. An entry may not outlive its branch, nor cover a spelling that is emitted.
    const UNREACHABLE: &[(&str, &str)] = &[
        (
            "No inspect",
            "nothing in this binary builds a message with that text; the two spellings that \
             reach the same branch and are emitted are `No snapshot` and `uid_map is empty`",
        ),
        (
            "not an <IFRAME>",
            "`commands::frame` throws `Element is not an <iframe>` and returns Chrome's \
             exception description unchanged, so only the lowercase spelling can arrive; this \
             one guards against that literal changing case",
        ),
        (
            "ReferenceError",
            "every path that hands a page exception to `error_hint` prefixes it with \
             `Evaluation error:` (`commands::eval`), which this same branch already matches. \
             `commands::frame` is the one path that returns a bare description, and the JS it \
             evaluates is chrome-agent's own, so a bare ReferenceError there would be a bug in \
             this binary rather than in the page",
        ),
    ];

    /// Hints riding on an `ok:true` response, so [`error_hint`] never sees them. Enumerated here
    /// to hold them to the same three rules as every other.
    fn hints_outside_error_hint(browser: &str) -> Vec<String> {
        let banner = crate::hit_test::Hit {
            tag: "DIV".into(),
            id: Some("gdpr-wall".into()),
            cls: Some("wall".into()),
            z: Some("9999".into()),
            text: "We use cookies".into(),
            modal: false,
            iframe: false,
            same_doc: true,
            actionable: true,
            uid: Some("n210".into()),
        };
        let mut dialog = banner.clone();
        dialog.tag = "DIALOG".into();
        dialog.modal = true;
        let mut intercepted = crate::hit_test::Dispatched::js();
        intercepted.receiver = Some(banner.clone());
        use crate::hit_test::OnIntercept;
        vec![
            no_download_hint(browser, 30, &crate::hit_test::Dispatched::js()),
            no_download_hint(browser, 30, &intercepted),
            undispatched_download_hint(browser),
            download_cap_hint(browser, 67_108_864),
            download_cancelled_hint(browser),
            download_unfinished_hint(browser),
            intercepted_refusal_hint(browser, Some(&banner), OnIntercept::Refuse),
            intercepted_refusal_hint(browser, Some(&banner), OnIntercept::Guard),
            intercepted_refusal_hint(browser, Some(&dialog), OnIntercept::Refuse),
            intercepted_refusal_hint(browser, None, OnIntercept::Refuse),
        ]
    }

    /// Every hint this module can produce, for the contract tests that judge all of them.
    fn every_hint(browser: &str) -> Vec<String> {
        RECOGNISED
            .iter()
            .map(|msg| error_hint(msg, browser).unwrap_or_else(|| panic!("no hint for {msg}")))
            .chain(hints_outside_error_hint(browser))
            .collect()
    }

    /// A hint's prose: everything outside its backticks. Backticks quote commands and URLs, whose
    /// punctuation (`?b=1`) must not read as a question to the sentence checks.
    fn prose(hint: &str) -> String {
        hint.split('`').step_by(2).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn every_recognised_failure_has_a_hint() {
        for msg in RECOGNISED {
            assert!(error_hint(msg, "default").is_some(), "no hint for {msg}");
        }
        assert!(error_hint("something random", "default").is_none());
    }

    /// Every `.rs` file of this module, read off disk rather than named one by one: the chain
    /// used to live in a single file and a split moved half of it, which an `include_str!` of
    /// one name cannot follow.
    fn hint_sources() -> Vec<(String, String)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/hints");
        let mut out: Vec<(String, String)> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|e| e == "rs"))
            .map(|path| {
                let text = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
                (path.file_name().unwrap_or_default().to_string_lossy().into_owned(), text)
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        assert!(
            !out.is_empty(),
            "no source read from {} — the scan below would prove nothing",
            dir.display()
        );
        out
    }

    /// Every `msg.contains("…")` spelling this module recognises, read out of its own sources —
    /// all of them, not just the file the chain happens to start in. Catches a branch added
    /// without a corpus entry, which no convention can.
    ///
    /// Only the part of each file before `#[cfg(test)]` is scanned: a spelling quoted in a test
    /// or in this doc comment is not a branch.
    fn predicates_of_error_hint() -> Vec<String> {
        const NEEDLE: &str = "msg.contains(\"";
        let sources = hint_sources();
        let mut declares_the_chain = false;
        let mut out = Vec::new();
        for (name, text) in &sources {
            let mut body = text.split("#[cfg(test)]").next().unwrap_or(text);
            declares_the_chain |= body.contains("pub fn error_hint");
            while let Some(at) = body.find(NEEDLE) {
                let after = &body[at + NEEDLE.len()..];
                let close = after.find('"').expect("an unterminated literal would not compile");
                let literal = &after[..close];
                assert!(
                    !literal.contains('\\'),
                    "this scan reads a predicate verbatim and decodes no escapes; {literal:?} \
                     in {name} has one"
                );
                out.push(literal.to_string());
                body = &after[close..];
            }
        }
        assert!(
            declares_the_chain,
            "no file under src/hints/ declares `pub fn error_hint`, so this scan lost the chain \
             rather than reading it; it read: {:?}",
            sources.iter().map(|(name, _)| name).collect::<Vec<_>>()
        );
        out
    }

    /// The corpus is exhaustive over the chain, by scan rather than by care.
    #[test]
    fn every_spelling_error_hint_recognises_is_in_the_corpus() {
        let predicates = predicates_of_error_hint();
        assert!(
            predicates.len() > 30,
            "the scan lost the chain rather than reading it: {predicates:?}"
        );
        let uncovered: Vec<&str> = predicates
            .iter()
            .map(String::as_str)
            .filter(|spelling| !RECOGNISED.iter().any(|msg| msg.contains(spelling)))
            .filter(|spelling| !UNREACHABLE.iter().any(|(known, _)| known == spelling))
            .collect();
        assert!(
            uncovered.is_empty(),
            "error_hint recognises spellings no test message reaches: {uncovered:?}. Add a \
             message this binary really emits to RECOGNISED, or — if nothing can produce it — \
             an entry to UNREACHABLE saying why."
        );
        for (spelling, why) in UNREACHABLE {
            assert!(
                predicates.iter().any(|found| found == spelling),
                "{spelling:?} is exempted from a branch that no longer exists ({why})"
            );
            assert!(
                !RECOGNISED.iter().any(|msg| msg.contains(spelling)),
                "{spelling:?} is exempted as unreachable ({why}) and the corpus reaches it — \
                 the exemption is hiding a hint that is now testable"
            );
        }
    }

    /// Rule 1, mechanically: no question, and no hedge (`might`, `maybe`, `may be`, …) about a
    /// measured state. An imperative with no command behind it is NOT refused; that is rule 2's
    /// business and stays with the human reviewer.
    #[test]
    fn no_hint_asks_a_question_or_hedges_about_what_was_measured() {
        for hint in every_hint("agent-7") {
            // `may be repeated` is the one licensed `may be`: a permission about the next
            // action, on the path where no event reached the page. Pinned to that hint below.
            let prose = prose(&hint).to_lowercase().replace("may be repeated", "");
            assert!(
                !prose.contains('?'),
                "rule 1: a hint states a fact rather than asking the reader: {hint}"
            );
            for hedge in ["might", "possibly", "perhaps", "maybe", "probably", "may be"] {
                assert!(
                    !prose.contains(hedge),
                    "rule 1: {hedge:?} hedges about something this tool measured: {hint}"
                );
            }
        }
        let licensed: Vec<String> = every_hint("agent-7")
            .into_iter()
            .filter(|hint| hint.contains("may be repeated"))
            .collect();
        assert_eq!(licensed.len(), 1, "the licence spread: {licensed:?}");
        assert!(
            licensed[0].contains("No mouse event was dispatched"),
            "the licence is for the hint where nothing reached the page: {}",
            licensed[0]
        );
    }

    /// Rule 2, mechanically: an agent copying `<uid>` runs a uid literally named `<uid>`.
    #[test]
    fn no_hint_hands_back_an_unresolved_placeholder() {
        for browser in ["default", "agent-7"] {
            for hint in every_hint(browser) {
                for placeholder in ["<uid>", "<url>", "<name>", "<selector>", "<n>", "<value>"] {
                    assert!(
                        !hint.contains(placeholder),
                        "the hint hands back {placeholder}: {hint}"
                    );
                }
            }
        }
    }

    /// Rule 3, mechanically: the tool cannot tell a delivered action from a lost one, so no hint
    /// may invite a retry.
    #[test]
    fn no_hint_invites_a_blind_retry() {
        for hint in every_hint("default") {
            for forbidden in [
                "Try running the command again",
                "try running the command again",
                "run the command again",
            ] {
                assert!(!hint.contains(forbidden), "the hint invites a blind retry: {hint}");
            }
        }
    }

    /// Rule 2, the other half: under `--browser agent-7`, a bare `chrome-agent inspect` inspects
    /// somebody else's session.
    #[test]
    fn a_command_in_a_hint_names_this_invocation_s_browser() {
        for hint in every_hint("agent-7") {
            for quoted in hint.split('`').skip(1).step_by(2) {
                if let Some(rest) = quoted.strip_prefix("chrome-agent ") {
                    assert!(
                        rest.starts_with("--browser agent-7 "),
                        "a hint runs a command against the wrong browser: {quoted}"
                    );
                }
            }
        }
    }

    #[test]
    fn uid_extraction_stops_at_the_end_of_the_uid() {
        assert_eq!(uid_in("Element uid=n47 has no visible box model."), Some("n47"));
        assert_eq!(uid_in("Refused to click uid=n5: covered"), Some("n5"));
        assert_eq!(uid_in("Element uid=e12 has no resolvable backend node."), Some("e12"));
        assert_eq!(uid_in("no uid here"), None);
        assert_eq!(uid_in("trailing uid="), None);
    }

    /// Clap's rendering of `chrome-agent click n1 --timeout 5`, verbatim.
    const POSITION_ERROR: &str = "error: unexpected argument '--timeout' found\n\n  tip: to pass '--timeout' as a value, use '-- --timeout'\n\nUsage: chrome-agent click <UID>\n\nFor more information, try '--help'.\n";

    fn argv(args: &[&str]) -> Vec<String> {
        std::iter::once("/path/to/target/debug/chrome-agent")
            .chain(args.iter().copied())
            .map(String::from)
            .collect()
    }

    /// Clap's tip solves a different problem: escaping a literal string that looks like a flag.
    #[test]
    fn the_misleading_tip_is_replaced_by_the_rule_that_was_broken() {
        let out = usage_error(POSITION_ERROR, &argv(&["click", "n1", "--timeout", "5"]));
        assert!(!out.contains("-- --timeout"), "clap's tip survived: {out}");
        assert!(!out.contains("as a value"), "{out}");
        // What clap got right stays: the error itself and the usage line.
        assert!(out.contains("error: unexpected argument '--timeout' found"), "{out}");
        assert!(out.contains("Usage: chrome-agent click <UID>"), "{out}");
        assert!(out.contains("hint: --timeout is read before the verb"), "{out}");
        assert!(!out.contains("\n\n\n"), "removing the tip left a double gap: {out:?}");
    }

    /// Rule 2, on the case it was written for: the caller's own line, reordered.
    #[test]
    fn the_suggested_command_is_this_invocation_with_the_flag_moved() {
        let out = usage_error(POSITION_ERROR, &argv(&["click", "n1", "--timeout", "5"]));
        assert!(out.contains("`chrome-agent --timeout 5 click n1`"), "{out}");
        // The resolved binary path is never quoted back; hints spell it `chrome-agent`.
        assert!(!out.contains("/path/to/target"), "{out}");
    }

    /// `--timeout=5` is a single argv entry carrying its own value, and any flag the caller
    /// already put before the verb has to survive the move.
    #[test]
    fn a_glued_value_and_the_other_flags_survive_the_reorder() {
        let out = usage_error(
            POSITION_ERROR,
            &argv(&["--browser", "agent-7", "click", "n1", "--timeout=5"]),
        );
        assert!(
            out.contains("`chrome-agent --timeout=5 --browser agent-7 click n1`"),
            "{out}"
        );
    }

    /// This rewrite applies to one error clap gets wrong, not to its output in general.
    #[test]
    fn every_other_usage_error_is_returned_untouched() {
        for rendered in [
            "error: unexpected argument '--nonsense' found\n\n  tip: to pass '--nonsense' as a value, use '-- --nonsense'\n",
            "error: the following required arguments were not provided:\n  <WHAT>\n",
            // Names the flag, but not as a position problem: `wait --timeout 5` with no pattern.
            "error: the following required arguments were not provided:\n  <WHAT>\n\nUsage: chrome-agent wait <WHAT> [PATTERN] --timeout <TIMEOUT>\n",
        ] {
            assert_eq!(usage_error(rendered, &argv(&["wait"])), rendered, "rewrote {rendered:?}");
        }
    }

    /// Every flag that cannot be global needs its own reason clause, or the hint states the rule
    /// without saying why.
    #[test]
    fn each_before_verb_flag_explains_itself_and_names_no_placeholder() {
        for (flag, why) in crate::cli::BEFORE_VERB_ONLY {
            let rendered = format!("error: unexpected argument '{flag}' found\n");
            let out = usage_error(&rendered, &argv(&["click", "n1", flag, "2"]));
            assert!(out.contains(why), "{flag} lost its reason: {out}");
            assert!(out.contains(&format!("`chrome-agent {flag} 2 click n1`")), "{out}");
            for placeholder in ["<value>", "<uid>", "<n>"] {
                assert!(!out.contains(placeholder), "{flag} hands back {placeholder}: {out}");
            }
        }
    }
}
