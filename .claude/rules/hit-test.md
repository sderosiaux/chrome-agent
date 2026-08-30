---
paths:
  - "src/hit_test*.rs"
  - "src/element_selector.rs"
  - "tests/hit_test_tests.rs"
  - "tests/intercept_guard_tests.rs"
  - "tests/refusal_report_tests.rs"
  - "tests/click_parity_tests.rs"
---

# Where a pointer event lands, before it is dispatched

## `click --selector` is the same verb as `click <uid>`

Both resolve the element's viewport centre and dispatch native CDP input — mouse normally, a
touch tap under `--touch`.

It used to call `el.click()`, which fires the handler whatever is stacked above the node: a click
on a button under a modal scrim reported success with the same shape as a click a user could have
made. Consequence, deliberate: a covered element now hands the click to whatever covers it, and
the response says so. The JS `click()` survives only as the zero-size fallback, where there is no
point to aim at.

### And `dblclick` is the same path as `click`

`element::aim_and_dispatch`, taking an `element::PointerVerb`. Both verbs × both targeting modes
= four entry points (`click`, `dblclick`, `click_selector`, `dblclick_selector`), all four of them
wrappers over that one function.

It was two copies of ~42 lines — the same `Aim::NoBox`/`Unprobed`/`At` match, the same
`NotSettled | OffTarget` early return down to the comment, the same `Intercepted` +
`should_refuse_intercept` refusal — differing in three tokens. `PointerVerb` carries exactly those
three: the JS fallback (`js_click`/`js_dblclick`), the native dispatch
(`dispatch_click_at`/`dblclick_at_coords`), and the word a refusal is written in
(`click`/`double-click`). A rule added to the aim path can no longer land on one verb and miss the
other; `a_double_click_refuses_on_the_same_interception_and_in_its_own_word` pins both halves of
that.

## The probe

`src/hit_test.rs`. One `Runtime.callFunctionOn` bound to the target's `objectId`: scroll into
view, take the centre of the LARGEST client rect, map it through the frame chain into top-level
coordinates, run `elementFromPoint` there with shadow descent, `closest('label').control`
retargeting and shadow-host containment.

The response carries `delivery` (`target_hit` | `intercepted` | `off_target` | `not_settled` |
`js` | `not_probed`), `aim`, and `intercepted_by {uid,tag,id,class,z_index,text,modal}`.

Cost: +31.7 ms per pointer action (measured, 20 clicks on a static in-viewport button, 3
interleaved A/B runs) — one `SETTLE_GAP_MS` sleep plus one round trip for the mandatory second
reading below. An interception costs two more calls (a handle for the receiver, then
`DOM.describeNode` for its uid — best effort, since scrims usually have no accessibility node).

It closed two false successes indistinguishable from a real click: `click_overlay.html` reported
`changed / focus_only` while `window.receiver === "scrim"`, and under `scroll-behavior: smooth`
the box model was read mid-animation and the event went hundreds of pixels past the target.

### Which fields actually carry the answer

Over 61 real sites across two audits: 8 interceptions, `hit_test_receiver` 8 times,
`modal_dialog` 0.

- `modal` is `el.matches(':modal')`, true only for the TOP LAYER — it needs a `<dialog>` opened with `showModal()` or a fullscreen element. The `<div role="dialog">` overlays most sites ship never enter the top layer and correctly arrive as `hit_test_receiver`. The branch stays and `intercept_modal_backdrop.html` covers it, but it is proven on a fixture, not in the wild.
- `z_index` was `auto` on 7 of the 8 and on both local fixtures, including a scrim that covers the page and a top-layer `<dialog>`. Stacking is usually DOM order plus `position`, and the top layer is not a z-index mechanism at all. Read `tag`/`id`/`class` instead: they named the receiver in every case measured (`div#scrim`, `dialog#terms`). The field is kept because `auto` is a true reading.

### Settling: convergence and the viewport are two facts

The probe scrolls with `behavior:'instant'`, then reads the point at least TWICE — an
in-viewport first reading does not convince on its own. It retries 5×30 ms until two consecutive
readings agree; if they never do, nothing is dispatched and the message says so rather than
"Clicked". The single-reading fast path was a false success: a page mid-smooth-scroll with the
element already partially on screen produced `target_hit` and dispatched at a point the element
was leaving. Fixture: `tests/fixtures/moving_target_inside_viewport.html`.

Collapsing the two conditions ordered an infinite loop: the loop exited only on
`in_viewport && agreed`, so a point that agreed with itself five times while off screen came out
as `not_settled` → `unknown / scroll_not_settled`, the one rung whose `next` is `retry`. Measured
on a consent wall at `(378, -14)` on seven attempts, identical to the pixel, because the wall is
`position: fixed` and the document's scroll is locked. Confirmed on a second site at
`(-263.7, 107.5)` — negative x with y inside the viewport.

`classify` takes `Settle::{Converged,Moving}` beside the probe:

| Reading | `delivery` | Verdict | `next` |
|---|---|---|---|
| Readings disagree | `not_settled` | `unknown / scroll_not_settled` | `retry` |
| Readings agree, point unaimable | `off_target` | `unknown / aim_point_off_target` | `inspect` |

`Unaimable` carries which of the two shapes it was — off screen, or merely outside the element's
own boxes — because the verdict is the same and the recovery is not: aim at a child with a box of
its own, versus change the page state pinning the element off screen.
Fixture: `tests/fixtures/fixed_wall_above_viewport.html`.

### Ranking and scope

`intercepted` ranks ABOVE `tree_delta`. The delta is a post-dispatch observation our own action
can explain (the scrim's handler moves the page); the interception is a pre-dispatch measurement
it cannot.

The default `dispatch` keeps pointer semantics: `elementFromPoint` matching the browser's real
input hit test is evidence (12/12 on the design-record fixtures), not equivalence, so a wrong
call costs a warning rather than the action.

No claim is made when the target is inside an iframe (`depth > 0`): the frame's own document
hit-tests cleanly while an overlay in the PARENT covering the `<iframe>` is invisible from there.
`hover` and `drag` are out of scope and report `not_probed`. `--xy` names no element, so only
"received by X" could ever be said there.

## A refusal carries what it measured

`hit_test_report::Refused`. `--on-intercept refuse` used to answer only
`{"ok":false,"error":"Did not click uid=n208: …"}` — no `hint`, no `intercepted_by`, no `next`.
The receiver had been measured and then flattened into prose, and it was the SAFE mode that
helped least.

`Refused` rides through the error channel as `ElementError::Refused` (the route
`commands::assert::NotHeld` already uses for exit 2). Both boundaries — `main` and
`pipe_dispatch::dispatch_single` — unpack it through `hit_test::refusal_in`, so
CLI, pipe and batch answer with the same object: `delivery`, `uid`, `aim`, `intercepted_by`,
`verdict`/`verdict_reason`/`next` from the same classifier every action uses, the specific
`verdict_hint`, a `hint` under the `hints.rs` contract, and `dispatched:false`.

- `ok` stays `false` and the CLI exits 1: nothing was dispatched, so the command did not do what it was asked.
- `next` is `dismiss`, the same token a dispatched interception gets — `next` prescribes an action, and the action is the same one.
- The hint forbids repeating the command *while the receiver is there*: a retry here is futile rather than dangerous.
- `dispatched:false` rides on every response that aimed and sent nothing, refusal or not. The `verdict_hint` on a refusal does not say the receiver "received the event instead" — it received nothing.

## `--on-intercept guard`

`hit_test::{OnIntercept::Guard, should_refuse_intercept}`, `Hit::looks_inert`.

The gap between "always send" and "never send" is measured, not hypothetical: a fifty-site audit
saw a click aimed at lequipe.fr's `chrono` link land on `BUTTON.Cmp__action--yes` ("oui,
j'accepte"), and `dispatch` accepted the site's GDPR wall on the caller's behalf. But `refuse`
would also have stopped on the five of eight receivers that were inert (`HEADER`, two text
`DIV`s, an `IMG`, a search `iframe`).

`Hit::actionable` answers "does the receiver DO anything" inside the SAME probe call, with no
extra CDP round trip:

- a native interactive tag (`BUTTON`/`A`/`INPUT`/`SELECT`/`TEXTAREA`/`LABEL`/`OPTION`/`SUMMARY`),
- an ARIA interactive role,
- explicit keyboard focusability (`tabIndex >= 0`),
- or a `cursor: pointer` computed style.

`modal` and `z_index` were ruled out by the same measurements above. `class` and `text` are what
let a person tell the two families apart on sight, and are deliberately NOT what the predicate
uses: a keyword list of consent phrases in every language is never complete.

`Hit::looks_inert` is `!iframe && !actionable`. An `<iframe>` refuses unconditionally, whatever
`actionable` reads: its content is opaque from here (cross-origin refuses to answer, same-origin
would need a second execution context this probe does not open).
`tests/fixtures/intercept_iframe_overlay.html` pins the resulting false positive — its iframe
holds one inert paragraph and `guard` refuses it anyway. No receiver identified at all (`None`)
also refuses. Between a false refusal on an inert search box and a false dispatch into an unseen
consent wall, this project accepts the former: a wrong refusal is overridden in one command
(`--on-intercept dispatch`), a wrongly-dispatched consent click cannot be undone.

**The default stays `dispatch`.** `guard`'s predicate has no comparable measured record yet, and
flipping the default would change behaviour for every ordinary pair of overlapping buttons.

A refusal under `guard` reuses the `refuse` payload contract exactly
(`Dispatched::skipped(...).under(on_intercept)`, `Refused::to_json`/`text_lines`). The two differ
only in the words `refusal_message`/`report()`/`hints::intercepted_refusal_hint` choose after
reading `on_intercept` off the response — "judged it a control" rather than a hardcoded "refuse
was set". That is what made a third mode possible without a third refusal contract.

## A selector-targeted pointer action resolves ONE handle

`hit_test::resolve_selector`: `Runtime.evaluate` with `returnByValue:false`, then
`DOM.describeNode` for the uid/role/name, then the probe and the dispatch through that same
`objectId`.

`click --selector` used to be three independent `querySelector` evaluations (pre-probe, action,
post-probe), which a re-render between round trips could bind to three different nodes while the
response described one. Same call count as before. `run_helpers::target_details` is no longer
called on those paths — the action reports the node it actually probed and clicked.

### A malformed selector is named as one, on every path

Two families, one sentence: `Selector '[' could not be used: SyntaxError: …`.

`resolve_selector` says it in Rust, because it holds the reply (a throw still comes back with an
`objectId` — for the thrown `DOMException`, which is why the handle must not be read before
`check_js_exception`). The verbs that resolve IN the page instead — `fill --selector`,
`type --selector` (via `focus_selector`), `frame` — say it in JS, from the shared prelude
`element_selector::bind_element_js`, which try/catches the `querySelector` and rethrows in those
words. Before, they surfaced Chrome's raw `SyntaxError: Failed to execute 'querySelector' on
'Document': …`: true, and silent about the fact that the argument was the problem. The same
prelude carries the `No element matches selector: …` throw, so the two failures stay two messages.
`tests/selector_syntax_tests.rs` walks both families across all five verbs.
