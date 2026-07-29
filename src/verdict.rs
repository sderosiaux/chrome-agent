//! What an action is allowed to claim about itself.
//!
//! Before this, an action that reported no `changed`/`delta` meant one of four things:
//! reporting was off, the session had no baseline yet, the post-action read failed, or the
//! page genuinely did not move. Three of those are "I don't know" and one is an assertion,
//! and nothing in the response told them apart — so an agent could not tell a quiet page
//! from a broken observation.
//!
//! This is the on-ramp to the full verdict taxonomy, not the taxonomy itself. It classifies
//! only what we already measure: the accessibility-tree delta and the document identity.
//! It deliberately does not emit `no_effect`, which the spec defines as requiring proof of
//! delivery (a hit test, or a postcondition read on the acted-on handle). Until that proof
//! exists, an empty delta is `no_delta` — a statement about the observation, not about the
//! action.

use std::fmt;

/// What we observed after an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    /// `--verdict off`: the page was never re-read.
    ReportingDisabled,
    /// The post-action read failed, so there is nothing to compare.
    ReadFailed,
    /// First action of this session on this page: nothing to compare against.
    NoBaseline,
    /// A comparison ran. The fields are the parts of it that decide the verdict.
    Compared {
        document_changed: bool,
        identity_known: bool,
        edits: usize,
        moved: usize,
        focus_moved: bool,
    },
}

/// The claim attached to an action's response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verdict {
    /// The page moved in a way we can point at.
    Changed,
    /// The document was replaced. Stored uids are dead.
    Navigated,
    /// We looked and the tree was identical. NOT a claim that the action did nothing:
    /// without proof of delivery we cannot distinguish "nothing happened" from "the click
    /// hit an overlay" or "the handler runs in 600ms".
    NoDelta,
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
            Self::NoDelta => "no_delta",
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
}

/// Classify an observation. Pure, first match wins.
#[must_use]
pub const fn classify(observation: Observation) -> Assessment {
    let (verdict, reason) = match observation {
        Observation::ReportingDisabled => (Verdict::NotChecked, "reporting_disabled"),
        Observation::ReadFailed => (Verdict::Unknown, "read_failed"),
        Observation::NoBaseline => (Verdict::Unknown, "no_baseline"),
        // Identity first: without it the uid spaces may belong to different documents, and
        // every count below is meaningless.
        Observation::Compared { identity_known: false, .. } => (Verdict::Unknown, "identity_unreadable"),
        Observation::Compared { document_changed: true, .. } => (Verdict::Navigated, "document_replaced"),
        Observation::Compared { edits, .. } if edits > 0 => (Verdict::Changed, "tree_delta"),
        Observation::Compared { moved, .. } if moved > 0 => (Verdict::Changed, "nodes_moved"),
        // Focus churn is subtracted from the delta on purpose (every click focuses
        // something), but it is still the page reacting to the action — the closest thing
        // to proof of delivery available without a hit test. Reporting it as `no_delta`
        // would throw away the one signal that separates "the click landed on an inert
        // element" from "the click never arrived".
        Observation::Compared { focus_moved: true, .. } => (Verdict::Changed, "focus_only"),
        Observation::Compared { .. } => (Verdict::NoDelta, "identical_tree"),
    };
    Assessment { verdict, reason }
}

/// What the agent should do next, when the verdict alone does not say.
#[must_use]
pub fn hint_for(assessment: Assessment) -> Option<&'static str> {
    match assessment.reason {
        "no_baseline" => Some(
            "No snapshot existed before this action, so nothing could be compared. Run `inspect` to establish one; the next action on this page will report what changed.",
        ),
        "read_failed" => Some(
            "The action ran, but reading the page afterwards failed, so what it did is unknown. Run `inspect` to see the current state.",
        ),
        "identical_tree" => Some(
            "The accessibility tree is unchanged. The action may have been absorbed by an overlay, or its effect may be invisible to the tree (canvas, styling), or slower than the observation window.",
        ),
        _ => None,
    }
}

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
        }
    }

    /// The four silences the change report used to collapse into one.
    #[test]
    fn every_silent_case_names_itself() {
        let cases = [
            (Observation::ReportingDisabled, Verdict::NotChecked, "reporting_disabled"),
            (Observation::ReadFailed, Verdict::Unknown, "read_failed"),
            (Observation::NoBaseline, Verdict::Unknown, "no_baseline"),
            (compared(0, 0, false), Verdict::NoDelta, "identical_tree"),
        ];
        let mut seen = std::collections::HashSet::new();
        for (observation, verdict, reason) in cases {
            let got = classify(observation);
            assert_eq!(got.verdict, verdict, "for {observation:?}");
            assert_eq!(got.reason, reason, "for {observation:?}");
            assert!(seen.insert((got.verdict, got.reason)), "two silences share a name: {got:?}");
        }
    }

    /// An unreadable identity outranks the counts: they were computed across uid spaces
    /// that may belong to different documents.
    #[test]
    fn an_unreadable_identity_is_unknown_whatever_the_counts_say() {
        let got = classify(Observation::Compared {
            document_changed: false,
            identity_known: false,
            edits: 40,
            moved: 3,
            focus_moved: true,
        });
        assert_eq!(got.verdict, Verdict::Unknown);
        assert_eq!(got.reason, "identity_unreadable");
    }

    #[test]
    fn a_replaced_document_is_navigated_not_changed() {
        let got = classify(Observation::Compared {
            document_changed: true,
            identity_known: true,
            edits: 0,
            moved: 0,
            focus_moved: false,
        });
        assert_eq!(got.verdict, Verdict::Navigated);
    }

    #[test]
    fn edits_and_reorders_both_count_as_changed() {
        assert_eq!(classify(compared(1, 0, false)).reason, "tree_delta");
        assert_eq!(classify(compared(0, 2, false)).reason, "nodes_moved");
    }

    /// Focus churn is kept out of the delta counts, but it is still the page reacting.
    /// Calling that `no_delta` would discard the only evidence the action was delivered.
    #[test]
    fn a_focus_move_alone_is_still_something_we_saw() {
        let got = classify(compared(0, 0, true));
        assert_eq!(got.verdict, Verdict::Changed);
        assert_eq!(got.reason, "focus_only");
    }

    /// `no_effect` is the spec's word for "delivery proven, window quiet, attribution
    /// clean". None of those are measured here, so the word must not appear.
    #[test]
    fn no_verdict_claims_the_action_had_no_effect() {
        for observation in [
            Observation::ReportingDisabled,
            Observation::ReadFailed,
            Observation::NoBaseline,
            compared(0, 0, false),
            compared(1, 0, false),
        ] {
            let got = classify(observation);
            assert_ne!(got.verdict.as_str(), "no_effect", "for {observation:?}");
        }
    }

    /// Every "I don't know" tells the agent how to find out.
    #[test]
    fn each_uncertain_verdict_carries_a_way_forward() {
        for observation in [Observation::ReadFailed, Observation::NoBaseline, compared(0, 0, false)] {
            let assessment = classify(observation);
            assert!(hint_for(assessment).is_some(), "no hint for {observation:?}");
        }
    }
}
