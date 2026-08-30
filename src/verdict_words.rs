//! What a verdict hands on: the sentence a person reads, the token an agent branches on, and the
//! advice each rung owes its caller in both lengths. Four tables, all keyed on the reason.
//!
//! Re-exported from `verdict.rs`, so call sites write `crate::verdict::gloss` and friends.

use std::fmt;

use crate::verdict::{Assessment, PageSight, Verdict};

/// One sentence naming what the verdict word means, for a reader who has never seen the taxonomy.
///
/// The text renderer reads this table rather than writing prose of its own, so the wording cannot
/// drift from the classifier. The per-verdict arm at the bottom is the floor for an untaught token.
#[must_use]
pub fn gloss(assessment: Assessment) -> &'static str {
    match assessment.reason {
        "tree_delta" => "the page moved in a way that can be pointed at",
        "nodes_moved" => "the same nodes, in a different order",
        "focus_only" => "nothing moved but focus, which is the only sign the action arrived",
        // Names its evidence: the one `changed` with no delta to point at, so the claim rests on
        // the read-back. "the element", not "the field" — select and check reach this rung too.
        "value_kept" => "the element held what was asked of it when it was read back, and the \
                         tree could not show it",
        "values_lost" => "the page moved, and a field that held a value now holds none",
        "document_replaced" => "the document was replaced, so every stored uid is dead",
        "hit_test_receiver" => "another element occupied the point aimed at and received the event",
        "modal_dialog" => "a modal dialog holds the top layer and receives every event outside it",
        "value_reverted" => "the element held nothing when it was read back",
        "value_rewritten" => "the element holds something other than what was written",
        "delivered_no_change" => {
            "the event reached the target and the tree stayed still while it was watched"
        }
        // The word an agent most easily over-reads; the gloss carries the limit it cannot.
        "identical_tree" => "the tree was identical while the tool watched — which is not the \
                             same as the action having no effect",
        "no_baseline" => "unverified — there was no earlier snapshot to compare against",
        "read_failed" => "unverified — the action ran and the page could not be read afterwards",
        "identity_unreadable" => {
            "unverified — the two trees may not belong to the same document"
        }
        "scroll_not_settled" => "nothing was dispatched: the aim point was still moving",
        "aim_point_off_target" => {
            "nothing was dispatched: no point on the element could be aimed at, and the \
             reading did not change while it was watched"
        }
        "reporting_disabled" => "not checked — the page was never re-read (--verdict off)",
        _ => match assessment.verdict {
            Verdict::Changed => "the page moved",
            Verdict::Navigated => "the document was replaced",
            Verdict::Intercepted => "another element received the event",
            Verdict::NotKept => "the element does not hold what was written",
            Verdict::NoEffect => "delivered, and nothing moved while it was watched",
            Verdict::Unchanged => "the tree was identical while the tool watched",
            Verdict::Unknown => "unverified",
            Verdict::NotChecked => "not checked",
        },
    }
}

/// What the caller should do next, in one token from a closed set of six. The verdict says what
/// happened; this says what to do about it, without an agent parsing prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Next {
    /// The action did what it was for. Carry on.
    Proceed,
    /// Read the page before acting again: the uids are dead, or nothing is known.
    Inspect,
    /// Repeat this action. Only ever from a rung that proves nothing was dispatched.
    Retry,
    /// Establish the outcome some other way before treating this as done.
    Confirm,
    /// Something is in the way. Deal with it, then act again.
    Dismiss,
    /// Do not repeat this action; it will produce the same answer.
    Stop,
}

impl Next {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proceed => "proceed",
            Self::Inspect => "inspect",
            Self::Retry => "retry",
            Self::Confirm => "confirm",
            Self::Dismiss => "dismiss",
            Self::Stop => "stop",
        }
    }
}

impl fmt::Display for Next {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Map a verdict onto the one thing to do about it. Pure, total, the only place the mapping exists.
///
/// Two rules the arms do not show on their own:
///
/// - `unknown` never yields `retry`: it means the action was not OBSERVED, not that it did not
///   happen, and a blind repeat is a second real click. `scroll_not_settled` is the one exception,
///   because nothing was dispatched.
/// - `proceed` becomes `inspect` whenever the page is `Unreadable`. Written over the token rather
///   than as an arm for `value_kept`, so a later rung cannot inherit "carry on while blind".
///   `--verdict off` is `Readable`, because the caller owns that silence.
#[must_use]
pub fn next_for(assessment: Assessment) -> Next {
    let next = next_when_the_page_was_read(assessment);
    if next == Next::Proceed && assessment.page == PageSight::Unreadable {
        return Next::Inspect;
    }
    next
}

/// The mapping itself, for a response that got to see the page.
// Two pairs of arms share a token and not a reason (`navigated`/`unknown` → inspect,
// `changed`/`not_checked` → proceed). They move independently, so they stay separate.
#[allow(clippy::match_same_arms)]
fn next_when_the_page_was_read(assessment: Assessment) -> Next {
    match assessment.reason {
        "scroll_not_settled" => Next::Retry,
        // A form that cleared itself on a successful submit and one that threw the input away
        // look identical here, so `proceed` would be a false success.
        "values_lost" => Next::Confirm,
        _ => match assessment.verdict {
            Verdict::Changed => Next::Proceed,
            // Nothing is wrong; nothing stored is usable either.
            Verdict::Navigated => Next::Inspect,
            // Neither a failure nor proof of success: everything the tool cannot see (canvas,
            // CSS, a late handler) looks like this. Confirm elsewhere rather than repeat.
            Verdict::Unchanged | Verdict::NoEffect => Next::Confirm,
            Verdict::Intercepted => Next::Dismiss,
            // The same write produces the same answer, so a repeat only edits the page again.
            Verdict::NotKept => Next::Stop,
            Verdict::Unknown => Next::Inspect,
            // The caller turned reporting off; the silence is theirs, not the page's.
            Verdict::NotChecked => Next::Proceed,
        },
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::verdict::{Delivered, Delivery, Observation, Postcondition, classify};

    /// Every reason the ladder can produce, derived by running it. A new rung appears here for
    /// free and fails the assertions until it has a gloss and a next step.
    pub(super) fn every_assessment() -> Vec<Assessment> {
        let mut out = Vec::new();
        let observations = [
            Observation::ReportingDisabled,
            Observation::ReadFailed,
            Observation::NoBaseline,
            Observation::Compared {
                document_changed: false,
                identity_known: false,
                edits: 0,
                moved: 0,
                focus_moved: false,
                values_lost: 0,
            },
            Observation::Compared {
                document_changed: true,
                identity_known: true,
                edits: 0,
                moved: 0,
                focus_moved: false,
                values_lost: 0,
            },
        ];
        let counted = [(0, 0, 0), (2, 0, 0), (0, 2, 0), (0, 0, 1)];
        let mut all: Vec<Observation> = observations.to_vec();
        for (edits, moved, values_lost) in counted {
            for focus_moved in [false, true] {
                all.push(Observation::Compared {
                    document_changed: false,
                    identity_known: true,
                    edits,
                    moved,
                    focus_moved,
                    values_lost,
                });
            }
        }
        for observation in all {
            for how in [
                Delivery::TargetHit,
                Delivery::Intercepted,
                Delivery::OffTarget,
                Delivery::NotSettled,
                Delivery::JsDispatch,
                Delivery::NotProbed,
            ] {
                for modal_receiver in [false, true] {
                    for observed_after_ms in [None, Some(60)] {
                        let delivered = Delivered { how, modal_receiver, observed_after_ms };
                        for postcondition in [
                            Postcondition::NotRead,
                            Postcondition::Kept,
                            Postcondition::Discarded,
                            Postcondition::Rewritten,
                        ] {
                            out.push(classify(observation, delivered, postcondition));
                        }
                    }
                }
            }
        }
        out
    }

    fn reasons() -> std::collections::BTreeSet<&'static str> {
        every_assessment().into_iter().map(|a| a.reason).collect()
    }

    /// The ladder as it stands, pinned so the coverage assertions below check a reviewed list.
    #[test]
    fn the_ladder_produces_exactly_these_reasons() {
        let expected: std::collections::BTreeSet<&str> = [
            "aim_point_off_target",
            "delivered_no_change",
            "document_replaced",
            "focus_only",
            "hit_test_receiver",
            "identical_tree",
            "identity_unreadable",
            "modal_dialog",
            "no_baseline",
            "nodes_moved",
            "read_failed",
            "reporting_disabled",
            "scroll_not_settled",
            "tree_delta",
            "value_kept",
            "value_reverted",
            "value_rewritten",
            "values_lost",
        ]
        .into_iter()
        .collect();
        assert_eq!(reasons(), expected);
    }

    /// A gloss per reason, none of them the per-verdict floor.
    #[test]
    fn every_reason_has_its_own_gloss() {
        let mut seen = std::collections::HashSet::new();
        for assessment in every_assessment() {
            let text = gloss(assessment);
            let floor = gloss(Assessment { verdict: assessment.verdict, reason: "unwritten", page: PageSight::Readable });
            assert_ne!(
                text, floor,
                "{} / {} falls back to the per-verdict floor",
                assessment.verdict, assessment.reason
            );
            assert!(!text.is_empty());
            seen.insert(assessment.reason);
        }
        assert_eq!(seen.len(), reasons().len());
    }

    /// Two reasons sharing a sentence would be two rungs the reader cannot tell apart.
    #[test]
    fn no_two_reasons_share_a_gloss() {
        let mut by_gloss: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        for assessment in every_assessment() {
            if let Some(other) = by_gloss.insert(gloss(assessment), assessment.reason) {
                assert_eq!(
                    other, assessment.reason,
                    "{other} and {} share a gloss",
                    assessment.reason
                );
            }
        }
    }

    /// The `unchanged` gloss must not read as "nothing happened", the conclusion the taxonomy
    /// forbids.
    #[test]
    fn the_unchanged_gloss_refuses_to_conclude_no_effect() {
        let text = gloss(Assessment { verdict: Verdict::Unchanged, reason: "identical_tree", page: PageSight::Readable });
        assert!(text.contains("not the same as"), "{text}");
        assert!(!text.contains("no effect having"), "{text}");
    }

    /// The tokens mislead: `unknown` reads as a broken tool, `not_checked` as a missing feature.
    #[test]
    fn the_uncertain_verdicts_are_glossed_in_plain_words() {
        for (reason, word) in
            [("no_baseline", "unverified"), ("read_failed", "unverified"), ("identity_unreadable", "unverified")]
        {
            let text = gloss(Assessment { verdict: Verdict::Unknown, reason, page: PageSight::Readable });
            assert!(text.starts_with(word), "{reason}: {text}");
        }
        let text = gloss(Assessment { verdict: Verdict::NotChecked, reason: "reporting_disabled", page: PageSight::Readable });
        assert!(text.starts_with("not checked"), "{text}");
    }

    /// The mapping the whole field exists for, verdict by verdict.
    #[test]
    fn each_verdict_maps_to_its_own_next_step() {
        for (verdict, reason, expected) in [
            (Verdict::Changed, "tree_delta", Next::Proceed),
            (Verdict::Navigated, "document_replaced", Next::Inspect),
            (Verdict::Unchanged, "identical_tree", Next::Confirm),
            (Verdict::NoEffect, "delivered_no_change", Next::Confirm),
            (Verdict::Intercepted, "hit_test_receiver", Next::Dismiss),
            (Verdict::Intercepted, "modal_dialog", Next::Dismiss),
            (Verdict::NotKept, "value_reverted", Next::Stop),
            (Verdict::NotKept, "value_rewritten", Next::Stop),
            (Verdict::Unknown, "no_baseline", Next::Inspect),
            (Verdict::Unknown, "read_failed", Next::Inspect),
            (Verdict::Unknown, "identity_unreadable", Next::Inspect),
            (Verdict::Unknown, "aim_point_off_target", Next::Inspect),
            (Verdict::Unknown, "scroll_not_settled", Next::Retry),
            (Verdict::NotChecked, "reporting_disabled", Next::Proceed),
            (Verdict::Changed, "values_lost", Next::Confirm),
            (Verdict::Changed, "value_kept", Next::Proceed),
        ] {
            let assessment = Assessment { verdict, reason, page: PageSight::Readable };
            assert_eq!(next_for(assessment), expected, "{verdict} / {reason}");
        }
    }

    /// An unobserved action is never repeated blind; the repeat would be a second real click.
    #[test]
    fn an_unobserved_action_is_never_answered_with_a_retry() {
        for assessment in every_assessment() {
            if assessment.verdict != Verdict::Unknown {
                continue;
            }
            if assessment.reason == "scroll_not_settled" {
                assert_eq!(next_for(assessment), Next::Retry, "nothing was dispatched here");
                continue;
            }
            assert_eq!(
                next_for(assessment),
                Next::Inspect,
                "{} would send an agent to repeat an action that may have landed",
                assessment.reason
            );
        }
    }

    /// `retry` is the dangerous token, so it is reachable from exactly one rung.
    #[test]
    fn only_a_dispatch_that_never_happened_earns_a_retry() {
        let retriable: Vec<&str> = every_assessment()
            .into_iter()
            .filter(|a| next_for(*a) == Next::Retry)
            .map(|a| a.reason)
            .collect();
        assert!(retriable.iter().all(|r| *r == "scroll_not_settled"), "{retriable:?}");
        assert!(!retriable.is_empty(), "the token must stay reachable, or it is not a vocabulary");
    }

    /// A closed vocabulary of six, and no token spelled two ways.
    #[test]
    fn the_vocabulary_is_six_tokens_and_round_trips() {
        let all = [Next::Proceed, Next::Inspect, Next::Retry, Next::Confirm, Next::Dismiss, Next::Stop];
        let spelled: std::collections::BTreeSet<&str> = all.iter().map(|n| n.as_str()).collect();
        assert_eq!(spelled.len(), 6);
        assert_eq!(
            spelled,
            ["confirm", "dismiss", "inspect", "proceed", "retry", "stop"].into_iter().collect()
        );
        for next in all {
            assert_eq!(next.to_string(), next.as_str());
        }
    }
}

/// The printed tables, checked against the code that produces them. `llm-guide.txt` is compiled
/// into `--help`, so a table that drifts from `next_for` is a second implementation of it, free to
/// promise a branch the tool does not take — including `retry` on an action that may have landed.
#[cfg(test)]
mod guide {
    use super::*;

    use crate::verdict::Delivery;

    const GUIDE: &str = include_str!("../llm-guide.txt");

    /// The two documents that reprint this module's tables in markdown.
    const SKILL: &str = include_str!("../skills/chrome-agent/SKILL.md");
    const README: &str = include_str!("../README.md");

    /// `verdict reason next …`, as the guide's fixed-column block spells it.
    fn rows() -> Vec<(&'static str, &'static str, &'static str)> {
        let known = [
            "changed",
            "navigated",
            "intercepted",
            "not_kept",
            "no_effect",
            "unchanged",
            "unknown",
            "not_checked",
        ];
        GUIDE
            .lines()
            .map(str::trim)
            .filter_map(|line| {
                let mut cols = line.split_whitespace();
                let verdict = cols.next()?;
                if !known.contains(&verdict) {
                    return None;
                }
                let (reason, next) = (cols.next()?, cols.next()?);
                // The `delivery` table shares the word "intercepted" in its first column. A row
                // of this table always names a vocabulary token third.
                let vocabulary =
                    [Next::Proceed, Next::Inspect, Next::Retry, Next::Confirm, Next::Dismiss, Next::Stop];
                vocabulary.iter().any(|n| n.as_str() == next).then_some((verdict, reason, next))
            })
            .collect()
    }

    #[test]
    fn the_guide_table_is_the_mapping_this_module_implements() {
        let rows = rows();
        assert_eq!(rows.len(), 18, "the guide lost rows, or grew columns: {rows:?}");
        for (verdict, reason, next) in rows {
            let assessment = Assessment { verdict: parse_verdict(verdict), reason: leak(reason), page: PageSight::Readable };
            assert_eq!(
                next_for(assessment).as_str(),
                next,
                "the guide promises {next} for {verdict}/{reason}"
            );
        }
    }

    /// One reason maps to one `next` everywhere but a confirmed write on an unreadable page; the
    /// row states the readable case, so the exception must be written under it in words.
    #[test]
    fn the_guide_states_the_one_next_that_depends_on_more_than_the_reason() {
        let blind = Assessment {
            verdict: Verdict::Changed,
            reason: "value_kept",
            page: PageSight::Unreadable,
        };
        assert_eq!(next_for(blind), Next::Inspect, "the exception this note is about");
        assert!(
            GUIDE.contains("next=inspect"),
            "the guide must name the token the exception answers"
        );
        assert!(
            GUIDE.contains("the field is confirmed, the"),
            "and say which of the two facts each half of the response carries"
        );
    }

    /// A rung an agent is never told about is a branch it cannot take.
    #[test]
    fn the_guide_documents_every_reason_the_ladder_can_produce() {
        let documented: std::collections::BTreeSet<&str> =
            rows().into_iter().map(|(_, reason, _)| reason).collect();
        for assessment in super::tests::every_assessment() {
            assert!(
                documented.contains(assessment.reason),
                "{} is undocumented in llm-guide.txt",
                assessment.reason
            );
        }
    }

    /// A markdown copy of the table, as `(verdict, reasons, nexts)`.
    ///
    /// One reader for two shapes: `SKILL.md` gives each reason its own row, `README.md` merges
    /// several into one. So a cell is read as a LIST, of which the one-reason case is an instance.
    fn markdown_rows(doc: &str) -> Vec<(String, Vec<String>, Vec<String>)> {
        const HEADER: &str = "| `verdict` | `verdict_reason` | `next` |";
        let table = doc.split_once(HEADER).expect("the verdict table's header row").1;
        table
            .lines()
            .map(str::trim)
            .skip_while(|line| !line.starts_with('|'))
            .skip(1) // the |---|---| separator
            .take_while(|line| line.starts_with('|'))
            .map(|line| {
                let cells: Vec<&str> = line.trim_matches('|').split('|').collect();
                assert!(cells.len() >= 3, "a verdict row needs three columns: {line}");
                (bare(cells[0]), cell_list(cells[1], ','), cell_list(cells[2], '/'))
            })
            .collect()
    }

    /// A markdown cell's token, without the backticks and bolding the prose dresses it in.
    fn bare(cell: &str) -> String {
        cell.replace(['`', '*'], "").trim().to_string()
    }

    fn cell_list(cell: &str, separator: char) -> Vec<String> {
        cell.split(separator).map(bare).filter(|token| !token.is_empty()).collect()
    }

    /// The same re-borrow `leak` does, from markdown, where backticks separate the token.
    fn leak_from(doc: &'static str, reason: &str) -> &'static str {
        doc.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .find(|word| *word == reason)
            .expect("the reason came from this document")
    }

    /// The two markdown reprints promise the same `next` this module returns. `proceed` and
    /// `confirm` are opposite branches, so a single wrong cell makes an agent do another thing.
    #[test]
    fn every_markdown_copy_of_the_table_is_the_mapping_this_module_implements() {
        for (name, doc, expected) in
            [("skills/chrome-agent/SKILL.md", SKILL, 18), ("README.md", README, 12)]
        {
            let rows = markdown_rows(doc);
            assert_eq!(rows.len(), expected, "{name} lost rows, or grew columns: {rows:?}");
            for (verdict, reasons, nexts) in rows {
                assert!(!reasons.is_empty(), "{name}: {verdict} names no reason");
                for reason in &reasons {
                    let seen = Assessment {
                        verdict: parse_verdict(&verdict),
                        reason: leak_from(doc, reason),
                        page: PageSight::Readable,
                    };
                    assert_eq!(
                        next_for(seen).as_str(),
                        nexts[0],
                        "{name} promises {} for {verdict}/{reason}",
                        nexts[0]
                    );
                    // A cell naming two tokens is the documented exception, and exactly one row.
                    // Asserting WHICH row stops any other row growing a second answer.
                    match nexts.len() {
                        1 => {}
                        2 => {
                            assert_eq!(
                                reason, "value_kept",
                                "{name}: only a confirmed write on a page that could not be \
                                 read has two answers, not {verdict}/{reason}"
                            );
                            let blind = Assessment { page: PageSight::Unreadable, ..seen };
                            assert_eq!(
                                next_for(blind).as_str(),
                                nexts[1],
                                "{name} promises {} for {verdict}/{reason} on a blind page",
                                nexts[1]
                            );
                        }
                        n => panic!("{name}: {verdict}/{reason} names {n} tokens in one cell"),
                    }
                }
            }
        }
    }

    /// The `delivery` readings the guide's fixed-column block lists. Scoped to the section rather
    /// than matched by shape, since `intercepted` is also a word of the verdict table. A row sits
    /// at the block's own indent; wrapped continuations sit deeper, and `#` notes end the block.
    fn delivery_rows_in_the_guide() -> Vec<&'static str> {
        let section = GUIDE
            .split_once("\"delivery\" on a pointer-targeted action")
            .expect("the delivery block's own heading")
            .1;
        let mut rows = Vec::new();
        let mut indent = None;
        for line in section.lines() {
            let body = line.trim_start();
            if body.is_empty() {
                continue;
            }
            if body.starts_with('#') {
                break;
            }
            let depth = line.len() - body.len();
            match indent {
                None if body.starts_with("target_hit") => indent = Some(depth),
                None => continue,
                Some(row) if depth != row => continue, // a wrapped continuation
                Some(_) => {}
            }
            rows.push(body.split_whitespace().next().expect("a non-empty line"));
        }
        rows
    }

    /// The same readings, from `SKILL.md`'s markdown table.
    fn delivery_rows_in_the_skill() -> Vec<String> {
        const HEADER: &str = "| `delivery` | Means | Licence |";
        SKILL
            .split_once(HEADER)
            .expect("the delivery table's header row")
            .1
            .lines()
            .map(str::trim)
            .skip_while(|line| !line.starts_with('|'))
            .skip(1)
            .take_while(|line| line.starts_with('|'))
            .map(|line| bare(line.trim_matches('|').split('|').next().expect("a first cell")))
            .collect()
    }

    /// Both copies of the `delivery` table name the six readings the code produces, in order.
    ///
    /// `Delivery::parse` answers `not_probed` for anything unrecognised, so a misspelled reading
    /// would collapse onto the floor and read as verified. A row must ROUND-TRIP, not merely
    /// parse, and the count is asserted too, or a skipped row leaves the rest self-consistent.
    #[test]
    fn both_copies_of_the_delivery_table_name_the_readings_the_code_produces() {
        let expected = [
            Delivery::TargetHit,
            Delivery::Intercepted,
            Delivery::OffTarget,
            Delivery::NotSettled,
            Delivery::JsDispatch,
            Delivery::NotProbed,
        ];
        let guide: Vec<String> =
            delivery_rows_in_the_guide().into_iter().map(str::to_string).collect();
        for (name, rows) in
            [("llm-guide.txt", guide), ("skills/chrome-agent/SKILL.md", delivery_rows_in_the_skill())]
        {
            assert_eq!(
                rows.len(),
                expected.len(),
                "{name} lists {} delivery readings, the code has {}: {rows:?}",
                rows.len(),
                expected.len()
            );
            for (row, want) in rows.iter().zip(expected) {
                assert_eq!(
                    Delivery::parse(row).as_str(),
                    row.as_str(),
                    "{name} names a delivery reading the code does not have: {row}"
                );
                assert_eq!(
                    Delivery::parse(row),
                    want,
                    "{name} lists the readings in another order: expected {} here, found {row}",
                    want.as_str()
                );
            }
        }
    }

    fn parse_verdict(word: &str) -> Verdict {
        match word {
            "changed" => Verdict::Changed,
            "navigated" => Verdict::Navigated,
            "intercepted" => Verdict::Intercepted,
            "not_kept" => Verdict::NotKept,
            "no_effect" => Verdict::NoEffect,
            "unchanged" => Verdict::Unchanged,
            "unknown" => Verdict::Unknown,
            "not_checked" => Verdict::NotChecked,
            other => panic!("unknown verdict word in the guide: {other}"),
        }
    }

    /// `Assessment.reason` is `&'static str` and so is the guide: this re-borrows a slice of it.
    fn leak(reason: &str) -> &'static str {
        GUIDE
            .split_whitespace()
            .find(|word| *word == reason)
            .expect("the reason came from the guide")
    }
}

/// What the agent should do next, when the verdict alone does not say.
///
/// Keyed on the reason, except for a confirmed write whose page could not be read: the Group A
/// rung outranks `read_failed`, so this is the only place that blindness gets said.
#[must_use]
pub fn hint_for(assessment: Assessment) -> Option<&'static str> {
    if assessment.reason == "value_kept" {
        return match assessment.page {
            // Both facts, in that order: what was measured, then what was not.
            PageSight::Unreadable => Some(
                "The element holds what was asked of it — that was read back on the element itself, which is why the verdict is `changed` and not `unknown`. Reading the PAGE afterwards failed, so nothing is known about what else moved: a navigation, a validation message, a field the form cleared. Run `inspect` to see the current state before acting on anything stored, and do not send the action a second time — it landed.",
            ),
            // A confirmed write on a page that was read needs no advice.
            PageSight::Readable => None,
        };
    }
    match assessment.reason {
        "no_baseline" => Some(
            "No snapshot existed before this action, so nothing could be compared. Run `inspect` to establish one; the next action on this page will report what changed.",
        ),
        "read_failed" => Some(
            "The action ran, but reading the page afterwards failed, so what it did is unknown. Run `inspect` to see the current state.",
        ),
        "identical_tree" => Some(
            "Nothing in the accessibility tree changed while this was watched. That is not the same as the action having no effect: a click absorbed by an overlay, an effect the tree cannot see (canvas, styling), and a handler that runs after the window all look like this. Confirm with `inspect` or `eval` before repeating the action — a repeat is a second real action.",
        ),
        // The action writes a more specific hint naming the receiver; this is the fallback.
        "hit_test_receiver" => Some(
            "The point this was aimed at belongs to another element, which received the event instead — `intercepted_by` names it. Deal with that element first (a banner or scrim usually has to be dismissed), or aim somewhere the target is actually exposed. Nothing is known about what the target would have done.",
        ),
        "modal_dialog" => Some(
            "A modal dialog holds the top layer, so it receives every pointer event outside itself. Close it (press Escape, or click its own dismiss control) before acting on anything behind it.",
        ),
        "scroll_not_settled" => Some(
            "Two readings of the aim point disagreed, so it was still moving when it was measured — nothing was dispatched, rather than dispatched at a coordinate the target had already left. This is the one rung where the repeat is the fix and is safe, because the page saw no event: run `wait` for the movement to end, then run the action again. A point that had STOPPED moving and was still unaimable reports `aim_point_off_target` instead, and that one does not improve on a repeat.",
        ),
        "aim_point_off_target" => Some(
            "No point on the element could be aimed at and the reading was stable — two probes 30ms apart agreed — so nothing was dispatched and a repeat measures the same coordinate. Two shapes reach here, and `aim` tells them apart: a coordinate on screen means the element has no box a pointer can reach (an inline link laid out across a gap, a container clipped to nothing), and a coordinate outside the viewport means the page is holding it there, which the probe's own scroll already failed to change. Run `inspect` to see where the element sits: for the first, aim at a child that has a box of its own; for the second, change the page's state — dismiss the layer pinning it — because no scroll will move it.",
        ),
        // `changed` alone would read as success on a form that quietly discards its input.
        "values_lost" => Some(
            "The page moved, and a field that held a value before this action holds none after it — `values_lost` names each one and what it held. Two things look identical here: a form that submitted successfully and cleared itself, and a form that threw the input away without sending it. Confirm which before treating the submit as done: check for the page's own confirmation (`assert text --contains`), or the request itself with `network`. Re-filling and re-submitting risks a second submission of work that already went through.",
        ),
        // Both name the field to read and forbid the retry: re-filling is the reflex, and it is
        // a second real edit that produces the same answer.
        "value_reverted" => Some(
            "The element held nothing when it was read back: `value.actual` is empty and `value.requested` is what was asked for. Do not fill it again — the same write already produced this, and a repeat is a second real edit against a field that discarded the first. A field that empties itself is a controlled component writing its own state over ours, or an input rejecting what it cannot parse (a number input given letters). Read `value.actual` to confirm, then either send a value of the type the field accepts, or drive the page's own control instead of the field. If the emptying may be a validator that runs later than the window in `observed_after_ms`, `wait` then `assert value` is what measures that.",
        ),
        "value_rewritten" => Some(
            "The element holds something other than what was asked for: `value.actual` is what it kept, `value.requested` is what was sent. This is what a mask, a trimmer or a normaliser does, and the write did land — in the page's own shape. Do not fill it again: the same write produces the same rewrite. Read both strings and decide whether `value.actual` is the value you wanted; if it is not, send it in the form the field accepts.",
        ),
        // The strong word carries its own limits: everything it cannot see, named.
        "delivered_no_change" => Some(
            "The event reached the target and the accessibility tree did not move within the window reported in `observed_after_ms`. Three effects are invisible to that measurement: a canvas or WebGL repaint, a change that is CSS-only (a class, an opacity, a transform), and a handler that runs after the window closed. If one of those is plausible, confirm with `screenshot` or `eval` rather than repeating the action.",
        ),
        _ => None,
    }
}

/// The same advice as `hint_for`, cut to a terminal line or two. `None` wherever `hint_for` is.
///
/// A second curated table, not a truncation: cutting at a character count ends mid-sentence, and
/// cutting at the first sentence loses the imperative — `value_reverted`'s prohibition is in the
/// SECOND one. Where the full hint forbids a repeat, the short one must too (tested below).
#[must_use]
pub fn short_hint(assessment: Assessment) -> Option<&'static str> {
    if assessment.reason == "value_kept" {
        return match assessment.page {
            PageSight::Unreadable => Some(
                "The element holds what was asked of it, read back on the element itself. The page after it could not be read: `inspect` before acting on anything stored, and do not send it a second time.",
            ),
            PageSight::Readable => None,
        };
    }
    match assessment.reason {
        // The prohibition comes first in both.
        "value_reverted" => Some(
            "Do not fill it again — the field discarded the first write. Read `value.actual`, then send a value the field accepts.",
        ),
        "value_rewritten" => Some(
            "Do not fill it again — the same write produces the same rewrite. Read `value.actual` and decide whether that is the value you wanted.",
        ),
        "values_lost" => Some(
            "Confirm the submit landed before re-filling — `assert text --contains` on the page's own confirmation, or `network`. Re-submitting may send the work twice.",
        ),
        "hit_test_receiver" => Some(
            "Deal with the element named above first, then repeat the action. Nothing is known about what the target would have done.",
        ),
        "modal_dialog" => Some(
            "Close the dialog (Escape, or its own dismiss control) before acting on anything behind it.",
        ),
        "scroll_not_settled" => Some(
            "Nothing was dispatched, so a repeat is safe: `wait` for the page to settle, then run this action again.",
        ),
        "aim_point_off_target" => Some(
            "Nothing was dispatched, and a repeat measures the same point. Run `inspect`: aim at a child with a box of its own, or clear whatever pins the element off screen.",
        ),
        "no_baseline" => Some(
            "Run `inspect` to establish a baseline; the next action on this page will report what changed.",
        ),
        "read_failed" => Some(
            "The action ran and the read after it did not. Run `inspect` to see the current state rather than repeating the action.",
        ),
        "identical_tree" => Some(
            "Not proof that the action had no effect. Confirm with `inspect` or `eval` rather than repeating — a repeat is a second real action.",
        ),
        "delivered_no_change" => Some(
            "Delivered, and the tree did not move inside the window. A canvas repaint, a CSS-only change or a late handler all look like this — confirm with `screenshot` or `eval` rather than repeating.",
        ),
        _ => None,
    }
}

#[cfg(test)]
mod short {
    use super::*;

    /// Roughly two lines on a normal terminal. The full text stays in `verdict_hint`.
    const LINE_PAIR: usize = 200;

    /// A reason with a full hint and no short one prints nothing in text mode.
    #[test]
    fn every_full_hint_has_a_short_form() {
        for assessment in super::tests::every_assessment() {
            assert_eq!(
                hint_for(assessment).is_some(),
                short_hint(assessment).is_some(),
                "{} / {} has one form and not the other",
                assessment.verdict,
                assessment.reason
            );
        }
    }

    /// Short AND whole sentences. A character-count truncation would fail the second half.
    #[test]
    fn a_short_hint_is_short_and_ends_where_a_sentence_ends() {
        for assessment in super::tests::every_assessment() {
            let Some(hint) = short_hint(assessment) else { continue };
            assert!(
                hint.len() <= LINE_PAIR,
                "{} is {} chars, over the two-line budget: {hint}",
                assessment.reason,
                hint.len()
            );
            assert!(hint.ends_with('.'), "{} is cut mid-sentence: {hint}", assessment.reason);
            assert!(!hint.contains('…'), "{} was truncated, not written: {hint}", assessment.reason);
        }
    }

    /// Shortening must not drop the line that stops an agent acting a second time.
    #[test]
    fn shortening_never_drops_a_prohibition() {
        for assessment in super::tests::every_assessment() {
            let (Some(full), Some(short)) = (hint_for(assessment), short_hint(assessment)) else {
                continue;
            };
            if full.contains("Do not") {
                assert!(
                    short.contains("Do not"),
                    "{} forbids a repeat in full and not in short: {short}",
                    assessment.reason
                );
            }
            // Softer half: a full hint warning about a duplicate leaves the short one warning too.
            let warns = |text: &str| {
                ["repeat", "again", "twice", "second"].iter().any(|w| text.contains(w))
            };
            if warns(full) {
                assert!(
                    warns(short),
                    "{} warns about a duplicate action in full and not in short: {short}",
                    assessment.reason
                );
            }
        }
    }

    /// `scroll_not_settled` is the one rung where the repeat IS the advice.
    #[test]
    fn the_safe_retry_still_says_to_retry() {
        let assessment = Assessment { verdict: Verdict::Unknown, reason: "scroll_not_settled", page: PageSight::Readable };
        let short = short_hint(assessment).expect("a short hint");
        assert!(short.contains("again"), "{short}");
        assert!(!short.contains("Do not"), "{short}");
        assert_eq!(next_for(assessment), Next::Retry);
    }
}
