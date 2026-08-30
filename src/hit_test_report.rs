//! What a pointer-targeted action claims about itself, including when it dispatched nothing.
//! `hit_test` measures; this is the claim, re-exported from there so call sites stay
//! `crate::hit_test::Dispatched`.
//!
//! A refusal is a value, not a sentence: [`Refused`] carries the whole measurement through the
//! error channel, so an agent never parses English to learn which element to close. `ok` stays
//! `false` and the CLI exits 1 — nothing was dispatched.

use serde_json::{json, Value};

use crate::hit_test::{Hit, SETTLE_ATTEMPTS, SETTLE_GAP_MS};
use crate::verdict::{Assessment, Delivered, Delivery, Observation, Postcondition};

/// Why no point on the element could be aimed at. One `off_target` token covers both shapes:
/// same verdict, different recovery, so the difference lives here and not in the vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unaimable {
    /// On screen and outside every one of the element's own boxes: an inline link across a gap,
    /// a container clipped to nothing.
    NoBoxToAimAt,
    /// Stopped moving and stayed outside the viewport: a `position: fixed` wall pinned past an
    /// edge with the document's scroll locked. No scroll will bring it back.
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
    /// The node acted on, resolved before the action from the handle that was probed and
    /// clicked, so this uid and the delta's are the same node by construction.
    pub uid: Option<String>,
    pub role: Option<String>,
    pub name: Option<String>,
    /// Which shape of `off_target` this was, when it was one.
    pub unaimable: Option<Unaimable>,
    /// Which `--on-intercept` policy produced this refusal, so the wording names the real mode.
    /// `None` reads as `Refuse`'s wording.
    pub on_intercept: Option<crate::hit_test::OnIntercept>,
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
            on_intercept: None,
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

    /// Carry which `--on-intercept` policy decided this refusal, for the message and the hint.
    #[must_use]
    pub const fn under(mut self, mode: crate::hit_test::OnIntercept) -> Self {
        self.on_intercept = Some(mode);
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
        // Only when false: a field present on every response is a field readers learn to skip.
        if !self.sent {
            out["dispatched"] = json!(false);
        }
        if let Some(receiver) = &self.receiver {
            out["intercepted_by"] = receiver.report();
            // Specific rather than the verdict's generic hint, because this one names the
            // element. Two wordings: nothing received an event that was never dispatched.
            out["verdict_hint"] = json!(if self.sent {
                format!(
                    "The event was aimed at the target's centre and {} occupies that point, so \
                     it received the event instead. Deal with it first (dismiss the banner or \
                     scrim, close the dialog), then repeat this action. Nothing is known about \
                     what the target itself would have done.",
                    receiver.describe()
                )
            } else {
                let because = match self.on_intercept {
                    Some(crate::hit_test::OnIntercept::Guard) => {
                        "--on-intercept guard judged it a control rather than static content"
                    }
                    _ => "--on-intercept refuse",
                };
                format!(
                    "The event would have been aimed at the target's centre and {} occupies \
                     that point, so nothing was dispatched ({because}). Deal with \
                     it first (dismiss the banner or scrim, close the dialog), then repeat this \
                     action — the page has seen no event from this command, so the repeat \
                     duplicates nothing. Repeating it while that element is still there \
                     produces this same refusal.",
                    receiver.describe()
                )
            });
        } else if self.unaimable == Some(Unaimable::StableOffViewport) {
            // The coordinate is what stops an agent scrolling at it forever; the generic
            // `aim_point_off_target` hint cannot name it.
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
    #[must_use]
    pub fn refusal_message(&self, verb: &str, target: &str) -> Option<String> {
        if self.sent {
            return None;
        }
        let budget = u64::from(SETTLE_ATTEMPTS) * SETTLE_GAP_MS;
        Some(match (self.delivery, self.unaimable) {
            // Still moving only — a point that settled off screen is the branch below.
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
            _ => {
                let who = self.receiver.as_ref().map_or_else(
                    || "another element".to_string(),
                    Hit::describe
                );
                match self.on_intercept {
                    Some(crate::hit_test::OnIntercept::Guard) => format!(
                        "Did not {verb} {target}: {who} occupies the point it would have been \
                         aimed at, and --on-intercept guard judged it a control rather than \
                         static content, so nothing was dispatched."
                    ),
                    _ => format!(
                        "Did not {verb} {target}: {who} occupies the point it would have been \
                         aimed at, and --on-intercept refuse was set."
                    ),
                }
            }
        })
    }
}

/// An action stopped before it dispatched, with everything it measured. Travels through the
/// error channel as `element::ElementError::Refused`, so no caller threads a second return type
/// through three dispatchers; each boundary asks [`crate::hit_test::refusal_in`] first.
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

    /// The same fields `Dispatched::named` puts on a success, applied to the error.
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

    /// The verdict this refusal carries, from the classifier every response uses.
    /// `ReportingDisabled` because the page was never re-read; it reaches no rung, an
    /// interception being Group A and settling the verdict whatever the observation says.
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

    /// The whole response, in the shape a dispatch returns minus what only a page read fills in.
    /// `browser` is rule 2 of `hints.rs`: the command in the hint must reach the browser THIS
    /// invocation drives, and only the error boundaries know its name.
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
        // `attach_verdict` never overwrites the specific `verdict_hint` with the generic one.
        crate::run_helpers::attach_verdict(&mut obj, self.assessment());
        obj["hint"] = json!(crate::hints::intercepted_refusal_hint(
            browser,
            self.dispatched.receiver.as_ref(),
            self.dispatched.on_intercept.unwrap_or(crate::hit_test::OnIntercept::Refuse)
        ));
        obj
    }

    /// The same response for a terminal, mirroring `render::action_lines`.
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
            crate::hints::intercepted_refusal_hint(
                browser,
                self.dispatched.receiver.as_ref(),
                self.dispatched.on_intercept.unwrap_or(crate::hit_test::OnIntercept::Refuse)
            )
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
            actionable: false,
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

    /// The classifier reads the response back: delivery token and receiver must survive it.
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

    /// No "Clicked", and no element described as having received anything.
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

    /// Two shapes, two messages: no reachable box, versus where the box actually is.
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

        // The off-screen shape needs its own hint; the generic one advises aiming at a child.
        let hint = off_screen.report()["verdict_hint"].as_str().unwrap_or_default().to_string();
        assert!(hint.contains("Repeating this action"), "{hint}");
        assert!(hint.contains("scroll` will not move it"), "{hint}");
        assert!(no_box.report()["verdict_hint"].is_null(), "the generic table covers that one");
    }

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

    /// A modal receiver keeps its own reason even when nothing was dispatched.
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

    /// Text mode carries the same three facts as JSON.
    #[test]
    fn the_terminal_form_names_the_receiver_and_the_branch() {
        let lines = refusal(false).text_lines("default");
        assert_eq!(lines[0], "in the way: div#scrim (n11)");
        assert_eq!(lines[1], "next: dismiss");
        assert!(lines[2].starts_with("hint: "), "{lines:?}");
    }
}
