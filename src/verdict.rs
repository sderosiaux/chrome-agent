//! What an action is allowed to claim about itself.
//!
//! No visible difference is reported as `unchanged` — a claim about the observation, not about
//! the action. An identical tree is also what an overlay swallowing the click, a canvas repaint
//! and a handler running after the window all look like, and "no effect" would make an agent
//! retry, which is a second real click. `no_effect` exists but only behind proof of delivery
//! (rung 15). Full taxonomy: `docs/design/verdict-taxonomy.md`.
//!
//! The ladder, ordered, first match wins. `classify` follows it literally.
//!
//! ```text
//! GROUP A — measured on this action's own target, and DISQUALIFYING
//!  1. delivery = not_settled     → unknown     / scroll_not_settled      (nothing dispatched)
//!  2. delivery = off_target      → unknown     / aim_point_off_target    (nothing dispatched)
//!  3. delivery = intercepted     → intercepted / hit_test_receiver | modal_dialog
//!  4. postcondition = discarded  → not_kept    / value_reverted
//!  5. postcondition = rewritten  → not_kept    / value_rewritten
//!
//! GROUP B — depends on comparing the stored tree with the live one
//!  6. identity unreadable        → unknown     / identity_unreadable
//!  7. document replaced          → navigated   / document_replaced
//!  8. values_lost non-empty      → changed     / values_lost
//!  9. edits > 0                  → changed     / tree_delta
//! 10. moved > 0                  → changed     / nodes_moved
//!
//! GROUP A — measured on this action's own target, and CONFIRMING
//! 11. postcondition = kept       → changed     / value_kept
//!
//! GROUP B — what is left when no comparison said anything
//! 12. reporting off              → not_checked / reporting_disabled
//! 13. post-action read failed    → unknown     / read_failed
//! 14. no baseline                → unknown     / no_baseline
//! 15. target_hit + window + quiet → no_effect  / delivered_no_change
//! 16. focus moved                → changed     / focus_only
//! 17. otherwise                  → unchanged   / identical_tree
//! ```
//!
//! Group A is measured on THIS action's own target and needs no comparable trees, so it precedes
//! Group B — including the identity rungs, since `goto` keeps `last_snapshot` while clearing
//! `uid_map` and the first action after one would otherwise report `navigated` about the `goto`.
//!
//! Orderings not to revert:
//!
//! - `target_hit` (15) stays in Group B: it is the LICENCE for `no_effect`, which claims a
//!   comparable tree stayed quiet. Promoted, a click that replaced the document would report it.
//! - A FAILED read-back (4, 5) preempts everything, including "which document is this": nothing
//!   downstream of it works whatever the page did.
//! - A CONFIRMED read-back (11) preempts nothing that describes the page, but outranks the three
//!   rungs that compared nothing (12, 13, 14). It reports a write the tree cannot show: a secret
//!   field renders as a fixed marker (`snapshot_secret::MARKER`, fixed so an unchanged secret
//!   does not read as a change), so a refill produces no visible delta.

use std::fmt;

/// The claim attached to an action's response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verdict {
    /// The page moved in a way we can point at.
    Changed,
    /// The document was replaced. Stored uids are dead.
    Navigated,
    /// Another element occupied the aim point and received the event. Who got it, never what
    /// they did with it.
    Intercepted,
    /// The element did not hold the write when we looked back. What the page kept, never why.
    NotKept,
    /// Delivery proven and the page stayed still. Only emitted with its measurement window.
    NoEffect,
    /// The tree was identical. A statement about the observation, not about the action.
    Unchanged,
    /// The honest floor. Always paired with a reason naming what was missing.
    Unknown,
    /// We did not look.
    NotChecked,
}

impl Verdict {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Changed => "changed",
            Self::Navigated => "navigated",
            Self::Intercepted => "intercepted",
            Self::NotKept => "not_kept",
            Self::NoEffect => "no_effect",
            Self::Unchanged => "unchanged",
            Self::Unknown => "unknown",
            Self::NotChecked => "not_checked",
        }
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A verdict and the observation that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assessment {
    pub verdict: Verdict,
    /// Why, in one machine-readable token. Never absent.
    pub reason: &'static str,
    /// Whether the page could be seen at all. Read only by `next_for` and the hint tables.
    pub page: PageSight,
}

/// Classify an observation, what the hit test said, and what the acted-on handle held.
/// Pure, ordered, first match wins. See the module docs for the ladder.
///
/// `intercepted` and `not_kept` sit ABOVE `tree_delta`: a delta is a whole-page observation our
/// own action can explain, while an interception is a PRE-dispatch measurement it cannot have
/// caused and a postcondition reads the ONE handle the caller named. A covered button's scrim
/// moves the page itself, so ranking `changed` first would report success for every intercepted
/// click. Trade-off: a page that moved elsewhere still reports `intercepted`/`not_kept`, with
/// the delta left on the response.
#[must_use]
pub const fn classify(
    observation: Observation,
    delivered: Delivered,
    postcondition: Postcondition,
) -> Assessment {
    let (verdict, reason) = match (observation, delivered.how) {
        // ---- GROUP A: measured on this action's own target. -------------------------------
        // Known whether or not the page was re-read and whether or not the two trees can be
        // compared, so the whole group precedes Group B.
        (_, Delivery::NotSettled) => (Verdict::Unknown, "scroll_not_settled"),
        (_, Delivery::OffTarget) => (Verdict::Unknown, "aim_point_off_target"),
        (_, Delivery::Intercepted) => (
            Verdict::Intercepted,
            if delivered.modal_receiver {
                "modal_dialog"
            } else {
                "hit_test_receiver"
            },
        ),
        // One verdict, two reasons: the recovery differs, and `value_reverted` on a phone mask
        // would be a token saying something false.
        (_, _) if matches!(postcondition, Postcondition::Discarded) => {
            (Verdict::NotKept, "value_reverted")
        }
        (_, _) if matches!(postcondition, Postcondition::Rewritten) => {
            (Verdict::NotKept, "value_rewritten")
        }
        // ---- GROUP B: depends on comparing the stored tree with the live one. -------------
        // Identity first within the group: without it the uid spaces may belong to different
        // documents and every count below is meaningless.
        (
            Observation::Compared {
                identity_known: false,
                ..
            },
            _,
        ) => (Verdict::Unknown, "identity_unreadable"),
        (
            Observation::Compared {
                document_changed: true,
                ..
            },
            _,
        ) => (Verdict::Navigated, "document_replaced"),
        // A field held a value before this action and holds none after it. Above `tree_delta`
        // because it IS one, and only the reason separates "the page moved" from "the page moved
        // and dropped what you had typed". The verdict stays `changed`: a form that clears itself
        // on a successful submit is correct, so `not_kept` would report a failure for a working
        // form. `not_kept` is reserved for the write THIS action made.
        (Observation::Compared { values_lost, .. }, _) if values_lost > 0 => {
            (Verdict::Changed, "values_lost")
        }
        (Observation::Compared { edits, .. }, _) if edits > 0 => (Verdict::Changed, "tree_delta"),
        (Observation::Compared { moved, .. }, _) if moved > 0 => (Verdict::Changed, "nodes_moved"),
        // ---- GROUP A again: the same read-back, this time confirming. ---------------------
        // Below the tree rungs (they name what changed and where) and above the three below
        // (they compared nothing at all).
        (_, _) if matches!(postcondition, Postcondition::Kept) => (Verdict::Changed, "value_kept"),
        // ---- GROUP B: no comparison was available to say anything. ------------------------
        (Observation::ReportingDisabled, _) => (Verdict::NotChecked, "reporting_disabled"),
        (Observation::ReadFailed, _) => (Verdict::Unknown, "read_failed"),
        (Observation::NoBaseline, _) => (Verdict::Unknown, "no_baseline"),
        // Two independent facts: delivery proven, and nothing but focus moved within a nameable
        // window. Without the window it falls to the floor below. Above `focus_only` because a
        // proven hit is better delivery evidence than focus churn, and clicking anything
        // focusable moves focus — `focus_only` first would make `no_effect` unreachable.
        (
            Observation::Compared {
                edits: 0, moved: 0, ..
            },
            Delivery::TargetHit,
        ) if delivered.observed_after_ms.is_some() => (Verdict::NoEffect, "delivered_no_change"),
        // Without that proof, focus churn is the only sign the action arrived at all.
        (
            Observation::Compared {
                focus_moved: true, ..
            },
            _,
        ) => (Verdict::Changed, "focus_only"),
        (Observation::Compared { .. }, _) => (Verdict::Unchanged, "identical_tree"),
    };
    // Carried, not classified: a Group A rung can be true about the handle while the page around
    // it went unobserved, which no verdict word can say.
    let page = match observation {
        Observation::ReadFailed => PageSight::Unreadable,
        Observation::ReportingDisabled | Observation::NoBaseline | Observation::Compared { .. } => {
            PageSight::Readable
        }
    };
    Assessment {
        verdict,
        reason,
        page,
    }
}

/// The human gloss, the next-step token and the two hint tables, all read off an `Assessment`.
pub use crate::verdict_words::{gloss, hint_for, next_for, short_hint};

/// What `classify` reads, and the sight it carries on the assessment.
pub use crate::verdict_evidence::{Delivered, Delivery, Observation, PageSight, Postcondition};

#[cfg(test)]
mod tests {
    use super::*;

    const fn compared(edits: usize, moved: usize, focus_moved: bool) -> Observation {
        Observation::Compared {
            document_changed: false,
            identity_known: true,
            edits,
            moved,
            focus_moved,
            values_lost: 0,
        }
    }

    /// Delivery proven, observed over a named window.
    const fn hit(observed_after_ms: u64) -> Delivered {
        Delivered {
            how: Delivery::TargetHit,
            modal_receiver: false,
            observed_after_ms: Some(observed_after_ms),
        }
    }

    const fn intercepted(modal_receiver: bool) -> Delivered {
        Delivered {
            how: Delivery::Intercepted,
            modal_receiver,
            observed_after_ms: None,
        }
    }

    /// No mouse event, or no answer from the hit test.
    fn plain(observation: Observation) -> Assessment {
        classify(observation, Delivered::NOT_PROBED, Postcondition::NotRead)
    }

    /// The hit test answered, and there is no value to read back.
    fn mouse(observation: Observation, delivered: Delivered) -> Assessment {
        classify(observation, delivered, Postcondition::NotRead)
    }

    /// A fill whose read-back disagreed with the request.
    fn reverted(observation: Observation) -> Assessment {
        classify(observation, Delivered::NOT_PROBED, Postcondition::Discarded)
    }

    /// Each silence names itself rather than collapsing into one.
    #[test]
    fn every_silent_case_names_itself() {
        let cases = [
            (
                Observation::ReportingDisabled,
                Verdict::NotChecked,
                "reporting_disabled",
            ),
            (Observation::ReadFailed, Verdict::Unknown, "read_failed"),
            (Observation::NoBaseline, Verdict::Unknown, "no_baseline"),
            (compared(0, 0, false), Verdict::Unchanged, "identical_tree"),
        ];
        let mut seen = std::collections::HashSet::new();
        for (observation, verdict, reason) in cases {
            let got = plain(observation);
            assert_eq!(got.verdict, verdict, "for {observation:?}");
            assert_eq!(got.reason, reason, "for {observation:?}");
            assert!(
                seen.insert((got.verdict, got.reason)),
                "two silences share a name: {got:?}"
            );
        }
    }

    /// The counts were computed across uid spaces that may belong to different documents.
    #[test]
    fn an_unreadable_identity_is_unknown_whatever_the_counts_say() {
        let got = plain(Observation::Compared {
            document_changed: false,
            identity_known: false,
            edits: 40,
            moved: 3,
            focus_moved: true,
            values_lost: 0,
        });
        assert_eq!(got.verdict, Verdict::Unknown);
        assert_eq!(got.reason, "identity_unreadable");
    }

    #[test]
    fn a_replaced_document_is_navigated_not_changed() {
        let got = plain(Observation::Compared {
            document_changed: true,
            identity_known: true,
            edits: 0,
            moved: 0,
            focus_moved: false,
            values_lost: 0,
        });
        assert_eq!(got.verdict, Verdict::Navigated);
    }

    #[test]
    fn edits_and_reorders_both_count_as_changed() {
        assert_eq!(plain(compared(1, 0, false)).reason, "tree_delta");
        assert_eq!(plain(compared(0, 2, false)).reason, "nodes_moved");
    }

    /// A pre-dispatch measurement outranks a post-dispatch one: the delta belongs to the scrim.
    #[test]
    fn an_interception_outranks_the_delta_it_caused() {
        for observation in [
            compared(4, 0, true),
            compared(0, 0, true),
            compared(0, 0, false),
        ] {
            let got = mouse(observation, intercepted(false));
            assert_eq!(got.verdict, Verdict::Intercepted, "for {observation:?}");
            assert_eq!(got.reason, "hit_test_receiver");
        }
        assert_eq!(
            mouse(compared(1, 0, false), intercepted(true)).reason,
            "modal_dialog"
        );
    }

    /// `document_replaced` means "I cannot compare two trees"; a hit test does not need them.
    #[test]
    fn a_definite_delivery_reading_outranks_a_replaced_document() {
        let navigated = Observation::Compared {
            document_changed: true,
            identity_known: true,
            edits: 0,
            moved: 0,
            focus_moved: false,
            values_lost: 0,
        };
        assert_eq!(
            mouse(navigated, intercepted(false)).verdict,
            Verdict::Intercepted
        );
        assert_eq!(mouse(navigated, intercepted(true)).reason, "modal_dialog");
        // Nothing dispatched is a fact about this action, whatever the document did.
        for how in [Delivery::NotSettled, Delivery::OffTarget] {
            let got = mouse(
                navigated,
                Delivered {
                    how,
                    ..Delivered::NOT_PROBED
                },
            );
            assert_eq!(got.verdict, Verdict::Unknown, "for {how:?}");
            assert_ne!(got.reason, "document_replaced");
        }
    }

    /// `target_hit` licenses `no_effect`, a claim about a quiet tree, and a replaced document
    /// has no such tree.
    #[test]
    fn a_real_navigation_is_still_reported_when_delivery_proves_nothing() {
        let navigated = Observation::Compared {
            document_changed: true,
            identity_known: true,
            edits: 0,
            moved: 0,
            focus_moved: false,
            values_lost: 0,
        };
        for how in [
            Delivery::TargetHit,
            Delivery::JsDispatch,
            Delivery::NotProbed,
        ] {
            let delivered = Delivered {
                how,
                modal_receiver: false,
                observed_after_ms: Some(60),
            };
            let got = classify(navigated, delivered, Postcondition::NotRead);
            assert_eq!(got.verdict, Verdict::Navigated, "for {how:?}");
            assert_eq!(got.reason, "document_replaced");
        }
    }

    /// A page that empties the field still moves focus, and `focus_only` would read as success.
    #[test]
    fn a_value_the_page_did_not_keep_outranks_the_delta_beside_it() {
        for observation in [
            compared(0, 0, true),
            compared(4, 0, true),
            compared(0, 2, false),
        ] {
            let got = reverted(observation);
            assert_eq!(got.verdict, Verdict::NotKept, "for {observation:?}");
            assert_eq!(got.reason, "value_reverted");
        }
    }

    /// The read-back happens inside the action, so `--verdict off` still sees the discard.
    #[test]
    fn a_reverted_value_is_reported_whatever_the_page_read_did() {
        for observation in [
            Observation::ReportingDisabled,
            Observation::ReadFailed,
            Observation::NoBaseline,
            compared(0, 0, false),
        ] {
            let got = reverted(observation);
            assert_eq!(got.verdict, Verdict::NotKept, "for {observation:?}");
            assert_eq!(got.reason, "value_reverted");
        }
    }

    /// A read of the handle just written to needs no comparable trees, so it precedes the
    /// identity rungs for the same reason the hit test does.
    #[test]
    fn a_reverted_value_outranks_an_unreadable_identity_and_a_replaced_document() {
        let unreadable = Observation::Compared {
            document_changed: false,
            identity_known: false,
            edits: 0,
            moved: 0,
            focus_moved: false,
            values_lost: 0,
        };
        assert_eq!(reverted(unreadable).reason, "value_reverted");
        let navigated = Observation::Compared {
            document_changed: true,
            identity_known: true,
            edits: 0,
            moved: 0,
            focus_moved: false,
            values_lost: 0,
        };
        assert_eq!(reverted(navigated).verdict, Verdict::NotKept);
        // Nothing dispatched, or another element took it: both explain the missing value.
        for how in [
            Delivery::NotSettled,
            Delivery::OffTarget,
            Delivery::Intercepted,
        ] {
            let delivered = Delivered {
                how,
                ..Delivered::NOT_PROBED
            };
            let got = classify(compared(0, 0, false), delivered, Postcondition::Discarded);
            assert_ne!(got.verdict, Verdict::NotKept, "for {how:?}");
        }
    }

    /// `tree_delta` names the node and the line, which is `value_kept` plus where to look.
    #[test]
    fn a_visible_delta_outranks_the_read_back_that_agrees_with_it() {
        let kept = |observation| classify(observation, Delivered::NOT_PROBED, Postcondition::Kept);
        assert_eq!(kept(compared(3, 0, false)).reason, "tree_delta");
        assert_eq!(kept(compared(0, 2, false)).reason, "nodes_moved");
        let lost = Observation::Compared {
            document_changed: false,
            identity_known: true,
            edits: 2,
            moved: 0,
            focus_moved: true,
            values_lost: 1,
        };
        assert_eq!(kept(lost).reason, "values_lost");
        // A confirmed write on a replaced page describes a field that is gone.
        for (document_changed, identity_known, reason) in [
            (true, true, "document_replaced"),
            (false, false, "identity_unreadable"),
        ] {
            let observation = Observation::Compared {
                document_changed,
                identity_known,
                edits: 0,
                moved: 0,
                focus_moved: true,
                values_lost: 0,
            };
            assert_eq!(kept(observation).reason, reason);
        }
    }

    /// A secret field renders as a fixed marker, so a refill changes nothing the diff can see.
    #[test]
    fn a_confirmed_write_the_tree_cannot_show_is_not_reported_as_focus_alone() {
        for observation in [compared(0, 0, true), compared(0, 0, false)] {
            let got = classify(observation, Delivered::NOT_PROBED, Postcondition::Kept);
            assert_eq!(
                got.verdict,
                Verdict::Changed,
                "the write did change the page: {got:?}"
            );
            assert_eq!(got.reason, "value_kept");
            assert_ne!(
                plain(observation).reason,
                got.reason,
                "and the old answer is gone"
            );
        }
    }

    /// `not_kept` survives `--verdict off` and a failed read, so a confirmed write must too.
    #[test]
    fn a_confirmed_write_outranks_every_admission_that_nothing_was_compared() {
        for observation in [
            Observation::ReportingDisabled,
            Observation::ReadFailed,
            Observation::NoBaseline,
        ] {
            let got = classify(observation, Delivered::NOT_PROBED, Postcondition::Kept);
            assert_eq!(got.verdict, Verdict::Changed, "for {observation:?}");
            assert_eq!(got.reason, "value_kept", "for {observation:?}");
        }
    }

    /// The field took the write and the page after it could not be read, so `next` and the hint
    /// carry the blindness the verdict cannot.
    #[test]
    fn a_confirmed_write_on_an_unread_page_still_says_inspect() {
        use crate::verdict_words::Next;

        let blind = classify(
            Observation::ReadFailed,
            Delivered::NOT_PROBED,
            Postcondition::Kept,
        );
        assert_eq!(blind.verdict, Verdict::Changed);
        assert_eq!(blind.reason, "value_kept");
        assert_eq!(blind.page, PageSight::Unreadable);
        assert_eq!(
            next_for(blind),
            Next::Inspect,
            "carrying on while blind is the one refusal"
        );
        let hint = hint_for(blind).expect("the blindness has to be said somewhere");
        assert!(hint.contains("inspect"), "{hint}");
        assert!(
            hint.contains("what else moved"),
            "and name what is unknown: {hint}"
        );

        // The same rung with the page in hand keeps `proceed` and needs no advice.
        for observation in [
            Observation::ReportingDisabled,
            Observation::NoBaseline,
            compared(0, 0, true),
        ] {
            let seen = classify(observation, Delivered::NOT_PROBED, Postcondition::Kept);
            assert_eq!(seen.page, PageSight::Readable, "for {observation:?}");
            assert_eq!(next_for(seen), Next::Proceed, "for {observation:?}");
            assert_eq!(hint_for(seen), None, "for {observation:?}");
        }
    }

    /// The rule is over the token, not over `value_kept`, so a later rung cannot inherit
    /// "carry on while blind".
    #[test]
    fn no_verdict_answers_proceed_about_a_page_it_could_not_read() {
        use crate::verdict_words::Next;

        for postcondition in [
            Postcondition::NotRead,
            Postcondition::Kept,
            Postcondition::Discarded,
            Postcondition::Rewritten,
        ] {
            for how in [
                Delivery::TargetHit,
                Delivery::Intercepted,
                Delivery::OffTarget,
                Delivery::NotSettled,
                Delivery::JsDispatch,
                Delivery::NotProbed,
            ] {
                let delivered = Delivered {
                    how,
                    modal_receiver: false,
                    observed_after_ms: Some(60),
                };
                let blind = classify(Observation::ReadFailed, delivered, postcondition);
                assert_ne!(
                    next_for(blind),
                    Next::Proceed,
                    "{} / {}",
                    blind.verdict,
                    blind.reason
                );
            }
        }
        // The retry survives: nothing was dispatched, so there is nothing to be blind about.
        let not_settled = Delivered {
            how: Delivery::NotSettled,
            ..Delivered::NOT_PROBED
        };
        let got = classify(Observation::ReadFailed, not_settled, Postcondition::NotRead);
        assert_eq!(next_for(got), Next::Retry);
    }

    /// A failed delivery explains a value more than the value explains the delivery.
    #[test]
    fn a_confirmed_write_does_not_outrank_a_delivery_that_failed() {
        for how in [
            Delivery::NotSettled,
            Delivery::OffTarget,
            Delivery::Intercepted,
        ] {
            let delivered = Delivered {
                how,
                ..Delivered::NOT_PROBED
            };
            let got = classify(compared(0, 0, true), delivered, Postcondition::Kept);
            assert_ne!(got.reason, "value_kept", "for {how:?}");
        }
    }

    /// Every click, press and drag keeps the verdict it had.
    #[test]
    fn a_command_with_no_read_back_is_unaffected() {
        for observation in [
            Observation::ReportingDisabled,
            Observation::ReadFailed,
            Observation::NoBaseline,
            compared(0, 0, false),
            compared(0, 0, true),
            compared(2, 0, false),
        ] {
            assert_eq!(
                classify(observation, Delivered::NOT_PROBED, Postcondition::NotRead),
                plain(observation),
                "for {observation:?}"
            );
        }
    }

    /// A submit handler that sets a status AND resets the form: `changed` stays (a form clearing
    /// itself on success is correct) and the reason carries the rest.
    #[test]
    fn a_value_this_action_destroyed_is_not_reported_as_a_plain_delta() {
        let lost = Observation::Compared {
            document_changed: false,
            identity_known: true,
            edits: 2,
            moved: 0,
            focus_moved: true,
            values_lost: 1,
        };
        let got = plain(lost);
        assert_eq!(got.verdict, Verdict::Changed, "the page did move: {got:?}");
        assert_eq!(got.reason, "values_lost");
        let hint = hint_for(got).expect("a values_lost hint");
        assert!(
            hint.contains("values_lost"),
            "the hint names the field to read: {hint}"
        );
        assert!(
            hint.contains("cleared itself"),
            "and states the ambiguity rather than declaring a failure: {hint}"
        );
    }

    /// A Group B rung: with no comparable tree there is no "before" to have lost anything from.
    #[test]
    fn a_lost_value_does_not_outrank_the_document_it_was_measured_in() {
        for (document_changed, identity_known, reason) in [
            (true, true, "document_replaced"),
            (false, false, "identity_unreadable"),
        ] {
            let got = plain(Observation::Compared {
                document_changed,
                identity_known,
                edits: 2,
                moved: 0,
                focus_moved: false,
                values_lost: 3,
            });
            assert_eq!(got.reason, reason);
        }
    }

    /// Collapsing the two would put `value_reverted` on every phone and currency mask on the web.
    #[test]
    fn an_emptied_field_and_a_rewritten_one_do_not_share_a_reason() {
        let emptied = classify(
            compared(0, 0, true),
            Delivered::NOT_PROBED,
            Postcondition::Discarded,
        );
        let rewritten = classify(
            compared(0, 0, true),
            Delivered::NOT_PROBED,
            Postcondition::Rewritten,
        );
        assert_eq!(emptied.verdict, Verdict::NotKept);
        assert_eq!(rewritten.verdict, Verdict::NotKept);
        assert_eq!(emptied.reason, "value_reverted");
        assert_eq!(rewritten.reason, "value_rewritten");
        assert_ne!(
            hint_for(emptied),
            hint_for(rewritten),
            "two recoveries, two hints"
        );
    }

    /// Both hints stop the retry: re-filling produces the same answer and edits the page again.
    #[test]
    fn the_not_kept_hints_forbid_the_refill_and_name_the_field() {
        for postcondition in [Postcondition::Discarded, Postcondition::Rewritten] {
            let assessment = classify(compared(0, 0, true), Delivered::NOT_PROBED, postcondition);
            let hint = hint_for(assessment).expect("a not_kept hint");
            assert!(
                hint.contains("value.actual"),
                "the hint must name what to read: {hint}"
            );
            assert!(
                hint.contains("Do not fill it again"),
                "the reflex here is a second fill, and it has to be forbidden in words: {hint}"
            );
        }
    }

    /// Refusing to aim is a fact, so it survives `--verdict off` and a failed read.
    #[test]
    fn a_refusal_to_dispatch_is_reported_whatever_the_page_read_did() {
        let no_aim = Delivered {
            how: Delivery::NotSettled,
            modal_receiver: false,
            observed_after_ms: None,
        };
        for observation in [
            Observation::ReportingDisabled,
            Observation::ReadFailed,
            Observation::NoBaseline,
            compared(0, 0, false),
        ] {
            let got = mouse(observation, no_aim);
            assert_eq!(got.verdict, Verdict::Unknown, "for {observation:?}");
            assert_eq!(got.reason, "scroll_not_settled");
        }
    }

    /// Focus is out of the delta counts, but without proof of delivery it is the only evidence
    /// the action arrived.
    #[test]
    fn a_focus_move_alone_is_still_something_we_saw() {
        let got = plain(compared(0, 0, true));
        assert_eq!(got.verdict, Verdict::Changed);
        assert_eq!(got.reason, "focus_only");
        // Same for a synthetic click, which performs no hit test.
        let js = Delivered {
            how: Delivery::JsDispatch,
            ..hit(60)
        };
        assert_eq!(mouse(compared(0, 0, true), js).reason, "focus_only");
    }

    /// With proof of delivery, focus is state rather than evidence: ranking it first would make
    /// `no_effect` unreachable for buttons.
    #[test]
    fn a_proven_hit_that_only_moved_focus_is_no_effect() {
        let got = mouse(compared(0, 0, true), hit(60));
        assert_eq!(got.verdict, Verdict::NoEffect);
        assert_eq!(got.reason, "delivered_no_change");
        // A real delta still outranks it.
        assert_eq!(mouse(compared(2, 0, true), hit(60)).reason, "tree_delta");
        assert_eq!(mouse(compared(0, 1, true), hit(60)).reason, "nodes_moved");
    }

    /// `no_effect` needs delivery PROVEN: no hit test (non-mouse command, JS `click()` fallback,
    /// target in an iframe) means the word must not appear.
    #[test]
    fn no_effect_is_refused_without_proof_of_delivery() {
        for how in [Delivery::JsDispatch, Delivery::NotProbed] {
            let delivered = Delivered {
                how,
                modal_receiver: false,
                observed_after_ms: Some(120),
            };
            let got = mouse(compared(0, 0, false), delivered);
            assert_eq!(got.verdict, Verdict::Unchanged, "for {how:?}");
            assert_eq!(got.reason, "identical_tree");
        }
        for observation in [
            Observation::ReportingDisabled,
            Observation::ReadFailed,
            Observation::NoBaseline,
            compared(1, 0, false),
        ] {
            assert_ne!(
                mouse(observation, hit(120)).verdict,
                Verdict::NoEffect,
                "for {observation:?}"
            );
        }
    }

    /// `no_effect` is a claim about a window, so without one it falls back.
    #[test]
    fn no_effect_needs_the_window_it_was_measured_over() {
        let got = mouse(compared(0, 0, false), hit(140));
        assert_eq!(got.verdict, Verdict::NoEffect);
        assert_eq!(got.reason, "delivered_no_change");

        let unmeasured = Delivered {
            observed_after_ms: None,
            ..hit(0)
        };
        assert_eq!(
            mouse(compared(0, 0, false), unmeasured).verdict,
            Verdict::Unchanged
        );
    }

    /// Every "I don't know" — and every claim with a blind spot — tells the agent what to do.
    #[test]
    fn each_uncertain_verdict_carries_a_way_forward() {
        for observation in [
            Observation::ReadFailed,
            Observation::NoBaseline,
            compared(0, 0, false),
        ] {
            assert!(
                hint_for(plain(observation)).is_some(),
                "no hint for {observation:?}"
            );
        }
        for delivered in [
            intercepted(false),
            intercepted(true),
            hit(60),
            Delivered {
                how: Delivery::NotSettled,
                ..Delivered::NOT_PROBED
            },
            Delivered {
                how: Delivery::OffTarget,
                ..Delivered::NOT_PROBED
            },
        ] {
            let assessment = mouse(compared(0, 0, false), delivered);
            assert!(hint_for(assessment).is_some(), "no hint for {delivered:?}");
        }
    }

    /// The hint must name the blind spots, or `no_effect` reads as a claim about the element.
    #[test]
    fn the_no_effect_hint_names_its_blind_spots() {
        let hint = hint_for(mouse(compared(0, 0, false), hit(60))).expect("no_effect hint");
        for blind_spot in ["canvas", "CSS-only", "after the window"] {
            assert!(hint.contains(blind_spot), "hint omits {blind_spot}: {hint}");
        }
        assert!(
            hint.contains("observed_after_ms"),
            "hint must scope itself to the window"
        );
    }
}
