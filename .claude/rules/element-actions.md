---
paths:
  - "src/element.rs"
  - "src/element_pointer.rs"
  - "src/element_controls.rs"
  - "src/read_back.rs"
  - "src/commands/click.rs"
  - "src/commands/dblclick.rs"
  - "src/commands/fill.rs"
  - "src/element_selector.rs"
  - "tests/observation_window_tests.rs"
  - "tests/selector_uid_tests.rs"
  - "tests/bulk_fill_tests.rs"
  - "tests/secret_field_tests.rs"
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

- Secret fields report `{redacted:true, requested_length, actual_length, verbatim}` instead: the response reaches stdout, the agent transcript and any `_record` file. What counts as one is below.
- Both strings are returned rather than reduced to one word. Masks reformat, controlled components rewrite, number inputs discard — a currency mask turns `1000` into `10.00`, which a digits-only comparison would call preserved.
- Refuses on `:disabled` (this catches an ancestor `<fieldset disabled>`, which `el.disabled` does not) and on `readOnly`.
- `caveat` when the write exceeded `maxlength`: that constrains the editing pipeline, not the value setter, so the field holds something the form will reject.

## What counts as a secret

`read_back::SECRET_FIELD` (re-exported as `element::SECRET_FIELD`), one JS expression over `el`,
inlined by `fill` (uid and selector), `type`, `select`, `assert value`, `snapshot_secret` and
`values_lost` — so the six cannot disagree about the same element.

It was `type === 'password'` plus three `autocomplete` tokens, which is a STRUCTURAL claim, and
three families of secret never make it: a "show password" toggle sets `type = 'text'` (that IS
the toggle), an OTP widget ships `type=text inputmode=numeric autocomplete="off"`, and an IBAN,
an account number or a national ID name nothing in any autocomplete list. All three reached
stdout, the transcript and any `--record` file in clear.

Four rules now, and the precision argument for each:

| Rule | Catches | Why it does not over-match |
|---|---|---|
| `type === 'password'` | the ordinary case | no false positives; the only one Chrome also masks in the a11y tree |
| whole-token `autocomplete` match, widened to the full credential/payment list (`cc-exp`, `cc-exp-month`/`-year`, `cc-name` and the holder-name tokens were missing — a card's expiry and holder are part of the same card) | a declared field | the page declared the purpose itself, so a false positive needs the page to be wrong about its own form |
| `inputMode === 'numeric'` ∧ `maxLength` 4–8 ∧ `autocomplete` empty or `off` | the OTP/PIN widget that declares nothing | below 4 is a CVC (rule 2) or a quantity spinner; above 8 is a phone or account number rule 4 catches; a field that DID declare a purpose (`postal-code`) is judged by rule 2 |
| word match on `name`/`id`/`aria-label`, normalised (camelCase split, then every non-alphanumeric a separator) | the toggle, the IBAN, the national ID | WORDS, not substrings: `pin` inside `pinterestUrl` does not match, which a bare `test()` would |

Fails towards redaction: a value wrongly withheld costs a read-back (`verbatim` and the two
lengths still classify it), a value wrongly printed cannot be taken back. The residual false
positive measured is an undeclared 5-digit numeric ZIP.

Rule 4 is a keyword list, so it is incomplete by construction. The escape hatch is
`asserted_secret: bool` on `element::fill_with` / `fill_selector_with` / `type_text_with` — the
caller stating what the DOM does not. It only ever ADDS redaction: there is deliberately no
override that turns redaction OFF, since that would be a way to print a password.

The caller reaches it as `fill --secret` / `type --secret`, and `{"cmd":"fill","secret":true}` /
`{"cmd":"type","secret":true}`. `select` does NOT take it: it reads secrecy off the element and
has no plumbing to honour an asserted one, so `SelectArgs` would be accepting a key that does
nothing — which is why `fill` has its own `FillArgs` rather than a field on the `ValueArgs` the
two used to share.

Both directions are pinned by `tests/secret_field_tests.rs` over
`tests/fixtures/secret_field_shapes.html`, including the four fields that must NOT be redacted.

## `type`

`element::type_text_with` reads the FOCUSED element through the same predicate, before the
insert (afterwards `document.activeElement` may be somewhere else), and returns the message the
response should carry: `Typed N chars`, or `Typed into a secret field (length withheld)`. Unlike
`fill` there is no `verbatim` to classify, so the length buys the caller nothing and only narrows
the value.

That message is what the response carries. Both front ends used to discard it and rebuild
`Typed {text.len()} chars` at the call site, which made the redaction inert — and counted BYTES
where this module counts characters. `pipe_dispatch::dispatch_type` is now the one caller, for
CLI, pipe and batch alike; it appends the `into selector '…'` clause and nothing else.

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

- **`press`** — a single printable character is sent with `text` so it actually types. Unmapped names are refused rather than dispatched with virtual key code 0. It has no secrecy notion: `press Enter` carries no value, and `press <char>` one character.
- **`dblclick`** — 4 mouse events (pressed/released twice, `click_count` 1 then 2), with a JS `dblclick` MouseEvent fallback. `--selector` resolves the viewport-centre coords then runs the native CDP double-click; it is a real double-click, not a single click. It aims through `element::aim_and_dispatch` exactly as `click` does, parameterised by `element::PointerVerb` — the rule and its refusals are written once (`.claude/rules/hit-test.md`).
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
