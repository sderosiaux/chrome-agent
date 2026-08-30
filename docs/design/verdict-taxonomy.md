# Verdict taxonomy — design spec

The design the v0.9.x work is an on-ramp to, not a description of what ships today. Derived from
113 fixtures and 107 cases where a plausible signal reports a confident wrong answer. Do not
redesign it from scratch.

## Status against the code

This document's own inventory has been overtaken. Read it as a record of reasoning.

- It described **five** verdicts. `src/verdict.rs` now carries **eight**: `Changed`, `Navigated`, `Intercepted`, `NotKept`, `NoEffect`, `Unchanged`, `Unknown`, `NotChecked`. The `no_effect` / `intercepted` split shipped, and `unchanged` became the honest floor for a path with no proof of delivery. The condition the spec named was met, not dropped: promoting it needed proof of delivery, and the hit test supplied it.
- Two of the four items under "what is not built" have shipped: the one-shot probe bound to the target's `objectId` (`hit_test::probe_once`) and the hit test itself (`src/hit_test.rs`). The attributed baseline and the scroll/keyboard rungs are still not built.
- Rung R4(a) below carries its own amendment, for a different reason: there the design was wrong, not merely overtaken.

## The nine verdicts

Every verdict is emitted only from a signal read in the same action. None is ever inferred from
the command's own success message — verified: "Checked selector '#qty'" is returned today for a
text input.

**changed** — a postcondition read on the acted-on handle equals the request
(fill/select/check/uncheck/upload), OR the target's own attribute set moved
(class/style/aria-expanded/aria-pressed/aria-selected/data-state/hidden), OR a scroll offset
moved, OR an attributed a11y delta is non-empty after noise subtraction. Always carries
`observed_after_ms`, `dispatch` (mouse|js|insert_text|key) and `scope` (frameId). Never means "did
what you intended".

**already_satisfied** — the goal state held BEFORE dispatch, read through the SAME accessor the
postcondition uses. Carries `mutation_dispatched` truthfully (false for check/uncheck, which
short-circuit; true for fill/select, which do not). Exists because `no_effect` here makes an agent
retry, and a retry on a checked box unchecks it.

**normalised** — the post-read is non-empty, differs from the request, and shares a token or digit
with it. Reports `requested` and `actual` verbatim. Never claims "content preserved" — verified
counterexample: fill `1000` → `10.00`.

**no_effect** — delivery was PROVEN (hit test resolved to the target, or `dispatch:"js"` which
cannot be intercepted, or the postcondition read succeeded) AND the observation window was quiet
AND attribution was clean. Always time-scoped: "no observable change within N ms". Never a claim
about the future, never a claim that the element is inert.

**blocked** — a DOM property read BEFORE dispatch proves the action cannot apply, and we refuse to
dispatch. Reasons: `dom_disabled` (`:disabled`), `inert` (`closest('[inert]')`),
`pointer_events_none`, `wrong_element_type`, `no_checkable_state`, `radio_not_uncheckable`,
`multi_select_unsupported`, `option_unreachable`, `readonly_text_entry` (type only),
`constraint_validation`, `scroll_locked`, `no_document_scroller`, `page_refused_to_unload`. Worded
as an observation at action time, never as a permanent property.

**intercepted** — MOUSE dispatch only. The pre-dispatch hit test at the exact dispatched (cx,cy),
after shadow descent and label retarget, resolves outside the target's flat subtree AND the aim
point is inside the target's client rects. Names the receiver as `tag#id.class` + position +
z-index + 40 chars of text. Overlay containers are absent from the a11y tree (verified for
`#scrim`, `#cookie-banner`, `#chart-hotspot`).

**stale** — the uid names a node with `isConnected === false`, OR the stored snapshot's `loaderId`
differs from the live one and the action is uid-targeted. NOT DISPATCHED. The refusal is what
makes the word safe: a detached node's handler still runs via the `js_click` fallback (verified),
and "stale" would then imply nothing happened.

**navigated** — the acting frame's `loaderId` changed. Sub-reason `navigation_failed` when the
committed URL is `chrome-error://`. Carries `caused_by` from
`Page.frameRequestedNavigation.reason`, or `caused_by:"unknown"` — never "your click navigated".
Implies the uid map was dropped.

**unknown** — the honest floor, always with a reason code and a hint: `scroll_not_settled`,
`aim_point_off_target`, `hit_test_disagreed`, `ambiguous`, `subframe_navigated`,
`scope_unreadable`, `baseline_projection_mismatch`, `unattributed_churn`, `navigation_in_flight`,
`canvas_target`.

### Deliberately not built

- **`pending`** — requires wrapping setTimeout/setInterval/rAF/fetch/XHR. `setInterval` and rAF never decrement, so the counter becomes a property of the page rather than of the action, and `maxDelay` becomes an unrelated number shipped as a re-check promise. It also breaks `setTimeout.toString()` returning `[native code]`, widening the stealth fingerprint. Cost of skipping: a 600 ms handler reads `no_effect`. Mitigated by mandatory `observed_after_ms` wording, not by a fabricated number.
- **`unobservable`** — folded into `unknown(reason=subframe_navigated|scope_unreadable)` plus a hint naming the frame to bind.

## Classifier ladder

ORDERED, FIRST MATCH WINS. Precedence, stated once and fixtured: target-property facts beat
coordinate facts; a satisfied goal beats an unreachable one. Exactly one verdict is emitted;
everything else becomes a payload field.

### Pre-dispatch

**R0. Scope + document identity** (all commands). `Page.getFrameTree` → walk `childFrames` to
`client.frame_context().frame_id` (or root). Read `{frameId, loaderId, url, urlFragment}`. Cost: 1
CDP call, ~0.3 ms, no page JS.

Polling, NOT event subscription: `CdpClient::events()` is a tokio broadcast that only delivers
post-subscribe messages, and `select_option`/`set_checked` never subscribe, so an event-derived
signal is structurally blind there.

- Frame absent or call fails → identity is `Unknown` (tri-state, not `false`). The action still runs; the verdict floor becomes `unknown(scope_unreadable)`. This replaces `compare()`'s `_ => false` arm, which defaults an unreadable signal to the confident answer.
- Stored `loaderId` != live AND the action is uid-targeted → **stale**, DO NOT DISPATCH.
- Stored `loaderId` != live AND the action is selector/xy-targeted → NOT a verdict. Emit `uid_map_invalidated:true` and continue; selector commands query the live document, and refusing them blocks a valid action.

**R1. Handle resolution + one-shot property probe.**

- uid path: `DOM.resolveNode` (already done by `resolve_uid`) then ONE `Runtime.callFunctionOn` on that objectId.
- selector path: `Runtime.evaluate` with `returnByValue:false` to get an objectId, then the same call. Today `fill_selector`/`click_selector` use `returnByValue:true` and return no handle, so pre-probe, action and post-probe are three independent `querySelector` calls that can bind three different nodes.

The probe returns in one round trip: `connected`, `rendered`, `rects`, `matches`, `retargeted`
(LABEL → `this.control`), `tag`, `type`, `disabled` (`:disabled`), `inert`, `pe`
(`pointerEvents`), `readOnly`, `multiple`, `size`, `isContentEditable`, `value`, `textContent`,
`checked` (`indeterminate` → `'mixed'`), `ariaChecked`, `role`, `selValue`, `selIdx`, `selText`,
`optDisabled`/`optHidden` for the requested option, `maxLength`, `validity:{valid,message}`,
`formInvalid`, `attrs:{class,style,aria-expanded,aria-pressed,aria-selected,data-state,hidden}`,
`rootIsShadow`. Cost: 1 `Runtime.callFunctionOn`, ~0.5–1 ms. Replaces every per-family probe.

MUST be bound to the target's objectId, not a bare `Runtime.evaluate`: `this.getRootNode()` on a
handle to a node inside a CLOSED shadow root returns that closed root and `.host` works, which is
what makes R4 correct for closed roots without `DOM.getNodeForLocation`.

- `connected === false` → **stale**, DO NOT DISPATCH.
- LABEL retarget: never block a `<label>`. Verified — `check --selector` on a label toggles its control on both paths. Report `retargeted_to`.

**R2. Already-satisfied** (fill / select / check / uncheck only). Zero cost, read off R1.

Compare the pre-state to the request through the accessor the POSTcondition will use. For
non-INPUT checkables that is `aria-checked`, not `!!el.checked` — verified live: today `check` on
`<div role=checkbox aria-checked=true>` reads `!!undefined` → false, clicks, and UNCHECKS it while
reporting success.

- select: a value-match and a text-match at different indices → **unknown(ambiguous)** with both candidates. Never `already_satisfied` under ambiguity.
- equal → **already_satisfied** + state + `mutation_dispatched`.

Ordered BEFORE R3 on purpose: `uncheck` on an already-unchecked radio must not be
`blocked(radio_not_uncheckable)`. The tool gets this right today; a precondition-first ladder
would break it.

**R3. Blocked** (provable target properties). Zero cost, read off R1. All refuse to dispatch.

| Command | Condition → reason |
|---|---|
| any | `disabled` → `dom_disabled`. Uses `:disabled`, which covers fieldset inheritance AND the first-`<legend>` carve-out; `el.disabled` misses the first, `closest('[disabled]')` misses the second |
| mouse | `inert` → `inert` (+ `also_received_by` from R4's receiver, since inert REDIRECTS rather than suppresses). `pe==='none'` → `pointer_events_none` (+ `also_obscured_by`) |
| fill | tag not in {INPUT,TEXTAREA} → `wrong_element_type`. SELECT is excluded because `fill_selector` picks `HTMLInputElement.prototype`'s setter and throws Illegal invocation. type in {checkbox,radio,file,button,submit,reset,image,range,color} → `wrong_element_type` |
| type | `readOnly` && a text-entry type → `readonly_text_entry`. NEVER for `press`: `press_key` maps only Enter/Tab/Escape/Backspace/Delete/arrows/Space, so a printable key inserts nothing regardless of readonly, and a readonly combobox legitimately responds to Space |
| select | tag!=='SELECT' → `wrong_element_type`. `multiple\|\|size>1` → `multi_select_unsupported` (`selectedIndex=` collapses the whole selection). option `:disabled`, `hidden` or `display:none` → `option_unreachable` |
| check/uncheck | INPUT with type not in {checkbox,radio} → `wrong_element_type` (`typeof el.checked === 'boolean'` is TRUE for text inputs). non-INPUT without a checkable role or `aria-checked` → `no_checkable_state`. radio && desired===false → `radio_not_uncheckable` |
| upload | not `input[type=file]` → `wrong_element_type` |
| click | a submit control && `formInvalid` → `constraint_validation`, naming the first `:invalid` field. Uses the pseudo-class, never `checkValidity()`, which fires `invalid` events into the page we are about to judge |
| scroll | `docMax===0` && (html/body `overflowY==='hidden'` or a `:modal`/`[aria-modal]`/`dialog[open]` exists) → `scroll_locked` / `no_document_scroller`, naming the pane or modal |

If `matches>1`, `blocked` carries `matches` + `judged:"first"` + a disambiguation hint. `blocked`
is the most action-terminating verdict, and a wrong-element `blocked` costs more than a
wrong-element `no_effect`.

**R4. Hit test** — MOUSE DISPATCH ONLY. Runs after `scrollIntoViewIfNeeded` and after the SECOND
`DOM.getBoxModel`, at the exact (cx,cy) about to be dispatched, immediately before the first
`Input.dispatchMouseEvent`. Cost: 1 `Runtime.callFunctionOn` bound to the target's objectId,
~0.5 ms.

Body: bounds check → `document.elementFromPoint(cx,cy)` → shadow descent
(`while (h && h.shadowRoot) { const n=h.shadowRoot.elementFromPoint(cx,cy); if(!n||n===h) break;
h=n; }`) → `landed = h===this || this.contains(h) || h.closest('label')?.control===this ||
(this.getRootNode() instanceof ShadowRoot && this.getRootNode().host.contains(h))` → `aimIn` =
`this.getClientRects()` contains (cx,cy). Also returns `h.ownerDocument===this.ownerDocument`,
`h.matches(':modal')`, `h.tagName==='IFRAME'`.

Rungs:

a. Out of viewport / null hit → the scroll may not have settled. Re-read until two consecutive reads agree (max 5, 30 ms apart), then re-test. Still MOVING → **unknown(scroll_not_settled)**, DO NOT DISPATCH.

   > **AMENDED IN IMPLEMENTATION.** Two reads that AGREE about a point still outside the viewport are **unknown(aim_point_off_target)**, not `scroll_not_settled`. This line originally said otherwise and shipped that way, and `scroll_not_settled` is the one rung whose `next` is `retry` — so a permanent miss (a `position: fixed` consent wall over a locked document scroll, measured at the same pixel seven times) was an instruction to loop. Convergence and the viewport are separate facts; `hit_test::classify` takes both.

b. `aimIn === false` → the dispatch point is not on the element. Verified on a wrapped inline link: the content centre falls in the gap between line boxes. NEVER `intercepted`. Re-aim at the centre of the largest client rect and re-test once; still false → **unknown(aim_point_off_target)** + a hint to use `--selector`.
c. The hit is an IFRAME AND the target's `ownerDocument` is not the hit-testing document → the target is INSIDE that frame. Emit NO interception claim, fall through. Only when the target is provably in the same document as the hit test may we say `cross_document_overlay`.
d. `landed === false` → **intercepted** + receiver. Sub-reason `modal_dialog` when `h.matches(':modal')`. Recovery probe: sample the border-box quadrant centres and inset corners; a point qualifies ONLY if `hit===target` (not a descendant — verified that a descendant can be a dismiss control that destroys the toast), and is reported as an OFFSET from the border-box origin, re-resolved at use time, never as an absolute viewport coordinate handed across turns.
e. `landed === true` → record `hit_test:"target"` and fall through.

NOT REACHED on the js paths. `element.rs` has two early returns to `js_click` (no box model at
resolve time, box model gone after scroll); both skip this region. `dispatch` is recorded as
`"js"` there, `intercepted` is forbidden, and no clean-hit-test claim is made. Keyed on the
dispatch mechanism, never on the `--selector` flag: `dblclick_selector` dispatches native mouse
while `click_selector` does not.

### Dispatch

The R8 baseline is taken HERE: after `scrollIntoViewIfNeeded`, after R4, immediately before the
first input event. Changes caused by our own scroll (verified: an `IntersectionObserver` firing
when `scrollIntoViewIfNeeded` brings a sentinel into view) belong to the scroll and are reported
under `unattributed{cause:"scrolled_into_view"}`.

### Post-dispatch

**R5. Navigation** (all commands). `Page.getFrameTree` again, 1 call.

- The acting frame's `loaderId` changed → **navigated**. URL equality is not the test: verified, a form GET to self changes `loaderId` with an identical URL, and a fragment click changes the URL with an identical `loaderId`.
- Committed URL starts with `chrome-error://` → **navigated(navigation_failed)** + the error code from the tree. `Page.navigate`'s `error_text` check is bypassed on click-initiated navigation.
- `Page.frameNavigated` with a `parentId` → NOT navigated. Top uids alive; that frame's content unread → continue to R8's multi-frame fallback, or `unknown(subframe_navigated)` if the frame is unreadable. Also key `wait_for_stabilization` off `Page.frameStoppedLoading` for that frameId — the current code blocks up to 10 s waiting for `loadEventFired`, which never fires for subframes (measured 10.16 s).
- `frameRequestedNavigation`/`frameStartedLoading` with no `frameNavigated` → **unknown(navigation_in_flight)**, never `no_effect`.
- `beforeunload` answered `dismiss` by our own handler during the window → **blocked(page_refused_to_unload)**. Requires `spawn_dialog_handler` to record `(type,decision,ts)` on the client instead of only writing to stderr. Zero extra CDP calls.

**R6. Postcondition read** (fill / type / select / check / uncheck / upload). 1
`Runtime.callFunctionOn` on the SAME objectId. Dead handle → **stale(raced)**. For form-value
actions, `blur()` first, then read: this catches deterministic blur-time normalisers and costs
nothing.

| Command | Reading → verdict |
|---|---|
| fill | `value===requested` → **changed** (+ caveats `readonly`, `exceeds_maxlength(n,len)`, or `validity.valid===false` carrying the browser's own `validationMessage` verbatim). `value===pre && pre!==requested` → **no_effect(refused)**. `value==="" && requested!==""` → **no_effect(rejected)**. Value differs and shares NO token or digit → **no_effect(sanitizer_substituted)** + a warning that the field moved pre→post as a side effect (verified: `type=range` "abc" → "50"). Otherwise → **normalised**, both strings verbatim, no cause guessed |
| type | `value.length - pre.length === requested.length && value !== pre` → **changed**. Bare containment is a tautology when the field already holds the string |
| select | `selValue===requested \|\| selText===requested \|\| optLabel===requested` → **changed** + `matched_by:{value\|text\|label}`. `matched_by:"value"` only when the option has a value attribute (a valueless `<option>` reports `value===text`). Otherwise → **normalised** with both strings. Also emit `side_effects`: other form controls whose value/checked moved (country→state cascades), read in the same call |
| check/uncheck | state through the SAME accessor as R2 equals desired → **changed**. Else → **no_effect** + hit_test/dispatch, so the agent does not hunt an overlay that does not exist |
| upload | `files.length>0` → **changed**. The only evidence that exists — a `display:none` file input has no a11y node and produces an empty delta |

**R7. Scroll measurement** (scroll only). 2 `Runtime.evaluate`, one before and one after, each
reading ONLY `document.scrollingElement.{scrollTop,scrollLeft,scrollHeight,clientHeight}` plus
`getComputedStyle(...).scrollBehavior`. NOT `querySelectorAll('*')` — that walk is keyed by
`id||tagName` (collides), capped at 64 (truncates silently in document order), and re-walked
independently before and after (misaligns when the action adds a scroller).

- `scroll <uid>`: probe the target's scrollable-ancestor chain and read the target's rect. In viewport after → **changed** (+ `already_in_view` when it did not move; the goal is a position, not a displacement). Not in viewport after → **blocked(clipped)**.
- Offset moved → **changed**, naming the scroller. `scrollBehavior==='smooth'` and the delta != the requested magnitude → **changed** + `settling:true`, and NO terminate hint.
- Offset unchanged && `top >= max-1` → **no_effect** + a hint that names the observation window and does NOT say "stop scrolling".
- `docMax===0` && not overflow-hidden && no modal → **no_effect(fits_viewport)**.

For every NON-scroll command an offset delta is orthogonal state: it goes to `scrolled:{from,to}`
and contributes nothing to the verdict, or every action issued in the wake of a smooth scroll
inherits a spurious `changed`.

**R8. Attributed delta** (click / dblclick / hover / press / drag — the commands with no
postcondition). Cost: 1 extra `Accessibility.getFullAXTree` for the pre-dispatch baseline. The
largest cost in the design, skippable with `--verdict fast`, which degrades these five commands to
`unknown` when the stored baseline was not taken by this command.

Noise subtraction (pure Rust in `diff.rs`, unit-testable with no Chrome):

- Drop any `~` line whose token symmetric difference is exactly `{focused}`, on ANY node — not only RootWebArea. Verified: in a real flow the noise floor is a TWO-line focus transfer including a real element, and the loose "drop RootWebArea lines" rule silently discards a `document.title` change, which is often the only SPA-route signal.
- Exclude `e*` uids from the counts (`snapshot.rs` assigns them positionally and never inserts them into `uid_map`, so `e1`-vs-`e1` across two snapshots is not an identity). Summarise as `anonymous:{appeared:[roles],disappeared:[roles]}`.
- Compare uid ORDER and INDENT DEPTH, not just text → emit `moved:[uids]`. `uid_lines` currently `trim_start()`s the indentation away and pairs by uid, so a reorder and a re-parent are both invisible; the ordered Vec already exists.

Attribution gate — a delta may be credited ONLY if all hold, else it goes under `unattributed`
and the verdict is **unknown(unattributed_churn)**:

- the baseline was taken by THIS command, after the scroll, immediately before dispatch;
- the baseline and the post-read used the SAME projection (verbose/max_depth/filter). `dispatch_inspect` persists a FILTERED snapshot as the baseline while `attach_change_report` re-reads with `max_depth:None`, so every uid the lens dropped scores as a death;
- `loaderId` unchanged across the action.

Then:

- surviving added+removed+changed+moved > 0 → **changed**.
- all zero, but a target attribute moved (class/style/aria-*/data-state) → **changed(evidence:target_attribute)**.
- all zero, top frame quiet, and same-process child frames exist → re-read `getFullAXTree(frameId)` per child and diff those. Non-empty → **changed(scope:multi_frame)**. A targeted widening, not a downgrade: "the page contains an iframe" fires on ~95% of real pages, and a verdict that always fires carries no information.
- all zero everywhere, a NEW page target with `openerId === our target_id` appeared (`Target.getTargets` before/after, filtered to `type=="page"`) → **changed(opened_tab)**, scoped: the current document did not change and the session is still attached to the old tab.
- all zero everywhere AND delivery proven (`hit_test:"target"` or `dispatch:"js"`) → **no_effect** + `observed_after_ms` + a window-scoped hint.
- all zero and delivery NOT proven → **unknown**.

Post-dispatch hit test (mouse only, 1 extra call): DOWNGRADE ONLY. pre=landed, post=not-landed,
delta EMPTY → `unknown(hit_test_disagreed)` with both observations. If the delta is non-empty the
disagreement is explained by the action's own effect (a menu opener paints its own backdrop over
itself) and the verdict stands. Never allowed to upgrade — that would defeat
`intercept_toast_dismiss_on_click`, where the interceptor removes itself on the click it ate.

**Total added cost per action:** R0+R5 = 2 `Page.getFrameTree`; R1+R6 = 2
`Runtime.callFunctionOn`; R4 = 1–2 `Runtime.callFunctionOn` (mouse only); R8 = 1
`getFullAXTree` (five commands only, skippable). Everything except the `getFullAXTree` is
sub-millisecond on an already-open local socket. The listener-ancestor walk (5–15 `DOMDebugger`
calls) is NOT built.

## Honest limits

- **Capture-phase `stopPropagation` on document or window.** The hit test is clean, the coordinates land on the target, nothing happens. `DOMDebugger.getEventListeners` enumerates listeners but not whether one swallows the event. Floor: `no_effect` — never `changed`, never `intercepted`. The event was received and discarded, not taken by something else.
- **No listener-absence probe.** `getEventListeners returns []` on the whole ancestor chain was the only path to a CONFIDENT `no_effect`, and every native behaviour with no listener defeats it: `<label for>`, `<summary>`/`<details>`, `<select>`, `<a href>`, `mailto:`/`tel:`, `<a download>`, a submit button inside a form. It also cannot see delegation. So `no_effect` is always the weak form — "delivered, page quiet for N ms" — never "nothing is wired to this element".
- **No pending-work probe (P3).** Cost paid: a handler on a 600 ms timer, a debounced fetch, or a 300 ms hover-intent menu all read as `no_effect`.
- **No global MutationObserver (P2).** It measures whether the page is alive, not whether the action did something: on any page with a clock, poller, chat socket or ad frame it returns >0 always, manufacturing `changed` for every no-op. The replacement is target-scoped — the target's own class/style/aria-*/data-state before and after. So a CSS class flipped on a SIBLING or ancestor, an opacity/transform/clip-path reveal, and a `height:0` collapse are invisible → `no_effect` + caveat.
- **Canvas, WebGL, WebGPU, OffscreenCanvas** leave no MutationRecord and no a11y change. Degrade to `unknown` ONLY when the target is the canvas or inside it; a chart elsewhere must not poison unrelated verdicts. An OffscreenCanvas in a worker is not clipped by the same box, so a `--verdict pixels` escape hatch would not cover it.
- **Effects that leave the page** — analytics beacons, localStorage/IndexedDB writes, `postMessage` to a parent, a click-initiated native download. Floor: `no_effect`, scoped to the page, not the world.
- **The race between the hit test and the mouse dispatch.** Testing immediately before dispatch narrows it; the post-dispatch re-test downgrades only when the delta is ALSO empty. A CSS transition finishing mid-dispatch that ALSO produces a delta therefore reads as `changed`. Downgrading on any disagreement would turn every successful menu opener into `unknown`, since its own effect paints a backdrop over the target.
- **Whether the element that received an intercepted click did what the agent wanted.** We prove which element received it; intent is unreadable. Never upgraded to `changed` because something useful happened.
- **Whether an observed change is the change that was ASKED for.** Every verdict answers "did the page react", not "did it do the right thing". A click that opens the wrong menu scores `changed` exactly like the right one. That gap belongs to a layer above this taxonomy.
- **An interceptor inside a CLOSED shadow root** (a target inside one is handled). `elementFromPoint` returns the host and `host.shadowRoot` is null, so the receiver cannot be named. Report the host with a "the receiver is inside a closed shadow tree" caveat.
- **User-agent shadow roots** — `<video>` controls, date pickers, the `<input type=file>` button. `getNodeForLocation` only descends with `includeUserAgentShadowDOM`, and those backendNodeIds are in no uid map. A hit on the target's own UA internals counts as landed; a hit on a DIFFERENT element's UA internals is named only as that element.
- **Real out-of-process iframes.** Not reproducible offline: `file://`, `srcdoc` and `data:` children stay in the parent's process, so `getFullAXTree(frameId)` and `createIsolatedWorld` keep working. On a genuinely cross-site frame those calls can fail (another CDP target, needing `attachToTarget`). Any failure of the multi-frame fallback yields `unknown(scope_unreadable)`, never `no_effect` and never a silent success.
- **The identity of the real receiver inside a cross-origin iframe.** The verdict is solid from the parent's hit test; the culprit's name is not. Report the `<iframe>` with its id/title/src and stop.
- **Attribution when the page mutates continuously AND the effect lands in the SAME subtree as the churn** — a chat pane where the click posts into a feed already ticking. The pre-dispatch control read separates disjoint subtrees only. Overlapping → `unknown`, delta listed and explicitly not credited.
- **Causation for a navigation or a new tab.** `Target.targetCreated` with our `openerId` proves a tab was opened by this page, not by our click. A page-scripted `location.reload()` in our window is byte-identical to one we caused. Report `caused_by:'unknown'` unless `Page.frameRequestedNavigation.reason` says otherwise.
- **Server-side 30x redirect chains** cannot be reproduced from `file://`. Without the Network domain we can only say the final URL differs; hop detail needs `Network.requestWillBeSent.redirectResponse`, off the stealth hot path.
- **Cross-process backendNodeId collision after a cross-origin navigation** — the reason `goto` clears the uid map. Not reproducible offline. Encoded in the design: uid absence is never the staleness test; `stale` is decided by `isConnected` under a `loaderId`-equality precondition.
- **Native OS surfaces** render outside the page's hit-testing area: an open `<select>` popup, a date picker, the autofill menu, the file picker, permission and print dialogs. `elementFromPoint` is clean and the click is eaten → `no_effect` with an explicit "a native popup may be open" note, never `changed`.
- **The `--selector` path can succeed where a human could not.** `el.click()` performs no hit test, fires no pointerdown/mousedown, carries `isTrusted:false` and grants no transient user activation. Verified: a `target=_blank` link is popup-blocked by selector and opens by uid; a checkbox under a full-viewport veil ends up checked. `intercepted` is not undetected there, it is inapplicable. Emit the effect-based verdict + `dispatch:'js'` + `obscured_by`, for every JS-dispatching command, not only click.
- **`--xy` has no intended element.** Only "received by X" can be reported; no interception verdict is possible.
- **App-level guards are indistinguishable from working controls before the action** — `aria-disabled` plus an early return, a `.is-loading` class, a pending-request flag, a disabled-looking div with no listener. Floor: `no_effect`, never `blocked`. Symmetrically, `blocked` is an observation at action time: we cannot say whether a disabled Submit becomes operable once validation settles.
- **Whether a value the DOM accepted survives the framework's state layer**, the form's submit interception, or server-side validation. A React input whose DOM value shows our text but whose state never updated is byte-identical to a working one. `changed` describes the DOM only.
- **Late reverts beyond the observation window.** Blur-time normalisers are caught (blur before the post-probe); a 5 s debounce is not. Every value verdict is a timestamped observation and must never be phrased as persistence.
- **Reorder detection is limited to uid order and indent depth within one snapshot projection.** A re-parent that preserves both is invisible, and a node moved between two subtrees at the same depth and index is not detected.
- **Non-DOM accessibility nodes** (Chrome's validation bubble, any browser-generated node) get positional `e*` uids that are not in `uid_map` and not stable across snapshots. Excluded from counts and summarised by role only — evidence that a node of role X appeared, never an identity.
- **`unknown` collapses two operationally different situations**: "we could not decide" and "the effect landed in a scope we could not read". The reason code and hint carry the difference (`subframe_navigated` → bind the frame; `hit_test_disagreed` → retry). An agent reading only the verdict word loses that. Deliberate tradeoff for a smaller verdict set; revisit if agents treat both as failure.
- **The pre-dispatch a11y baseline doubles the snapshot cost** for click/dblclick/hover/press/drag. `--verdict fast` skips it, and those five degrade to `unknown` whenever the stored baseline was not taken by this command. There is no configuration that both skips the baseline and keeps the confident verdict.
- **`resolve_page_target` claims the first UNCLAIMED page target** for a page name it has never seen, so a later `--page report` can silently attach to a popup an earlier click opened. `opened_tab` is reported as evidence, but the target is not reserved.
- **Focus destination when it leaves the document.** Not reproducible headless (Tab from the last focusable element wraps, because there is no browser chrome); under `--headed` focus can move to the address bar. When no node gains `focused` and `activeElement` is BODY, report `focus:{from,to:null,note:'focus left the document'}` rather than inventing a destination. No fixture: a headless one would encode behaviour that does not reproduce headed.
- **Divergence between `document.elementFromPoint` and the browser's real input hit test** — top-layer content, compositor-side scroll offsets, in-flight transform animations. Every design-record fixture was cross-checked by running the real coordinate click and observing the handler; the two agreed 12/12. Evidence, not proof of equivalence. If `DOM.getNodeForLocation` is adopted as a refinement, its frame-piercing and top-layer behaviour must be verified first, and this CLI exposes no raw-CDP passthrough, so that verification has not been done.

## Implementation order

**Slice 0 — diff hygiene.** Pure Rust, no verdict behaviour change (nothing consumes it yet). In
`diff.rs`: subtract `~` lines whose token symmetric difference is exactly `{focused}` on ANY node;
exclude `e*` uids from counts and summarise under `anonymous`; stop `trim_start()`ing indentation
in `uid_lines` and compare uid order + depth to emit `moved`. Unit-tested with no Chrome, reverts
in one commit. Fixes the two structural blindnesses (focus artifact, reorder) that poison every
later rung.

**Slice 1 — document identity by `loaderId`.** Add `{frame_id, loader_id}` to
`session::PageSession`; read them with `Page.getFrameTree` at snapshot time and action time; make
`diff::compare` take a tri-state identity (Same/Different/Unknown) instead of the URL comparison,
with `Unknown` refusing to claim a diff. Fixes four verified false positives (fragment, pushState,
hash router, frame switch) and two false negatives (form GET to self, same-URL reload) in one
edit. No new verdicts — `document_changed` simply becomes correct. Independent of Slice 0.

**Slice 2 — single-handle selector resolution.** Pure refactor. Resolve one objectId via
`Runtime.evaluate` with `returnByValue:false` and act through `Runtime.callFunctionOn` on that
handle, returning `querySelectorAll(sel).length` alongside. Same call count. Closes the hole where
pre-probe, action and post-probe each bind a different node after a re-render, and unlocks every
Slice 3 probe for the selector path. Existing tests must pass unmodified.

**Slice 3 — the probe and the postcondition verdicts.** Add the one-shot R1 probe, the `Verdict`
enum, and rungs R1/R2/R3/R6 for fill, type, select, check, uncheck, upload. Delivers
`already_satisfied`, `blocked`, `normalised`, `changed`, `stale` with zero dependence on the a11y
diff. Independently revertable: it only adds keys to the response.

**Slice 4 — attributed baseline.** Take the pre-dispatch snapshot inside the acting command, after
`scrollIntoViewIfNeeded` and before the first input event; store the projection parameters with
every snapshot and refuse to compare across differing projections; route what the gate rejects
under `unattributed` with a cause. Gates R8. Also fixes the verified case where an earlier `eval`
or `extract --scroll` lands its changes in the next action's delta. Ships behind `--verdict fast`.

**Slice 5 — hit test and `intercepted`.** R4 bound to the target's objectId: shadow descent, label
retarget, shadow-host containment, `aimIn` against `getClientRects`, the out-of-viewport settle
loop with a hard refusal, and the border-box-offset recovery probe requiring `hit===target`.
Record `dispatch` on every action and forbid `intercepted` on non-mouse paths. Includes the hover
and drag fix: both use `resolve_uid`'s PRE-scroll centre and never call `scrollIntoViewIfNeeded`,
so their coordinates are document-space on any scrolled page.

**Slice 6 — CLI/pipe parity.** Route CLI Type / Press / Scroll / Hover through `output_action` so
they carry the same payload shape as pipe. Also fix `press_key` to send `text` for single
printable characters, or reject unmapped keys loudly instead of returning `ok:true`. Prerequisite
for asserting any verdict on those four commands.

**Slice 7 — scroll and keyboard rungs.** R7's before/after `scrollingElement` measurement with the
overflow/modal discrimination and the smooth-behaviour flag; the `scrolled:{from,to}` orthogonal
field for non-scroll commands; `focus:{from,to}` and `title:{from,to}`; and the isolated-world
capture-phase `invalid` listener that turns press-Enter constraint refusal into
`blocked(constraint_validation)` without firing `invalid` events into the page.

**Slice 8 — frame scope.** Stamp every snapshot and change report with `(frameId, loaderId)` and
refuse to compare across frameIds; resolve the `loaderId` of the BOUND frame rather than the root;
add the multi-frame delta fallback that runs only when the top-frame delta is empty; handle
`Page.frameNavigated` with a `parentId` as a subframe event and key `wait_for_stabilization` off
`Page.frameStoppedLoading`, which also removes the measured 10.16 s stall.

**Slice 9 — refinements**, each independently revertable: the downgrade-only post-dispatch hit
test; `opened_tab` via `Target.getTargets` `openerId`, run only when the delta is empty; recording
the dialog handler's `(type, decision)` so a dismissed `beforeunload` becomes
`blocked(page_refused_to_unload)`; blur-before-post-probe for form-value actions; and the
`error_hint` branch for `TypeError: Illegal invocation` pointing at `type` after focus.

## Fixture plan

113 fixtures. Those already on disk are in `tests/fixtures/`; see its README. KEEP = exists, NEW =
to be written.

### Interception

- KEEP `intercept_dead_scrim` — intercepted, receiver `div#scrim`. Canonical: invisible to the a11y tree, the diff AND a screenshot.
- KEEP `intercept_sticky_header` — target below the fold → intercepted. Pins that the hit test uses the post-scroll box-model read.
- KEEP `intercept_partial_overlay` — intercepted + `partially_covered` + a clear point as a border-box OFFSET where `hit===target`.
- KEEP `intercept_pointer_events_none_target` — `blocked(pointer_events_none)` + `also_obscured_by:div#page`. Pins target-property-beats-coordinate.
- KEEP `intercept_pseudo_element_overlay` — intercepted via generated content on `div#card`. An ANCESTOR hit is not automatically safe.
- KEEP `intercept_toast_dismiss_on_click` — intercepted. The hit test is taken at dispatch time; a post-hoc probe reports a clear point 150 ms later.
- KEEP `intercept_modal_backdrop` — a background uid under a `showModal()` dialog → `intercepted(modal_dialog)` naming `DIALOG#terms`. Not blocked: blocked teaches the agent to wait instead of closing the modal.
- NEW `intercept_label_span_forwards_click` — visually-hidden checkbox whose visible box is a sibling `<span>` inside the label → changed. `hit instanceof HTMLLabelElement` misses this; `hit.closest('label').control===target` catches it.
- NEW `intercept_inert_drawer` — uid under an `inert` element (no `<dialog>`) → `blocked(inert)` naming the ancestor. Naive answers: intercepted-by-BODY, or uid-not-found with wrong advice.
- NEW `intercept_shadow_host_open` — button inside an OPEN shadow root → changed. `elementFromPoint` returns the host and `Node.contains` does not cross the boundary, so naive says intercepted on every design-system button.
- NEW `intercept_iframe_contains_target` — button INSIDE a srcdoc iframe → effect-based verdict, no interception claim. Without it the `tagName==='IFRAME'` rule is a baked-in false positive.
- KEEP `intercept_ad_iframe` — `intercepted(cross_document_overlay)`. Only valid alongside the pair above.
- NEW `intercept_wrapped_inline_link` — inline `<a>` wrapping across two line boxes → re-aim succeeds → changed, else `unknown(aim_point_off_target)`. Never intercepted-by-the-paragraph.
- NEW `intercept_smooth_scroll_below_fold` — `scroll-behavior:smooth`, target below the fold → changed after the settle loop. Fails today: dispatch at cy=3028 with innerHeight 469.
- NEW `intercept_disabled_under_banner` — `<button disabled>` also covered by a banner → `blocked(dom_disabled)` + `also_obscured_by`. The only fixture where both families' preconditions hold.
- NEW `intercept_js_click_fallback` — `display:none` element (no box model) → `dispatch:"js"`, no hit-test claim, synthetic-click advisory. Absence of evidence encoded as absence.

### Verdict basics

- KEEP `verdict_inert_no_listener` — inert button by uid AND `--selector` → `no_effect` in both, identical. Fails today by uid: the focus token alone makes `changed:1`.
- KEEP `verdict_handler_throws` — `no_effect`, exceptions under `page_errors` with no causal wording and no file:line.
- KEEP `verdict_slow_600ms` — `no_effect` carrying `observed_after_ms`. The assertion is on the WORDING, that it is never a claim about the future.
- KEEP `verdict_late_timer_misattribution` — 2000 ms timer, re-baseline, click an inert button inside its window → `no_effect` + delta under `unattributed`.
- KEEP `verdict_visual_only_class` — `#recolour` → `changed(evidence:target_attribute)` quoting the class change; `#hide` → plain changed.
- KEEP `verdict_canvas_only` — click the canvas → `unknown(canvas_target)` + screenshot hint; plus the negative half, an unrelated `<canvas>` must not change other verdicts.
- KEEP `verdict_ambient_ticker` — `goto_ticker.html` plus an inert button → `unknown(unattributed_churn)`. Never changed.
- NEW `verdict_scroll_into_view_lazy_load` — listener-free button below the fold whose `scrollIntoViewIfNeeded` trips an IntersectionObserver → `no_effect` + rows under `unattributed{cause:scrolled_into_view}`. Defeats all three naive probes at once.
- NEW `verdict_stale_baseline_eval` — goto, inspect, eval injecting a heading, click an inert button → `no_effect` + heading under `unattributed`. The cheapest attribution test in the corpus.
- NEW `verdict_reorder_no_text_change` — reorder three rows via `appendChild` → changed + `moved:[uids]`. Today: "No changes detected."
- NEW `verdict_opens_new_tab` — `<a target=_blank>` by uid → `changed(opened_tab)`; by `--selector` → `no_effect` + a hint naming the missing user activation.

### Form values

- NEW `form_value_fieldset_disabled` — fill inside `<fieldset disabled>` → `blocked(dom_disabled)`. `this.disabled` is false here and `:disabled` is true.
- KEEP `form_value_readonly_input` — changed + caveat `readonly`. Naive (the snapshot's readonly token) says blocked.
- KEEP `form_value_number_rejects_letters` — fill "abc" → `no_effect` (readback ""). Naive says changed from the focus delta.
- KEEP `form_value_controlled_revert` — `no_effect`. Needs the pre-value read; without it, indistinguishable from `already_satisfied`.
- KEEP `form_value_already_correct` — `already_satisfied`.
- NEW `form_value_cents_mask` — fill "1000" → normalised, both strings verbatim. The digits-only "content preserved" sub-label reports 1000→10.00 as lossless.
- KEEP `form_value_maxlength_divergence` — fill → changed + caveat naming cap and length; type → `normalised(truncated)`. Same element, two verdicts: the verdict is a function of (element, command).
- KEEP `form_value_contenteditable` — fill → `blocked(wrong_element_type)`; type → changed graded by length delta, not bare containment. Also pins the Illegal-invocation `error_hint` branch.
- KEEP `form_value_reset_on_submit` — fill then submit → the click reports changed AND names the field it cleared. The only fixture asserting cross-field re-probing.
- NEW `form_value_checkbox_value_setter` — fill "true" on a checkbox → `blocked(wrong_element_type)`. The read-back, the family's own trusted signal, reports an exact match here.
- NEW `form_value_navigating_change_handler` — change handler navigates → navigated. Pins R5 before R6.
- NEW `form_value_range_sanitizer` — fill "abc" on `type=range` → `no_effect(sanitizer_substituted)` + a warning that the field moved 30→50 collaterally. Naive says normalised.
- NEW `form_value_out_of_range` — fill "999" on min=1 max=10 → changed + caveat carrying `validity.valid=false` and the browser's `validationMessage`.
- NEW `form_value_blur_normaliser` — fill "  ab cd  " into a field rewritten on blur → normalised. Converts a previously undetectable case into a caught one.
- NEW `form_value_duplicate_selector` — selector matching a hidden mobile copy first → changed + a loud caveat naming `matches:2` and `rects:0`.
- NEW `form_value_type_focus_escapes` — fill field A, then type into a DISABLED field B → the command must assert `activeElement` after focus and error; the fixture asserts the characters never reach field A.

### Select

- KEEP `verdict_select_disabled_option` (optgroup half) — `blocked(option_unreachable)`, no dispatch.
- NEW `verdict_select_hidden_option` — `option[hidden]` (`:disabled` false) → `blocked(option_unreachable)`.
- NEW `verdict_select_multiple` — `<select multiple>` with 3 selected → `blocked(multi_select_unsupported)`. Highest-severity silent data loss in the corpus.
- KEEP `verdict_select_value_text_mismatch` — "Canada" as one option's value and another's text → `unknown(ambiguous)` with both resolutions. Never `already_satisfied`.
- NEW `verdict_select_option_label_attr` — `<option value=y label='Visible Y'>y</option>`: select by the name inspect prints must work, and `selected_text` must report the label.
- KEEP `verdict_select_reverting_handler` (`#sync`, `#later`) — `#sync` → normalised inside the same call; `#later` → changed carrying `observed_after_ms`. `#soon` dropped: it pins a timing constant.
- KEEP `verdict_select_hash_navigation` — handler sets `location.hash` → changed, uids alive. Today: `document_changed:true` + "your uids are dead" while listing live uids.
- NEW `verdict_select_dependent` — country→state cascade → changed + `side_effects` naming `#state`'s move.

### Check / uncheck

- KEEP `verdict_check_already_checked` — `already_satisfied`, `mutation_dispatched:false`.
- NEW `verdict_check_aria_already_on` — `<div role=checkbox aria-checked=true>` → `already_satisfied`, no dispatch. Today it clicks and UNCHECKS it while reporting success.
- NEW `verdict_check_indeterminate` — uncheck an indeterminate checkbox → changed. Today: "Already unchecked", nothing dispatched, mixed state destroyed.
- KEEP `verdict_check_reverting_handler` — `no_effect` + `hit_test:'target'` so the agent does not hunt an overlay.
- KEEP `verdict_check_overlay_intercepted` — by uid → intercepted naming `#cookie-veil`; by `--selector` → changed + `dispatch:'js'` + `obscured_by`. Two honest verdicts, one page.
- KEEP `verdict_check_radio_group` — uncheck a CHECKED radio → `blocked(radio_not_uncheckable)`; uncheck an already-unchecked one → `already_satisfied`. Pins R2-before-R3.
- KEEP `verdict_check_text_input` (`#qty`) — `blocked(wrong_element_type)`.

### Navigation

- NEW `nav_reload_same_url` — `location.reload()` → navigated, uid map dropped. Today: +N/-N with `document_changed:false` across two documents.
- KEEP `nav_form_get_self` — GET form to itself → navigated. URL identical, `loaderId` changed.
- KEEP `nav_fragment_only` — click an anchor → changed; click AGAIN → `no_effect`. The second click forces the `:target` read to be before-and-after.
- KEEP `nav_fragment_dead_target` — anchor with no matching element → `no_effect`, from the empty DOM delta with the fragment as a field.
- KEEP `nav_hash_router` — changed, per-scope uid report. `:target === null` in both this and the fixture above, so fragment evidence can never be a verdict source.
- KEEP `nav_pushstate_append` — changed `added:1`. Today: `document_changed:true` and `added:0`, hiding the paragraph that appeared.
- KEEP `nav_soft_rerender_identical` — `#refresh` → changed + `rebuilt:true`, naming that previously-held uids are dead. Not stale: the map handed back is already fresh.
- KEEP `nav_ghost_click_after_rerender` (step 3) — detached uid → stale, NOT DISPATCHED, no `ok:true`.
- KEEP `nav_meta_refresh` — `pending_navigation{in_ms,to}` from the meta tag + a note that history is REPLACED so `back` will not return.
- KEEP `nav_delayed_redirect` (next-command assertion) — the click step asserts nothing; the following command asserts stale for a uid action and `uid_map_invalidated` for a selector action.
- KEEP `nav_new_tab_link` — uid → `changed(opened_tab)`; `--selector` → `no_effect` + a hint naming the popup block.
- NEW `nav_broken_link` — link to a missing `file://` path → `navigated(navigation_failed)` + the error code, worded so it does not read as arrival. The only offline demo of cross-document uid collision.
- NEW `nav_iframe_self_navigate` — bind `#checkout`, then a click navigates the iframe → navigated scoped to the bound frame. Today: root `loaderId` unchanged, `document_url` returns "", the diff reported as same-document movement.
- NEW `nav_beforeunload_dismissed` — under `--dialog dismiss` → `blocked(page_refused_to_unload)`. The evidence exists in-process and is currently written only to stderr.
- NEW `nav_modal_aria_hidden` — a dialog that `aria-hidden`s the shell, then closes → `hidden:[uids]`, never stale; closing resurrects the same ids.
- NEW `nav_filtered_baseline` — `inspect --filter button` then click → the post-read uses the baseline's projection, or `unknown(baseline_projection_mismatch)`. Never stale from a 0.0 uid overlap.

### Shadow DOM and frames

- KEEP `shadow_closed_root_button` — uid inside a CLOSED shadow root → changed. Pins that the probe is bound to the target's objectId.
- KEEP `iframe_effect_lands_inside` — top-frame button whose effect lands in `#cart` → `changed(scope:multi_frame)`. Never `no_effect`.
- KEEP `iframe_click_xy_lands_inside` — `--xy` into a frame → `changed(scope:multi_frame)` + `received_by_frame`. No interception verdict is possible for `--xy`.
- KEEP `iframe_nested_srcdoc` — every change report carries the `(frameId, loaderId)` it was computed for, and two reports with different frameIds are never compared.
- KEEP `iframe_frame_switch_false_navigation` — `frame #side`, inspect, `frame main`, click → changed. Today `document_changed:true`, which a verdict layer renders as navigated.
- KEEP `iframe_cross_origin_data` — a link navigating a `data:` subframe → `unknown(subframe_navigated)`, top uids alive. Also asserts the command returns well under 10 s (today 10.16 s).

### Scroll

- KEEP `scroll_bottom_terminates` (2 steps) — a working scroll → changed naming the scroller; at max → `no_effect` with a hint naming the window that does NOT say "stop scrolling".
- NEW `scroll_bottom_but_more_coming` — at max with a 400 ms append → the hint must contain no terminate instruction, and a second scroll after `wait text` returns changed. This is what makes the terminate hint safe to ship.
- KEEP `scroll_nested_pane_only` — scroll on an `overflow:hidden` document → `blocked(no_document_scroller)` naming region "Feed"; `scroll <uid>` → changed.
- NEW `scroll_short_page` — content fits the viewport, overflow not hidden → `no_effect(fits_viewport)`. The pair forces the overflow read.
- NEW `scroll_locked_by_modal` — `body{overflow:hidden}` with a dialog open → `blocked(scroll_locked)` naming the modal.
- KEEP `scroll_wrong_scroller` — changed naming `#document` + `other_scrollers` as a capped, truncation-flagged advisory, never a verdict driver.
- KEEP `scroll_lazy_settles_late` — changed + `settled:false`. Never `no_effect`.
- KEEP `scroll_virtualized_rows` — changed AND stale, stale from per-node `isConnected` on the removed subset.
- NEW `scroll_virtualized_recycled_rows` — recycled rows (same uids, new text) → changed + `relabelled:[{uid,was,now}]`. The silent-wrong-row case set intersection cannot see.
- NEW `scroll_smooth_behavior` — changed + `settling:true`, no terminate hint.
- NEW `scroll_uid_already_in_view` — `scroll <uid>` on an element already centred → changed + `already_in_view:true`. Never `no_effect`: the postcondition is a position.

### Keyboard

- KEEP `press_tab_focus_churn` — `no_effect` for content + a `focus:{from,to}` line; a RootWebArea `document.title` change on the same page must SURVIVE the filter and be reported as `title:{from,to}`.
- KEEP `press_enter_submits` (count assertion) — surviving `changed===1` after artifact filtering, with the RootWebArea line routed to its own key rather than merely absent.
- KEEP `press_enter_blocked_by_required` — `blocked(constraint_validation)` naming the field, driven by an isolated-world capture-phase `invalid` listener installed before the action.
- NEW `press_enter_preventdefault_with_invalid_sibling` — a handler preventDefaults and renders results while an untouched required field is invalid → changed. The pair forces the causal signal over the state read.
- KEEP `press_escape_closes_modal` — Escape #1 → changed summarised as "modal closed", dialog nodes under `hidden` not `stale`; Escape #2 → `no_effect`.
- NEW `press_escape_div_overlay_ignores` — Escape against a div overlay with no keydown handler → `no_effect` that does NOT assert "nothing left to dismiss" (`:modal` returns 0 while a full-screen overlay blocks every click).

### Hover and blocked

- KEEP `hover_opacity_only` — hover twice → byte-identical verdicts (`no_effect` + caveat). The pass criterion is the equality, not the undetectability.
- KEEP `hover_intercepted_by_overlay` (on a SCROLLED page) — intercepted naming `div#veil`. Fails today for a second reason: hover uses the pre-scroll centre and never calls `scrollIntoViewIfNeeded`.
- KEEP `blocked_button_disabled` (double-submit half) — first click → changed; second → `blocked(dom_disabled)`. The disabled read is a PRE-dispatch read; a post-read inverts every self-disabling submit button on the web.
- KEEP `blocked_aria_disabled_only` (`#aria-live-btn`, `#aria-guarded-btn`) — changed and `no_effect`. Neither may be blocked: the AX `disabled` property is set for `aria-disabled` too.
- KEEP `blocked_fieldset_disabled` (`#fs-btn`, `#inner-btn`, `#legend-btn`) — blocked, blocked, changed. Discriminates `:disabled` from both `el.disabled` and `closest('[disabled]')`.
- KEEP `blocked_readonly_input` (part B, autofocused from the page) — `blocked(readonly_text_entry)`. The focus must come from the fixture, not a separate eval, or the baseline is poisoned.
- KEEP `blocked_upload_targets` (`#silent-file`, `#text-field`) — changed from a `files.length` read with an empty delta; `blocked(wrong_element_type)`.
- KEEP `blocked_check_non_checkbox` (`#plain-div`, `#aria-cb-on`) — `blocked(no_checkable_state)`; uncheck the ARIA box → changed, and it must not say "Already unchecked".
- KEEP `blocked_select_not_a_select` (`#disabled-select`, "Atlantis") — `blocked(dom_disabled)`; a non-matching value is `invalid_argument`, NOT blocked, so the agent retries the value instead of abandoning the element.
- KEEP `blocked_detached_target` — uid detached by a re-render → stale, not dispatched. The handler demonstrably runs via `js_click` today.
- KEEP `blocked_zero_size_hits_neighbour` ("Collapse all") — intercepted by `#backdrop-card`, not blocked and not changed.
- KEEP `blocked_offscreen_scrolls_into_view` ("Off-canvas action") — `no_effect` with the dispatched coordinate in the message. We cannot honestly call an enabled, laid-out element blocked.
- KEEP `blocked_inert_subtree` (B and C) — `--xy` → `blocked(inert)`; `--selector` → changed + `dispatch:'js'`. Same element, opposite honest verdicts, keyed on the dispatch mechanism.
- NEW `blocked_submit_invalid_form` — submit with an invalid required field → `blocked(constraint_validation)`. Naive says changed because Chrome's validation bubble enters the tree as an added alert node. Highest-frequency real-world case.
- NEW `blocked_label_control_disabled` — `--selector` a `<label>` whose control is disabled → `blocked(dom_disabled, via label)` after retargeting through `el.control`. Naive says changed: the label's own handler ran.
- NEW `blocked_first_match_ambiguous` — `--selector '.submit'` where the first match is a disabled duplicate → blocked + `matches:2` + `judged:'first'` + a disambiguation hint.
