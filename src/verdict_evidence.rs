//! The evidence a verdict is made from, and the one thing it carries but never prints.
//!
//! Split out of `verdict.rs` for the repo's 1000-line file cap and re-exported from it, so every
//! call site stays `crate::verdict::Delivery` / `crate::verdict::Postcondition`. The ladder, the
//! verdict words and the classifier stay next door: this file is only what `classify` reads and
//! what it hands on.
//!
//! Every type here has a floor variant that means "no evidence" — `Delivery::NotProbed`,
//! `Postcondition::NotRead` — and never a claim. That is the property the ladder depends on.

use std::fmt;

/// What the hit test said about a pointer-targeted action's delivery before dispatch.
///
/// `NotProbed` is the floor and the default: every command that dispatches no pointer event —
/// and every mouse path where the probe could not answer, or answered about a document we
/// cannot hit-test (a target inside an iframe) — reports it. It is an absence of evidence
/// and never licenses a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// The aim point resolved to the target itself, a descendant, its label's control, or
    /// its shadow host. This is the only value that licenses `no_effect`.
    TargetHit,
    /// The aim point resolved to an element outside the target's flat subtree.
    Intercepted,
    /// No point on the target could be aimed at, and the reading is STABLE — two consecutive
    /// probes agreed. Two shapes reach here: a point outside every one of the element's own
    /// client rects, and a point that stopped moving outside the viewport (a `position: fixed`
    /// wall pinned past an edge, a document whose scroll is locked). Nothing was dispatched,
    /// and a repeat measures the same coordinate.
    OffTarget,
    /// Two consecutive readings of the aim point disagreed: it was still moving when the settle
    /// budget ran out. Nothing was dispatched, and the miss is TRANSIENT — this is the one
    /// reading a repeat can fix.
    NotSettled,
    /// The action went through a JS `click()`/`MouseEvent`, which performs no hit test.
    /// Interception is not undetected here, it is inapplicable.
    JsDispatch,
    /// No hit test was run, or its answer does not cover the document that matters.
    NotProbed,
}

impl Delivery {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TargetHit => "target_hit",
            Self::Intercepted => "intercepted",
            Self::OffTarget => "off_target",
            Self::NotSettled => "not_settled",
            Self::JsDispatch => "js",
            Self::NotProbed => "not_probed",
        }
    }

    /// Read back a delivery the action wrote onto its own response.
    ///
    /// Anything unrecognised is `NotProbed`: an unknown token is not evidence.
    #[must_use]
    pub fn parse(token: &str) -> Self {
        match token {
            "target_hit" => Self::TargetHit,
            "intercepted" => Self::Intercepted,
            "off_target" => Self::OffTarget,
            "not_settled" => Self::NotSettled,
            "js" => Self::JsDispatch,
            _ => Self::NotProbed,
        }
    }
}

impl fmt::Display for Delivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a read-back on the acted-on handle said once the action had run.
///
/// `NotRead` is the floor and the default: every command that has no postcondition to read
/// (click, dblclick, press, hover, drag, scroll) reports it, as does any response that never
/// carried one. It is an absence of evidence and never licenses a claim.
///
/// Only `fill` and the bulk fills reach the two failing variants. `check`/`uncheck` and
/// `select` read their state back too and reach `Kept` through the same field, but they REFUSE
/// when the reading disagrees with the request — the error is the report — so a successful
/// response from those two is `Kept`, or `NotRead` for a `check` that dispatched nothing
/// because the element already held the state.
///
/// `Kept` is evidence in its own right and has its own rung, ranked below every statement that
/// describes the page and above every admission that none was available — see the ladder in the
/// module docs for why the two outcomes of one measurement are not ranked symmetrically.
///
/// `Discarded` and `Rewritten` are one verdict and two reasons, because they are one fact
/// ("the element does not hold what was asked for") with two different recoveries: an empty
/// field means the write cannot land this way, a rewritten one means it landed in a shape the
/// page chose. Collapsing them would put `value_reverted` on every phone and currency mask on
/// the web, which is a machine-readable token saying something false.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Postcondition {
    /// Nothing was read back, or the response carried no readable answer.
    NotRead,
    /// The handle held what was asked for when we looked.
    Kept,
    /// The handle held nothing: the page took the write and kept none of it.
    Discarded,
    /// The handle held something other than what was asked for — a mask, a normaliser, a
    /// controlled component that wrote its own value over ours.
    Rewritten,
}

/// Everything the classifier reads about delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delivered {
    pub how: Delivery,
    /// The receiver of an intercepted click was in the top layer (`:modal`). Its own reason
    /// token, because the recovery differs: close the dialog rather than re-aim.
    pub modal_receiver: bool,
    /// Milliseconds between the dispatch and the observation. `no_effect` is only ever a
    /// claim about a window, so without a window the verdict falls back to `unchanged`.
    pub observed_after_ms: Option<u64>,
}

impl Delivered {
    /// The floor: no pointer event, or no answer from the hit test.
    pub const NOT_PROBED: Self =
        Self { how: Delivery::NotProbed, modal_receiver: false, observed_after_ms: None };
}

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
        /// Fields that held a value before this action and hold none after it. Derived from
        /// the diff, so unlike a postcondition it belongs in Group B: without a comparable
        /// tree there is nothing to have lost.
        values_lost: usize,
    },
}

/// What the tool managed to see of the page after the action.
///
/// Deliberately NOT part of the verdict, and never printed: the verdict is about the thing this
/// action did, while this is about whether anything else could be observed. It rides on the
/// assessment because `next` needs it and no word of the verdict carries it — see `next_for`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSight {
    /// The page was read, or the caller declined the read (`--verdict off`). Either way, no
    /// observation is missing that the caller did not choose to miss.
    Readable,
    /// The tool tried to read the page after the action and could not. Whatever it says about
    /// the handle it wrote to, it is blind to everything else.
    Unreadable,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two spellings of the same delivery must not exist: the token on the response is what
    /// the classifier reads back, so it has to round-trip.
    #[test]
    fn every_delivery_token_round_trips() {
        for how in [
            Delivery::TargetHit,
            Delivery::Intercepted,
            Delivery::OffTarget,
            Delivery::NotSettled,
            Delivery::JsDispatch,
            Delivery::NotProbed,
        ] {
            assert_eq!(Delivery::parse(how.as_str()), how);
        }
        assert_eq!(Delivery::parse("something else"), Delivery::NotProbed);
    }
}
