# Test fixtures

Plain HTML files loaded over `file://`. No network, no build step, and nothing
non-deterministic: no `Math.random`, no clock reads, no timers whose duration varies.
A fixture that behaves differently on two runs is worse than no fixture.

## `extract_*`

Pages with repeating records, driving `extract`. `extract_hn_subtext.html` reproduces
Hacker News' real row structure (a story row, a subtext row carrying content, a spacer),
which is what exposed the merge that grouped two different record types into one list.

## `form_value_*`

Staged corpus for the action verdict work, not yet referenced by any test.

Each one pins a case where reading back an input's value after `fill` gives an answer a
naive classifier gets wrong: a phone mask that makes the value *longer* than requested,
a `maxlength` that truncates it, a controlled input that reverts it, a number input that
discards letters, a form that resets on submit. The comment at the top of each file
states the expected verdict and the signal that produces it.

They came out of an adversarial pass whose only brief was to find cases where a
plausible detection signal reports a confident wrong answer. `form_value_phone_mask.html`
is the sharpest: a digits-only comparison says the content was preserved, and a currency
variant of the same mask turns `1000` into `10.00`.

## `read_back_kinds.html`, `select_secret_autocomplete.html`

One page holding a text input, a `<select>` and two checkboxes — the controls that read
their own state back — so the verdicts `fill`, `select` and `check` report can be compared
without the page being part of the difference. The already-checked box is the case that must
NOT claim a read-back: nothing is dispatched, so there is no post-action moment.

`select_secret_autocomplete.html` is deliberately contrived markup: a `<select>` declaring
`autocomplete="new-password"` is nonsense, and it is the only way to reach
`element::SECRET_FIELD` on a dropdown. It pins the redaction where the predicate actually
fires, rather than leaving it to hold by luck. Its control select carries DIFFERENT option
labels on purpose: `snapshot_secret` scrubs any node echoing a secret's value, so identical
labels made the control come back redacted too — for a different reason, which is exactly the
confusion a control is supposed to remove.

## `checkable_kinds.html`

Native checkbox, ARIA checkbox, text input, radio. Covers the two readings of "is this
checked" that `!!el.checked` gets wrong in opposite directions.

## Everything else

`frame_*` for iframe binding, `dialog_click.html` for the JS dialog handler,
`goto_ticker.html` for a page that never stops mutating (the settle probe's ceiling).
