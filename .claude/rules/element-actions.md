---
paths:
  - "src/element.rs"
  - "src/element_controls.rs"
  - "src/read_back.rs"
  - "src/commands/click.rs"
  - "src/commands/dblclick.rs"
  - "src/commands/select.rs"
  - "src/commands/check.rs"
  - "src/commands/upload.rs"
  - "src/commands/drag.rs"
  - "tests/observation_window_tests.rs"
  - "tests/selector_uid_tests.rs"
  - "tests/bulk_fill_tests.rs"
  - "tests/checkable_tests.rs"
  - "tests/select_readback_tests.rs"
  - "tests/press_key_tests.rs"
---

# Acting on an element, and reading back what the page kept

## One observation window

`element::READ_BACK_MS`, 60 ms. Every read-back waits the same window and reports
`observed_after_ms` beside the value.

The four paths used to disagree: `fill` read synchronously (0 ms), so a microtask revert was
reported as kept — `verbatim:true` on an already-emptied field
(`tests/fixtures/form_value_microtask_revert.html`); `check --selector` waited 60 ms;
`check <uid>` waited whatever a CDP round trip cost; `select` returned the option text from the
same synchronous script that set `selectedIndex`, so a controlled component that snapped the
selection back was still reported as selected.

60 ms catches a revert on the microtask queue, in `setTimeout(0)` or in an animation frame. It
does NOT catch a validator firing at 400 ms (`form_value_late_revert.html`). No fixed window
could, which is why the window is reported rather than persistence asserted.

`check`'s `observed_after_ms` is absent when the element already held the state: nothing was
dispatched, so there was no post-action moment (`CheckOutcome`).

All four report WHAT they read, not just when: `value:{requested,actual,verbatim}` on the
response, which carries the reading to the classifier.

## `fill`

`value: {requested, actual, verbatim, observed_after_ms, caveat?}`.

- Secret fields (`type=password`, or `autocomplete` naming a password/card/CVC/one-time code) report `{redacted:true, requested_length, actual_length, verbatim}` instead: the response reaches stdout, the agent transcript and any `_record` file.
- Both strings are returned rather than reduced to one word. Masks reformat, controlled components rewrite, number inputs discard — a currency mask turns `1000` into `10.00`, which a digits-only comparison would call preserved.
- Refuses on `:disabled` (this catches an ancestor `<fieldset disabled>`, which `el.disabled` does not) and on `readOnly`.
- `caveat` when the write exceeded `maxlength`: that constrains the editing pipeline, not the value setter, so the field holds something the form will reject.

## Bulk fill

`run_helpers::bulk_fill_report`. `fill-form` and `fill_and_submit` used to drop every
`FillOutcome` and answer "Filled 3 fields" — right about the count, silent about the mask that
reformatted one. Both return `values: [{uid|selector, value:{...}}]`, redacted as a single fill
is. For `fill_and_submit` this is the *only* witness: the change report runs after the submit, by
which time the form has moved on.

## `check` / `uncheck`

Classify before acting:

| Element | Read through |
|---|---|
| native `<input type=checkbox\|radio>` | `.checked`, with `indeterminate` as mixed |
| `aria-checked` or a checkable role | that attribute |
| anything else | refused |

`!!el.checked` was wrong in both directions: absent on a `<div role=checkbox>` (so a checked box
was clicked OFF while reporting success) and present-but-meaningless on any other input.

Idempotent — queries `this.checked` via `callFunctionOn` and clicks only if the state differs.
Unchecking a radio is refused. The state is read back after the click.

## `select`

Matches by `option.value` first, then by `option.text.trim()`. Dispatches `change`, waits
`READ_BACK_MS`, re-reads and reports `observed_after_ms`.

A selection the page reverted inside the window is refused (exit 1, naming the option actually
held) rather than reported as made: an agent that submits a form believing a different option is
chosen cannot recover from that answer.

Both the uid and the selector path run one in-page script that sets, dispatches, waits and
re-reads, bound to the same node throughout.

## Other verbs

- **`press`** — a single printable character is sent with `text` so it actually types. Unmapped names are refused rather than dispatched with virtual key code 0.
- **`dblclick`** — 4 mouse events (pressed/released twice, `click_count` 1 then 2), with a JS `dblclick` MouseEvent fallback. `--selector` resolves the viewport-centre coords then runs the native CDP double-click (`dblclick_selector`); it is a real double-click, not a single `click_selector`.
- **`upload`** — validates that file paths exist before the CDP call. Uses `DOM.setFileInputFiles` with `backendNodeId` (uid) or `nodeId` (selector).
- **`drag`** — 5-step linear interpolation between source and destination centres, 16 ms between moves.
- **JS click fallback** — when the a11y tree reports "disabled" but the DOM does not, `click` falls back to `.click()`.

## A targeted action names the node it hit

`run_helpers::target_details` → `hit_test::resolve_selector` for fill/select/check/uncheck/upload;
`hit_test::resolve_selector` directly for click/dblclick, which read the uid off the single handle
the probe already resolved.

Every click/dblclick/fill/select/check/uncheck/upload response carries `uid`, whether the caller
aimed by selector or by uid. Without it the message quoted the selector back while the change
report named uids, so an agent could not check that the delta describes the node it aimed at, and
a selector matching several elements gave no clue which one was used.

Resolved BEFORE the action (`Runtime.evaluate` → `DOM.describeNode` → `n{backendNodeId}`):
afterwards the element may be detached and the answer would describe a different page. Costs one
CDP round trip on the selector path.

## The settle wait that cost ten seconds

`element::wait_for_stabilization`, `element::main_frame_navigated`.

`wait_for_stabilization` probed 50 ms for `Page.frameNavigated` and, on seeing one, waited up to
10 s for `Page.loadEventFired`. But `frameNavigated` fires for EVERY frame and `loadEventFired`
only for the top one, so a subframe navigating armed a wait for an event that cannot come. On
shop.app that cost 10,051 ms for `click --xy` where `inspect` took 51 ms, with no field, no
message and no reason (`--timeout` never fires: 10 s is under the 30 s default).

The predicate is now the top frame (`frame.parentId` absent), and the 50 ms probe reads its whole
window instead of returning on the first event, so a click that spawns a tracker AND navigates
still waits for the navigation. Measured after: 10.12 s → 0.15 s, same page, same coordinate.
Fixtures: `click_spawns_subframe.html` for the shape, `click_navigates_away.html` for the control
that must still wait.
