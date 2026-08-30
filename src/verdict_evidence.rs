//! The evidence a verdict is made from, and the one thing it carries but never prints.
//! Re-exported from `verdict.rs`, so call sites write `crate::verdict::Delivery`.
//!
//! Invariant the ladder depends on: every type here has a floor variant meaning "no evidence" —
//! `Delivery::NotProbed`, `Postcondition::NotRead` — and never a claim.

use std::fmt;

/// What the hit test said about a pointer-targeted action's delivery before dispatch.
///
/// `NotProbed` is the floor and the default: every command that dispatches no pointer event, and
/// every mouse path where the probe could not answer or answered about a document we cannot
/// hit-test (a target inside an iframe).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// The aim point resolved to the target, a descendant, its label's control or its shadow
    /// host. The only value that licenses `no_effect`.
    TargetHit,
    /// The aim point resolved to an element outside the target's flat subtree.
    Intercepted,
    /// No point on the target could be aimed at, and two consecutive probes agreed. Reached by a
    /// point outside every client rect of the element, and by one that stopped moving outside the
    /// viewport. Nothing was dispatched, and a repeat measures the same coordinate.
    OffTarget,
    /// Two readings of the aim point disagreed: still moving when the settle budget ran out.
    /// Nothing was dispatched, and the miss is TRANSIENT — the one reading a repeat can fix.
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

    /// Read back a delivery the action wrote onto its own response. Anything unrecognised is
    /// `NotProbed`: an unknown token is not evidence.
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
/// `NotRead` is the floor and the default: every command with no postcondition to read (click,
/// dblclick, press, hover, drag, scroll), and any response that never carried one.
///
/// Only `fill` and the bulk fills reach the two failing variants. `check`/`uncheck` and `select`
/// read their state back too and reach `Kept` through the same field, but they REFUSE when the
/// reading disagrees — the error is the report — so their successful responses are `Kept`, or
/// `NotRead` for a `check` that dispatched nothing because the state already held.
///
/// `Discarded` and `Rewritten` are one fact with two recoveries: an empty field means the write
/// cannot land this way, a rewritten one that it landed in a shape the page chose. Collapsing
/// them would put `value_reverted` on every phone and currency mask on the web.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Postcondition {
    /// Nothing was read back, or the response carried no readable answer.
    NotRead,
    /// The handle held what was asked for when we looked.
    Kept,
    /// The handle held nothing: the page took the write and kept none of it.
    Discarded,
    /// The handle held something else — a mask, a normaliser, a controlled component.
    Rewritten,
}

/// Everything the classifier reads about delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delivered {
    pub how: Delivery,
    /// The receiver of an intercepted click was in the top layer (`:modal`). Its own reason
    /// token, because the recovery differs: close the dialog rather than re-aim.
    pub modal_receiver: bool,
    /// Milliseconds between dispatch and observation. `no_effect` is a claim about a window, so
    /// without one the verdict falls back to `unchanged`.
    pub observed_after_ms: Option<u64>,
}

impl Delivered {
    /// The floor: no pointer event, or no answer from the hit test.
    pub const NOT_PROBED: Self = Self {
        how: Delivery::NotProbed,
        modal_receiver: false,
        observed_after_ms: None,
    };
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
        /// Fields that held a value before this action and hold none after it. Derived from the
        /// diff, so it is Group B: without a comparable tree there is nothing to have lost.
        values_lost: usize,
    },
}

/// What the tool managed to see of the page after the action.
///
/// Not part of the verdict and never printed: the verdict is about what this action did, this is
/// about whether anything else could be observed. It rides on the assessment for `next_for`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSight {
    /// The page was read, or the caller declined the read (`--verdict off`) and owns the silence.
    Readable,
    /// The read after the action failed, so the tool is blind to everything but its own handle.
    Unreadable,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The token on the response is what the classifier reads back, so it has to round-trip.
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
