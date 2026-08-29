//! What an action is allowed to claim about itself.
//!
//! Before this, an action that reported no `changed`/`delta` meant one of four things:
//! reporting was off, the session had no baseline yet, the post-action read failed, or the
//! page genuinely did not move. Three of those are "I don't know" and one is an assertion,
//! and nothing in the response told them apart — so an agent could not tell a quiet page
//! from a broken observation.
//!
//! # Why `unchanged` and not "no effect"
//!
//! When an action produces no visible difference, there are two things one could say:
//!
//! - "the action had no effect" — a claim about the action
//! - "the page did not change while I watched" — a claim about the observation
//!
//! Only the second is something we can know. An identical tree is also what you get when
//! the click was swallowed by an overlay, when the effect is invisible to the accessibility
//! tree (a canvas repaint, a CSS class), and when the handler runs after the observation
//! window closed. In all three the action DID something, and answering "no effect" would be
//! wrong in the way that costs the most: an agent that believes it retries, and the retry
//! is a second real click.
//!
//! So the verdict is `unchanged`, and `verdict_hint` spells out those three possibilities.
//!
//! The full taxonomy this is an on-ramp to (`docs/design/verdict-taxonomy.md`) does define
//! `no_effect`, but only behind proof that the action was delivered: a hit test at the
//! dispatched coordinates, or a postcondition read on the acted-on handle. The hit test
//! (`src/hit_test.rs`) now exists for pointer-targeted actions, so `unchanged` has split: a click whose
//! delivery was proven and whose window stayed quiet is `no_effect`, one whose aim point
//! belonged to another element is `intercepted`, and everything with no proof of delivery
//! stays on the old floor.
//!
//! # The ladder, in one place
//!
//! Ordered, first match wins. Three separate rungs have been proposed for this list by three
//! people, so it is written out once here and `classify` follows it literally.
//!
//! The split into two groups is the whole ordering principle. Group A is measured on THIS
//! action's own target — a hit test at the coordinate about to be dispatched, or a read of the
//! handle that was just written to. None of it depends on the stored tree and the live tree
//! being comparable, so it does not compete with Group B, it precedes it. Group B is every
//! claim that does depend on that comparison.
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
//! `target_hit` (15) is the one delivery reading that stays in Group B, and the asymmetry with
//! (3) is deliberate: it is not a verdict on its own, it is the LICENCE for `no_effect`, and
//! `no_effect` is a claim that a comparable tree stayed quiet. Promoting it would make a click
//! that replaced the document report `no_effect` — the strongest word in the vocabulary, about
//! a page that is no longer there.
//!
//! # Why the confirming half of Group A sits at (11) and not at (6)
//!
//! A read-back is one measurement with two outcomes, so both belong to Group A — but they do
//! not preempt the same things, because a failure and a success are not symmetric in what the
//! caller has to do about them.
//!
//! A FAILED write (4, 5) preempts everything: nothing downstream of it was going to work, and
//! the recovery — read `value.actual`, do not send the same write again — is valid whatever the
//! rest of the page did. So it outranks even "I cannot tell which document this is".
//!
//! A CONFIRMED write does not preempt anything that describes the page. It is the expected
//! outcome, it answers only "the field holds what was asked for", and every rung above it
//! answers a question the caller still has: which document is on screen (6, 7), what else moved
//! (9, 10), and whether the page dropped a value it held before (8). So a fill whose write
//! landed AND whose delta shows the field's own line still reports `tree_delta`: it names what
//! changed and where, which is strictly more than "the write stuck". `value_kept` is what is
//! left when the tree cannot show it.
//!
//! Which is not a rare corner. A secret field renders as a fixed marker in the tree
//! (`snapshot_secret::MARKER`, deliberately fixed so an unchanged secret does not read as a
//! change), so re-filling one produces NO value change to see: the ladder fell through to (16)
//! and answered `changed / focus_only` — "nothing moved but focus, which is the only sign the
//! action arrived" — on a response whose own `value` object said `verbatim: true`. Two claims
//! about one action, and the weaker one in the field an agent branches on. Measured on
//! `snapshot_secret_values.html`: `verdict:"changed"`, `verdict_reason:"focus_only"`,
//! `value:{redacted:true, verbatim:true, actual_length:16}`.
//!
//! It still outranks (12, 13, 14), and for the reason Group A exists at all: those three say
//! nothing was compared, and a measurement outranks an absence of one. The same rung ranked the
//! other way for a FAILED write already — `not_kept` is reported through `--verdict off` and
//! through a failed read — and a read-back that counts as evidence when it fails and not when
//! it succeeds is exactly the asymmetry this module exists to remove.
//!
//! Group A over `document_replaced` (7) is what fixed the demo sequence. `goto` keeps
//! `last_snapshot` while clearing `uid_map` (deliberately — it is what lets `diff` say
//! `document_changed` instead of erroring), so the first action after any `goto` that follows
//! an `inspect` compares against a snapshot from the PREVIOUS page and reports
//! `navigated / document_replaced`. That described the `goto`, not the action. Measured on
//! `click_overlay.html`: `delivery:"intercepted"` and `verdict:"navigated"` on one response,
//! with the interception — the thing the caller has to act on — nowhere in the verdict. Same
//! artefact hid `not_kept` on a `goto` → `fill`. A real navigation still reports `navigated`
//! whenever delivery is unknown or unprobed, which is every command that dispatches no mouse
//! event and writes no value, and the `changed`/`delta`/identity fields stay on the response
//! either way.

use std::fmt;

/// The claim attached to an action's response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verdict {
    /// The page moved in a way we can point at.
    Changed,
    /// The document was replaced. Stored uids are dead.
    Navigated,
    /// Another element occupied the point the mouse event was aimed at, so that element
    /// received it. A claim about who got the event, never about what they did with it.
    Intercepted,
    /// The write reached the element and the element did not hold it when we looked back.
    /// A claim about what the page kept, never about why it did not keep it.
    NotKept,
    /// The event was proven delivered to the target and the page stayed still while we
    /// watched. Only ever emitted with the window it was measured over.
    NoEffect,
    /// We looked and the tree was identical. A statement about the observation, not about
    /// the action — see the module docs for why the difference is the whole point.
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
    /// Why, in one machine-readable token. Never absent: a verdict without its reason is
    /// the same ambiguity one level up.
    pub reason: &'static str,
    /// Whether the page could be seen at all. Written by `classify` from the observation, read
    /// only by `next_for` and the hint tables; it reaches no response field.
    pub page: PageSight,
}

/// Classify an observation, what the hit test said, and what the acted-on handle held.
/// Pure, ordered, first match wins.
///
/// # Why `intercepted` sits ABOVE `tree_delta`
///
/// Everywhere else in this ladder, what we saw on the page outranks what we inferred. The
/// interception rung inverts that on purpose, and the asymmetry is the point: a delta is a
/// post-dispatch observation which our own action can explain, while an interception is a
/// PRE-dispatch measurement that the action cannot have caused. When a covered button is
/// clicked, the scrim's own handler usually moves the page — so the delta is real, belongs to
/// the scrim, and says nothing about the button the caller named. Ranking `changed` first
/// would hand back "the page moved" for every intercepted click, which is exactly the
/// false success this rung exists to remove.
///
/// The receiver still may have done something useful; `intercepted` claims only who got the
/// event. The delta stays on the response for the caller to read.
///
/// # Why `not_kept` sits ABOVE `tree_delta` too
///
/// The same asymmetry, from the other side of the dispatch. A delta is a whole-page
/// observation that our own action can explain; a postcondition is a read of the ONE handle
/// the caller named, and a handle that does not hold the requested value cannot be explained
/// by the effect the action was asked to have. A fill focuses its field, and focus alone is
/// a `changed` (see `focus_only` below) — so on `form_value_microtask_revert.html` the page
/// emptied the field and the verdict was `changed / focus_only`, a success beside a
/// `verbatim: false` nobody had to read. That is the false success this rung removes.
///
/// Accepted trade-off, the same one already accepted for `intercepted`: a field the page
/// reverted on a page that genuinely moved elsewhere reports `not_kept` and not `changed`.
/// The delta stays on the response, and the write not landing is the fact the caller has to
/// act on first — everything downstream of it was going to be wrong.
///
/// It sits BELOW nothing except the other Group A rungs: nothing was dispatched, or another
/// element took the event, each of which explains a missing value rather than being explained
/// by it. It sits ABOVE the identity and navigation rungs because a read of the handle that
/// was just written to does not depend on two trees being comparable — see the ladder in the
/// module docs for the full ordering and why `target_hit` is the exception.
///
/// # Why `value_kept` sits BELOW `tree_delta`, where `not_kept` sits above it
///
/// When the write landed AND the tree shows it, the tree wins: `tree_delta` names the node and
/// the line that changed, which is everything `value_kept` says plus where to look. The
/// read-back rung is for when the tree CANNOT show it — a secret field renders as a fixed
/// marker, so re-filling one leaves no value change to diff, and the ladder used to fall
/// through to `focus_only` beside a `verbatim: true`.
///
/// The reverse ranking for a failed write is not an inconsistency: a failure has to preempt the
/// page's own news because nothing downstream of it works, while a success does not — it answers
/// none of the questions (which document, what else moved, what was dropped) that the rungs
/// above it answer. The module docs argue this at length.
#[must_use]
pub const fn classify(
    observation: Observation,
    delivered: Delivered,
    postcondition: Postcondition,
) -> Assessment {
    let (verdict, reason) = match (observation, delivered.how) {
        // ---- GROUP A: measured on this action's own target. -------------------------------
        // Known whether or not the page was re-read, and whether or not the two trees can be
        // compared, so this whole group precedes Group B rather than competing with it.
        // Refusing to aim, aiming at another element, and a field that threw the write away
        // are facts; every rung below is either a comparison or an admission of ignorance.
        (_, Delivery::NotSettled) => (Verdict::Unknown, "scroll_not_settled"),
        (_, Delivery::OffTarget) => (Verdict::Unknown, "aim_point_off_target"),
        (_, Delivery::Intercepted) => (
            Verdict::Intercepted,
            if delivered.modal_receiver { "modal_dialog" } else { "hit_test_receiver" },
        ),
        // One verdict, two reasons: the recovery differs, and `value_reverted` on a phone
        // mask would be a machine-readable token saying something false.
        (_, _) if matches!(postcondition, Postcondition::Discarded) => {
            (Verdict::NotKept, "value_reverted")
        }
        (_, _) if matches!(postcondition, Postcondition::Rewritten) => {
            (Verdict::NotKept, "value_rewritten")
        }
        // ---- GROUP B: depends on comparing the stored tree with the live one. -------------
        // Identity first within the group: without it the uid spaces may belong to different
        // documents and every count below is meaningless.
        (Observation::Compared { identity_known: false, .. }, _) => (Verdict::Unknown, "identity_unreadable"),
        (Observation::Compared { document_changed: true, .. }, _) => (Verdict::Navigated, "document_replaced"),
        // A field this page held before the action holds nothing after it. Above `tree_delta`
        // because it IS one — the field's own line is part of the delta — and the reason token
        // is the only thing that separates "the page moved" from "the page moved and dropped
        // what you had typed". The verdict stays `changed` on purpose: a form that clears
        // itself on a successful submit is correct, extremely common behaviour, and `not_kept`
        // there would report a failure for a form that worked. `not_kept` stays reserved for
        // the write THIS action made not sticking, which is a different fact with a different
        // recovery; this is about a value an EARLIER action wrote. The evidence rides on the
        // response as `values_lost`, so a caller who disagrees with the wording can read it.
        (Observation::Compared { values_lost, .. }, _) if values_lost > 0 => {
            (Verdict::Changed, "values_lost")
        }
        (Observation::Compared { edits, .. }, _) if edits > 0 => (Verdict::Changed, "tree_delta"),
        (Observation::Compared { moved, .. }, _) if moved > 0 => (Verdict::Changed, "nodes_moved"),
        // ---- GROUP A again: the same read-back, this time confirming. ---------------------
        // Below the tree rungs above (they name what changed and where, which is more than
        // "the write stuck") and above the three below (they compared nothing at all). The
        // asymmetry with `not_kept` is argued in the module docs.
        (_, _) if matches!(postcondition, Postcondition::Kept) => (Verdict::Changed, "value_kept"),
        // ---- GROUP B: no comparison was available to say anything. ------------------------
        (Observation::ReportingDisabled, _) => (Verdict::NotChecked, "reporting_disabled"),
        (Observation::ReadFailed, _) => (Verdict::Unknown, "read_failed"),
        (Observation::NoBaseline, _) => (Verdict::Unknown, "no_baseline"),
        // The strong word, and the only rung that needs two independent facts: the event was
        // proven delivered to the target, and nothing but focus moved within a window we can
        // name. Without the window it stays on the floor below.
        //
        // This is placed ABOVE `focus_only`, which is the one ordering the hit test changes.
        // Focus churn used to be reported as `changed` because it was the closest thing to
        // proof of delivery available — the only signal separating "landed on an inert
        // element" from "never arrived". A proven hit IS that proof, so on this path focus
        // goes back to being what it is: a `focus:{from,to}` field, not a change to the page.
        // Keeping `focus_only` first here would make `no_effect` unreachable for anything
        // focusable, since clicking a button always focuses it.
        (Observation::Compared { edits: 0, moved: 0, .. }, Delivery::TargetHit)
            if delivered.observed_after_ms.is_some() =>
        {
            (Verdict::NoEffect, "delivered_no_change")
        }
        // Without that proof, focus churn is still the page reacting to the action, and
        // reporting it as `unchanged` would throw the signal away.
        (Observation::Compared { focus_moved: true, .. }, _) => (Verdict::Changed, "focus_only"),
        (Observation::Compared { .. }, _) => (Verdict::Unchanged, "identical_tree"),
    };
    // Carried, not classified: only `next_for` and the hints read it. A Group A rung can be a
    // true statement about the handle while the page around it went unobserved, and that is the
    // one thing the verdict word cannot say.
    let page = match observation {
        Observation::ReadFailed => PageSight::Unreadable,
        Observation::ReportingDisabled | Observation::NoBaseline | Observation::Compared { .. } => {
            PageSight::Readable
        }
    };
    Assessment { verdict, reason, page }
}

/// The human gloss, the next-step token and the two hint tables, moved to `verdict_words` for
/// the 1000-line file cap and re-exported here: all four are read off an `Assessment` this
/// module produced, and a caller should not have to know they live next door.
pub use crate::verdict_words::{gloss, hint_for, next_for, short_hint};

/// What `classify` reads, and the sight it carries on the assessment: moved to
/// `verdict_evidence` for the 1000-line file cap and re-exported here, so a caller still writes
/// `crate::verdict::Postcondition` next to the ladder that ranks it.
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

    /// A mouse action whose delivery was proven, observed over a window we can name.
    const fn hit(observed_after_ms: u64) -> Delivered {
        Delivered {
            how: Delivery::TargetHit,
            modal_receiver: false,
            observed_after_ms: Some(observed_after_ms),
        }
    }

    const fn intercepted(modal_receiver: bool) -> Delivered {
        Delivered { how: Delivery::Intercepted, modal_receiver, observed_after_ms: None }
    }

    /// Shorthand for the pre-hit-test callers: no mouse event, or no answer.
    fn plain(observation: Observation) -> Assessment {
        classify(observation, Delivered::NOT_PROBED, Postcondition::NotRead)
    }

    /// A mouse action: the hit test answered, and there is no value to read back.
    fn mouse(observation: Observation, delivered: Delivered) -> Assessment {
        classify(observation, delivered, Postcondition::NotRead)
    }

    /// A fill whose read-back disagreed with the request. No mouse event, so no hit test.
    fn reverted(observation: Observation) -> Assessment {
        classify(observation, Delivered::NOT_PROBED, Postcondition::Discarded)
    }

    /// The four silences the change report used to collapse into one.
    #[test]
    fn every_silent_case_names_itself() {
        let cases = [
            (Observation::ReportingDisabled, Verdict::NotChecked, "reporting_disabled"),
            (Observation::ReadFailed, Verdict::Unknown, "read_failed"),
            (Observation::NoBaseline, Verdict::Unknown, "no_baseline"),
            (compared(0, 0, false), Verdict::Unchanged, "identical_tree"),
        ];
        let mut seen = std::collections::HashSet::new();
        for (observation, verdict, reason) in cases {
            let got = plain(observation);
            assert_eq!(got.verdict, verdict, "for {observation:?}");
            assert_eq!(got.reason, reason, "for {observation:?}");
            assert!(seen.insert((got.verdict, got.reason)), "two silences share a name: {got:?}");
        }
    }

    /// An unreadable identity outranks the counts: they were computed across uid spaces
    /// that may belong to different documents.
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

    /// The deliberate asymmetry: a pre-dispatch measurement outranks a post-dispatch one.
    /// The scrim's own handler moves the page, so the delta is real and belongs to the
    /// scrim — reporting `changed` there is the false success this rung removes.
    #[test]
    fn an_interception_outranks_the_delta_it_caused() {
        for observation in [compared(4, 0, true), compared(0, 0, true), compared(0, 0, false)] {
            let got = mouse(observation, intercepted(false));
            assert_eq!(got.verdict, Verdict::Intercepted, "for {observation:?}");
            assert_eq!(got.reason, "hit_test_receiver");
        }
        assert_eq!(mouse(compared(1, 0, false), intercepted(true)).reason, "modal_dialog");
    }

    /// The whole Group A / Group B split, on the sequence every demo uses.
    ///
    /// `document_replaced` means "I cannot compare two trees"; a hit test is measured on this
    /// action's own target and does not need them comparable. And the artefact is common: `goto`
    /// keeps `last_snapshot` while clearing `uid_map`, so the first action after a `goto` that
    /// followed an `inspect` compares against the PREVIOUS page. Measured on
    /// `click_overlay.html`: `delivery:"intercepted"` beside `verdict:"navigated"`, with the
    /// interception — the only thing the caller can act on — absent from the verdict.
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
        assert_eq!(mouse(navigated, intercepted(false)).verdict, Verdict::Intercepted);
        assert_eq!(mouse(navigated, intercepted(true)).reason, "modal_dialog");
        // Nothing dispatched is a fact about this action too, whatever the document did.
        for how in [Delivery::NotSettled, Delivery::OffTarget] {
            let got = mouse(navigated, Delivered { how, ..Delivered::NOT_PROBED });
            assert_eq!(got.verdict, Verdict::Unknown, "for {how:?}");
            assert_ne!(got.reason, "document_replaced");
        }
    }

    /// The other half of that, and the reason `target_hit` is not in Group A: it is not a
    /// verdict, it is the licence for `no_effect`, which is a claim about a tree that stayed
    /// quiet. On a replaced document there is no such tree.
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
        for how in [Delivery::TargetHit, Delivery::JsDispatch, Delivery::NotProbed] {
            let delivered =
                Delivered { how, modal_receiver: false, observed_after_ms: Some(60) };
            let got = classify(navigated, delivered, Postcondition::NotRead);
            assert_eq!(got.verdict, Verdict::Navigated, "for {how:?}");
            assert_eq!(got.reason, "document_replaced");
        }
    }

    /// The bug this rung exists for, frozen. On `form_value_microtask_revert.html` the page
    /// emptied the field and the only page-level movement was the focus the fill itself
    /// caused — so the ladder answered `changed / focus_only`, a success sitting beside a
    /// `verbatim: false` that nothing forced the reader to notice.
    #[test]
    fn a_value_the_page_did_not_keep_outranks_the_delta_beside_it() {
        for observation in [compared(0, 0, true), compared(4, 0, true), compared(0, 2, false)] {
            let got = reverted(observation);
            assert_eq!(got.verdict, Verdict::NotKept, "for {observation:?}");
            assert_eq!(got.reason, "value_reverted");
        }
    }

    /// The read-back happens inside the action, so it is evidence whether or not the page was
    /// re-read afterwards — `--verdict off` still knows the value was thrown away.
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

    /// A read of the handle that was just written to does not depend on two trees being
    /// comparable either, so it precedes the identity rungs for the same reason the hit test
    /// does. Verified on the same artefact: a `goto` → `fill` on a reverting page answered
    /// `navigated / document_replaced` while `verbatim:false` sat on the response.
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
        // Nothing was dispatched, or another element took it: both are why the value is
        // missing, and both tell the agent something it can act on that `not_kept` does not.
        for how in [Delivery::NotSettled, Delivery::OffTarget, Delivery::Intercepted] {
            let delivered = Delivered { how, ..Delivered::NOT_PROBED };
            let got = classify(compared(0, 0, false), delivered, Postcondition::Discarded);
            assert_ne!(got.verdict, Verdict::NotKept, "for {how:?}");
        }
    }

    /// A fill whose value change IS visible in the tree keeps reporting the tree: `tree_delta`
    /// names the node and the line, which is everything `value_kept` says plus where to look.
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
        // And the two rungs that say which document this is: a confirmed write on a page that
        // was replaced describes a field that is gone, and `inspect` is the only way forward.
        for (document_changed, identity_known, reason) in
            [(true, true, "document_replaced"), (false, false, "identity_unreadable")]
        {
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

    /// The bug this rung exists for, frozen. A secret field renders as a fixed marker, so
    /// re-filling one changes nothing the diff can see: the ladder fell through to
    /// `changed / focus_only` — "the only sign the action arrived" — on a response whose own
    /// `value` object said the write was read back verbatim.
    #[test]
    fn a_confirmed_write_the_tree_cannot_show_is_not_reported_as_focus_alone() {
        for observation in [compared(0, 0, true), compared(0, 0, false)] {
            let got = classify(observation, Delivered::NOT_PROBED, Postcondition::Kept);
            assert_eq!(got.verdict, Verdict::Changed, "the write did change the page: {got:?}");
            assert_eq!(got.reason, "value_kept");
            assert_ne!(plain(observation).reason, got.reason, "and the old answer is gone");
        }
    }

    /// The same measurement ranked the same way in both directions: `not_kept` is reported
    /// through `--verdict off` and through a failed read, so a confirmed write must be too.
    /// Evidence that counts when it fails and not when it succeeds is the asymmetry this
    /// module exists to remove.
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

    /// The verdict and the next step have different subjects, and this is the pair that proves
    /// it: the field took the write (measured, so `changed / value_kept` stands) and the page
    /// after it could not be read (so "carry on" would mean carrying on against a page nobody
    /// has seen). `read_failed` used to carry that blindness; the Group A rung now outranks it,
    /// so `next` and the hints carry it instead.
    #[test]
    fn a_confirmed_write_on_an_unread_page_still_says_inspect() {
        use crate::verdict_words::Next;

        let blind = classify(Observation::ReadFailed, Delivered::NOT_PROBED, Postcondition::Kept);
        assert_eq!(blind.verdict, Verdict::Changed);
        assert_eq!(blind.reason, "value_kept");
        assert_eq!(blind.page, PageSight::Unreadable);
        assert_eq!(next_for(blind), Next::Inspect, "carrying on while blind is the one refusal");
        let hint = hint_for(blind).expect("the blindness has to be said somewhere");
        assert!(hint.contains("inspect"), "{hint}");
        assert!(hint.contains("what else moved"), "and name what is unknown: {hint}");

        // The same rung with the page in hand keeps `proceed`, and needs no advice at all.
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

    /// The rule is written over the token, not over `value_kept`, so a rung added later cannot
    /// inherit "carry on while blind" by accident. Nothing else may change: a `retry` comes only
    /// from a rung that dispatched nothing, and `--verdict off` is a silence the caller chose.
    #[test]
    fn no_verdict_answers_proceed_about_a_page_it_could_not_read() {
        use crate::verdict_words::Next;

        for postcondition in
            [Postcondition::NotRead, Postcondition::Kept, Postcondition::Discarded, Postcondition::Rewritten]
        {
            for how in [
                Delivery::TargetHit,
                Delivery::Intercepted,
                Delivery::OffTarget,
                Delivery::NotSettled,
                Delivery::JsDispatch,
                Delivery::NotProbed,
            ] {
                let delivered = Delivered { how, modal_receiver: false, observed_after_ms: Some(60) };
                let blind = classify(Observation::ReadFailed, delivered, postcondition);
                assert_ne!(next_for(blind), Next::Proceed, "{} / {}", blind.verdict, blind.reason);
            }
        }
        // And the retry survives it: nothing was dispatched, so there is nothing to be blind about.
        let not_settled = Delivered { how: Delivery::NotSettled, ..Delivered::NOT_PROBED };
        let got = classify(Observation::ReadFailed, not_settled, Postcondition::NotRead);
        assert_eq!(next_for(got), Next::Retry);
    }

    /// It is still only a postcondition: nothing dispatched, or another element taking the
    /// event, explains a value more than the value explains them.
    #[test]
    fn a_confirmed_write_does_not_outrank_a_delivery_that_failed() {
        for how in [Delivery::NotSettled, Delivery::OffTarget, Delivery::Intercepted] {
            let delivered = Delivered { how, ..Delivered::NOT_PROBED };
            let got = classify(compared(0, 0, true), delivered, Postcondition::Kept);
            assert_ne!(got.reason, "value_kept", "for {how:?}");
        }
    }

    /// A command with nothing to read back is untouched by the rung: every click, press and
    /// drag keeps the verdict it had.
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

    /// S3, the archetype: a submit handler that sets a status AND resets the form. Both
    /// `ok:true` and `changed / tree_delta` were true, and neither said the field the agent had
    /// just filled was empty again. The verdict word stays `changed` — a form that clears itself
    /// on a successful submit is correct behaviour, so calling it `not_kept` would report a
    /// failure for a form that worked — and the reason carries what the counts could not.
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
        assert!(hint.contains("values_lost"), "the hint names the field to read: {hint}");
        assert!(
            hint.contains("cleared itself"),
            "and states the ambiguity rather than declaring a failure: {hint}"
        );
    }

    /// It is still a Group B rung: with no comparable tree there is no "before" to have lost
    /// anything from, so the rungs that say which document this is come first.
    #[test]
    fn a_lost_value_does_not_outrank_the_document_it_was_measured_in() {
        for (document_changed, identity_known, reason) in
            [(true, true, "document_replaced"), (false, false, "identity_unreadable")]
        {
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

    /// A rewrite is the same verdict and a different reason: the field holds something, the
    /// recovery is to read it rather than to conclude the write cannot land. Collapsing the
    /// two would put `value_reverted` on every phone and currency mask on the web.
    #[test]
    fn an_emptied_field_and_a_rewritten_one_do_not_share_a_reason() {
        let emptied = classify(compared(0, 0, true), Delivered::NOT_PROBED, Postcondition::Discarded);
        let rewritten =
            classify(compared(0, 0, true), Delivered::NOT_PROBED, Postcondition::Rewritten);
        assert_eq!(emptied.verdict, Verdict::NotKept);
        assert_eq!(rewritten.verdict, Verdict::NotKept);
        assert_eq!(emptied.reason, "value_reverted");
        assert_eq!(rewritten.reason, "value_rewritten");
        assert_ne!(hint_for(emptied), hint_for(rewritten), "two recoveries, two hints");
    }

    /// Both hints have one job: stop the retry. Re-filling produces the same answer and edits
    /// the page a second time, so each must name the field to read instead.
    #[test]
    fn the_not_kept_hints_forbid_the_refill_and_name_the_field() {
        for postcondition in [Postcondition::Discarded, Postcondition::Rewritten] {
            let assessment =
                classify(compared(0, 0, true), Delivered::NOT_PROBED, postcondition);
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

    /// Refusing to aim is a fact, not an absence of one: it survives `--verdict off` and a
    /// failed read, both of which would otherwise report ignorance about the page instead.
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

    /// Focus churn is kept out of the delta counts, but it is still the page reacting.
    /// Calling that `unchanged` would discard the only evidence the action was delivered.
    /// Without proof of delivery, focus is the only evidence the action arrived at all.
    #[test]
    fn a_focus_move_alone_is_still_something_we_saw() {
        let got = plain(compared(0, 0, true));
        assert_eq!(got.verdict, Verdict::Changed);
        assert_eq!(got.reason, "focus_only");
        // Same for a synthetic click, which performs no hit test.
        let js = Delivered { how: Delivery::JsDispatch, ..hit(60) };
        assert_eq!(mouse(compared(0, 0, true), js).reason, "focus_only");
    }

    /// With proof of delivery it is not evidence any more, it is state — and clicking anything
    /// focusable moves it, so ranking it first would make `no_effect` unreachable for buttons.
    #[test]
    fn a_proven_hit_that_only_moved_focus_is_no_effect() {
        let got = mouse(compared(0, 0, true), hit(60));
        assert_eq!(got.verdict, Verdict::NoEffect);
        assert_eq!(got.reason, "delivered_no_change");
        // A real delta still outranks it: the page moved, whatever focus did.
        assert_eq!(mouse(compared(2, 0, true), hit(60)).reason, "tree_delta");
        assert_eq!(mouse(compared(0, 1, true), hit(60)).reason, "nodes_moved");
    }

    /// `no_effect` needs delivery PROVEN. Without a hit test — every non-mouse command, the
    /// JS `click()` fallback, a target inside an iframe — the word must not appear.
    #[test]
    fn no_effect_is_refused_without_proof_of_delivery() {
        for how in [Delivery::JsDispatch, Delivery::NotProbed] {
            let delivered =
                Delivered { how, modal_receiver: false, observed_after_ms: Some(120) };
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

    /// `no_effect` is a claim about a window. Without the window it falls back rather than
    /// being phrased as a claim about the element.
    #[test]
    fn no_effect_needs_the_window_it_was_measured_over() {
        let got = mouse(compared(0, 0, false), hit(140));
        assert_eq!(got.verdict, Verdict::NoEffect);
        assert_eq!(got.reason, "delivered_no_change");

        let unmeasured = Delivered { observed_after_ms: None, ..hit(0) };
        assert_eq!(mouse(compared(0, 0, false), unmeasured).verdict, Verdict::Unchanged);
    }

    /// Every "I don't know" — and every claim with a blind spot — tells the agent what to do.
    #[test]
    fn each_uncertain_verdict_carries_a_way_forward() {
        for observation in [Observation::ReadFailed, Observation::NoBaseline, compared(0, 0, false)] {
            assert!(hint_for(plain(observation)).is_some(), "no hint for {observation:?}");
        }
        for delivered in [
            intercepted(false),
            intercepted(true),
            hit(60),
            Delivered { how: Delivery::NotSettled, ..Delivered::NOT_PROBED },
            Delivered { how: Delivery::OffTarget, ..Delivered::NOT_PROBED },
        ] {
            let assessment = mouse(compared(0, 0, false), delivered);
            assert!(hint_for(assessment).is_some(), "no hint for {delivered:?}");
        }
    }

    /// The `no_effect` hint must name what the measurement cannot see, or the word reads as
    /// a claim about the element rather than about a window.
    #[test]
    fn the_no_effect_hint_names_its_blind_spots() {
        let hint = hint_for(mouse(compared(0, 0, false), hit(60))).expect("no_effect hint");
        for blind_spot in ["canvas", "CSS-only", "after the window"] {
            assert!(hint.contains(blind_spot), "hint omits {blind_spot}: {hint}");
        }
        assert!(hint.contains("observed_after_ms"), "hint must scope itself to the window");
    }
}
