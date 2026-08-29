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

## `download_click.html`

The three shapes a click-triggered download takes, on one page: an `<a download>` whose `href`
is a `Blob`, a button whose handler creates, clicks and removes the anchor so nothing in the DOM
ever names the file, and a button that changes the page and downloads nothing.

The point of the first two is that neither has an address anything outside the page could fetch,
which is the gap `download <url>` cannot reach by construction. The third is the one that has to
stay honest: the click lands, the handler writes a status line, and the response must say no
download began rather than hang or claim a file. Its `#status` line is what proves the click was
delivered — without it, a test asserting "no download" would also pass on a click that missed.

The fourth case (a transfer that begins and does not finish) cannot be a `file://` fixture: it
needs a server that answers slowly, so it lives in `download_click_tests.rs` as `SlowServer`.

## `checkable_kinds.html`

Native checkbox, ARIA checkbox, text input, radio. Covers the two readings of "is this
checked" that `!!el.checked` gets wrong in opposite directions.

## `snapshot_filter_baseline.html`

Thirteen accessibility nodes and an empty `#slot` to inject into, chosen so that every
narrowing flag on `inspect` hides a DIFFERENT part of the tree: a heading and a footer
outside `main` (which `--uid <main>` drops), a combobox with two options nested two levels
deep (which `--max-depth 1` drops), one link (which `--urls` annotates and nothing else
does), and exactly one button (so `--filter button` keeps one line out of thirteen).

The count is the point. Injecting one button into `#slot` makes the honest answer to the
next `diff` exactly `added=1, removed=0, changed=0`, so a baseline that was stored narrowed
does not merely look different — it reports a number, and the number names how much of the
page the flag had hidden. Measured before the fix: `--filter` 13, `--max-depth 1` 10,
`--uid <main>` 5, `--urls` 1 but `changed=1`. A fixture with a round tree would have let
several of those collapse onto the same wrong answer.

The link exists for `--urls` alone and is what makes that path fail in the OTHER direction:
`url="…"` is appended to the line, the next read renders it bare, and the link comes back as
`changed` rather than `added`. Without a link on the page the whole class would have looked
like one bug instead of two.
## `fixed_wall_above_viewport.html`

A consent wall in `position: fixed` whose controls sit past an edge of the viewport, on a
document whose own scroll is locked (`html,body{overflow:hidden}` over 3000px of content). It
reproduces the cause and not just the symptom: an element merely below the fold is scrolled into
view by the probe, while this one cannot be — `scroll` answers "Scrolled into view" and moves
nothing, and every reading of the aim point comes back identical to the pixel. That is the shape
the settle loop used to report as `not_settled` (transient, `next: retry`), which is an
instruction to loop forever.

Two controls, two geometries, because the first fix could have been reading one edge: `#reject`
sits ABOVE the top edge (negative y, the shape measured on a consent wall at `(378, -14)`) and
`#drawer-reject` sits PAST the left edge (negative x with y inside the viewport, the shape
measured on a second site at `(-263.7, 107.5)`). Both must report `off_target` →
`aim_point_off_target` → `inspect`.

## Everything else

`frame_*` for iframe binding, `dialog_click.html` for the JS dialog handler,
`goto_ticker.html` for a page that never stops mutating (the settle probe's ceiling).
