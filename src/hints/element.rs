//! What to do about an error a page, an element or a click produced.
//!
//! Everything here happens once a document exists, so it has a uid, an element, a selector or a
//! dispatched event to name. Two families: [`error_hint`], the chain every failed command's
//! message is read by, and the hints a *successful* click carries, which ride on an `ok:true`
//! response and so never reach it. Both are held to the three rules by the scans in [`super`].

use super::{invocation, uid_in};

/// A `download --uid`/`--selector` whose click landed and produced no download. The click was
/// dispatched, so rule 3 forbids a second one first; `inspect --urls` then answers both what the
/// click did and whether the element is a plain link whose href needs no click.
#[must_use]
pub fn no_download_hint(
    browser: &str,
    waited_secs: u64,
    dispatched: &crate::hit_test::Dispatched,
) -> String {
    let run = invocation(browser);
    // A named receiver explains the silence and is stated first: the recovery is that element,
    // not the download machinery.
    let cause = match dispatched.receiver.as_ref() {
        Some(receiver) => format!(
            "The click was delivered, but {} occupied the point it was aimed at, so that element \
             received it and the element you named never did. ",
            receiver.describe()
        ),
        None => String::new(),
    };
    format!(
        "{cause}Chrome reported no download beginning in the {waited_secs}s after the click, so \
         nothing was written to disk. Do not click again: the first click reached the page, and \
         the page has no way to tell a retry from a second deliberate action — on an export or a \
         purchase that is two of them. Run `{run} inspect --urls` to see what the click did \
         instead and whether the element is a plain link; when it is, its href downloads with no \
         click at all."
    )
}

/// A `download --uid`/`--selector` where the hit test refused to aim, so nothing was dispatched.
///
/// The one branch here where a retry is safe, and the hint says so: forbidding it would strand
/// the caller on a recoverable refusal.
#[must_use]
pub fn undispatched_download_hint(browser: &str) -> String {
    let run = invocation(browser);
    format!(
        "No mouse event was dispatched, so the page is exactly as it was and this click may be \
         repeated. The aim point was still moving, outside the viewport, or nowhere inside the \
         element's own boxes — an animated scroll and a zero-size element are what that looks \
         like. Run `{run} inspect` to see where the element actually sits, then aim again."
    )
}

/// A click-triggered download this tool cancelled because it went past `--max-bytes`.
#[must_use]
pub fn download_cap_hint(browser: &str, max_bytes: usize) -> String {
    let run = invocation(browser);
    format!(
        "The transfer passed the {max_bytes}-byte ceiling this invocation set, so it was \
         cancelled and the partial file removed. Raise the ceiling and click once more only if \
         you accept a second click on the page: `{run} download --max-bytes {} …` with the same \
         target. Nothing partial was kept.",
        max_bytes.saturating_mul(4)
    )
}

/// A click-triggered download Chrome itself ended.
#[must_use]
pub fn download_cancelled_hint(browser: &str) -> String {
    let run = invocation(browser);
    format!(
        "Chrome ended the transfer before it finished, which is what a server closing the \
         connection, a blocked file type and a revoked blob URL all look like from here. Do not \
         click again blind: the first click landed. Run `{run} console` to read what the page \
         logged while the download was running."
    )
}

/// A click-triggered download still in flight when the window closed.
#[must_use]
pub fn download_unfinished_hint(browser: &str) -> String {
    let run = invocation(browser);
    format!(
        "The transfer was still running when the wait ended, so the bytes on disk were a prefix \
         of the file and were discarded rather than handed back as one. The download itself was \
         real: raise the wait instead of clicking again, with `{run} --timeout 300 download` and \
         the same target."
    )
}

/// A pointer action `--on-intercept refuse` stopped before it dispatched. The retry is futile
/// rather than dangerous, so what is forbidden is repeating the command *while the receiver is
/// there*. `modal` gets the Escape dismissal; `Guard` and `Refuse` name their different reasons.
#[must_use]
pub fn intercepted_refusal_hint(
    browser: &str,
    receiver: Option<&crate::hit_test::Hit>,
    mode: crate::hit_test::OnIntercept,
) -> String {
    let run = invocation(browser);
    let who = receiver.map_or_else(
        || "Another element".to_string(),
        crate::hit_test::Hit::describe,
    );
    if receiver.is_some_and(|hit| hit.modal) {
        return format!(
            "{who} is a modal dialog: it holds the top layer, so every pointer event outside it \
             goes to it, and nothing was dispatched here. Run `{run} press Escape` to close it, \
             then repeat this action — the page saw no event from this command, so the repeat \
             duplicates nothing. Repeating it while the dialog is open produces this same \
             refusal."
        );
    }
    let because = match mode {
        crate::hit_test::OnIntercept::Guard => {
            "--on-intercept guard judged it a control rather than static content"
        }
        _ => "--on-intercept refuse was set",
    };
    format!(
        "{who} occupies the point this action would have been aimed at, and {because}, so \
         nothing was dispatched and the page is exactly as it was. Do not repeat this command \
         while that element is there: it will refuse identically. Run `{run} inspect` to find \
         that element's own dismiss control, act on it, and aim here again once it is gone."
    )
}

/// A uid the snapshot PRINTED and never stored, because there is no DOM element behind it.
///
/// One text, two routes: the `e…` arm of "not found", and the `no resolvable backend node`
/// guard. Re-inspecting only renumbers a positional `e{n}`; the way past is `--selector`.
fn anonymous_node_hint(run: &str) -> String {
    format!(
        "This uid names an accessibility node with no DOM element behind it — the `e…` \
         uids in a snapshot are Chrome's own generated nodes, and they cannot be resolved \
         to something clickable. Run `{run} inspect` and act on a uid beginning with `n`; \
         when the thing you want exists only as a generated node, aim at its DOM owner \
         with --selector instead."
    )
}

/// What to do about `msg`, phrased for the browser this invocation is driving.
#[must_use]
pub fn error_hint(msg: &str, browser: &str) -> Option<String> {
    let run = invocation(browser);
    // Must stay first. Its tail is serde quoting the CDP payload, so it is the one message
    // carrying text this binary did not write, and any content predicate below could claim it.
    if msg.contains("response parse") {
        // An acknowledgement this tool cannot read is still one: a call that ACTED was
        // dispatched and answered before the parse failed. Not split into an acts/reads pair
        // the way `cdp::client::timeout_message` is — `ResponseParse` carries only a
        // `serde_json::Error`, so the CDP method is gone by here; the caller evaluates it.
        Some(format!(
            "Chrome answered this CDP call and this version could not read the answer, so what \
             became of the call is not known from here. If the command acts on the page — a \
             click, a fill, a press, a select — it was dispatched and answered before the parse \
             failed, and the effect is real whether or not this tool could read the receipt. Do \
             not repeat it: the page cannot tell a retry from a second deliberate action, and on \
             a submit or a purchase that is two of them. Run `{run} inspect` and read the state \
             the command was supposed to produce. If the command only reads, nothing was changed \
             and the cause is a version gap: `{run} status` names the browser this session \
             recorded, and a Chrome much newer or older than the bundled Chromium is the usual \
             one."
        ))
    // Chrome 136+ refuses CDP on the default user profile, so this bites only under --connect.
    // Matched before the generic "Connection refused" branch so the actionable hint wins.
    } else if msg.contains("Failed to connect to page") || msg.contains("DevToolsActivePort") {
        Some("Could not attach over CDP. Chrome 136+ disables remote debugging on the default profile: drop --connect to let chrome-agent launch its own dedicated profile, or relaunch your Chrome with a separate --user-data-dir.".to_string())
    // The two file-reading failures, before the CDP branch below. Both carry the OS `No such
    // file or directory` in their tail, which that branch used to claim — so `macro run` on an
    // unknown name was answered with "the browser it named is gone", contradicting its own
    // message. Neither reached a browser, so both are recognised by their own prefix instead.
    } else if msg.contains("No macro named") {
        Some(format!(
            "Nothing was opened and nothing ran, so the page is exactly as it was: that name is \
             not in this machine's macro store. Run `{run} macro list` for the names that are."
        ))
    } else if msg.contains("Cannot read replay file") {
        let path = super::single_quoted(msg).unwrap_or("the path");
        Some(format!(
            "Nothing was read, so no command in the file ran and the page is exactly as it was. \
             A replay file is the JSONL transcript `--record` writes, one JSON command per line. \
             Run `ls -l {path}` to see whether it exists and is readable, then replay a path that \
             does."
        ))
    } else if msg.contains("Connection refused") {
        Some(format!(
            "Nothing answered CDP at the endpoint this session recorded, so the browser it \
             named is gone — crashed, or killed from outside this tool. chrome-agent starts \
             its own Chrome, so nothing has to be launched by hand: run `{run} close` to drop \
             the dead session entry, and the next command opens a fresh browser. If you \
             passed --connect, that port has no listener and the Chrome you meant to attach \
             to is the thing to start."
        ))
    } else if msg.contains("uid=") && msg.contains("not found") {
        // Two failures wear one message. A stale uid is repaired by re-inspecting; an `e…` uid
        // was never stored, and re-inspecting only renumbers it. See `anonymous_node_hint`.
        if uid_in(msg).is_some_and(|uid| uid.starts_with('e')) {
            Some(anonymous_node_hint(&run))
        } else {
            Some(format!(
                "That uid is not in this page's snapshot: uids change when the document is \
                 replaced, and `goto` clears the map on purpose. Run `{run} inspect` and act on \
                 a uid from its output."
            ))
        }
    } else if msg.contains("Navigation failed") {
        Some(super::navigation::navigation_failure(msg, &run))
    // `No previous snapshot` is what `run` and `pipe_dispatch` actually emit for `diff`; it does
    // not contain `No snapshot`, so matching only that spelling left the message hintless.
    } else if msg.contains("No snapshot")
        || msg.contains("No previous snapshot")
        || msg.contains("No inspect")
        || msg.contains("uid_map is empty")
    {
        Some(format!(
            "No snapshot is stored for this page, so there are no uids to resolve and no \
             baseline to report a change against. Run `{run} inspect` first: it is what \
             creates both."
        ))
    } else if msg.contains("was dispatched and Chrome did not acknowledge it") {
        // Must precede the generic timeout branch: its "use --timeout N" is wrong twice here —
        // an input event has its own shorter budget, and the page may already have acted.
        Some(format!(
            "The event left this tool and Chrome never confirmed what became of it, so the \
             page may have received it and may not. Do not repeat the action: the page cannot \
             tell a retry from a second deliberate click, and on a submit or a purchase that is \
             two of them. Run `{run} inspect` and read the state the action was supposed to \
             produce; act on what you see rather than on what was intended. A pointer event \
             answers in milliseconds on a healthy page — a page that is mid-navigation, or a \
             renderer that has stopped answering, is what this looks like."
        ))
    } else if msg.contains("Timeout") || msg.contains("timeout") {
        Some("Use --timeout N for slow pages".to_string())
    } else if msg.contains("not interactable") || msg.contains("no visible box model") {
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
        // The guard route to the same text; the `e…` uid arm above is the reachable one.
        Some(anonymous_node_hint(&run))
    } else if msg.contains("may not have an article") || msg.contains("Readability") {
        Some(format!(
            "Readability found no article on this page. Run `{run} text --selector \"main\"` \
             for the scoped visible text instead."
        ))
    } else if msg.contains("Provide a uid") || msg.contains("Provide --uid") {
        Some("Specify what to target: uid (e.g. n47), --selector \"css\", or --xy x,y".to_string())
    } else if msg.contains("bound frame's isolated world") {
        // Before the plain-absence branch below, whose text is a substring of this one: a frame
        // binding downgrades the absence from "no WebMCP here" to "not visible from here".
        Some(format!(
            "This checked the bound frame's isolated world, where document.modelContext came \
             back undefined — the same blindness `eval` already has for a frame's main-world \
             variables, just hitting a property instead. That is NOT proof this frame has no \
             tools: a polyfill the frame's own script installed on its main-world document is \
             invisible here. Run `{run} frame main` to check the top document instead, or accept \
             that a frame's own WebMCP tools cannot currently be confirmed absent from outside it."
        ))
    } else if msg.contains("document.modelContext is undefined") {
        // Before the generic JS-error branch: a known cause, not a page bug to debug.
        Some(format!(
            "This page's document.modelContext is undefined, so no WebMCP tool can be listed \
             or called here. Either this browser was not launched with --chrome-arg \
             --enable-features=WebMCP,WebMCPTesting, or the page registers no polyfill for it. \
             --chrome-arg is fixed for the life of a named browser: run `{run} close --purge` \
             and relaunch with the flag, or check the page's own script for a document.modelContext \
             polyfill."
        ))
    } else if msg.contains("no WebMCP tool named") {
        Some(format!(
            "That name matched none of this page's registered tools when getTools() was last \
             checked. Run `{run} webmcp list` to see the names actually registered — a tool can \
             also disappear if the page unregistered it since the last list."
        ))
    } else if msg.contains("not of type 'RegisteredTool'") {
        // Native executeTool()'s own error, reachable only through raw `eval`.
        Some(format!(
            "WebMCP's executeTool() requires the actual tool object getTools() returned, not a \
             bare name — this TypeError is what results from passing one directly. Run \
             `{run} webmcp list` to see the registered tools, then `{run} webmcp call` with the \
             name from that list; it resolves the tool object for you before calling executeTool()."
        ))
    } else if msg.contains("executeTool")
        && (msg.contains("is not valid JSON") || msg.contains("Failed to parse input arguments"))
    {
        // Also unreachable through `webmcp call`, which always passes a validated JSON string.
        // Two spellings: a polyfill's `JSON.parse` throws the first, native WebMCP the second.
        Some(format!(
            "executeTool()'s second argument must be a JSON string, not an object — this is what \
             results from passing one directly. Run `{run} webmcp call` instead and give --args \
             a JSON object or string; chrome-agent serializes it before it ever reaches executeTool()."
        ))
    } else if msg.contains("Evaluation error")
        || msg.contains("TypeError")
        || msg.contains("ReferenceError")
        || msg.contains("SyntaxError")
    {
        Some("JS error in page context. Check expression syntax. Use --selector to scope to an element.".to_string())
    } else if msg.contains("CDP message exceeded") {
        // Before the lost-transport branch: the connection did end, but the size is the cause
        // and "the connection dropped" would send the reader looking for a dead browser.
        Some(format!(
            "Chrome's answer was larger than the ceiling this connection reads, so it was not \
             read and the connection ended. That ceiling is this tool's own bound on how much \
             one reply may allocate here, not a failure of the page or of the browser, which is \
             still running. Do not repeat the command blind: it reached Chrome and Chrome \
             answered, so anything it changed on the page is already changed. Ask for less \
             instead — when the answer was a document, `{run} text --selector \"main\"` returns \
             one region rather than the whole page; when it was a file, downloading it by click \
             (download --uid or --selector) streams it to disk through Chrome instead of \
             carrying it over CDP."
        ))
    } else if msg.contains("dispatcher task exited") || msg.contains("transport closed") {
        Some(format!(
            "The connection to Chrome dropped, so whether this command reached the page is \
             unknown. Do not repeat it blind: a click or a fill that was already delivered \
             becomes a second real one, and the page has no way to tell them apart. Run \
             `{run} inspect` and read the state the action was supposed to produce before \
             deciding. In pipe mode the session is over — the browser and its page survive it, \
             so start a new one."
        ))
    } else if msg.contains("not an <iframe>") || msg.contains("not an <IFRAME>") {
        Some(
            "Only <iframe> is supported. For <frame>/<frameset>, use eval to access frame content."
                .to_string(),
        )
    } else if msg.contains("No child frame found") {
        Some("Iframe not found. Check the selector matches an <iframe> element.".to_string())
    } else if msg.contains("not a <select>") {
        Some(
            "Element is not a <select>. For custom dropdowns, click to open then click the option."
                .to_string(),
        )
    } else if msg.contains("No option matching") {
        Some("No dropdown option matched. Use inspect --uid to check available options, or try the visible text.".to_string())
    } else if msg.contains("File not found") {
        Some("Check the file path exists on disk.".to_string())
    } else if msg.contains("invalid regular expression") {
        Some("--matches takes a Rust regex (regex-lite): \\d \\w \\s are ASCII-only, and there is no \\p{...} or lookaround. For a plain substring use --contains.".to_string())
    } else if msg.contains("expected a JSON array") {
        Some(
            "Batch expects a JSON array of commands on stdin: [{\"cmd\":\"inspect\"}, ...]"
                .to_string(),
        )
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The uid is in the message, so the hint substitutes it instead of printing `<uid>`.
    #[test]
    fn the_hint_for_an_unaimable_element_names_the_element() {
        let hint =
            error_hint("Element uid=n47 has no visible box model.", "default").expect("a hint");
        assert!(hint.contains("chrome-agent scroll n47"), "{hint}");
        // Rule 1: state the measurement, do not hedge about it.
        assert!(!hint.contains("may be hidden"), "{hint}");
    }

    /// The generic timeout advice would read as "try again" about an event the page may have
    /// acted on, so this branch forbids the retry instead.
    #[test]
    fn an_unacknowledged_input_forbids_the_retry_rather_than_raising_the_budget() {
        let msg = "Input.dispatchMouseEvent was dispatched and Chrome did not acknowledge it \
                   within 8s, so what the page did with it is unknown. The event may already \
                   have reached the page.";
        let hint = error_hint(msg, "agent-7").expect("a hint");
        assert!(hint.contains("Do not repeat the action"), "{hint}");
        assert!(
            hint.contains("`chrome-agent --browser agent-7 inspect`"),
            "{hint}"
        );
        assert!(
            !hint.contains("--timeout N"),
            "the generic branch must not swallow this: {hint}"
        );
    }

    /// A reply past the wire ceiling is a size, and used to arrive as `transport closed` —
    /// which reads as a dead browser and sends the reader to `close`, on a browser that is
    /// running fine.
    #[test]
    fn an_oversized_reply_is_not_read_as_a_dropped_connection() {
        let oversize = error_hint(
            "transport: a CDP message exceeded the 100663296-byte ceiling this connection reads",
            "agent-7",
        )
        .expect("a hint");
        let dropped = error_hint("transport closed", "agent-7").expect("a hint");
        assert_ne!(oversize, dropped);
        assert!(
            oversize.contains("larger than the ceiling"),
            "the fact: {oversize}"
        );
        assert!(oversize.contains("still running"), "{oversize}");
        assert!(
            !oversize.contains("start a new one"),
            "the dropped-connection advice: {oversize}"
        );
        assert!(
            oversize.contains("`chrome-agent --browser agent-7 text --selector \"main\"`"),
            "one command, on this browser: {oversize}"
        );
    }

    /// A lost transport may have landed the command, so repeating it is a second real action.
    #[test]
    fn a_lost_connection_forbids_the_retry_and_says_what_to_read() {
        for msg in ["dispatcher task exited", "transport closed"] {
            let hint = error_hint(msg, "default").expect("a hint");
            assert!(hint.contains("unknown"), "the fact comes first: {hint}");
            assert!(hint.contains("Do not repeat it blind"), "{hint}");
            assert!(hint.contains("chrome-agent inspect"), "{hint}");
        }
    }

    /// Rule 1: this tool launches its own Chrome, so it never asks whether Chrome is running.
    #[test]
    fn a_refused_connection_does_not_ask_whether_chrome_is_running() {
        let hint = error_hint("Connection refused", "default").expect("a hint");
        assert!(!hint.contains("Is Chrome running"), "{hint}");
        assert!(
            hint.contains("chrome-agent close"),
            "the recovery is one command: {hint}"
        );
        assert!(
            !hint.contains("136"),
            "the Chrome 136 branch must not swallow this one: {hint}"
        );
    }

    #[test]
    fn connect_failure_hints_at_chrome_136() {
        // Both spellings point at the Chrome 136+ default-profile restriction.
        for msg in [
            "Failed to connect to page after 8 attempts: Connection refused",
            "DevToolsActivePort file doesn't exist",
        ] {
            let hint = error_hint(msg, "default").expect("connect failure should have a hint");
            assert!(
                hint.contains("136"),
                "hint should mention Chrome 136: {hint}"
            );
            assert!(
                hint.contains("--connect"),
                "hint should mention --connect: {hint}"
            );
        }
    }

    /// A generated node with no DOM element and an unparseable CDP reply are different causes
    /// with different recoveries, so they get different hints.
    #[test]
    fn an_unresolvable_node_and_an_unparseable_reply_do_not_share_a_hint() {
        let node = error_hint("Element uid=e12 has no resolvable backend node.", "default")
            .expect("a hint");
        let parse = error_hint("response parse: invalid type", "default").expect("a hint");
        assert_ne!(node, parse);
        assert!(
            node.contains("--selector"),
            "the route past a generated node: {node}"
        );
        assert!(parse.contains("CDP"), "the cause has to be named: {parse}");
        for hint in [&node, &parse] {
            assert!(!hint.contains("Page structure issue"), "{hint}");
        }
    }

    /// `e…` uids are printed to the reader, so `click e12` is an ordinary thing to try. Both
    /// routes to the explanation must agree, and neither may give the stale-uid advice:
    /// re-inspecting renumbers an `e{n}` rather than repairing it.
    #[test]
    fn an_anonymous_uid_is_explained_rather_than_sent_round_a_re_inspect() {
        let printed = "Element uid=e12 not found. Run 'chrome-agent inspect' to get fresh uids.";
        let anonymous = error_hint(printed, "default").expect("a hint");
        assert!(
            anonymous.contains("no DOM element behind it"),
            "{anonymous}"
        );
        assert!(
            anonymous.contains("--selector"),
            "the route past it: {anonymous}"
        );
        assert!(
            !anonymous.contains("uids change when the document is replaced"),
            "an `e…` uid is not a stale uid: {anonymous}"
        );
        // One text, two routes, and they may not drift apart.
        assert_eq!(
            anonymous,
            error_hint("Element uid=e12 has no resolvable backend node.", "default")
                .expect("a hint")
        );
        // A stale `n…` uid keeps the advice that does repair it.
        let stale = error_hint(
            "Element uid=n5 not found. Run 'chrome-agent inspect' to get fresh uids.",
            "default",
        )
        .expect("a hint");
        assert_ne!(stale, anonymous);
        assert!(
            stale.contains("uids change when the document is replaced"),
            "{stale}"
        );
    }

    /// An acknowledgement this tool could not read is still an acknowledgement: for anything
    /// that acts, "the command never ran" is false and invites the retry rule 3 forbids.
    #[test]
    fn an_unreadable_acknowledgement_does_not_claim_the_command_never_ran() {
        let hint = error_hint("response parse: invalid type", "default").expect("a hint");
        assert!(
            !hint.contains("never ran"),
            "the false claim survived: {hint}"
        );
        assert!(
            hint.contains("Do not repeat it"),
            "rule 3, in words: {hint}"
        );
        assert!(
            hint.contains("dispatched and answered"),
            "the fact it got wrong: {hint}"
        );
        // Rule 2 allows two routes when the criterion is stated, and the caller can evaluate
        // this one without the CDP method the message has already lost.
        assert!(hint.contains("If the command acts on the page"), "{hint}");
        assert!(hint.contains("If the command only reads"), "{hint}");
        assert!(hint.contains("chrome-agent inspect"), "{hint}");
        assert!(hint.contains("chrome-agent status"), "{hint}");
    }

    /// The quoted CDP payload can carry any word a later branch matches on. Matching this
    /// branch first is what stops that; the decoys below pin it.
    #[test]
    fn a_payload_quoted_into_a_parse_failure_cannot_be_read_as_another_failure() {
        let parse = error_hint("response parse: invalid type", "default").expect("a hint");
        for payload in [
            // Aimed at the `Timeout`/`timeout` branch.
            "response parse: invalid type: string \"timeout\", expected u64 at line 1 column 42",
            // The same shape aimed at three other branches below it.
            "response parse: missing field `result`, payload was {\"error\":\"uid=n5 not found\"}",
            "response parse: invalid value, payload was {\"message\":\"Navigation failed\"}",
            "response parse: invalid type, payload was {\"detail\":\"Connection refused\"}",
        ] {
            assert_eq!(
                error_hint(payload, "default").expect("a hint"),
                parse,
                "a quoted payload was read as the failure it names: {payload}"
            );
        }
    }

    /// Every `WebMCP` error is also an `Evaluation error`/`TypeError`/`SyntaxError`, so each
    /// branch must name its own cause rather than fall through to the generic JS-error hint.
    #[test]
    fn webmcp_errors_do_not_fall_through_to_the_generic_js_error_hint() {
        let generic = "JS error in page context. Check expression syntax. Use --selector to scope to an element.";

        let no_context = error_hint(
            "Evaluation error: Error: chrome-agent: document.modelContext is undefined on this page.",
            "default",
        )
        .unwrap();
        assert_ne!(no_context, generic);
        assert!(no_context.contains("--chrome-arg"), "{no_context}");
        assert!(
            no_context.contains("--enable-features=WebMCP"),
            "{no_context}"
        );

        let unknown_tool = error_hint(
            "Evaluation error: Error: chrome-agent: no WebMCP tool named \"foo\". Known tools: bar.",
            "default",
        )
        .unwrap();
        assert_ne!(unknown_tool, generic);
        assert!(
            unknown_tool.contains("chrome-agent webmcp list"),
            "{unknown_tool}"
        );

        let bare_name = error_hint(
            "Evaluation error: TypeError: The provided value is not of type 'RegisteredTool'.",
            "default",
        )
        .unwrap();
        assert_ne!(bare_name, generic);
        assert!(
            bare_name.contains("chrome-agent webmcp call"),
            "{bare_name}"
        );

        let object_args = error_hint(
            "Evaluation error: SyntaxError: \"[object Object]\" is not valid JSON\n    at executeTool (x)",
            "default",
        )
        .unwrap();
        assert_ne!(object_args, generic);
        assert!(object_args.contains("--args"), "{object_args}");

        // Native WebMCP spells the same complaint differently; one cause keeps one hint.
        let native_args = error_hint(
            "Evaluation error: Failed to parse input arguments (executeTool)",
            "default",
        )
        .unwrap();
        assert_eq!(
            native_args, object_args,
            "one cause, one hint: {native_args}"
        );
    }

    /// The frame-scoped absence and the plain absence share the substring "document.modelContext
    /// is undefined", so the more specific one has to be checked first or it is unreachable.
    #[test]
    fn a_frame_scoped_absence_is_not_read_as_a_plain_absence() {
        let plain = error_hint(
            "Evaluation error: Error: chrome-agent: document.modelContext is undefined on this page.",
            "default",
        )
        .unwrap();
        assert!(plain.contains("--chrome-arg"), "{plain}");
        assert!(!plain.contains("bound frame"), "{plain}");

        let frame = error_hint(
            "Evaluation error: Error: chrome-agent: document.modelContext is undefined in the \
             bound frame's isolated world — this does not prove the frame has no tools, since a \
             polyfill the frame's own main-world script installs is invisible here.",
            "default",
        )
        .unwrap();
        assert_ne!(plain, frame);
        assert!(frame.contains("frame main"), "{frame}");
        assert!(
            !frame.contains("--chrome-arg"),
            "a frame binding is not a launch-flag problem: {frame}"
        );
    }

    /// A file this tool could not open is not a browser that died. The ENOENT tail used to send
    /// both of these to the dead-browser hint, which told a `macro run` to `close` its browser —
    /// contradicting the message's own "`macro list` shows what exists".
    #[test]
    fn a_missing_file_is_not_reported_as_a_dead_browser() {
        let dead = error_hint("Connection refused", "default").expect("a hint");

        let macro_run = error_hint(
            "No macro named 'checkout' (/home/a/.chrome-agent/macros/checkout.json): No such \
             file or directory (os error 2). `macro list` shows what exists.",
            "default",
        )
        .expect("a hint");
        assert_ne!(macro_run, dead);
        assert!(!macro_run.contains("chrome-agent close"), "{macro_run}");
        assert!(macro_run.contains("chrome-agent macro list"), "{macro_run}");
        assert!(
            macro_run.contains("nothing ran"),
            "rule 1, the fact: {macro_run}"
        );

        let replay = error_hint(
            "Cannot read replay file '/tmp/nope.jsonl': No such file or directory (os error 2)",
            "default",
        )
        .expect("a hint");
        assert_ne!(replay, dead);
        assert_ne!(replay, macro_run, "two files, two stores, two recoveries");
        assert!(!replay.contains("chrome-agent close"), "{replay}");
        assert!(
            replay.contains("/tmp/nope.jsonl"),
            "the path is in the message: {replay}"
        );
    }

    /// The message `diff` emits in both modes is `No previous snapshot`, which does not contain
    /// `No snapshot` — so the branch matched nothing and the error travelled hintless.
    #[test]
    fn the_message_diff_actually_emits_reaches_the_missing_snapshot_hint() {
        let canonical = error_hint("No snapshot stored for this page", "default").expect("a hint");
        for emitted in [
            "No previous snapshot. Run 'chrome-agent inspect' first.",
            "No previous snapshot. Run inspect first.",
        ] {
            assert_eq!(
                error_hint(emitted, "default").expect("a hint"),
                canonical,
                "{emitted}"
            );
        }
    }

    /// A missing snapshot gets one command and the reason it is needed.
    #[test]
    fn the_missing_snapshot_hint_says_why_inspect_is_needed() {
        let hint = error_hint("No snapshot stored for this page", "default").expect("a hint");
        assert!(hint.contains("chrome-agent inspect"), "{hint}");
        assert!(hint.contains("uids"), "no snapshot means no uids: {hint}");
        assert!(
            hint.contains("baseline"),
            "and no baseline to compare against: {hint}"
        );
    }

    /// A message this module does not recognise gets no hint, rather than a generic one.
    #[test]
    fn an_unrecognised_message_gets_no_hint() {
        assert!(error_hint("something random", "default").is_none());
    }

    /// Every hint on a DISPATCHED click forbids the retry in words; the one on a click that was
    /// never dispatched must not.
    #[test]
    fn a_delivered_click_forbids_the_second_one_and_an_undelivered_one_permits_it() {
        let dispatched = [
            no_download_hint("default", 30, &crate::hit_test::Dispatched::js()),
            download_cancelled_hint("default"),
        ];
        for hint in dispatched {
            assert!(
                hint.contains("Do not click again") || hint.contains("do not click again"),
                "a delivered click must forbid the retry: {hint}"
            );
        }
        // Nothing was dispatched, so aiming again is safe and the hint has to say so.
        let safe = undispatched_download_hint("default");
        assert!(safe.contains("may be repeated"), "{safe}");
        assert!(!safe.contains("Do not click again"), "{safe}");
        // Raising the wait is not clicking again, and the hint has to say which one it means.
        let unfinished = download_unfinished_hint("default");
        assert!(
            unfinished.contains("raise the wait instead of clicking again"),
            "{unfinished}"
        );
    }

    /// Rule 1: with no measured receiver, the hint states the wait window and claims nothing more.
    #[test]
    fn an_intercepted_click_names_its_receiver_in_the_hint() {
        let plain = no_download_hint("default", 5, &crate::hit_test::Dispatched::js());
        assert!(!plain.contains("occupied the point"), "{plain}");
        assert!(plain.contains("5s"), "the window is the fact: {plain}");
    }

    /// The hit test named the receiver, so the hint names it too — and runs against this
    /// invocation's browser.
    #[test]
    fn the_refusal_hint_names_the_receiver_and_this_browser() {
        let receiver = crate::hit_test::Hit {
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
        let hint = intercepted_refusal_hint(
            "agent-7",
            Some(&receiver),
            crate::hit_test::OnIntercept::Refuse,
        );
        assert!(
            hint.starts_with("div#gdpr-wall.wall"),
            "rule 1, the fact first: {hint}"
        );
        assert!(
            hint.contains("`chrome-agent --browser agent-7 inspect`"),
            "rule 2, one command, on this invocation's browser: {hint}"
        );
        assert!(
            hint.contains("Do not repeat this command while that element is there"),
            "rule 3: the retry here is futile rather than dangerous, and has to be refused in \
             words or an agent will spin on it: {hint}"
        );
        for placeholder in ["<uid>", "<selector>", "<name>"] {
            assert!(!hint.contains(placeholder), "{hint}");
        }
    }

    /// A modal has one dismissal every page agrees on; a banner does not. The receiver's own
    /// `modal` flag picks between the two wordings.
    #[test]
    fn a_modal_receiver_gets_the_dismissal_a_modal_actually_has() {
        let mut dialog = crate::hit_test::Hit {
            tag: "DIALOG".into(),
            id: Some("terms".into()),
            cls: None,
            z: None,
            text: "Terms".into(),
            modal: true,
            iframe: false,
            same_doc: true,
            actionable: true,
            uid: None,
        };
        let refuse = crate::hit_test::OnIntercept::Refuse;
        let modal = intercepted_refusal_hint("default", Some(&dialog), refuse);
        assert!(modal.contains("`chrome-agent press Escape`"), "{modal}");
        dialog.modal = false;
        assert_ne!(
            modal,
            intercepted_refusal_hint("default", Some(&dialog), refuse)
        );
        // With nothing to name, it is still a hint rather than a silence.
        let anonymous = intercepted_refusal_hint("default", None, refuse);
        assert!(anonymous.starts_with("Another element"), "{anonymous}");
        assert!(anonymous.contains("chrome-agent inspect"), "{anonymous}");
    }

    /// `guard` refused for a measured reason and `refuse` because the caller asked. The words
    /// say which, or the response alone cannot tell them apart.
    #[test]
    fn a_guard_refusal_names_its_own_reason_not_the_callers_choice() {
        let receiver = crate::hit_test::Hit {
            tag: "BUTTON".into(),
            id: None,
            cls: Some("Cmp__action Cmp__action--yes".into()),
            z: None,
            text: "oui, j'accepte".into(),
            modal: false,
            iframe: false,
            same_doc: true,
            actionable: true,
            uid: Some("n42".into()),
        };
        let guard = intercepted_refusal_hint(
            "default",
            Some(&receiver),
            crate::hit_test::OnIntercept::Guard,
        );
        let refuse = intercepted_refusal_hint(
            "default",
            Some(&receiver),
            crate::hit_test::OnIntercept::Refuse,
        );
        assert_ne!(guard, refuse);
        assert!(guard.contains("judged it a control"), "{guard}");
        assert!(!guard.contains("refuse was set"), "{guard}");
        assert!(refuse.contains("refuse was set"), "{refuse}");
    }
}
