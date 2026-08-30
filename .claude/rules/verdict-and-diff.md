---
paths:
  - "src/verdict*.rs"
  - "src/commands/diff.rs"
  - "src/pipe_report.rs"
  - "src/render.rs"
  - "tests/verdict_report_tests.rs"
  - "tests/action_report_tests.rs"
  - "tests/values_lost_tests.rs"
  - "tests/text_output_tests.rs"
  - "tests/diff_tests.rs"
  - "tests/focus_evidence_tests.rs"
  - "tests/read_back_verdict_tests.rs"
  - "tests/fill_verdict_tests.rs"
---

# What an action may claim about itself

## The change report

After a mutating command the page is re-read and the response carries
`changed{added,removed,changed,unchanged,moved,anonymous,document_changed,identity_known}` +
`delta`, plus `focus{from,to}` when focus moved.

- `goto` is excluded (`pipe_dispatch::mutates_page`): the caller navigated on purpose, and a truncated slice of the destination is neither a delta nor a usable snapshot. `--inspect` is for that.
- The baseline snapshot is always taken at full depth.
- Bounded by `--budget` (1200 chars, 0 = uncapped). `--verdict off` skips the read entirely and restores pre-0.8 output and latency. Costs one `getFullAXTree` per action.
- Wiring: CLI through `run_helpers::output_action`; pipe and batch through a central hook in `pipe_dispatch::dispatch_single` (`mutates_page` + `attach_change_report`, both in `pipe_report.rs`). Adding a command means adding it to `mutates_page` and nothing else.

## The vocabulary

`verdict` + `verdict_reason` ride on every mutating response in all three modes. Without them the
absence of `changed`/`delta` meant four different things: `--verdict off`, no baseline yet, a
failed post-action read, or a page that genuinely did not move.

| Verdict | Reasons |
|---|---|
| `changed` | `tree_delta`, `nodes_moved`, `focus_only`, `values_lost`, `value_kept` |
| `navigated` | `document_replaced` |
| `not_kept` | `value_reverted`, `value_rewritten` |
| `intercepted` | `hit_test_receiver`, `modal_dialog` |
| `no_effect` | `delivered_no_change` |
| `unchanged` | `identical_tree` |
| `unknown` | `no_baseline`, `read_failed`, `identity_unreadable`, `scroll_not_settled`, `aim_point_off_target` |
| `not_checked` | `reporting_disabled` |

Uncertain verdicts carry a `verdict_hint` naming the next action. The classifier is pure and
unit-tested; wiring is `pipe_report::attach_verdict_for` over `run_helpers::attach_verdict`.

`unchanged` claims the tree was identical while we watched. It deliberately does not claim "no
effect": an identical tree is also what an overlay swallowing the click, a canvas repaint and a
handler firing after the window look like, and "no effect" makes an agent retry a click that
already landed. `unchanged` is the floor for every path with no proof of delivery.

## The ladder: two groups

Written out once as an ordered table in `src/verdict.rs`'s module docs; `classify` follows it
literally.

- **Group A** — measured on THIS action's own target: the hit test at the coordinate about to be dispatched (`not_settled`, `off_target`, `intercepted`) and the read-back of the handle just written to (`value_reverted`, `value_rewritten`, `value_kept`).
- **Group B** — every claim that depends on comparing the stored tree with the live one, including `identity_unreadable` and `document_replaced`.

Group A precedes Group B because none of it needs the two trees to be comparable. This matters
because `goto` keeps `last_snapshot` while clearing `uid_map`, so the first action after
`inspect` → `goto` compares against the previous page. Before the split, `click_overlay.html`
answered `delivery:"intercepted"` with `verdict:"navigated"`, and `goto` → `fill` on
`form_value_microtask_revert.html` answered `navigated` with `verbatim:false` beside it.

`target_hit` is the one delivery reading that stays in Group B. It is not a verdict, it is the
licence for `no_effect`, which claims a comparable tree stayed quiet — promoting it would report
`no_effect` about a page that is no longer there.

## Postconditions outrank the delta

`verdict::Postcondition`, `pipe_report::postcondition_from_response`.

A delta is a whole-page observation the action itself can explain; a postcondition reads the ONE
handle the caller named, which the requested effect cannot explain. So a failed read-back
preempts `tree_delta`, the identity and navigation rungs, and `--verdict off`.

- `not_kept / value_reverted` — the element holds nothing. `not_kept / value_rewritten` — it holds something else (a mask, a trimmer, a controlled component). Two reasons, one verdict: one fact with two recoveries, and collapsing them would put "reverted" on every phone and currency mask on the web.
- Read off the response: `value.verbatim`, or every field of a bulk fill's `values`, judged on its worst. All three modes settle it in one place. A redacted secret keeps `verbatim` and its lengths, which is enough to classify without printing.
- `check`/`uncheck` and `select` cannot reach the failing variants — each already REFUSES when its own read-back disagrees, and the error is the report.

### Rung 11: a confirmed write is Group A evidence too

Secret-field redaction renders a value as a fixed marker (fixed on purpose: a length or hash
would make every snapshot of an unchanged secret read as a change). Re-filling a secret field
therefore produces no diffable value change, and the ladder used to fall through to
`changed / focus_only` on a response whose own `value` said
`{redacted:true, verbatim:true, actual_length:16}`.

`changed / value_kept` names the read-back as its evidence. Its rank:

- BELOW `values_lost` / `tree_delta` / `nodes_moved` — those say what changed and where.
- BELOW `identity_unreadable` / `document_replaced` — a confirmed value on a replaced page describes a field that is gone.
- ABOVE `reporting_disabled` / `read_failed` / `no_baseline` — those compared nothing at all.

The two outcomes of one read-back are deliberately asymmetric: a failure preempts everything, a
success preempts nothing that describes the page.

Boundary: filling an EMPTY secret field still reports `tree_delta`, because the marker appearing
where the tree showed no value is visible. Only a refill needs the rung.

**Three verbs feed it** (`src/read_back.rs`). `select` and `check`/`uncheck` already performed
the same measurement — set a state, dispatch, wait `READ_BACK_MS`, re-read, refuse on
disagreement — so both now write the same key in the same vocabulary:
`value:{requested,actual,verbatim}`. `select` reports the option text; `check` reports
`checked`/`unchecked`/`indeterminate`, the words its own message uses, not the probe's
`true`/`false`. One key, because `postcondition_from_response` reads exactly one.

- `verbatim` is the measurement, not the control flow: `select` compares by option INDEX (two options sharing a label are not each other); `check` compares strings against the state actually read.
- `observed_after_ms` stays at the TOP level rather than moving inside `value`. The window covers the whole action, and `render::observation_line` skips itself whenever a `value` exists — a kept state prints no `value:` line, so nesting it would have deleted "observed: 60 ms" from every successful check.
- Secrets are redacted on the same predicate a fill uses, in the report and in the message (`SelectOutcome::label`). Fixture: `tests/fixtures/select_secret_autocomplete.html`.
- NOT fed by a `check` whose element already held the state: nothing was dispatched, so there is no write of ours to have been kept. It keeps answering `no_baseline`/`identical_tree`.

**One `next` that is not a function of the reason alone.** When the post-action page read ALSO
failed, the verdict stays `changed / value_kept` (true about the field) while `next` answers
`inspect` rather than `proceed`, because "carry on" is a claim about a page this response never
saw. Written as a rule over the token (`proceed` + page unreadable → `inspect`, carried by
`PageSight` on the assessment, set by `classify` and never printed) so a rung added later cannot
inherit "carry on while blind". `retry` is untouched (it only comes from a rung that dispatched
nothing) and so is `--verdict off`. Fixture: `tests/fixtures/blocks_after_fill.html`, which pins
the main thread 200 ms after the write — after the 60 ms read-back.

`hint_for` lives in `verdict_words.rs` beside its short form;
`Delivery`/`Postcondition`/`Observation`/`PageSight` in `verdict_evidence.rs`.

## Focus is delivery evidence, except when it is the document

`diff::focus_to_document`, `pipe_report`.

`focus_only` is the only delivery evidence available on a path with no hit test (`--xy`, a JS
click, a target inside a frame), and its claim is that the action ARRIVED somewhere.

Chrome marks the `RootWebArea` node `focused` whenever the document (`<body>`) holds focus, which
is exactly what a click on something non-focusable leaves behind. Measured on
`tests/fixtures/focus_after_click.html`: `click --xy` on an inert paragraph answered
`changed / focus_only`, `next: proceed`, `focus: {from: null, to: "n1"}`, while the page's own
`document.activeElement` read `BODY`.

- `focus_moved` excludes the case where the ONLY event is the document GAINING focus; that click falls to `unchanged / identical_tree` (`next: confirm`).
- A blur still counts: if a real element LOST focus, something of ours reached the page. Only the destination is judged.
- `focus:{from,to}` and the `focus:` delta line are untouched — the browser reading was correct, only the classifier input was wrong.
- `focus.to` names what RECEIVED focus, routinely a focusable ANCESTOR of the node clicked (clicking a `<span>` inside an `<a href>` reports the link). An agent must read `uid` on the response for the element it aimed at, not `focus.to`.

## `values_lost`: an action says what it destroyed

`diff::LostValue`, `pipe_report::attach_values_lost`. Field:
`values_lost: [{uid, role, name, was}]`, verdict pair `changed / values_lost`.

Archetype `tests/fixtures/form_value_reset_on_submit.html`: `fill` reports `verbatim:true`, the
submit handler sets a status AND calls `form.reset()`, so ground truth is `{"email":"","status":"sent"}`
while the click answered `ok:true` and `changed / tree_delta`. Both true, neither saying the
field just filled was empty again.

- The verdict word stays `changed`. `not_kept` was rejected: a form that clears itself on a successful submit is correct and extremely common. `not_kept` is reserved for the write THIS action made; `values_lost` is about a value an EARLIER action wrote.
- The hint names the ambiguity the tool cannot resolve (submitted-and-cleared vs discarded-without-sending) and the two ways to settle it.
- Only value→nothing counts. A REPLACED value is a mask and the `~` line already has both sides; a node that vanished is a `removed`.
- Extraction is pure and unit-tested with no Chrome.
- `was` is redacted per `element::SECRET_FIELD` by asking the page, and it FAILS CLOSED: a field whose kind could not be read is redacted. A redacted entry carries no length either, because the only length available is the accessibility tree's, which for a `type=password` is the mask's.
- Capped at 10, with `values_lost_total` beside it.

## `no_effect` needs two facts

Delivery proven (`target_hit`, so never a JS `click()`, never `--xy`, never a target in a frame)
AND the tree quiet for a window we can name.

- `observed_after_ms` is mandatory, measured from dispatch (`CdpClient::mark_dispatch`) to the post-action read. Without it the verdict falls back to `unchanged`.
- Its hint names the three effects the measurement cannot see: a canvas/WebGL repaint, a CSS-only change, a handler that runs after the window.
- On a proven hit it outranks `focus_only`. Focus was only ever `changed` because it was the best available delivery evidence, and clicking anything focusable moves it — keeping `focus_only` first would make `no_effect` unreachable for every button.

## `next` is the branch, not the verdict word

`verdict_words::next_for`. One token from a closed set of six on every mutating response in all
three modes: `proceed` · `inspect` · `retry` · `confirm` · `dismiss` · `stop`.

| Verdict | `next` |
|---|---|
| `changed` | `proceed` |
| `navigated`, `unknown` | `inspect` |
| `unchanged`, `no_effect` | `confirm` |
| `intercepted` | `dismiss` |
| `not_kept` | `stop` |

Two reason-level exceptions: `values_lost` → `confirm` (submitted-and-cleared and
discarded-without-sending look identical), and `scroll_not_settled` → `retry`.

**`unknown` never maps to `retry`.** A blind repeat is a second real click. The rule existed only
as a sentence in the hints contract where nothing could check it; `next` makes it structural and
a unit test walks every rung. `scroll_not_settled` is the one exception because nothing was
dispatched, so there is no first action to duplicate.

The mapping is pure, and a test parses the table in `llm-guide.txt` and asserts it against
`next_for`, so the guide cannot promise a branch the tool does not take.

## Text output says what the tool measured

`src/render.rs`, `verdict_words::gloss`.

The text branch of `run_helpers::output_action_with` used to print only the command's message,
the delta and `verdict: <word> (<reason>)`. On `form_value_microtask_revert.html` that read
`Filled selector '#micro'` + `verdict: unknown (no_baseline)`, exit 0, while `value.actual` on
the same response said the field was empty.

Text mode now renders:

- `value:` — only when the page did NOT keep the write. A clean fill stays one line; a tool that narrates its successes teaches the reader to skip its output.
- `values lost:` with the field and what it held.
- `received by:` with the receiver's uid.
- A gloss on the verdict word, e.g. `verdict: unchanged (identical_tree) — the tree was identical while the tool watched — which is not the same as the action having no effect`. One table keyed on the reason (`verdict_words::gloss`), so the prose cannot drift from the classifier.
- `hint:` from a SECOND table (`verdict_words::short_hint`), one or two lines. The full `verdict_hint` in JSON is unchanged and stays long. A curated second table rather than a truncation: cutting at a character count ends mid-sentence, and cutting at the first sentence loses the imperative — the prohibition in `value_reverted` is in the second sentence. Tests enforce that a short form exists for every full hint, ends on a sentence, fits two lines, and says "Do not" wherever the full text does.

Secrets are redacted here exactly as in JSON: two lengths, never the value, because this line
reaches stdout, the transcript and any `_record` file.

Colour (`std::io::IsTerminal`, no dependency) only on a tty: bold red for `NOT KEPT` /
`INTERCEPTED`, green for a page that moved, yellow for everything a caller must not read as
success. A pipe gets the same bytes plus the new lines, pinned by `tests/text_output_tests.rs`.

## Document identity

`(frameId, loaderId)` from `Page.getFrameTree`, stored as
`PageSession.last_snapshot_frame`/`_loader`. `diff::Identity` is tri-state
(`Same`/`Different`/`Unknown`) and `compare` diffs only on `Same`.

A URL is the wrong signal both ways: it moves on a fragment jump and on `pushState` where every
uid survives, and stays put across a reload or a form GET to the same address where none does.
`Unknown` returns the page rather than guessing. Measured before the fix: 328 bogus `~` lines,
18,764 tokens, versus 16,148 for re-inspecting the destination.

Polling, not events: `CdpClient::events()` only delivers post-subscribe messages and several
commands never subscribe.

## Diff hygiene

- Focus churn is reported as `focus: from -> to` and kept out of `changed`; every click focuses something, so it was the loudest line in most reports. `focus_to_document` rides beside it and must not be counted as delivery evidence.
- `e{n}` uids (the fallback for nodes with no `backendDOMNodeId`, `snapshot.rs`) are renumbered per snapshot and never matched, only counted as `anonymous`.
- A reorder is counted as `moved`. Pairing by uid alone left every uid present and reported "No changes detected" for a drag-and-drop.
