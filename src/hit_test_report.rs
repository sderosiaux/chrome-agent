//! What a pointer-targeted action says about itself, and what it says when it dispatched
//! nothing.
//!
//! Split out of `hit_test.rs` for the repo's 1000-line file cap and re-exported from it, so
//! every call site stays `crate::hit_test::Dispatched`. The seam is deliberate rather than
//! arithmetic: next door is the measurement (a probe, a settle loop, a classifier), and here is
//! the claim the response makes about it.
//!
//! # Why a refusal is a value and not a sentence
//!
//! `--on-intercept refuse` used to answer, in full:
//!
//! ```json
//! {"error":"Did not click uid=n208: div.gdpr-lmd-standard.gdpr-lmd-wall occupies the point it
//!  would have been aimed at, and --on-intercept refuse was set.","ok":false}
//! ```
//!
//! No `hint` — which `hints.rs` promises every error carries — no `intercepted_by`, no `next`.
//! The receiver had been measured, named and then flattened into prose, so an agent had to
//! parse an English sentence to learn which element to close. And it was the SAFE mode that
//! helped least: `dispatch` returns the whole structured payload, while the mode a caller picks
//! precisely because it would rather re-plan than act returned the least to re-plan with.
//!
//! [`Refused`] carries the measurement through the error channel — the same route
//! `commands::assert::NotHeld` takes for exit 2 — and each of the three modes turns it into the
//! same object. `ok` stays `false` and the CLI still exits 1: nothing was dispatched, so the
//! command did not do what it was asked to.

use serde_json::{json, Value};

use crate::hit_test::{Hit, SETTLE_ATTEMPTS, SETTLE_GAP_MS};
use crate::verdict::{Assessment, Delivered, Delivery, Observation, Postcondition};

/// Why no point on the element could be aimed at.
///
/// `off_target` is one token over two shapes, on purpose — both mean "nothing was dispatched
/// and a repeat measures the same thing", which is the whole content of the verdict. They
/// differ only in the recovery, so the difference is carried here, where the message and the
/// hint are written, rather than in the vocabulary an agent branches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unaimable {
    /// The aim point is on screen and outside every one of the element's own boxes: an inline
    /// link laid out across a gap, a container clipped to nothing.
    NoBoxToAimAt,
    /// The aim point stopped moving and stayed outside the viewport. What a `position: fixed`
    /// wall pinned past an edge looks like when the document's own scroll is locked, so no
    /// scroll this tool can perform will bring it back.
    StableOffViewport,
}

/// What a pointer-targeted action did, and to whom.
#[derive(Debug)]
pub struct Dispatched {
    pub delivery: Delivery,
    /// False when the aim never settled, or the caller asked to refuse an interception.
    pub sent: bool,
    pub aim: Option<(f64, f64)>,
    pub receiver: Option<Hit>,
    /// The node that was acted on, resolved before the action from the same handle that was
    /// probed and clicked — so the uid in the response and the uid in the delta are the same
    /// node by construction, whichever way the caller aimed.
    pub uid: Option<String>,
    pub role: Option<String>,
    pub name: Option<String>,
    /// Which shape of `off_target` this was, when it was one.
    pub unaimable: Option<Unaimable>,
}

impl Dispatched {
    const fn bare(delivery: Delivery, sent: bool) -> Self {
        Self {
            delivery,
            sent,
            aim: None,
            receiver: None,
            uid: None,
            role: None,
            name: None,
            unaimable: None,
        }
    }

    /// A JS `click()`/`MouseEvent`: no hit test happened and none could have.
    #[must_use]
    pub const fn js() -> Self {
        Self::bare(Delivery::JsDispatch, true)
    }

    #[must_use]
    pub fn landed(delivery: Delivery, aim: (f64, f64), receiver: Option<Hit>) -> Self {
        Self { aim: Some(aim), receiver, ..Self::bare(delivery, true) }
    }

    /// Aimed, refused, nothing sent. Keeps the receiver so the refusal can name it.
    #[must_use]
    pub fn skipped(delivery: Delivery, aim: (f64, f64), receiver: Option<Hit>) -> Self {
        Self { aim: Some(aim), receiver, ..Self::bare(delivery, false) }
    }

    /// Carry why the aim failed, for the message and the hint that follow from it.
    #[must_use]
    pub const fn unaimed(mut self, cause: Option<Unaimable>) -> Self {
        self.unaimable = cause;
        self
    }

    /// Carry over the identity of the node an action resolved for itself.
    #[must_use]
    pub fn named(mut self, uid: Option<String>, role: Option<String>, name: Option<String>) -> Self {
        self.uid = uid;
        self.role = role;
        self.name = name;
        self
    }

    /// The fields this outcome contributes to the response.
    #[must_use]
    pub fn report(&self) -> Value {
        let mut out = json!({"delivery": self.delivery.as_str()});
        if let Some(uid) = &self.uid {
            out["uid"] = json!(uid);
        }
        if let Some(role) = &self.role {
            out["role"] = json!(role);
        }
        if let Some(name) = &self.name {
            out["name"] = json!(name);
        }
        if let Some((x, y)) = self.aim {
            out["aim"] = json!([x, y]);
        }
        // Only when it is false. A response that says `dispatched: true` on every successful
        // action teaches the reader to skip the field, and this one has to be read.
        if !self.sent {
            out["dispatched"] = json!(false);
        }
        if let Some(receiver) = &self.receiver {
            out["intercepted_by"] = receiver.report();
            // Written here rather than left to the verdict's generic hint: this one can name
            // the element, and an agent that has to guess which overlay to deal with is back
            // to spending a turn finding out.
            //
            // Two wordings, because "it received the event instead" is false in the refusing
            // mode — the scrim received nothing either. The paragraph that names the receiver
            // is the same; what changes is what happened to the event and whether the repeat
            // is a second real action.
            out["verdict_hint"] = json!(if self.sent {
                format!(
                    "The event was aimed at the target's centre and {} occupies that point, so \
                     it received the event instead. Deal with it first (dismiss the banner or \
                     scrim, close the dialog), then repeat this action. Nothing is known about \
                     what the target itself would have done.",
                    receiver.describe()
                )
            } else {
                format!(
                    "The event would have been aimed at the target's centre and {} occupies \
                     that point, so nothing was dispatched (--on-intercept refuse). Deal with \
                     it first (dismiss the banner or scrim, close the dialog), then repeat this \
                     action — the page has seen no event from this command, so the repeat \
                     duplicates nothing. Repeating it while that element is still there \
                     produces this same refusal.",
                    receiver.describe()
                )
            });
        } else if self.unaimable == Some(Unaimable::StableOffViewport) {
            // The generic `aim_point_off_target` hint has to cover both shapes; this one knows
            // which shape it is and where the point ended up, and a coordinate off screen is
            // the fact that stops an agent scrolling at it forever.
            out["verdict_hint"] = json!(format!(
                "The aim point{} stopped moving and is outside the viewport, so nothing was \
                 dispatched. The probe already scrolled the element into view before measuring, \
                 which means the page is holding it there — a `position: fixed` container \
                 pinned past an edge, or a document whose own scroll is locked. Repeating this \
                 action measures the same coordinate and refuses again; `scroll` will not move \
                 it either. Run `inspect` to see what the page is showing, and change that \
                 state (dismiss the layer holding it) before aiming here again.",
                self.aim.map_or_else(String::new, |(x, y)| format!(" ({x:.0}, {y:.0})"))
            ));
        }
        out
    }

    /// The message for an action that never dispatched, or `None` when it did.
    ///
    /// "Clicked" would be false for all three refusals, and a false message is what the change
    /// report cannot undo.
    #[must_use]
    pub fn refusal_message(&self, verb: &str, target: &str) -> Option<String> {
        if self.sent {
            return None;
        }
        let budget = u64::from(SETTLE_ATTEMPTS) * SETTLE_GAP_MS;
        Some(match (self.delivery, self.unaimable) {
            // No longer "still moving, or outside the viewport": that OR is exactly what made a
            // permanent miss read as a temporary one. This branch is now only the first half.
            (Delivery::NotSettled, _) => format!(
                "Did not {verb} {target}: the aim point was still moving after {budget}ms of \
                 settling, so nothing was dispatched."
            ),
            (Delivery::OffTarget, Some(Unaimable::StableOffViewport)) => format!(
                "Did not {verb} {target}: its aim point{} is outside the viewport and stayed \
                 there across {budget}ms of readings, so nothing was dispatched.",
                self.aim.map_or_else(String::new, |(x, y)| format!(" ({x:.0}, {y:.0})"))
            ),
            (Delivery::OffTarget, _) => format!(
                "Did not {verb} {target}: no point inside the element's own boxes could be \
                 aimed at, so nothing was dispatched."
            ),
            _ => format!(
                "Did not {verb} {target}: {} occupies the point it would have been aimed at, \
                 and --on-intercept refuse was set.",
                self.receiver.as_ref().map_or_else(
                    || "another element".to_string(),
                    Hit::describe
                )
            ),
        })
    }
}

/// An action `--on-intercept refuse` stopped before it dispatched, with everything it measured.
///
/// Travels through the error channel as `element::ElementError::Refused`, so no caller has to
/// thread a second return type through three dispatchers; each mode's error boundary asks
/// [`crate::hit_test::refusal_in`] whether the error it is holding is one of these, and prints
/// [`Refused::to_json`] instead of `{"ok":false,"error":…}`.
#[derive(Debug)]
pub struct Refused {
    message: String,
    dispatched: Dispatched,
}

impl Refused {
    #[must_use]
    pub const fn new(message: String, dispatched: Dispatched) -> Self {
        Self { message, dispatched }
    }

    /// Carry over the identity of the node the action resolved for itself.
    ///
    /// The same fields a successful outcome gets from `Dispatched::named`, applied to the
    /// error: "a targeted action names the node it hit" held on every path except the one
    /// where the caller has the most re-planning to do, because the naming happened after the
    /// `?` that never returned.
    #[must_use]
    pub fn naming(
        mut self,
        uid: Option<String>,
        role: Option<String>,
        name: Option<String>,
    ) -> Self {
        self.dispatched = self.dispatched.named(uid, role, name);
        self
    }

    /// The verdict this refusal carries, from the same classifier every other response uses.
    ///
    /// `ReportingDisabled` is the honest observation: the page was never re-read, because
    /// nothing was done to it. It reaches no rung — an interception is Group A, measured on
    /// this action's own target, so it settles the verdict whatever the observation says — and
    /// it is what leaves `PageSight::Readable`, which is also true: nothing was hidden from us.
    #[must_use]
    pub fn assessment(&self) -> Assessment {
        crate::verdict::classify(
            Observation::ReportingDisabled,
            Delivered {
                how: self.dispatched.delivery,
                modal_receiver: self.dispatched.receiver.as_ref().is_some_and(|hit| hit.modal),
                observed_after_ms: None,
            },
            Postcondition::NotRead,
        )
    }

    /// The whole response, in the shape the dispatching mode returns minus what only a page
    /// read could fill in.
    ///
    /// `browser` is here for rule 2 of `hints.rs`: the command in the hint has to reach the
    /// browser THIS invocation is driving, and only the error boundaries know its name.
    #[must_use]
    pub fn to_json(&self, browser: &str) -> Value {
        let mut obj = json!({"ok": false, "error": self.message});
        if let (Some(map), Some(fields)) =
            (obj.as_object_mut(), self.dispatched.report().as_object())
        {
            for (key, value) in fields {
                map.insert(key.clone(), value.clone());
            }
        }
        // The same three fields every action writes, from the same table, so the words on a
        // refusal cannot drift from the words on a dispatch. `verdict_hint` is already on the
        // response — `attach_verdict` never overwrites the specific one with the generic one.
        crate::run_helpers::attach_verdict(&mut obj, self.assessment());
        obj["hint"] = json!(crate::hints::intercepted_refusal_hint(
            browser,
            self.dispatched.receiver.as_ref()
        ));
        obj
    }

    /// The same response for a terminal, after the `error:` line `main` prints.
    ///
    /// Mirrors the fields `render::action_lines` prints for a dispatch — who got in the way,
    /// what to do next, and the advice — because a person reading a refusal needs the same
    /// three things as an agent parsing one.
    #[must_use]
    pub fn text_lines(&self, browser: &str) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(receiver) = &self.dispatched.receiver {
            let uid = receiver
                .uid
                .as_deref()
                .map_or_else(String::new, |uid| format!(" ({uid})"));
            lines.push(format!("in the way: {}{uid}", receiver.describe()));
        }
        lines.push(format!("next: {}", crate::verdict::next_for(self.assessment())));
        lines.push(format!(
            "hint: {}",
            crate::hints::intercepted_refusal_hint(browser, self.dispatched.receiver.as_ref())
        ));
        lines
    }
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Refused {}

#[cfg(test)]
mod tests {
    use super::*;

    fn scrim(modal: bool) -> Hit {
        Hit {
            tag: "DIV".into(),
            id: Some("scrim".into()),
            cls: None,
            z: Some("9999".into()),
            text: "We use cookies".into(),
            modal,
            iframe: false,
            same_doc: true,
            uid: Some("n11".into()),
        }
    }

    fn refusal(modal: bool) -> Refused {
        let dispatched = Dispatched::skipped(Delivery::Intercepted, (200.0, 130.0), Some(scrim(modal)))
            .named(Some("n7".into()), Some("button".into()), None);
        let message = dispatched
            .refusal_message("click", "uid=n7")
            .expect("a refusal message");
        Refused::new(message, dispatched)
    }

    /// The response is what the classifier reads back, so the delivery token and the receiver
    /// must both survive the trip.
    #[test]
    fn the_report_carries_the_receiver_and_a_hint_that_names_it() {
        let report =
            Dispatched::landed(Delivery::Intercepted, (200.0, 130.0), Some(scrim(false))).report();
        assert_eq!(report["delivery"], "intercepted");
        assert_eq!(report["intercepted_by"]["id"], "scrim");
        assert_eq!(report["intercepted_by"]["uid"], "n11");
        assert_eq!(report["aim"], json!([200.0, 130.0]));
        assert!(report["dispatched"].is_null(), "an action that ran says nothing here: {report}");
        assert!(
            report["verdict_hint"].as_str().unwrap().contains("div#scrim"),
            "the hint has to name the receiver: {report}"
        );
    }

    /// An action that did not dispatch must not answer "Clicked" — and must not describe the
    /// element it refused to click as having received anything.
    #[test]
    fn a_refusal_says_what_it_did_not_do() {
        let not_settled = Dispatched::skipped(Delivery::NotSettled, (10.0, 20.0), None);
        let msg = not_settled.refusal_message("click", "uid=n9").expect("a refusal message");
        assert!(msg.starts_with("Did not click uid=n9"), "{msg}");
        assert!(msg.contains("150ms"), "the settle budget is stated: {msg}");
        assert!(
            Dispatched::landed(Delivery::TargetHit, (10.0, 20.0), None)
                .refusal_message("click", "uid=n9")
                .is_none()
        );

        let report = Dispatched::skipped(Delivery::Intercepted, (5.0, 6.0), Some(scrim(false))).report();
        assert_eq!(report["dispatched"], Value::Bool(false));
        let hint = report["verdict_hint"].as_str().unwrap_or_default();
        assert!(
            !hint.contains("received the event"),
            "nothing was sent, so nothing received it: {hint}"
        );
        assert!(hint.contains("nothing was dispatched"), "{hint}");
    }

    /// The two shapes of `off_target` produce two messages: one says the element has no box a
    /// pointer can reach, the other says where its box actually is.
    #[test]
    fn the_two_shapes_of_off_target_do_not_share_a_message() {
        let no_box = Dispatched::skipped(Delivery::OffTarget, (300.0, 90.0), None)
            .unaimed(Some(Unaimable::NoBoxToAimAt));
        let off_screen = Dispatched::skipped(Delivery::OffTarget, (378.0, -14.0), None)
            .unaimed(Some(Unaimable::StableOffViewport));
        let first = no_box.refusal_message("click", "uid=n1").expect("a message");
        let second = off_screen.refusal_message("click", "uid=n1").expect("a message");
        assert_ne!(first, second);
        assert!(first.contains("no point inside the element's own boxes"), "{first}");
        assert!(second.contains("outside the viewport"), "{second}");
        assert!(second.contains("(378, -14)"), "the coordinate is the evidence: {second}");

        // And the off-screen one carries its own hint, because the generic one advises aiming
        // at a child — which is the recovery for the other shape.
        let hint = off_screen.report()["verdict_hint"].as_str().unwrap_or_default().to_string();
        assert!(hint.contains("Repeating this action"), "{hint}");
        assert!(hint.contains("scroll` will not move it"), "{hint}");
        assert!(no_box.report()["verdict_hint"].is_null(), "the generic table covers that one");
    }

    /// Every field a caller has to branch on, on the response that dispatched nothing.
    #[test]
    fn a_refusal_answers_with_the_payload_a_dispatch_would_have() {
        let response = refusal(false).to_json("agent-7");
        assert_eq!(response["ok"], Value::Bool(false));
        assert_eq!(response["dispatched"], Value::Bool(false));
        assert_eq!(response["delivery"], "intercepted");
        assert_eq!(response["uid"], "n7", "the node that was aimed at is still named");
        assert_eq!(response["intercepted_by"]["id"], "scrim");
        assert_eq!(response["intercepted_by"]["uid"], "n11");
        assert_eq!(response["verdict"], "intercepted");
        assert_eq!(response["verdict_reason"], "hit_test_receiver");
        assert_eq!(response["next"], "dismiss");
        let hint = response["hint"].as_str().expect("every error carries a hint");
        assert!(hint.contains("div#scrim"), "{hint}");
        assert!(hint.contains("chrome-agent --browser agent-7"), "{hint}");
    }

    /// A modal receiver gets its own reason and its own advice, in the refusing mode too: the
    /// recovery is to close the dialog, not to look for a scrim's dismiss control.
    #[test]
    fn a_modal_receiver_keeps_its_own_reason_when_nothing_was_dispatched() {
        let response = refusal(true).to_json("default");
        assert_eq!(response["verdict_reason"], "modal_dialog");
        assert_eq!(response["next"], "dismiss");
        assert!(
            response["hint"].as_str().unwrap_or_default().contains("Escape"),
            "{response}"
        );
    }

    /// Text mode gets the same three facts as JSON: who is in the way, the branch, the advice.
    #[test]
    fn the_terminal_form_names_the_receiver_and_the_branch() {
        let lines = refusal(false).text_lines("default");
        assert_eq!(lines[0], "in the way: div#scrim (n11)");
        assert_eq!(lines[1], "next: dismiss");
        assert!(lines[2].starts_with("hint: "), "{lines:?}");
    }
}
