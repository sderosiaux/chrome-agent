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

/// What to do about `msg`, phrased for the browser this invocation is driving.
#[must_use]
pub fn error_hint(msg: &str, browser: &str) -> Option<String> {
    let run = invocation(browser);
    // Chrome 136+ refuses CDP on the *default* user profile. chrome-agent launches
    // its own dedicated profile so this only bites when --connect points at a Chrome
    // started on the normal profile. Matched before the generic "Connection refused"
    // branch so the actionable hint wins.
    if msg.contains("Failed to connect to page") || msg.contains("DevToolsActivePort") {
        Some("Could not attach over CDP. Chrome 136+ disables remote debugging on the default profile: drop --connect to let chrome-agent launch its own dedicated profile, or relaunch your Chrome with a separate --user-data-dir.".to_string())
    } else if msg.contains("Connection refused") || msg.contains("No such file") {
        // Was "Is Chrome running? Try: chrome-agent goto <url>" — a question whose premise
        // is false (this tool starts its own Chrome) and a command with a placeholder in it.
        Some(format!(
            "Nothing answered CDP at the endpoint this session recorded, so the browser it \
             named is gone — crashed, or killed from outside this tool. chrome-agent starts \
             its own Chrome, so nothing has to be launched by hand: run `{run} close` to drop \
             the dead session entry, and the next command opens a fresh browser. If you \
             passed --connect, that port has no listener and the Chrome you meant to attach \
             to is the thing to start."
        ))
    } else if msg.contains("uid=") && msg.contains("not found") {
        Some(format!(
            "That uid is not in this page's snapshot: uids change when the document is \
             replaced, and `goto` clears the map on purpose. Run `{run} inspect` and act on \
             a uid from its output."
        ))
    } else if msg.contains("Navigation failed") {
        Some("Check the URL is valid and the page is reachable".to_string())
    } else if msg.contains("No snapshot") || msg.contains("No inspect") || msg.contains("uid_map is empty") {
        // Was "Run 'chrome-agent inspect' first" — the right command and no reason, so a
        // caller who thought a uid should still work had nothing to correct.
        Some(format!(
            "No snapshot is stored for this page, so there are no uids to resolve and no \
             baseline to report a change against. Run `{run} inspect` first: it is what \
             creates both."
        ))
    } else if msg.contains("Timeout") || msg.contains("timeout") {
        Some("Use --timeout N for slow pages".to_string())
    } else if msg.contains("not interactable") || msg.contains("no visible box model") {
        // Was "Element may be hidden. Try: chrome-agent scroll <uid>" — a hedge about a
        // state we measured, and a placeholder where the uid was already known.
        Some(match uid_in(msg) {
            Some(uid) => format!(
                "This element has no box on screen, so there is no point to aim at and \
                 nothing was dispatched. That is what `display:none`, a zero-size box, and \
                 an element scrolled out of a clipped container all look like. Run `{run} \
                 scroll {uid}` to bring it into view, then repeat the action; if it stays \
                 unaimable, the element is not rendered and no coordinate will reach it."
            ),
            None => format!(
                "This element has no box on screen, so there is no point to aim at and \
                 nothing was dispatched. That is what `display:none`, a zero-size box, and \
                 an element scrolled out of a clipped container all look like. Run `{run} \
                 inspect` to find the element that is actually rendered and act on that."
            ),
        })
    } else if msg.contains("No element matches selector") {
        Some(format!(
            "The selector matched nothing in the live document. Run `{run} eval \
             \"document.querySelectorAll('…').length\"` with your selector to see what it \
             matches before acting through it."
        ))
    } else if msg.contains("no resolvable backend node") {
        // Was "Page structure issue. Try: chrome-agent click --selector or chrome-agent
        // eval" — two options, no criterion, and the cause never named.
        Some(format!(
            "This uid names an accessibility node with no DOM element behind it — the `e…` \
             uids in a snapshot are Chrome's own generated nodes, and they cannot be resolved \
             to something clickable. Run `{run} inspect` and act on a uid beginning with `n`; \
             when the thing you want exists only as a generated node, aim at its DOM owner \
             with --selector instead."
        ))
    } else if msg.contains("response parse") {
        Some(format!(
            "Chrome answered this CDP call in a shape this version could not parse, so the \
             command never ran. Run `{run} status` to confirm the browser is the one this \
             session recorded; a Chrome much newer or older than the bundled Chromium is the \
             usual cause."
        ))
    } else if msg.contains("may not have an article") || msg.contains("Readability") {
        Some(format!(
            "Readability found no article on this page. Run `{run} text --selector \"main\"` \
             for the scoped visible text instead."
        ))
    } else if msg.contains("Provide a uid") || msg.contains("Provide --uid") {
        Some("Specify what to target: uid (e.g. n47), --selector \"css\", or --xy x,y".to_string())
    } else if msg.contains("Evaluation error") || msg.contains("TypeError") || msg.contains("ReferenceError") || msg.contains("SyntaxError") {
        Some("JS error in page context. Check expression syntax. Use --selector to scope to an element.".to_string())
    } else if msg.contains("dispatcher task exited") || msg.contains("transport closed") {
        // Was "Browser connection lost. Try running the command again." On a click or a fill
        // the first attempt may already have been delivered, and the page cannot tell a
        // deliberate second one from a retry.
        Some(format!(
            "The connection to Chrome dropped, so whether this command reached the page is \
             unknown. Do not repeat it blind: a click or a fill that was already delivered \
             becomes a second real one, and the page has no way to tell them apart. Run \
             `{run} inspect` and read the state the action was supposed to produce before \
             deciding. In pipe mode the session is over — the browser and its page survive it, \
             so start a new one."
        ))
    } else if msg.contains("not an <iframe>") || msg.contains("not an <IFRAME>") {
        Some("Only <iframe> is supported. For <frame>/<frameset>, use eval to access frame content.".to_string())
    } else if msg.contains("No child frame found") {
        Some("Iframe not found. Check the selector matches an <iframe> element.".to_string())
    } else if msg.contains("not a <select>") {
        Some("Element is not a <select>. For custom dropdowns, click to open then click the option.".to_string())
    } else if msg.contains("No option matching") {
        Some("No dropdown option matched. Use inspect --uid to check available options, or try the visible text.".to_string())
    } else if msg.contains("File not found") {
        Some("Check the file path exists on disk.".to_string())
    } else if msg.contains("invalid regular expression") {
        Some("--matches takes a Rust regex (regex-lite): \\d \\w \\s are ASCII-only, and there is no \\p{...} or lookaround. For a plain substring use --contains.".to_string())
    } else if msg.contains("expected a JSON array") {
        Some("Batch expects a JSON array of commands on stdin: [{\"cmd\":\"inspect\"}, ...]".to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every error message this module claims to recognise, as the code actually spells it.
    /// Used by the contract tests below so a new branch is covered by them automatically.
    const RECOGNISED: &[&str] = &[
        "Failed to connect to page after 8 attempts: Connection refused",
        "DevToolsActivePort file doesn't exist",
        "Connection refused",
        "No such file or directory",
        "Element uid=n5 not found. Run 'chrome-agent inspect' to get fresh uids.",
        "Navigation failed",
        "No snapshot stored for this page",
        "uid_map is empty",
        "Timeout waiting for selector",
        "Element uid=n47 has no visible box model.",
        "Refused to click uid=n47: not interactable",
        "not interactable",
        "No element matches selector: .missing",
        "Element uid=e12 has no resolvable backend node.",
        "response parse: invalid type",
        "Page may not have an article structure",
        "Readability failed",
        "Provide a uid, --selector, or --xy to identify the click target.",
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
    ];

    #[test]
    fn every_recognised_failure_has_a_hint() {
        for msg in RECOGNISED {
            assert!(error_hint(msg, "default").is_some(), "no hint for {msg}");
        }
        assert!(error_hint("something random", "default").is_none());
    }

    /// Rule 2, mechanically. A hint that prints `<uid>` or `<url>` is a template, and an
    /// agent that copies it runs a uid literally named `<uid>`.
    #[test]
    fn no_hint_hands_back_an_unresolved_placeholder() {
        for msg in RECOGNISED {
            for browser in ["default", "agent-7"] {
                let hint = error_hint(msg, browser).expect("a hint");
                for placeholder in ["<uid>", "<url>", "<name>", "<selector>", "<n>"] {
                    assert!(
                        !hint.contains(placeholder),
                        "the hint for {msg:?} hands back {placeholder}: {hint}"
                    );
                }
            }
        }
    }

    /// Rule 3, mechanically. The words that invited a second real click are gone, and no hint
    /// may reintroduce them: the tool cannot tell a delivered action from a lost one.
    #[test]
    fn no_hint_invites_a_blind_retry() {
        for msg in RECOGNISED {
            let hint = error_hint(msg, "default").expect("a hint");
            for forbidden in [
                "Try running the command again",
                "try running the command again",
                "run the command again",
            ] {
                assert!(
                    !hint.contains(forbidden),
                    "the hint for {msg:?} invites a blind retry: {hint}"
                );
            }
        }
    }

    /// Rule 2, the other half: a command in a hint has to reach the browser the caller is
    /// actually driving. Under `--browser agent-7`, `chrome-agent inspect` inspects somebody
    /// else's session.
    #[test]
    fn a_command_in_a_hint_names_this_invocation_s_browser() {
        for msg in RECOGNISED {
            let hint = error_hint(msg, "agent-7").expect("a hint");
            if !hint.contains("chrome-agent ") {
                continue; // No invocation to get wrong.
            }
            for word in hint.split('`').skip(1).step_by(2) {
                if let Some(rest) = word.strip_prefix("chrome-agent ") {
                    assert!(
                        rest.starts_with("--browser agent-7 "),
                        "hint for {msg:?} runs a command against the wrong browser: {word}"
                    );
                }
            }
        }
    }

    /// The uid is in the message; the hint used to print `<uid>` beside it.
    #[test]
    fn the_hint_for_an_unaimable_element_names_the_element() {
        let hint = error_hint("Element uid=n47 has no visible box model.", "default")
            .expect("a hint");
        assert!(hint.contains("chrome-agent scroll n47"), "{hint}");
        // And it states the measurement instead of hedging about it.
        assert!(!hint.contains("may be hidden"), "{hint}");
    }

    #[test]
    fn uid_extraction_stops_at_the_end_of_the_uid() {
        assert_eq!(uid_in("Element uid=n47 has no visible box model."), Some("n47"));
        assert_eq!(uid_in("Refused to click uid=n5: covered"), Some("n5"));
        assert_eq!(uid_in("Element uid=e12 has no resolvable backend node."), Some("e12"));
        assert_eq!(uid_in("no uid here"), None);
        assert_eq!(uid_in("trailing uid="), None);
    }

    /// A lost transport is the one failure where the tool knows least and the reflex costs
    /// most: the command may have landed, and repeating it is a second real action.
    #[test]
    fn a_lost_connection_forbids_the_retry_and_says_what_to_read() {
        for msg in ["dispatcher task exited", "transport closed"] {
            let hint = error_hint(msg, "default").expect("a hint");
            assert!(hint.contains("unknown"), "the fact comes first: {hint}");
            assert!(hint.contains("Do not repeat it blind"), "{hint}");
            assert!(hint.contains("chrome-agent inspect"), "{hint}");
        }
    }

    /// The premise of the old hint was false: this tool launches its own Chrome, so asking
    /// whether Chrome is running sent the reader off to start one by hand.
    #[test]
    fn a_refused_connection_does_not_ask_whether_chrome_is_running() {
        let hint = error_hint("Connection refused", "default").expect("a hint");
        assert!(!hint.contains("Is Chrome running"), "{hint}");
        assert!(hint.contains("chrome-agent close"), "the recovery is one command: {hint}");
        assert!(!hint.contains("136"), "the Chrome 136 branch must not swallow this one: {hint}");
    }

    #[test]
    fn connect_failure_hints_at_chrome_136() {
        // The page-attach failure and the missing-port marker both point the user
        // at the Chrome 136+ default-profile restriction and the --connect workaround.
        for msg in [
            "Failed to connect to page after 8 attempts: Connection refused",
            "DevToolsActivePort file doesn't exist",
        ] {
            let hint = error_hint(msg, "default").expect("connect failure should have a hint");
            assert!(hint.contains("136"), "hint should mention Chrome 136: {hint}");
            assert!(hint.contains("--connect"), "hint should mention --connect: {hint}");
        }
    }

    /// Two failures that used to share one hint: a generated accessibility node with no DOM
    /// element behind it, and a CDP reply we could not parse. Different causes, different
    /// recoveries, and the old text named neither.
    #[test]
    fn an_unresolvable_node_and_an_unparseable_reply_do_not_share_a_hint() {
        let node = error_hint("Element uid=e12 has no resolvable backend node.", "default")
            .expect("a hint");
        let parse = error_hint("response parse: invalid type", "default").expect("a hint");
        assert_ne!(node, parse);
        assert!(node.contains("--selector"), "the route past a generated node: {node}");
        assert!(parse.contains("CDP"), "the cause has to be named: {parse}");
        for hint in [&node, &parse] {
            assert!(!hint.contains("Page structure issue"), "{hint}");
        }
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

    /// A missing snapshot has one command and now also the reason it is needed.
    #[test]
    fn the_missing_snapshot_hint_says_why_inspect_is_needed() {
        let hint = error_hint("No snapshot stored for this page", "default").expect("a hint");
        assert!(hint.contains("chrome-agent inspect"), "{hint}");
        assert!(hint.contains("uids"), "no snapshot means no uids: {hint}");
        assert!(hint.contains("baseline"), "and no baseline to compare against: {hint}");
    }
}
