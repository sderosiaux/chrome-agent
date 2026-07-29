# Verdict taxonomy — design spec

Rescued from a workflow output under `/tmp`, which a reboot clears. This is the design the
v0.9.x work is an on-ramp to, not a description of what ships today.

**What is built** (`src/verdict.rs`): five verdicts — `changed`, `navigated`, `unchanged`,
`unknown`, `not_checked` — classified from the accessibility delta and the document
identity, which is everything currently measured. `unchanged` is the placeholder for the
`no_effect` / `intercepted` split below: promoting it needs proof of delivery, which needs
the hit test in slice 5.

**What is not**: the one-shot probe bound to the target's objectId, the hit test, the
attributed baseline, and the scroll/keyboard rungs. Do not redesign what follows — it was
derived from 113 fixtures and 107 cases where a plausible signal reports a confident wrong
answer.


## The nine verdicts

NINE verdicts. Every one is emitted only from a signal we read ourselves in the same action; none is ever inferred from the command's own success message (verified: "Checked selector '#qty'" is returned today for a text input).

changed — a postcondition read on the acted-on handle equals the request (fill/select/check/uncheck/upload), OR the target's own attribute set moved (class/style/aria-expanded/aria-pressed/aria-selected/data-state/hidden), OR a scroll offset measurement moved, OR an attributed a11y delta is non-empty after noise subtraction. Always carries observed_after_ms, dispatch (mouse|js|insert_text|key), and scope (frameId). Never means "did what you intended".

already_satisfied — the goal state held BEFORE dispatch, read through the SAME accessor the postcondition uses. Carries mutation_dispatched truthfully (false for check/uncheck which already short-circuit; true for fill/select which do not). Exists because no_effect here makes an agent retry and a retry on a checked box unchecks it.

normalised — post-read is non-empty, differs from the request, and shares a token/digit with it. Reports requested and actual verbatim. Never claims "content preserved" (verified counterexample: fill "1000" -> "10.00").

no_effect — delivery was PROVEN (hit test resolved to the target, or dispatch:"js" which cannot be intercepted, or the postcondition read succeeded) AND the observation window was quiet AND attribution was clean. Always time-scoped: "no observable change within N ms". Never a claim about the future, never a claim that the element is inert.

blocked — a DOM property read BEFORE dispatch proves the action cannot apply, and we refuse to dispatch. Reasons: dom_disabled (`:disabled`), inert (`closest('[inert]')`), pointer_events_none, wrong_element_type, no_checkable_state, radio_not_uncheckable, multi_select_unsupported, option_unreachable, readonly_text_entry (type only), constraint_validation, scroll_locked, no_document_scroller, page_refused_to_unload. Worded as an observation at action time, never as a permanent property.

intercepted — MOUSE dispatch only. Pre-dispatch hit test at the exact dispatched (cx,cy), after shadow descent and label retarget, resolves outside the target's flat subtree, AND the aim point is inside the target's client rects. Names the receiver as tag#id.class + position + z-index + 40 chars of text (overlay containers are absent from the a11y tree — verified for #scrim, #cookie-banner, #chart-hotspot).

stale — the uid names a node with isConnected === false, OR the stored snapshot's loaderId differs from the live one and the action is uid-targeted. NOT DISPATCHED. Refusal is what makes the word safe: a detached node's handler still runs via the js_click fallback (verified) and "stale" would then imply nothing happened.

navigated — the acting frame's loaderId changed. Sub-reason navigation_failed when the committed URL is chrome-error://. Carries caused_by from Page.frameRequestedNavigation.reason, or caused_by:"unknown" — never "your click navigated". Implies the uid map was dropped.

unknown — the honest floor, always with a reason code and a hint: scroll_not_settled, aim_point_off_target, hit_test_disagreed, ambiguous, subframe_navigated, scope_unreadable, baseline_projection_mismatch, unattributed_churn, navigation_in_flight, canvas_target.

DELIBERATELY NOT BUILT: `pending`. It requires wrapping setTimeout/setInterval/rAF/fetch/XHR. setInterval and rAF never decrement, so the counter becomes a property of the page, not the action, and maxDelay becomes an unrelated number shipped as a re-check promise. It also breaks `setTimeout.toString()` returning [native code], widening the stealth fingerprint. Cost of skipping it: a 600ms handler reads no_effect. Mitigated by mandatory observed_after_ms wording, not by a fabricated number.
DELIBERATELY NOT BUILT: `unobservable`. Folded into unknown(reason=subframe_navigated|scope_unreadable) plus a hint naming the frame to bind. Tradeoff stated in honest_limits.

## Classifier ladder

ORDERED, FIRST MATCH WINS. Precedence rule stated once and fixtured: target-property facts beat coordinate facts; a satisfied goal beats an unreachable one. Exactly one verdict is emitted; everything else becomes a payload field.

--- PRE-DISPATCH ---

R0. SCOPE + DOCUMENT IDENTITY (all commands)
  Call: Page.getFrameTree -> walk childFrames to client.frame_context().frame_id (or root). Read {frameId, loaderId, url, urlFragment}.
  Cost: 1 CDP call, ~0.3ms, no page JS. Polling, NOT event subscription — CdpClient::events() is a tokio broadcast (client.rs:159) that only delivers post-subscribe messages, and select_option/set_checked never subscribe at all (element.rs:525-663), so an event-derived signal is structurally blind there.
  - frame absent / call fails -> identity is Unknown (tri-state, not `false`). Action still runs; the verdict floor becomes unknown(scope_unreadable). This replaces compare()'s `_ => false` arm (diff.rs:110), which today defaults an unreadable signal to the CONFIDENT answer.
  - stored loaderId != live loaderId AND the action is uid-targeted -> **stale**, DO NOT DISPATCH.
  - stored loaderId != live loaderId AND the action is selector/xy-targeted -> NOT a verdict. Emit field uid_map_invalidated:true and continue. (Selector commands query the live document; refusing them blocks a valid action.)

R1. HANDLE RESOLUTION + ONE-SHOT PROPERTY PROBE
  uid path: DOM.resolveNode (already done by resolve_uid, element.rs:31) then ONE Runtime.callFunctionOn on that objectId.
  selector path: Runtime.evaluate with returnByValue:FALSE to get an objectId, then the same callFunctionOn. (Today fill_selector/click_selector use returnByValue:true and return no handle — element_selector.rs:107-146 — so pre-probe, action and post-probe are three independent querySelector calls that can bind three different nodes.)
  Probe returns, in one round trip: {connected:this.isConnected, rendered:this.getClientRects().length>0, rects:[...], matches:document.querySelectorAll(sel).length, retargeted (LABEL -> this.control), tag, type, disabled:t.matches(':disabled'), inert:t.closest('[inert]')!==null, pe:getComputedStyle(t).pointerEvents, readOnly, multiple, size, isContentEditable, value, textContent, checked:(t.indeterminate?'mixed':t.checked), ariaChecked:t.getAttribute('aria-checked'), role, selValue, selIdx, selText:(o.label||o.text), optDisabled/optHidden for the requested option, maxLength, validity:{valid,message}, formInvalid:(t.form && !t.form.noValidate && t.form.matches(':invalid')), attrs:{class,style,aria-expanded,aria-pressed,aria-selected,data-state,hidden}, rootIsShadow:(this.getRootNode() instanceof ShadowRoot)}
  Cost: 1 Runtime.callFunctionOn, returnByValue, ~0.5-1ms. Replaces every per-family probe in the design records.
  MUST be bound to the target's objectId, not a bare Runtime.evaluate: `this.getRootNode()` on a handle to a node inside a CLOSED shadow root returns that closed root and `.host` works, which is what makes R4 correct for closed roots without DOM.getNodeForLocation.
  - connected === false -> **stale**, DO NOT DISPATCH.
  - LABEL retarget: never block a <label>. Verified: check --selector on a label genuinely toggles its control on both paths. Report retargeted_to.

R2. ALREADY-SATISFIED (fill / select / check / uncheck only)
  Cost: zero — read off R1.
  Compare pre-state to request through the accessor the POSTcondition will use. For non-INPUT checkables that is aria-checked, not `!!el.checked`. (Verified live: today `check` on <div role=checkbox aria-checked=true> reads !!undefined -> false, clicks, and UNCHECKS it while reporting success.)
  - select: if a value-match and a text-match exist at different indices -> **unknown(ambiguous)** with both candidates. Never already_satisfied under ambiguity.
  - equal -> **already_satisfied** + state + mutation_dispatched.
  Ordered BEFORE R3 on purpose: `uncheck` on an already-unchecked radio must not be blocked(radio_not_uncheckable). Today the tool gets this right; a precondition-first ladder would break it.

R3. BLOCKED (provable target properties)
  Cost: zero — read off R1. All refuse to dispatch.
  any: disabled -> dom_disabled. Uses `:disabled`, which covers fieldset inheritance AND the first-<legend> carve-out; `el.disabled` misses the first (verified false inside fieldset[disabled]) and `closest('[disabled]')` misses the second.
  mouse path: inert -> inert (+ also_received_by from R4's receiver, since inert REDIRECTS the event rather than suppressing it). pe==='none' -> pointer_events_none (+ also_obscured_by).
  fill: tag not in {INPUT,TEXTAREA} -> wrong_element_type (SELECT excluded: fill_selector picks HTMLInputElement.prototype's setter and throws Illegal invocation). type in {checkbox,radio,file,button,submit,reset,image,range,color} -> wrong_element_type.
  type: readOnly && text-entry type -> readonly_text_entry. NEVER for `press` (press_key maps only Enter/Tab/Escape/Backspace/Delete/arrows/Space — element.rs:250 — so a printable key inserts nothing regardless of readonly, and a readonly combobox legitimately responds to Space).
  select: tag!=='SELECT' -> wrong_element_type. multiple||size>1 -> multi_select_unsupported (selectedIndex= collapses the whole selection). option `:disabled` || hidden || display:none -> option_unreachable.
  check/uncheck: INPUT with type not in {checkbox,radio} -> wrong_element_type (`typeof el.checked === 'boolean'` is TRUE for text inputs). non-INPUT without role in {checkbox,radio,switch,menuitemcheckbox,menuitemradio} or without aria-checked -> no_checkable_state. radio && desired===false -> radio_not_uncheckable.
  upload: not input[type=file] -> wrong_element_type.
  click on a submit control && formInvalid -> constraint_validation, naming the first `:invalid` field. Uses the pseudo-class, never checkValidity() (which fires `invalid` events into the page we are about to judge).
  scroll: docMax===0 && (html/body overflowY==='hidden' || a :modal|[aria-modal]|dialog[open] exists) -> scroll_locked / no_document_scroller, naming the pane or the modal.
  If matches>1, blocked carries matches + judged:"first" + a disambiguation hint. blocked is the most action-terminating verdict; a wrong-element blocked costs more than a wrong-element no_effect.

R4. HIT TEST — MOUSE DISPATCH ONLY
  Runs after scrollIntoViewIfNeeded and after the SECOND DOM.getBoxModel (element.rs:104-111), at the exact (cx,cy) about to be dispatched, immediately before the first Input.dispatchMouseEvent.
  Cost: 1 Runtime.callFunctionOn bound to the target's objectId, ~0.5ms.
  Body: bounds check -> document.elementFromPoint(cx,cy) -> `while (h && h.shadowRoot) { const n=h.shadowRoot.elementFromPoint(cx,cy); if(!n||n===h) break; h=n; }` -> landed = h===this || this.contains(h) || h.closest('label')?.control===this || (this.getRootNode() instanceof ShadowRoot && this.getRootNode().host.contains(h)) -> aimIn = this.getClientRects() contains (cx,cy) -> also returns h.ownerDocument===this.ownerDocument, h.matches(':modal'), h.tagName==='IFRAME'.
  Rungs:
  a. out of viewport / null hit -> the scroll has not settled. Re-read DOM.getBoxModel until two consecutive reads agree (max 5, 30ms apart), then re-test. Still out -> **unknown(scroll_not_settled)**, DO NOT DISPATCH. Splits the "7px below the fold" shrug from the verified `scroll-behavior:smooth` case where the dispatch fired 2500px away, and fixes a live click bug.
  b. aimIn === false -> the dispatch point is not on the element (verified on a wrapped inline link: content_center falls in the gap between line boxes). NEVER intercepted. Re-aim at the centre of the largest client rect, re-test once; still false -> **unknown(aim_point_off_target)** + hint to use --selector.
  c. hit is an IFRAME AND the target's ownerDocument is not the hit-testing document -> the target is INSIDE that frame; emit NO interception claim, fall through. Only when the target is provably in the same document as the hit test may we say cross_document_overlay.
  d. landed === false -> **intercepted** + receiver. Sub-reason modal_dialog when h.matches(':modal'). Recovery probe: sample the border-box quadrant centres and inset corners; a point qualifies ONLY if hit===target (not a descendant — verified that a descendant can be a dismiss control that destroys the toast) and is reported as an OFFSET from the border-box origin, re-resolved at use time, never as an absolute viewport coordinate handed across turns.
  e. landed === true -> record hit_test:"target" and fall through.
  NOT REACHED on the js paths. element.rs has two early returns to js_click (no box model at resolve time, and box model gone after scroll); both skip this region entirely. dispatch is recorded as "js" there, intercepted is forbidden, and no clean-hit-test claim is made. Keyed on the dispatch mechanism, never on the --selector flag: dblclick_selector dispatches native mouse (element_selector.rs:48-84) while click_selector does not.

--- DISPATCH ---
Baseline for R8 is taken HERE: after scrollIntoViewIfNeeded, after R4, immediately before the first input event. Changes caused by our own scroll (verified: an IntersectionObserver firing when scrollIntoViewIfNeeded brings a sentinel into view) belong to the scroll and are reported under unattributed{cause:"scrolled_into_view"}.

--- POST-DISPATCH ---

R5. NAVIGATION (all commands)
  Call: Page.getFrameTree again. Cost: 1 call.
  - acting frame's loaderId changed -> **navigated**. URL equality is not the test: verified a form GET to self changes loaderId with an identical URL, and a fragment click changes the URL with an identical loaderId.
  - committed URL starts with chrome-error:// -> navigated(navigation_failed) + the error code from the tree. Page.navigate's error_text check (goto.rs:76) is bypassed on click-initiated navigation.
  - Page.frameNavigated seen with a parentId -> NOT navigated. Top uids alive; that frame's content unread -> continue to R8's multi-frame fallback, or unknown(subframe_navigated) if the frame is unreadable. Also key wait_for_stabilization off Page.frameStoppedLoading for that frameId (element.rs:347 currently blocks up to 10s waiting for loadEventFired, which never fires for subframes — measured 10.16s).
  - frameRequestedNavigation/frameStartedLoading seen with no frameNavigated -> **unknown(navigation_in_flight)**, never no_effect.
  - beforeunload answered `dismiss` by our own handler during the window -> **blocked(page_refused_to_unload)**. Requires spawn_dialog_handler to record (type,decision,ts) on the client instead of only writing to stderr. Zero extra CDP calls.

R6. POSTCONDITION READ (fill / type / select / check / uncheck / upload)
  Cost: 1 Runtime.callFunctionOn on the SAME objectId. Dead handle -> **stale(raced)**.
  For form-value actions, blur() first, then read (catches deterministic blur-time normalisers; costs nothing).
  fill: value===requested -> **changed** (+caveats readonly | exceeds_maxlength(n,len) | validity.valid===false carrying the browser's own validationMessage verbatim). value===pre && pre!==requested -> **no_effect**(refused). value==="" && requested!=="" -> **no_effect**(rejected). value differs and shares NO token/digit with the request -> **no_effect**(sanitizer_substituted) + a warning that the field moved pre->post as a side effect (verified: type=range "abc" -> "50"). Otherwise -> **normalised**, both strings verbatim, no cause guessed.
  type: value.length - pre.length === requested.length && value !== pre -> **changed**. Bare containment is a tautology when the field already holds the string.
  select: selValue===requested || selText===requested || optLabel===requested -> **changed** + matched_by:{value|text|label}. matched_by:"value" only when the option has a value attribute (a valueless <option> reports value===text). Otherwise -> **normalised** with requested and actual. Also emit side_effects: other form controls whose value/checked moved (country->state cascades), read in the same call.
  check/uncheck: state through the SAME accessor as R2 equals desired -> **changed**. Else -> **no_effect** + hit_test/dispatch so the agent does not hunt an overlay that does not exist.
  upload: files.length>0 -> **changed**. The only evidence that exists — a display:none file input has no a11y node and produces an empty delta.

R7. SCROLL MEASUREMENT (scroll only)
  Cost: 2 Runtime.evaluate (one before, one after), each reading ONLY document.scrollingElement.{scrollTop,scrollLeft,scrollHeight,clientHeight} plus getComputedStyle(...).scrollBehavior. NOT querySelectorAll('*') — that walk is keyed by `id||tagName` (collides), capped at 64 (truncates silently in document order), and re-walked independently before and after (misaligns when the action adds a scroller).
  `scroll <uid>`: probe the target's scrollable-ancestor chain and read the target's rect. In viewport after -> **changed** (+already_in_view when it did not move; the goal is a position, not a displacement). Not in viewport after -> **blocked(clipped)**.
  offset moved -> **changed**, naming the scroller. scrollBehavior==='smooth' and the delta != the requested magnitude -> changed + settling:true and NO terminate hint.
  offset unchanged && top >= max-1 -> **no_effect** + a hint that names the observation window and does NOT say "stop scrolling".
  docMax===0 && not overflow-hidden && no modal -> **no_effect(fits_viewport)**.
  For every NON-scroll command an offset delta is orthogonal state: it goes to a `scrolled:{from,to}` field and contributes nothing to the verdict (otherwise every action issued in the wake of a smooth scroll inherits a spurious changed).

R8. ATTRIBUTED DELTA (click / dblclick / hover / press / drag — the commands with no postcondition)
  Cost: 1 extra Accessibility.getFullAXTree for the pre-dispatch baseline. This is the largest cost in the design and is skippable with --verdict fast (which degrades these five commands to unknown when the stored baseline was not taken by this command).
  Noise subtraction (pure Rust in diff.rs, unit-testable with no Chrome):
   - drop any `~` line whose token symmetric difference is exactly {focused}, on ANY node — not only RootWebArea. Verified: in a real flow the noise floor is a TWO-line focus transfer including a real element, and the loose "drop RootWebArea lines" rule silently discards a document.title change, which is often the only SPA-route signal.
   - exclude e* uids from the counts (snapshot.rs:314 assigns them positionally and never inserts them into uid_map, so e1-vs-e1 across two snapshots is not an identity). Summarise as anonymous:{appeared:[roles],disappeared:[roles]}.
   - compare uid ORDER and INDENT DEPTH, not just text -> emit moved:[uids]. uid_lines (diff.rs:195) trim_start()s the indentation away and pairs by uid, so a reorder and a re-parent are both invisible today; the ordered Vec already exists.
  Attribution gate — a delta may be credited ONLY if all hold, else it goes under `unattributed` and the verdict is **unknown(unattributed_churn)**:
   - the baseline was taken by THIS command, after the scroll, immediately before dispatch;
   - the baseline and the post-read used the SAME projection (verbose/max_depth/filter). dispatch_inspect (pipe_dispatch.rs:163) persists a FILTERED snapshot as the baseline while attach_change_report re-reads with max_depth:None, so every uid the lens dropped scores as a death;
   - loaderId unchanged across the action.
  Then:
   - surviving added+removed+changed+moved > 0 -> **changed**.
   - all zero, but a target attribute moved (class/style/aria-*/data-state) -> **changed(evidence:target_attribute)**.
   - all zero, top frame quiet, and same-process child frames exist -> re-read getFullAXTree(frameId) per child and diff those. Non-empty -> **changed(scope:multi_frame)**. This is a targeted widening, not a downgrade: "the page contains an iframe" fires on ~95% of real pages and a verdict that always fires carries no information.
   - all zero everywhere, a NEW page target with openerId === our target_id appeared (Target.getTargets before/after, filtered to type=="page") -> **changed(opened_tab)**, scoped: the current document did not change and the session is still attached to the old tab.
   - all zero everywhere AND delivery proven (hit_test:"target" or dispatch:"js") -> **no_effect** + observed_after_ms + a window-scoped hint.
   - all zero and delivery NOT proven -> **unknown**.
  Post-dispatch hit test (mouse only, 1 extra call): DOWNGRADE ONLY. pre=landed, post=not-landed, delta EMPTY -> unknown(hit_test_disagreed) with both observations. If the delta is non-empty the disagreement is explained by the action's own effect (a menu opener paints its own backdrop over itself) and the verdict stands. Never allowed to upgrade — that would defeat intercept_toast_dismiss_on_click, where the interceptor removes itself on the click it ate.

TOTAL ADDED COST PER ACTION: R0+R5 = 2 Page.getFrameTree; R1+R6 = 2 Runtime.callFunctionOn; R4 = 1-2 Runtime.callFunctionOn (mouse only); R8 = 1 getFullAXTree (five commands only, skippable). Everything except the getFullAXTree is sub-millisecond on an already-open local socket. The listener-ancestor walk (5-15 DOMDebugger calls) is NOT built — see honest_limits.

## Honest limits

- Capture-phase stopPropagation on document or window. The hit test is clean, the coordinates land on the target, and nothing happens. DOMDebugger.getEventListeners can enumerate listeners but not whether one swallows the event. Floor: no_effect. Never changed, never intercepted — the event was received and discarded, not taken by something else.
- We do NOT build the listener-absence probe at all. `getEventListeners returns [] on the whole ancestor chain` was the design records' only path to a CONFIDENT no_effect, and it is defeated by every native behaviour with no listener: <label for>, <summary>/<details>, <select>, <a href>, mailto:/tel:, <a download>, and a submit button inside a form. It also cannot see delegation. Consequence: no_effect is always the weak form — 'delivered, page quiet for N ms' — never 'nothing is wired to this element'.
- We do NOT build a pending-work probe (P3). setInterval never decrements and requestAnimationFrame re-arms every frame, so the counter is a property of the page rather than of the action, and maxDelay becomes an unrelated number shipped as a re-check promise. Patching timers also breaks setTimeout.toString() returning [native code], widening the stealth fingerprint. Cost paid: a handler on a 600ms timer, a debounced fetch, or a 300ms hover-intent menu all read as no_effect. Mitigated only by mandatory observed_after_ms wording.
- We do NOT build a global MutationObserver (P2). It measures whether the page is alive, not whether the action did something: on any page with a clock, a poller, a chat socket or an ad frame it returns >0 always, manufacturing changed for every no-op. Replacement is target-scoped: we read the target's own class/style/aria-*/data-state before and after. Consequence: a CSS class flipped on a SIBLING or an ancestor, an opacity/transform/clip-path reveal, and a height:0 collapse are all invisible -> no_effect + caveat.
- Canvas, WebGL, WebGPU and OffscreenCanvas painting leave no MutationRecord and no a11y change. We degrade to unknown ONLY when the target is the canvas or inside it; a chart elsewhere on the page must not poison unrelated verdicts. An OffscreenCanvas in a worker is not even clipped by the same box, so the --verdict pixels escape hatch would not cover it either.
- Effects that leave the page: analytics beacons, localStorage/IndexedDB writes, postMessage to a parent, a click-initiated browser-native download (Page.downloadWillBegin is not wired). Floor: no_effect scoped to the page, not to the world.
- The race between the hit test and the mouse dispatch. Testing immediately before dispatch narrows it; the post-dispatch re-test downgrades only when the delta is ALSO empty. A CSS transition that finishes mid-dispatch AND produces a delta (clicking a menu item while the menu is still animating open, so the neighbour is activated) therefore reads as changed. The alternative — downgrading on any disagreement — turns every successful menu opener into unknown, because its own effect paints a backdrop over the target.
- Whether the element that received an intercepted click did what the agent wanted. We prove which element received it; intent is unreadable. Never upgraded to changed on the grounds that something useful happened.
- Whether an observed change is the change that was ASKED for. Every verdict answers 'did the page react', not 'did it do the right thing'. A click that opens the wrong menu scores changed exactly like the right one. That gap belongs to a layer above this taxonomy and the wording must not imply it is closed.
- An interceptor inside a CLOSED shadow root (as opposed to a target inside one, which we handle). elementFromPoint returns the host and host.shadowRoot is null, so the receiver cannot be named. We report the host with an explicit 'the receiver is inside a closed shadow tree' caveat.
- User-agent shadow roots — <video> controls, date pickers, the <input type=file> button. getNodeForLocation only descends with includeUserAgentShadowDOM, and those backendNodeIds are in no uid map. Our containment climb treats a hit on the target's own UA internals as landed; a hit on a DIFFERENT element's UA internals is named only as that element.
- Real out-of-process iframes. Not reproducible offline: file://, srcdoc and data: children all stay in the parent's process, so getFullAXTree(frameId) and createIsolatedWorld keep working. On a genuinely cross-site frame those calls can fail (the frame is another CDP target needing attachToTarget). Any failure of the multi-frame fallback yields unknown(scope_unreadable), never no_effect and never a silent success. Testing it needs two local HTTP origins, outside the file:// fixture contract.
- The identity of the real receiver inside a cross-origin iframe. The verdict is solid from the parent's hit test; the culprit's name is not. We report the <iframe> element with its id/title/src and stop.
- Attribution when the page is continuously mutating AND the effect lands in the SAME subtree as the churn — a chat pane where the click posts into the feed that is already ticking. The pre-dispatch control read separates disjoint subtrees only. Overlapping -> unknown, delta listed and explicitly not credited.
- Causation for a navigation or a new tab. Target.targetCreated with our openerId proves a tab was opened by this page, not by our click. A page-scripted location.reload() firing during our action window is byte-identical to one we caused. We report caused_by:'unknown' unless Page.frameRequestedNavigation.reason says otherwise.
- Server-side 30x redirect chains cannot be reproduced from file://. Without the Network domain we can only say the final URL differs from the requested one; hop detail needs Network.requestWillBeSent.redirectResponse, which is off the stealth hot path.
- Cross-process backendNodeId collision after a cross-origin navigation — the reason goto clears the uid map. Not reproducible offline (file:// stays in one renderer where ids are monotonic). Consequence encoded in the design: uid absence is never the staleness test; stale is decided by isConnected under a loaderId-equality precondition.
- Native OS surfaces render outside the page's hit-testing area: an open <select> popup, a date picker, the autofill menu, the file picker, permission and print dialogs. elementFromPoint is clean and the click is eaten -> no_effect with an explicit 'a native popup may be open' note, never changed.
- The --selector path can succeed where a human could not. el.click() performs no hit test, fires no pointerdown/mousedown, carries isTrusted:false and grants no transient user activation (verified: a target=_blank link is popup-blocked by selector and opens by uid; a checkbox under a full-viewport veil ends up checked). intercepted is not undetected there, it is inapplicable. We emit the effect-based verdict + dispatch:'js' + obscured_by, and the same advisory is emitted for every JS-dispatching command (fill/select/upload/check --selector), not only click.
- --xy has no intended element. Only 'received by X' can be reported; no interception verdict is possible.
- App-level guards are indistinguishable from working controls before the action: aria-disabled plus an early return, a .is-loading class, a pending-request flag, a disabled-looking div with no listener. Floor: no_effect, never blocked. Symmetrically, blocked is worded as an observation at action time — we cannot say whether a disabled Submit becomes operable once validation settles.
- Whether a value the DOM accepted survives the framework's state layer, the form's submit interception, or server-side validation. A React input whose DOM value shows our text but whose state never updated is byte-identical to a working one. changed describes the DOM only.
- Late reverts beyond the observation window. Blur-time normalisers are caught (we blur before the post-probe); a 5s debounce is not. Every value verdict is a timestamped observation and must never be phrased as persistence.
- Reorder detection is limited to uid order and indent depth within one snapshot projection. A re-parent that preserves both is invisible, and a node moved between two subtrees at the same depth and index is not detected.
- Non-DOM accessibility nodes (Chrome's validation bubble and any other browser-generated node) get positional e* uids that are not in uid_map and are not stable across snapshots. We exclude them from counts and summarise by role only — usable as evidence that a node of role X appeared, never as an identity.
- unknown collapses two operationally different situations: 'we could not decide' and 'the effect landed in a scope we could not read'. The reason code and hint carry the difference (subframe_navigated tells the agent to bind the frame; hit_test_disagreed tells it to retry). An agent that reads only the verdict word loses that. Deliberate tradeoff for a smaller verdict set; revisit if agents are observed treating both as failure.
- The pre-dispatch a11y baseline doubles the snapshot cost for click/dblclick/hover/press/drag. --verdict fast skips it, and those five commands then degrade to unknown whenever the stored baseline was not taken by this command. There is no configuration in which we both skip the baseline and keep the confident verdict.
- resolve_page_target (run_helpers.rs:294) claims the first UNCLAIMED page target for a page name it has never seen. A later `--page report` command can therefore silently attach to a popup that an earlier click opened. We report opened_tab as evidence but do not reserve the target.
- Focus destination when it leaves the document. Not reproducible headless (Tab from the last focusable element wraps to the first because there is no browser chrome). Under --headed on a real profile focus can move to the address bar. When no node gains focused and activeElement is BODY we report focus:{from,to:null,note:'focus left the document'} rather than inventing a destination. No fixture: a headless one would encode behaviour that does not reproduce headed.
- Divergence between document.elementFromPoint and the browser's real input hit test — top-layer content, compositor-side scroll offsets, in-flight transform animations. Every design-record fixture was cross-checked by running the real coordinate click and observing the handler, and the two agreed 12/12; that is evidence, not proof of equivalence. If DOM.getNodeForLocation is later adopted as a refinement, its frame-piercing and top-layer behaviour must be verified first — this CLI exposes no raw-CDP passthrough, so that verification has not been done.

## Implementation order

0. Slice 0 — DIFF HYGIENE, pure Rust, no behaviour change to any verdict (nothing consumes it yet). In src/commands/diff.rs: subtract `~` lines whose token symmetric difference is exactly {focused} on ANY node; exclude e* uids from the counts and summarise them under `anonymous`; stop trim_start()ing indentation in uid_lines and compare uid order + depth to emit `moved`. Ships behind the existing counts, fully unit-tested with no Chrome, reverts in one commit. Fixes the two structural blindnesses (focus artifact, reorder) that poison every later rung.

1. Slice 1 — DOCUMENT IDENTITY BY loaderId. Add {frame_id, loader_id} to session::PageSession next to last_snapshot_url; read them with Page.getFrameTree at snapshot time and at action time; make diff::compare take a tri-state identity (Same/Different/Unknown) instead of the URL string comparison at diff.rs:107-111, with Unknown refusing to claim a diff rather than defaulting to `false`. Fixes four verified false positives (fragment, pushState, hash router, frame switch) and two verified false negatives (form GET to self, same-URL reload) in one edit. No new verdicts yet — document_changed simply becomes correct. Independent of Slice 0.

2. Slice 2 — SINGLE-HANDLE SELECTOR RESOLUTION, pure refactor. Change click_selector / fill_selector / focus_selector / select_option_selector / set_checked_selector (element_selector.rs, element.rs:561/632) to resolve one objectId via Runtime.evaluate with returnByValue:false and then act through Runtime.callFunctionOn on that handle, returning querySelectorAll(sel).length alongside. Same call count. Closes the hole where pre-probe, action and post-probe each bind a different node after a re-render, and unlocks every probe in Slice 3 for the selector path. No verdict change; existing tests must pass unmodified.

3. Slice 3 — THE PROBE AND THE POSTCONDITION VERDICTS. Add the one-shot R1 probe, the Verdict enum, and rungs R1/R2/R3/R6 for fill, type, select, check, uncheck and upload. Delivers already_satisfied, blocked, normalised, changed and stale for every value-bearing command with zero dependence on the a11y diff. No hit test, no delta changes, no click verdicts. This is where most of the value lands and it is independently revertable because it only adds keys to the response.

4. Slice 4 — ATTRIBUTED BASELINE. Take the pre-dispatch snapshot inside the acting command, AFTER scrollIntoViewIfNeeded and before the first input event; store the projection parameters (verbose/max_depth/filter) with every snapshot and refuse to compare across differing projections; route everything the gate rejects under `unattributed` with a cause. Gates R8. Also fixes the verified case where an earlier `eval` or `extract --scroll` lands its changes in the next action's delta. Ships behind --verdict fast for callers who want the old latency.

5. Slice 5 — HIT TEST AND intercepted. R4 bound to the target's objectId: shadow descent, label retarget via closest('label').control, shadow-host containment, aimIn against getClientRects, out-of-viewport settle loop with a hard refusal, and the border-box-offset recovery probe requiring hit===target. Record `dispatch` (mouse|js|insert_text|key) on every action and forbid intercepted on non-mouse paths. Includes the hover and drag fix: both currently use resolve_uid's PRE-scroll centre and never call scrollIntoViewIfNeeded (element.rs:299, 766), so their coordinates are document-space on any scrolled page.

6. Slice 6 — CLI/PIPE PARITY. Route CLI Type / Press / Scroll / Hover (run.rs:647/664/674/736) through output_action so they carry the same payload shape as pipe, which already lists them in mutates_page. Also fix press_key to send `text` for single printable characters or reject unmapped keys loudly instead of returning ok:true (element.rs:250 maps only Enter/Tab/Escape/Backspace/Delete/arrows/Space). Pure plumbing, independently revertable, and a prerequisite for asserting any verdict on those four commands.

7. Slice 7 — SCROLL AND KEYBOARD RUNGS. R7's before/after scrollingElement measurement with the overflow/modal discrimination and the smooth-behaviour flag; the `scrolled:{from,to}` orthogonal field for non-scroll commands; the focus:{from,to} and title:{from,to} fields; and the isolated-world capture-phase `invalid` listener that turns press-Enter constraint refusal into blocked(constraint_validation) without firing invalid events into the page.

8. Slice 8 — FRAME SCOPE. Stamp every snapshot and every change report with (frameId, loaderId) and refuse to compare across frameIds; resolve the loaderId of the BOUND frame rather than the root; add the multi-frame delta fallback (getFullAXTree per same-process child) that runs only when the top-frame delta is empty; handle Page.frameNavigated with a parentId as a subframe event and key wait_for_stabilization off Page.frameStoppedLoading for that frameId, which also removes the measured 10.16s stall.

9. Slice 9 — REFINEMENTS, each independently revertable: the downgrade-only post-dispatch hit test; opened_tab via Target.getTargets openerId comparison, run only when the delta is empty; recording the dialog handler's (type, decision) on CdpClient so a dismissed beforeunload becomes blocked(page_refused_to_unload); blur-before-post-probe for form-value actions; and the error_hint branch for `TypeError: Illegal invocation` (run_helpers.rs:262) pointing at `type` after focus instead of 'check expression syntax'.


## Fixture plan

113 fixtures selected. Those already on disk are in `tests/fixtures/`; see its README.

- KEEP intercept_dead_scrim — click <uid> -> intercepted, receiver div#scrim. Canonical: invisible to the a11y tree, the diff AND a screenshot; only the hit test sees it.
- KEEP intercept_sticky_header — click <uid> below the fold -> intercepted. Uniquely pins that the hit test uses the SECOND (post-scroll) box-model read, not resolve_uid's.
- KEEP intercept_partial_overlay — click <uid> -> intercepted + partially_covered + a clear point expressed as a border-box OFFSET where hit===target (not a descendant).
- KEEP intercept_pointer_events_none_target — click <uid> -> blocked(pointer_events_none) + also_obscured_by:div#page. Pins target-property-beats-coordinate precedence.
- KEEP intercept_pseudo_element_overlay — click <uid> -> intercepted via generated content on div#card. Pins that an ANCESTOR hit is not automatically safe, against case 4 where it is explained.
- KEEP intercept_toast_dismiss_on_click — click <uid> -> intercepted. Pins that the hit test is taken at dispatch time; a post-hoc probe reports a clear point 150ms later.
- KEEP intercept_modal_backdrop — click a background uid while a showModal() dialog holds the top layer -> intercepted(modal_dialog) naming DIALOG#terms. NOT blocked: blocked teaches the agent to wait on the target instead of closing the modal.
- NEW intercept_label_span_forwards_click — click the <uid> of a visually-hidden checkbox whose visible box is a SIBLING <span> inside the label -> changed. Replaces intercept_label_forwards_click: `hit instanceof HTMLLabelElement` misses the real-world shape; `hit.closest('label').control===target` catches it.
- NEW intercept_inert_drawer — click a uid under an element with `inert` set (no <dialog>) -> blocked(inert) naming the [inert] ancestor. Verified naive answers: intercepted-by-BODY, or a uid-not-found whose advice is wrong.
- NEW intercept_shadow_host_open — click the <uid> of a button inside an OPEN shadow root -> changed. elementFromPoint returns the host and Node.contains does not cross the boundary, so both escapes fail at once and naive says intercepted on every design-system button.
- NEW intercept_iframe_contains_target — click the <uid> of a button INSIDE a srcdoc iframe -> the effect-based verdict, no interception claim. Paired half of intercept_ad_iframe; without it the tagName==='IFRAME' rule is a baked-in false positive.
- KEEP intercept_ad_iframe — click <uid> under an ad iframe -> intercepted(cross_document_overlay). Only valid alongside the pair above.
- NEW intercept_wrapped_inline_link — click the <uid> of an inline <a> wrapping across two line boxes -> re-aim succeeds -> changed; if it cannot, unknown(aim_point_off_target). Never intercepted-by-the-paragraph.
- NEW intercept_smooth_scroll_below_fold — html{scroll-behavior:smooth}, target below the fold, click <uid> -> changed after the settle loop. Fails today: dispatch fired at cy=3028 with innerHeight 469.
- NEW intercept_disabled_under_banner — click a <button disabled> also covered by a banner -> blocked(dom_disabled) + also_obscured_by. The only fixture where both families' preconditions hold.
- NEW intercept_js_click_fallback — click the <uid> of a display:none element (no box model) -> dispatch:"js", no hit-test claim, advisory that the click was synthetic. Pins that absence of evidence is encoded as absence.
- KEEP verdict_inert_no_listener — click an inert button by uid AND by --selector -> no_effect in BOTH, identical verdicts. Fails today by uid: the focus token alone makes changed:1.
- KEEP verdict_handler_throws — click --selector -> no_effect, exceptions under page_errors with NO causal wording and no file:line attribution.
- KEEP verdict_slow_600ms — click --selector -> no_effect carrying observed_after_ms; assertion is on the WORDING, that it is never phrased as a claim about the future.
- KEEP verdict_late_timer_misattribution — arm a 2000ms timer, re-baseline, click an inert button inside its window -> no_effect + the delta under unattributed.
- KEEP verdict_visual_only_class — click --selector #recolour -> changed(evidence:target_attribute) quoting the class change; control #hide -> plain changed.
- KEEP verdict_canvas_only — click the canvas itself -> unknown(canvas_target) + screenshot hint; plus the negative half: an unrelated <canvas> elsewhere must NOT change any other verdict on the page.
- KEEP verdict_ambient_ticker — extend the existing tests/fixtures/goto_ticker.html with an inert button; click it -> unknown(unattributed_churn), delta under unattributed. Never changed.
- NEW verdict_scroll_into_view_lazy_load — click a listener-free button below the fold whose scrollIntoViewIfNeeded trips an IntersectionObserver -> no_effect + rows under unattributed{cause:scrolled_into_view}. Defeats all three naive probes at once.
- NEW verdict_stale_baseline_eval — goto, inspect, eval that injects a heading, click an inert button -> no_effect + heading under unattributed. No timers; the cheapest attribution test in the corpus.
- NEW verdict_reorder_no_text_change — click a button that reorders three rows via appendChild -> changed + moved:[uids]. Today: 'No changes detected.'
- NEW verdict_opens_new_tab — <a target=_blank> by uid -> changed(opened_tab); by --selector -> no_effect + a hint naming the missing user activation.
- NEW form_value_fieldset_disabled — fill --selector inside <fieldset disabled> -> blocked(dom_disabled). Replaces form_value_disabled_input: `this.disabled` is false here and `:disabled` is true.
- KEEP form_value_readonly_input — fill -> changed + caveat readonly. Naive (reading the snapshot's readonly token) says blocked.
- KEEP form_value_number_rejects_letters — fill 'abc' -> no_effect (readback ''). Naive says changed from the focus delta.
- KEEP form_value_controlled_revert — fill -> no_effect. Requires the pre-value read; without it indistinguishable from already_satisfied.
- KEEP form_value_already_correct — fill -> already_satisfied. The verdict that does not exist today.
- NEW form_value_cents_mask — fill '1000' -> normalised, both strings verbatim. Replaces form_value_trim_normalises and form_value_phone_mask: the digits-only 'content preserved' sub-label reports 1000->10.00 as lossless.
- KEEP form_value_maxlength_divergence — fill -> changed + caveat naming cap and length; type -> normalised(truncated). Same element, two verdicts: proves the verdict is a function of (element, command).
- KEEP form_value_contenteditable — fill -> blocked(wrong_element_type) pre-dispatch; type -> changed graded by length delta, not bare containment. Also pins the Illegal invocation error_hint branch (run_helpers.rs:262).
- KEEP form_value_reset_on_submit — fill then click submit -> click reports changed AND names the field it cleared. Only fixture asserting cross-field re-probing.
- NEW form_value_checkbox_value_setter — fill 'true' on <input type=checkbox> -> blocked(wrong_element_type). The read-back, the family's own trusted signal, reports an exact match here.
- NEW form_value_navigating_change_handler — fill whose change handler navigates -> navigated. Pins R5 before R6.
- NEW form_value_range_sanitizer — fill 'abc' on type=range -> no_effect(sanitizer_substituted) + a warning that the field moved 30->50 collaterally. Naive says normalised.
- NEW form_value_out_of_range — fill '999' on min=1 max=10 -> changed + caveat carrying validity.valid=false and the browser's own validationMessage.
- NEW form_value_blur_normaliser — fill '  ab cd  ' into a field rewritten on blur -> normalised. Converts a case the design records filed as undetectable into a caught one.
- NEW form_value_duplicate_selector — fill a selector matching a hidden mobile copy first -> changed + a loud caveat naming matches:2 and rects:0 on the one we filled.
- NEW form_value_type_focus_escapes — fill field A, then type into a DISABLED field B -> the command must assert activeElement after focus and error; the fixture asserts the 6 characters never reach field A.
- KEEP verdict_select_disabled_option (optgroup half only) — select -> blocked(option_unreachable), no mutation dispatched.
- NEW verdict_select_hidden_option — select an option[hidden] (`:disabled` false) -> blocked(option_unreachable).
- NEW verdict_select_multiple — select on <select multiple> with 3 already selected -> blocked(multi_select_unsupported). Highest-severity silent data loss in the corpus.
- KEEP verdict_select_value_text_mismatch — select 'Canada' where it is one option's value and another's text -> unknown(ambiguous) with both resolutions. Never already_satisfied.
- NEW verdict_select_option_label_attr — <option value=y label='Visible Y'>raw text y</option>: select by the name inspect prints must work, and selected_text must report the label.
- KEEP verdict_select_reverting_handler (#sync and #later only) — #sync -> normalised decided inside the same call; #later -> changed carrying observed_after_ms. #soon dropped: it pins a timing constant.
- KEEP verdict_select_hash_navigation — select whose handler sets location.hash -> changed, uids alive. Today: document_changed:true + 'your uids are dead' while listing live uids.
- NEW verdict_select_dependent — country->state cascade -> changed + side_effects naming #state's value move.
- KEEP verdict_check_already_checked — check -> already_satisfied, mutation_dispatched:false.
- NEW verdict_check_aria_already_on — check a <div role=checkbox aria-checked=true> -> already_satisfied, no dispatch. Today it clicks and UNCHECKS it while reporting success.
- NEW verdict_check_indeterminate — uncheck an indeterminate checkbox -> changed. Today: 'Already unchecked', nothing dispatched, mixed state destroyed by the sibling check command.
- KEEP verdict_check_reverting_handler — check -> no_effect + hit_test:'target' so the agent does not hunt an overlay.
- KEEP verdict_check_overlay_intercepted — check <uid> -> intercepted naming #cookie-veil; check --selector -> changed + dispatch:'js' + obscured_by. Two honest verdicts, one page.
- KEEP verdict_check_radio_group — uncheck a CHECKED radio -> blocked(radio_not_uncheckable); uncheck an ALREADY-UNCHECKED radio -> already_satisfied. The pair pins R2-before-R3.
- KEEP verdict_check_text_input (#qty only) — check a text input -> blocked(wrong_element_type).
- NEW nav_reload_same_url — click a button calling location.reload() -> navigated, uid map dropped. Replaces nav_cross_document_link: today this is +N/-N with document_changed:false across two documents.
- KEEP nav_form_get_self — submit a GET form to itself -> navigated. URL identical, loaderId changed.
- KEEP nav_fragment_only — click an anchor -> changed; click it AGAIN -> no_effect. The second click is what forces the :target read to be before-and-after, not post-hoc.
- KEEP nav_fragment_dead_target — click an anchor with no matching element -> no_effect, decided from the empty DOM delta with the fragment reading as a field.
- KEEP nav_hash_router — click a hash-route link -> changed, per-scope uid report. Paired with the fixture above: `:target === null` in BOTH, so fragment evidence can never be a verdict source.
- KEEP nav_pushstate_append — click -> changed added:1. Today: document_changed:true and added:0, hiding the paragraph that appeared.
- KEEP nav_soft_rerender_identical — click #refresh -> changed + rebuilt:true, naming that uids held before this call are dead. Not stale: the map handed back is already fresh.
- KEEP nav_ghost_click_after_rerender (step 3 only) — click a detached uid -> stale, NOT DISPATCHED, and no ok:true.
- KEEP nav_meta_refresh — goto -> pending_navigation{in_ms,to} from the meta tag read + a note that history is REPLACED so `back` will not return.
- KEEP nav_delayed_redirect (next-command assertion only) — the click step asserts nothing; the following command asserts stale for a uid action and uid_map_invalidated for a selector action.
- KEEP nav_new_tab_link — uid -> changed(opened_tab); --selector -> no_effect + a hint naming the popup block.
- NEW nav_broken_link — click a link to a missing file:// path -> navigated(navigation_failed) + the error code, worded so it does not read as arrival. Also the only offline demo of cross-document uid collision.
- NEW nav_iframe_self_navigate — bind #checkout, then a click inside it navigates the iframe -> navigated scoped to the bound frame. Today: root loaderId unchanged, document_url returns '', diff across two documents reported as same-document movement.
- NEW nav_beforeunload_dismissed — beforeunload under --dialog dismiss -> blocked(page_refused_to_unload). The evidence exists inside the process and is currently written only to stderr.
- NEW nav_modal_aria_hidden — open a dialog that aria-hidden's the shell, then close it -> hidden:[uids], never stale; closing resurrects the same ids.
- NEW nav_filtered_baseline — inspect --filter button then click -> the post-read uses the baseline's projection, or unknown(baseline_projection_mismatch). Never stale from a 0.0 uid overlap.
- KEEP shadow_closed_root_button — click <uid> inside a CLOSED shadow root -> changed. Pins that the hit-test probe is bound to the target's objectId (this.getRootNode() returns the closed root only from a handle to the inner node).
- KEEP iframe_effect_lands_inside — click a top-frame button whose effect lands in #cart -> changed(scope:multi_frame) via the child-frame delta fallback. Never no_effect.
- KEEP iframe_click_xy_lands_inside — click --xy into a frame -> changed(scope:multi_frame) + received_by_frame. No interception verdict is possible for --xy.
- KEEP iframe_nested_srcdoc — every change report carries the (frameId, loaderId) it was computed for, and two reports with different frameIds are never compared.
- KEEP iframe_frame_switch_false_navigation — frame #side, inspect, frame main, click -> changed. Today document_changed:true, which a verdict layer renders as navigated.
- KEEP iframe_cross_origin_data — click a link that navigates a data: subframe -> unknown(subframe_navigated), top uids alive. Also asserts the command returns in well under 10s (today: 10.16s).
- KEEP scroll_bottom_terminates (2 steps) — one working scroll -> changed naming the scroller; at max -> no_effect with a hint that names the window and does NOT say 'stop scrolling'.
- NEW scroll_bottom_but_more_coming — at max, a fixed 400ms append -> the hint must contain no terminate instruction, and a second scroll after `wait text` returns changed. This is what makes the terminate hint safe to ship.
- KEEP scroll_nested_pane_only — scroll down on an overflow:hidden document -> blocked(no_document_scroller) naming region 'Feed'; scroll <uid> -> changed.
- NEW scroll_short_page — content fits the viewport, overflow not hidden -> no_effect(fits_viewport). Paired with the fixture above; the pair forces the overflow read.
- NEW scroll_locked_by_modal — body{overflow:hidden} with a dialog open -> blocked(scroll_locked) naming the modal.
- KEEP scroll_wrong_scroller — scroll down -> changed naming #document + other_scrollers as a capped, truncation-flagged advisory, never a verdict driver.
- KEEP scroll_lazy_settles_late — scroll -> changed + settled:false. Never no_effect.
- KEEP scroll_virtualized_rows — scroll -> changed AND stale, where stale comes from per-node isConnected on the removed subset.
- NEW scroll_virtualized_recycled_rows — rows RECYCLED (same uids, new text) -> changed + relabelled:[{uid,was,now}]. The silent-wrong-row case that set intersection cannot see.
- NEW scroll_smooth_behavior — scroll down -> changed + settling:true, no terminate hint.
- NEW scroll_uid_already_in_view — scroll <uid> on an element already centred -> changed + already_in_view:true. Never no_effect: the postcondition is a position.
- KEEP press_tab_focus_churn — press Tab -> no_effect for content + a focus:{from,to} line; and a RootWebArea document.title change on the same page must SURVIVE the filter and be reported as title:{from,to}.
- KEEP press_enter_submits (count assertion only) — surviving changed===1 after artifact filtering, and the RootWebArea line routed to its own key rather than merely absent.
- KEEP press_enter_blocked_by_required — press Enter -> blocked(constraint_validation) naming the field, driven by an isolated-world capture-phase `invalid` listener installed before the action.
- NEW press_enter_preventdefault_with_invalid_sibling — a page handler preventDefaults and renders results while an untouched required field is invalid -> changed. Paired with the fixture above; the pair forces the causal signal over the state read.
- KEEP press_escape_closes_modal — Escape #1 -> changed summarised as 'modal closed', with the dialog nodes reported under hidden not stale; Escape #2 -> no_effect.
- NEW press_escape_div_overlay_ignores — Escape against a div overlay with no keydown handler -> no_effect that does NOT assert 'nothing left to dismiss' (`:modal` returns 0 while a full-screen overlay blocks every click).
- KEEP hover_opacity_only — hover twice -> byte-identical verdicts (no_effect + caveat) both times. The pass criterion is the equality, not the undetectability.
- KEEP hover_intercepted_by_overlay (on a SCROLLED page) — hover <uid> -> intercepted naming div#veil. Fails today for a second reason: hover uses resolve_uid's pre-scroll centre and never calls scrollIntoViewIfNeeded (element.rs:299-324).
- KEEP blocked_button_disabled (double-submit half) — first click on 'Submit order' -> changed; second -> blocked(dom_disabled). Pins that the disabled read is a PRE-dispatch read; a post-read inverts every self-disabling submit button on the web.
- KEEP blocked_aria_disabled_only (#aria-live-btn, #aria-guarded-btn) — changed and no_effect respectively. Neither may be blocked: the AX `disabled` property is set for aria-disabled too.
- KEEP blocked_fieldset_disabled (#fs-btn, #inner-btn, #legend-btn) — blocked, blocked, changed. Discriminates `:disabled` from both el.disabled and closest('[disabled]').
- KEEP blocked_readonly_input (part B, autofocused from the page) — type into a readonly input -> blocked(readonly_text_entry). The focus must come from the fixture, not a separate eval command, or the baseline is poisoned.
- KEEP blocked_upload_targets (#silent-file, #text-field) — changed from a files.length read with an empty delta; blocked(wrong_element_type).
- KEEP blocked_check_non_checkbox (#plain-div, #aria-cb-on) — blocked(no_checkable_state); uncheck the ARIA box -> changed, and it must not say 'Already unchecked'.
- KEEP blocked_select_not_a_select (#disabled-select, 'Atlantis') — blocked(dom_disabled); and a non-matching value is invalid_argument, NOT blocked, so the agent retries the value instead of abandoning the element.
- KEEP blocked_detached_target — click a uid detached by a re-render -> stale, not dispatched. The handler demonstrably runs via js_click today.
- KEEP blocked_zero_size_hits_neighbour ('Collapse all' only) — intercepted by #backdrop-card, not blocked and not changed.
- KEEP blocked_offscreen_scrolls_into_view ('Off-canvas action' only) — no_effect with the dispatched coordinate in the message. We cannot honestly call an enabled, laid-out element blocked.
- KEEP blocked_inert_subtree (B and C) — click --xy -> blocked(inert); click --selector -> changed + dispatch:'js'. Same element, opposite honest verdicts, keyed on the dispatch mechanism.
- NEW blocked_submit_invalid_form — click submit with an invalid required field -> blocked(constraint_validation). Naive says changed because Chrome's validation bubble enters the tree as an added alert node. Highest-frequency real-world case in the corpus.
- NEW blocked_label_control_disabled — click --selector a <label> whose control is disabled -> blocked(dom_disabled, via label) after retargeting through el.control. Naive says changed: the label's own handler ran.
- NEW blocked_first_match_ambiguous — click --selector '.submit' where the first match is a disabled duplicate -> blocked + matches:2 + judged:'first' + a disambiguation hint.
