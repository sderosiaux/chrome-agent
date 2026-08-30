//! What to do about an error, in the words the next action needs.
//!
//! Split out of `run_helpers.rs` for the repo's 1000-line file cap and re-exported from it,
//! so the three error paths (`main`, `pipe`, `pipe_dispatch`) keep their call sites.
//!
//! # The contract every hint holds to
//!
//! 1. **One fact about what is known.** Not a question, not a guess. "Is Chrome running?"
//!    asked the reader something the tool knows better than they do — chrome-agent launches
//!    its own Chrome — and "Element may be hidden" hedged about a state we had just measured.
//! 2. **Exactly one imperative command, with real values substituted.** A hint reading
//!    `chrome-agent scroll <uid>` is not a command, it is a template the reader has to
//!    finish, and an agent copying it verbatim runs a uid literally named `<uid>`. The uid is
//!    in the error message and the browser name is in the invocation, so both get filled in.
//!    Where two routes exist, the criterion that chooses between them is stated; two options
//!    with no criterion is the same shrug as no hint at all.
//! 3. **When a retry would be dangerous, forbid it in words.** "Try running the command
//!    again" was the advice on a lost transport — where the command may already have been
//!    delivered, so the retry is a second real click, a second real fill, a second order.
//!
//! The hints are built as owned strings rather than `&'static str` for rule 2: a static
//! string cannot carry the uid or the browser name this invocation is actually about.
//!
//! # Where the three files are cut
//!
//! `hints.rs` reached 1194 lines, over the repo's 1000-line cap, on the one file the contract
//! above obliges a new error class to touch. It is now a directory, following the same pattern
//! `element.rs`/`element_controls.rs` and `hit_test.rs`/`hit_test_report.rs` use — one module per
//! seam, everything re-exported so no call site changes. A directory rather than sibling modules
//! because two of the pieces below are private to this module ([`invocation`], [`uid_in`]) and a
//! sibling cannot see them; a child can, and this crate has no `lib.rs` to make a third route
//! possible.
//!
//! * [`navigation`] — the failures that happen before a document exists. None of them has a uid,
//!   an element or a snapshot to talk about, which is what everything else here has.
//! * [`element`] — the `error_hint` chain, and the hints a *successful* click carries when it
//!   downloaded nothing or dispatched nothing.
//! * here — the contract itself, the two builders both of the above write through, the one clap
//!   error this tool rewrites, and the tests that hold the whole corpus to the three rules.
//!
//! The test corpus lives here for the same reason: it is a statement about the module, not about
//! either half of it, and the scan that keeps it exhaustive reads [`element`]'s source from here.

mod element;
mod navigation;

pub use element::{
    download_cancelled_hint, download_cap_hint, download_unfinished_hint, error_hint,
    intercepted_refusal_hint, no_download_hint, undispatched_download_hint,
};

/// The invocation prefix that reaches THIS session's browser.
///
/// `--browser` defaults to `"default"`, and a hint that drops it when the caller passed a
/// name points the reader at another agent's browser — the exact isolation the flag exists
/// to provide.
fn invocation(browser: &str) -> String {
    if browser == "default" {
        "chrome-agent".to_string()
    } else {
        format!("chrome-agent --browser {browser}")
    }
}

/// The uid an error names, when it names one.
///
/// `uid=n47 has no visible box model` knows which element it is about; the hint used to
/// print `<uid>` anyway.
fn uid_in(msg: &str) -> Option<&str> {
    let rest = msg.split("uid=").nth(1)?;
    let end = rest.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// Rewrite a clap usage error that is really about flag position.
///
/// `chrome-agent click n1 --timeout 5` is rejected — `--timeout` is one of the two flags that
/// cannot be `global = true` (`cli::BEFORE_VERB_ONLY` says why) — and clap answers
/// `tip: to pass '--timeout' as a value, use '-- --timeout'`. That tip is correct advice for a
/// different problem: passing a literal string that happens to look like a flag. Nobody typing
/// `--timeout 5` wants the string "--timeout", so the reader is sent to escape an argument they
/// never meant as one, and the rule they actually broke is never stated.
///
/// The tip is removed and replaced under this module's contract: one fact, and one imperative
/// command with THIS invocation's real values in it. The command is built by moving the flag and
/// its value ahead of everything else, so it is the caller's own line, reordered — not a template
/// and not an example of a different command.
///
/// Anything else is returned untouched: this is the one error clap gets wrong, not a general
/// filter over its output.
#[must_use]
pub fn usage_error(rendered: &str, argv: &[String]) -> String {
    // Both conditions matter. The flag has to be one of the two, AND clap has to have rejected
    // it as unexpected — `wait --timeout 5` with no pattern is also an error naming `--timeout`,
    // and it has nothing to do with position.
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
            // …and the blank line that followed it, or the output grows a double gap.
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
/// Rule 2 of the contract: a hint that prints `chrome-agent --timeout <n> …` is a template the
/// reader has to finish. The values are all in `argv`, so they get filled in. Reordering rather
/// than reconstructing means any other flag the caller passed survives, wherever it was.
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
        // Skip argv[0]: it is whatever path the shell resolved, and every command in a hint
        // reads `chrome-agent …`.
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

    /// Every error message this module claims to recognise, as the code actually spells it.
    /// Used by the contract tests below so a new branch is covered by them automatically —
    /// and, since `every_spelling_error_hint_recognises_is_in_the_corpus`, so that a new branch
    /// that is NOT added here fails the suite instead of passing unnoticed.
    const RECOGNISED: &[&str] = &[
        "Failed to connect to page after 8 attempts: Connection refused",
        "DevToolsActivePort file doesn't exist",
        "Connection refused",
        "No such file or directory",
        "Element uid=n5 not found. Run 'chrome-agent inspect' to get fresh uids.",
        // The same message about a uid the snapshot PRINTED and never stored. It is a
        // different failure with a different recovery, and it is how a reader who typed
        // `click e12` off an `inspect` actually arrives.
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
        // `CdpClientError::Timeout` renders its payload behind a lowercase `timeout: ` prefix
        // (`cdp/client.rs`), and this shape carries no capital `Timeout` anywhere — which is
        // why the branch tests both spellings, and why the lowercase one needs an entry of
        // its own to be exercised at all.
        "timeout: Runtime.evaluate did not answer within 30s. An in-page promise that never settles (awaitPromise) is the usual cause; raise --timeout if the page is merely slow.",
        "Input.dispatchMouseEvent was dispatched and Chrome did not acknowledge it within 8s, so what the page did with it is unknown. The event may already have reached the page.",
        "Element uid=n47 has no visible box model.",
        "Refused to click uid=n47: not interactable",
        "not interactable",
        "No element matches selector: .missing",
        "Element uid=e12 has no resolvable backend node.",
        "response parse: invalid type",
        // The same failure with the CDP payload serde quotes back inside it. This message is
        // the one place in the chain whose tail is written by somebody else, so it is also the
        // one place a decoy can appear: `timeout` here belongs to Chrome's payload and not to
        // this tool's own timeout.
        "response parse: invalid type: string \"timeout\", expected u64 at line 1 column 42",
        "Page may not have an article structure",
        "Readability failed",
        "Provide a uid, --selector, or --xy to identify the click target.",
        // The second spelling, from `commands::assert` and `run.rs`. It shares the branch and
        // had no entry until the scan below found it.
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
        // Native WebMCP's own complaint about a non-string second argument, which a polyfill
        // spells as the `JSON.parse` failure above. `commands::webmcp` records the fragment and
        // not Chromium's sentence around it, so this entry carries the fragment and the
        // `executeTool` the branch also requires — the two things the recognition keys on — and
        // claims nothing about the rest of the wording.
        "Evaluation error: Failed to parse input arguments (executeTool)",
    ];

    /// Spellings [`error_hint`] tests for that nothing in this binary can currently produce.
    ///
    /// The scan below refuses a predicate with no corpus entry, because a predicate nobody has
    /// written a message for is a hint nobody has read. These three are the exception, and each
    /// carries the reason it is one rather than being quietly deleted: they are guards, and a
    /// guard's whole job is to be unreached. The scan holds them to two rules of their own — an
    /// entry may not outlive the branch it exempts, and it may not cover a spelling that IS
    /// emitted, which is what would turn this list into a way of hiding an untested hint.
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

    /// The hints that ride on a response the command answered `ok:true` for, so [`error_hint`]
    /// never sees them. They are held to the same three rules as every other, which is the
    /// whole reason they are enumerated here rather than tested one at a time next door.
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

    /// A hint's prose: everything outside its backticks.
    ///
    /// A hint quotes two kinds of literal that way — the command to run, and the URL or host the
    /// tool echoed back out of the error. `goto https://x.test/a?b=1` carries a question mark
    /// that is the caller's own URL and not a question put to the reader, so the checks that
    /// judge sentences read the sentences.
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

    /// Every `msg.contains("…")` spelling in the [`error_hint`] chain, read out of its source.
    ///
    /// The corpus above is written by hand and the chain it claims to cover is not, so the two
    /// drift in one direction only: a branch gets added, and nothing notices that no test message
    /// reaches it. Reading the source is what a convention cannot do, and it is cheap here because
    /// the chain is one function in one file.
    fn predicates_of_error_hint() -> Vec<&'static str> {
        const SOURCE: &str = include_str!("element.rs");
        const NEEDLE: &str = "msg.contains(\"";
        let start = SOURCE
            .find("pub fn error_hint")
            .expect("the chain is in element.rs");
        // Its closing brace is the first one alone at column 0 after it.
        let end = SOURCE[start..]
            .find("\n}\n")
            .map_or(SOURCE.len(), |offset| start + offset);
        let mut body = &SOURCE[start..end];
        let mut out = Vec::new();
        while let Some(at) = body.find(NEEDLE) {
            let after = &body[at + NEEDLE.len()..];
            let close = after.find('"').expect("an unterminated literal would not compile");
            let literal = &after[..close];
            assert!(
                !literal.contains('\\'),
                "this scan reads a predicate verbatim and decodes no escapes; {literal:?} has one"
            );
            out.push(literal);
            body = &after[close..];
        }
        out
    }

    /// The corpus is exhaustive over the chain, by scan rather than by care.
    ///
    /// It was not, when this was written: six spellings had no message behind them. Three were
    /// live and untested — `Provide --uid` (`commands::assert`, `run.rs`), the lowercase
    /// `timeout` (`CdpClientError::Timeout`'s own Display prefix), and native `WebMCP`'s
    /// `Failed to parse input arguments` — and three were guards, which are listed above with
    /// the reason each cannot be reached.
    #[test]
    fn every_spelling_error_hint_recognises_is_in_the_corpus() {
        let predicates = predicates_of_error_hint();
        assert!(
            predicates.len() > 30,
            "the scan lost the chain rather than reading it: {predicates:?}"
        );
        let uncovered: Vec<&str> = predicates
            .iter()
            .copied()
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
                predicates.contains(spelling),
                "{spelling:?} is exempted from a branch that no longer exists ({why})"
            );
            assert!(
                !RECOGNISED.iter().any(|msg| msg.contains(spelling)),
                "{spelling:?} is exempted as unreachable ({why}) and the corpus reaches it — \
                 the exemption is hiding a hint that is now testable"
            );
        }
    }

    /// Rule 1, mechanically. A hint states a fact about what is known.
    ///
    /// Two shapes are refused. A question asks the reader something the tool knows better — "Is
    /// Chrome running?" is the one this contract was written against, on a tool that launches
    /// its own Chrome. And a hedge — `might`, `possibly`, `perhaps`, `maybe`, `probably`,
    /// `may be` — describes as uncertain a state that was measured, which is what "Element may
    /// be hidden" did over a box model that had just been read.
    ///
    /// What is deliberately NOT refused is an imperative with no command behind it: "Check the
    /// file path exists on disk." is a fact and an instruction, and the two navigation failures
    /// with no recovery inside this tool are the same shape on purpose. That is rule 2's
    /// business and it stays with the human reviewer.
    #[test]
    fn no_hint_asks_a_question_or_hedges_about_what_was_measured() {
        for hint in every_hint("agent-7") {
            // `may be repeated` is the one licensed use of `may be`, and measurement is what
            // licenses it: it is a permission about the caller's NEXT action, on the one path
            // where the hit test refused to aim and the page saw no event at all. The
            // assertion below pins it to that hint, so the licence cannot spread into a hedge.
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

    /// Rule 2, mechanically. A hint that prints `<uid>` or `<url>` is a template, and an
    /// agent that copies it runs a uid literally named `<uid>`.
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

    /// Rule 3, mechanically. The words that invited a second real click are gone, and no hint
    /// may reintroduce them: the tool cannot tell a delivered action from a lost one.
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

    /// Rule 2, the other half: a command in a hint has to reach the browser the caller is
    /// actually driving. Under `--browser agent-7`, `chrome-agent inspect` inspects somebody
    /// else's session.
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

    /// The tip clap offers here is advice for a different problem: escaping a literal string
    /// that looks like a flag. Nobody typing `--timeout 5` meant the string "--timeout".
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
        // The resolved binary path is never quoted back — every command in a hint is spelled
        // `chrome-agent`.
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

    /// Every flag that cannot be global needs its own clause, or the hint states the rule
    /// without the reason and the reader cannot tell whether it is a bug.
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
