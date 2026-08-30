//! What to do about an error a page, an element or a click produced.
//!
//! Split out of `hints.rs` for the repo's 1000-line file cap and re-exported from [`super`], so
//! every call site stays `crate::hints::error_hint` / `crate::hints::no_download_hint`. The seam
//! is the one the navigation module draws from the other side: everything here happens once a
//! document exists, so it has a uid, an element, a selector or a dispatched event to name.
//!
//! Two families live here, and only one of them is an error:
//!
//! * [`error_hint`], the chain every failed command's message is read by. It is the one function
//!   the project's contract obliges a new error class to touch, which is why the file it lives in
//!   is the one that needed the room.
//! * The hints a *successful* click carries — a download that never began, one Chrome cancelled,
//!   one still in flight, and a pointer action `--on-intercept` refused to dispatch. None of them
//!   is reachable through [`error_hint`]: the command answered `ok:true` and the hint rides on the
//!   response beside a field that says what did not happen. They are held to the same three rules
//!   by the same scans, in [`super`].

use super::{invocation, uid_in};

/// A `download --uid`/`--selector` whose click landed and produced no download.
///
/// Rule 3 carries this one. The click WAS dispatched, so the reflex — click again, maybe it
/// works — is a second real click on a page that cannot tell it from a deliberate one, and on an
/// "Export" button that means two orders, two exports, two of whatever the button does. So the
/// prohibition is in words and comes before the command.
///
/// Rule 2 is satisfied by ONE command, `inspect --urls`, chosen because it answers both questions
/// a caller has here with a single call: what the click did instead (the tree), and whether the
/// element was a plain link whose href can be fetched with no click at all (the URLs). Offering
/// two commands with no criterion is the shrug this contract exists to remove.
#[must_use]
pub fn no_download_hint(
    browser: &str,
    waited_secs: u64,
    dispatched: &crate::hit_test::Dispatched,
) -> String {
    let run = invocation(browser);
    // A hit test that named another receiver explains the silence, and the recovery is that
    // element rather than the download machinery. It is stated first because it is the only
    // branch where the caller's next action is not `inspect`.
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
/// The one branch on this command where a retry is safe, and the hint has to say so: `not_settled`
/// (the aim point was still moving, or outside the viewport) and `off_target` (no point inside the
/// element's own boxes) both stop before the dispatch, so the page never saw an event. Forbidding
/// the retry here would strand the caller on a recoverable refusal — the mirror of the mistake the
/// other three hints avoid.
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

/// A pointer action `--on-intercept refuse` stopped before it dispatched.
///
/// The one error in this module whose fact is a measurement rather than a symptom: the hit test
/// named the element sitting on the aim point, so rule 1 is satisfied by naming it rather than
/// by describing the class of thing it is.
///
/// Rule 3 applies in its second flavour. A retry here is not dangerous — nothing was
/// dispatched, so it cannot double an action — it is *futile*, and an agent that reads "safe to
/// repeat" will repeat it until it runs out of turns. So the prohibition is on repeating the
/// command *while the receiver is still there*, and the command named is the one that leads to
/// getting rid of it.
///
/// Two wordings, because a modal dialog has one dismissal every page agrees on and a banner or
/// scrim does not — the criterion is the receiver's own `modal` flag, which rule 2 requires
/// stating rather than offering two commands and a shrug. `mode` picks a third wording within
/// the non-modal case: `Guard` refused because the receiver looked like a control, `Refuse`
/// because the caller asked for every interception to stop — the fact each names is different,
/// so the words are too.
#[must_use]
pub fn intercepted_refusal_hint(
    browser: &str,
    receiver: Option<&crate::hit_test::Hit>,
    mode: crate::hit_test::OnIntercept,
) -> String {
    let run = invocation(browser);
    let who = receiver.map_or_else(|| "Another element".to_string(), crate::hit_test::Hit::describe);
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
/// One text, two routes, and until this release it had none. The message it was written for —
/// `Element uid=e12 has no resolvable backend node.` — cannot currently be produced: all five
/// sites that build it first do `uid_map.get(uid).ok_or_else(… "not found" …)`, and an `e…` uid
/// is never in the map, so the "not found" arm claims every one of them. `ElementRef` also has a
/// single variant today, so `backend_node_id()` never answers `None` for a uid that IS in the
/// map. The branch stays — it is the guard for the day that enum grows — and the text is now
/// also reached the way a caller actually gets here.
///
/// That route matters because `e…` uids are printed to the reader (`snapshot_render.rs`), so
/// `click e12` is an ordinary thing to try. It used to answer "Run 'chrome-agent inspect' to get
/// fresh uids", which is false advice for this uid in particular: the `e{n}` counter is
/// positional, so re-inspecting renumbers rather than repairs, and the caller would run the
/// command, get a different `e…`, and be exactly as stuck. What is true is that the node has no
/// DOM element, and the way past it is its DOM owner under `--selector`.
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
    // First in the chain, and the position is the point. Every other branch below recognises a
    // sentence this binary wrote; this one recognises a fixed prefix (`CdpClientError::
    // ResponseParse` is `#[error("response parse: {0}")]`) followed by text from serde, which
    // in turn quotes the CDP payload that failed to parse. So the tail of this message is
    // whatever Chrome happened to send, and any content-based predicate below could claim it —
    // a payload carrying the word "timeout" would be read as a timeout, a payload carrying
    // "uid=" and "not found" as a stale uid. Matching first costs nothing, because
    // `response parse` is a string no other failure in this binary produces.
    if msg.contains("response parse") {
        // "so the command never ran" is what this said, and it was false for a whole family:
        // an acknowledgement we cannot read is still an acknowledgement, so a call that ACTED
        // — an input event, a `Runtime.callFunctionOn` that writes a value — was dispatched
        // and answered before the parse failed. Telling the caller it never ran is the exact
        // invitation rule 3 forbids, on the one page that cannot tell a retry from a second
        // deliberate action.
        //
        // Deliberately NOT split into two texts the way `cdp::client::timeout_message` splits
        // its own. That split is made at the site that knows the CDP method and is read back
        // out of the WORDING, which is why `INPUT_ACK_DEADLINE`'s message says "dispatched"
        // first. `ResponseParse` carries a `serde_json::Error` and nothing else, so by the time
        // the message reaches here the method is gone. Keying an `Input.` branch off a wording
        // this binary does not produce would add exactly the unreachable branch the corpus scan
        // in `super` exists to catch. If the error ever names the method, the split belongs
        // here and should copy that function's shape rather than invent a third.
        //
        // So the criterion rule 2 asks for is stated in terms the caller can evaluate without
        // it: they know whether the command they ran acts or reads.
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
    // Chrome 136+ refuses CDP on the *default* user profile. chrome-agent launches
    // its own dedicated profile so this only bites when --connect points at a Chrome
    // started on the normal profile. Matched before the generic "Connection refused"
    // branch so the actionable hint wins.
    } else if msg.contains("Failed to connect to page") || msg.contains("DevToolsActivePort") {
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
        // Two failures wear one message. A uid that WAS in a snapshot and is not in this one
        // is stale, and re-inspecting repairs it. A uid beginning with `e` never was: the
        // snapshot printed it and stored nothing, so "get fresh uids" sends the reader round
        // a loop that renumbers instead of resolving. See `anonymous_node_hint`.
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
    } else if msg.contains("No snapshot") || msg.contains("No inspect") || msg.contains("uid_map is empty") {
        // Was "Run 'chrome-agent inspect' first" — the right command and no reason, so a
        // caller who thought a uid should still work had nothing to correct.
        Some(format!(
            "No snapshot is stored for this page, so there are no uids to resolve and no \
             baseline to report a change against. Run `{run} inspect` first: it is what \
             creates both."
        ))
    } else if msg.contains("was dispatched and Chrome did not acknowledge it") {
        // Rule 3, on the one failure in this module where the tool KNOWS the action may have
        // landed. The generic timeout branch below would answer "use --timeout N for slow
        // pages", which is wrong twice over: the budget was not the caller's to raise (an
        // input event has its own, shorter one), and the advice reads as "try again with more
        // patience" about an event the page may have already acted on.
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
        // eval" — two options, no criterion, and the cause never named. The other route to
        // this same text, and the only one a caller reaches today, is the `e…` uid arm above.
        Some(anonymous_node_hint(&run))
    } else if msg.contains("may not have an article") || msg.contains("Readability") {
        Some(format!(
            "Readability found no article on this page. Run `{run} text --selector \"main\"` \
             for the scoped visible text instead."
        ))
    } else if msg.contains("Provide a uid") || msg.contains("Provide --uid") {
        Some("Specify what to target: uid (e.g. n47), --selector \"css\", or --xy x,y".to_string())
    } else if msg.contains("bound frame's isolated world") {
        // Matched before the plain "document.modelContext is undefined" branch below (that
        // text is also a substring of this one): a frame binding changes what the absence
        // proves, from "this page has no WebMCP" to "not visible from here".
        Some(format!(
            "This checked the bound frame's isolated world, where document.modelContext came \
             back undefined — the same blindness `eval` already has for a frame's main-world \
             variables, just hitting a property instead. That is NOT proof this frame has no \
             tools: a polyfill the frame's own script installed on its main-world document is \
             invisible here. Run `{run} frame main` to check the top document instead, or accept \
             that a frame's own WebMCP tools cannot currently be confirmed absent from outside it."
        ))
    } else if msg.contains("document.modelContext is undefined") {
        // Matched before the generic JS-error branch: this is a specific, known cause
        // (`webmcp list`/`webmcp call`'s own guard), not a page bug to debug.
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
        // Native executeTool()'s own error, reachable only through raw `eval` — `webmcp call`
        // resolves the tool object itself and cannot produce this.
        Some(format!(
            "WebMCP's executeTool() requires the actual tool object getTools() returned, not a \
             bare name — this TypeError is what results from passing one directly. Run \
             `{run} webmcp list` to see the registered tools, then `{run} webmcp call` with the \
             name from that list; it resolves the tool object for you before calling executeTool()."
        ))
    } else if msg.contains("executeTool") && (msg.contains("is not valid JSON") || msg.contains("Failed to parse input arguments")) {
        // Also native executeTool()'s own error, also unreachable through `webmcp call`: it
        // always hands executeTool a validated JSON string. The two spellings are two Chromes:
        // a polyfill's own `JSON.parse` throws the first, native WebMCP answers the second.
        Some(format!(
            "executeTool()'s second argument must be a JSON string, not an object — this is what \
             results from passing one directly. Run `{run} webmcp call` instead and give --args \
             a JSON object or string; chrome-agent serializes it before it ever reaches executeTool()."
        ))
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

    /// The uid is in the message; the hint used to print `<uid>` beside it.
    #[test]
    fn the_hint_for_an_unaimable_element_names_the_element() {
        let hint = error_hint("Element uid=n47 has no visible box model.", "default")
            .expect("a hint");
        assert!(hint.contains("chrome-agent scroll n47"), "{hint}");
        // And it states the measurement instead of hedging about it.
        assert!(!hint.contains("may be hidden"), "{hint}");
    }

    /// An input event whose acknowledgement expired is the second failure of that family, and
    /// the generic timeout advice ("use --timeout N for slow pages") is exactly the wrong one:
    /// it reads as "be more patient and try again" about an event the page may have acted on.
    #[test]
    fn an_unacknowledged_input_forbids_the_retry_rather_than_raising_the_budget() {
        let msg = "Input.dispatchMouseEvent was dispatched and Chrome did not acknowledge it \
                   within 8s, so what the page did with it is unknown. The event may already \
                   have reached the page.";
        let hint = error_hint(msg, "agent-7").expect("a hint");
        assert!(hint.contains("Do not repeat the action"), "{hint}");
        assert!(hint.contains("`chrome-agent --browser agent-7 inspect`"), "{hint}");
        assert!(!hint.contains("--timeout N"), "the generic branch must not swallow this: {hint}");
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

    /// The text above had no reachable caller for its whole life: every site that builds
    /// "no resolvable backend node" first fails on "not found", and an `e…` uid is never in
    /// the map. It is also the only text in this repo that explains what an `e…` uid IS —
    /// and those uids are printed to the reader, so `click e12` is an ordinary thing to try.
    ///
    /// The message that reaches it is the ordinary one, so the two routes have to agree; and
    /// the generic stale-uid advice, which is what this used to answer, must not: re-inspecting
    /// renumbers an `e{n}` rather than repairing it.
    #[test]
    fn an_anonymous_uid_is_explained_rather_than_sent_round_a_re_inspect() {
        let printed = "Element uid=e12 not found. Run 'chrome-agent inspect' to get fresh uids.";
        let anonymous = error_hint(printed, "default").expect("a hint");
        assert!(anonymous.contains("no DOM element behind it"), "{anonymous}");
        assert!(anonymous.contains("--selector"), "the route past it: {anonymous}");
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
        assert!(stale.contains("uids change when the document is replaced"), "{stale}");
    }

    /// An acknowledgement this tool could not read is still an acknowledgement.
    ///
    /// The hint used to end "so the command never ran", which is true of a read and false of
    /// everything that acts: the event was dispatched and Chrome answered, and only our parse
    /// of its answer failed. Told the command never ran, an agent clicks again — the one reflex
    /// rule 3 exists to stop, on the one page that cannot tell the second click from the first.
    #[test]
    fn an_unreadable_acknowledgement_does_not_claim_the_command_never_ran() {
        let hint = error_hint("response parse: invalid type", "default").expect("a hint");
        assert!(!hint.contains("never ran"), "the false claim survived: {hint}");
        assert!(hint.contains("Do not repeat it"), "rule 3, in words: {hint}");
        assert!(hint.contains("dispatched and answered"), "the fact it got wrong: {hint}");
        // Rule 2 allows two routes when the criterion that chooses is stated, and the caller
        // can evaluate this one without knowing the CDP method the message has already lost.
        assert!(hint.contains("If the command acts on the page"), "{hint}");
        assert!(hint.contains("If the command only reads"), "{hint}");
        assert!(hint.contains("chrome-agent inspect"), "{hint}");
        assert!(hint.contains("chrome-agent status"), "{hint}");
    }

    /// This message's tail is serde's, and serde's quotes the CDP payload that would not parse —
    /// so it is the one message in the chain carrying text nobody here wrote. Every branch below
    /// matches on content, so a payload holding the right word would be claimed by the wrong
    /// one. Matching it first is what makes that impossible, and the decoys are what pin it.
    #[test]
    fn a_payload_quoted_into_a_parse_failure_cannot_be_read_as_another_failure() {
        let parse = error_hint("response parse: invalid type", "default").expect("a hint");
        for payload in [
            // Would have hit the `Timeout`/`timeout` branch: the flagged collision.
            "response parse: invalid type: string \"timeout\", expected u64 at line 1 column 42",
            // And the same shape aimed at three other branches below it.
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

    /// The four `WebMCP` branches all name their specific cause, and none of them falls through
    /// to the generic "JS error in page context" catch-all that would otherwise claim them —
    /// every `WebMCP` error is also an `Evaluation error`/`TypeError`/`SyntaxError`.
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
        assert!(no_context.contains("--enable-features=WebMCP"), "{no_context}");

        let unknown_tool = error_hint(
            "Evaluation error: Error: chrome-agent: no WebMCP tool named \"foo\". Known tools: bar.",
            "default",
        )
        .unwrap();
        assert_ne!(unknown_tool, generic);
        assert!(unknown_tool.contains("chrome-agent webmcp list"), "{unknown_tool}");

        let bare_name = error_hint(
            "Evaluation error: TypeError: The provided value is not of type 'RegisteredTool'.",
            "default",
        )
        .unwrap();
        assert_ne!(bare_name, generic);
        assert!(bare_name.contains("chrome-agent webmcp call"), "{bare_name}");

        let object_args = error_hint(
            "Evaluation error: SyntaxError: \"[object Object]\" is not valid JSON\n    at executeTool (x)",
            "default",
        )
        .unwrap();
        assert_ne!(object_args, generic);
        assert!(object_args.contains("--args"), "{object_args}");

        // Native WebMCP spells the same complaint differently — `commands::webmcp` records the
        // fragment "Failed to parse input arguments" and not Chromium's sentence around it — and
        // that spelling had no test at all until the scan next door found it uncovered.
        let native_args = error_hint(
            "Evaluation error: Failed to parse input arguments (executeTool)",
            "default",
        )
        .unwrap();
        assert_eq!(native_args, object_args, "one cause, one hint: {native_args}");
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
        assert!(!frame.contains("--chrome-arg"), "a frame binding is not a launch-flag problem: {frame}");
    }

    /// A missing snapshot has one command and now also the reason it is needed.
    #[test]
    fn the_missing_snapshot_hint_says_why_inspect_is_needed() {
        let hint = error_hint("No snapshot stored for this page", "default").expect("a hint");
        assert!(hint.contains("chrome-agent inspect"), "{hint}");
        assert!(hint.contains("uids"), "no snapshot means no uids: {hint}");
        assert!(hint.contains("baseline"), "and no baseline to compare against: {hint}");
    }

    /// A message this module does not recognise gets no hint, rather than a generic one.
    #[test]
    fn an_unrecognised_message_gets_no_hint() {
        assert!(error_hint("something random", "default").is_none());
    }

    /// Rule 3 is the whole point of the download set: the click reached the page, so the
    /// reflex — click again, it probably failed — is a second real click. Every hint on a
    /// DISPATCHED click has to forbid it in words, and the one on a click that was never
    /// dispatched must not.
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
        // Nothing was dispatched here, so the page never saw an event and aiming again is safe —
        // saying otherwise would strand the caller on a recoverable refusal.
        let safe = undispatched_download_hint("default");
        assert!(safe.contains("may be repeated"), "{safe}");
        assert!(!safe.contains("Do not click again"), "{safe}");
        // Raising the wait is not clicking again, and the hint has to say which one it means.
        let unfinished = download_unfinished_hint("default");
        assert!(unfinished.contains("raise the wait instead of clicking again"), "{unfinished}");
    }

    /// Rule 1: when the hit test already measured why nothing downloaded, the hint states it
    /// rather than sending the caller to `inspect` for a fact this response holds.
    #[test]
    fn an_intercepted_click_names_its_receiver_in_the_hint() {
        let plain = no_download_hint("default", 5, &crate::hit_test::Dispatched::js());
        assert!(!plain.contains("occupied the point"), "{plain}");
        assert!(plain.contains("5s"), "the window is the fact: {plain}");
    }

    /// The refusal hint holds the same three rules as every other, on the one error whose fact
    /// is a measurement: the receiver was named by the hit test, so the hint names it too.
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
        let hint = intercepted_refusal_hint("agent-7", Some(&receiver), crate::hit_test::OnIntercept::Refuse);
        assert!(hint.starts_with("div#gdpr-wall.wall"), "rule 1, the fact first: {hint}");
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

    /// A modal has one dismissal every page agrees on; a banner does not. Two wordings, and
    /// the criterion that picks between them is the receiver's own flag, not the reader's guess.
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
        assert_ne!(modal, intercepted_refusal_hint("default", Some(&dialog), refuse));
        // And with nothing to name, it is still a hint rather than a silence.
        let anonymous = intercepted_refusal_hint("default", None, refuse);
        assert!(anonymous.starts_with("Another element"), "{anonymous}");
        assert!(anonymous.contains("chrome-agent inspect"), "{anonymous}");
    }

    /// `guard` refused for a different, measured reason than a caller who asked for `refuse`
    /// outright — the words say which, or an agent cannot tell "the caller chose this" from
    /// "the tool judged the receiver a control" from the response alone.
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
