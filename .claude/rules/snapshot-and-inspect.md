---
paths:
  - "src/snapshot*.rs"
  - "src/commands/inspect.rs"
  - "src/commands/diff.rs"
  - "tests/snapshot_baseline_tests.rs"
  - "tests/snapshot_secret_tests.rs"
---

# Reading the accessibility tree, and what gets stored as the baseline

## Noise filtering

StaticText/InlineTextBox are stripped (66% token reduction). `--filter` selects by role with
aliases: textbox → searchbox + combobox, input → all input roles, button → menuitem.

## A display flag narrows what is printed, never the baseline

`snapshot::Views`, `snapshot_render.rs`.

A reduction asked for on the OUTPUT — `--filter`, `--max-depth`, `--uid`, `--limit`, `--urls`,
and the `--inspect` of an action in CLI or pipe — must never reach what is persisted as
`last_snapshot` or as the `uid_map`. Seven paths broke this rule at once and each produced a
false positive in the next `diff`. The measurements, the counts per path, the Wikipedia case
where the flag flipped `next` from `confirm` to `proceed`, and the uid_map repro are in
`docs/design/display-flag-baseline.md`. `--max-chars`/`--offset` were already excluded and stay
so.

**The fix is one rendering rule in one place.** `take_views` fetches ONE `getFullAXTree` and
renders it twice: the full pass is persisted, the reduced pass is printed. Two CDP reads were
rejected — the page moves between them, and then the tree shown is not the baseline stored beside
it. The CLI action path was already doing that for `--inspect --max-depth`, so it now costs one
round trip less and gained an invariant.

**The stored `uid_map` is the full one too.** A deliberate behaviour change: a display flag
deciding which nodes the next command may act on is the same bug in another hat. The old
behaviour was not a safety property — the uid was never invalid, merely unprinted — and the error
message advised a re-inspect that would only have worked if the caller dropped the flag. What is
given up is the ability to say "you have not looked at this node yet", which nothing used and
which `--verdict off` and `goto`'s map-clearing already make unreliable. Pinned by
`a_display_flag_does_not_narrow_the_uid_map` and
`a_display_flag_does_not_flip_the_verdict_of_the_next_action`.

Cost, paid in `snapshot_render.rs`: `e{n}` uids come from a counter walked in traversal order, so
a truncated traversal renumbers what it keeps and the `e1` printed would name a different node
from the `e1` stored. The reduced pass therefore INHERITS the full pass's anonymous numbering
(`preassigned`) rather than restarting its counter. That is the one piece of state the two
renderings share.

`--limit` needed its own answer: `scroll_collect` returns a UNION over scroll positions, which
never described the page at any single moment and cannot be a baseline. It now takes one more
full reading after the scrolling stops, and keeps the union only in the `uid_map` — an item that
scrolled out of a virtualized list is gone from the final tree, and dropping its uid would take
away the only handle the caller was given for it.

## Secret values are redacted in the tree

`src/snapshot_secret.rs`.

`inspect` prints the tree and every action report quotes the same lines inside `delta`, so one
response used to carry both `value:{"redacted":true}` and
`delta:"~ uid=n2 textbox \"Card number\" value=\"4111111111111111\" -> …"`. Chrome masks a
`type=password` in the tree, which is what hid the leak: it is the other half of
`element::SECRET_FIELD` — a card number, a security code or a one-time code in a `type=text`
field.

The marker is applied at the single point the tree renders a value and a name
(`snapshot::format_node_with_tracking`), so `inspect`, `diff` and every `delta` inherit it.

**Three things are redacted, not one:**

1. the field's `value=`;
2. the accessible names of its descendants — Chrome exposes an input's editable content as a `generic` CHILD whose name is the value, so masking the input alone left the digits on the next line;
3. any node echoing the same string of four or more characters elsewhere — the `<span>` a checkout uses to show the card it is about to charge.

Below four characters the string is not searched for outside its own field: a security code of
`123` also appears in a price, a date and a street number.

**How secret-ness is decided.** It is a property of the element, and the a11y tree carries neither
`type` nor `autocomplete`, so the page is asked: ONE `Runtime.evaluate` walking
`input,textarea,select,[autocomplete]` (descending into same-origin iframes, exactly the set
`getFullAXTree` can report on) with `element::SECRET_FIELD` as the predicate, then one
`DOM.describeNode` per field FOUND. Measured +0.8 ms on a form holding 60 filled inputs and no
secret, +1.7 ms on a checkout holding four, and nothing on a page with no filled field. The
design that asked about every value-carrying node cost ~110 ms on that same 60-input form, on
every action.

It fails closed at every step: a value on a node with no `backendDOMNodeId`, a scan that throws,
a field that cannot be named — all redact.

**The marker is the fixed string `<redacted>`**, quoted like any value so `diff` still reads it as
one, and never derived from what it hides: two snapshots of the same field must compare equal, or
every secret on the page would be reported as changed by every action.

The cost is stated rather than worked around. A secret whose value the page really replaced now
compares EQUAL, so filling `4111…` over `4242…` reports `focus_only` instead of a value delta. A
truncated hash would have kept that half, and a truncated hash of a 3-digit security code is
brute-forceable from the transcript offline.

Two halves survive: a secret that DISAPPEARS is still visible, because an empty value emits no
`value=` token at all, so `values_lost` keeps naming the card number a submit destroyed
(redacted, `tests/values_lost_tests.rs`); and the acting command still witnesses its own write,
since `fill` reports `{redacted:true, verbatim:true, actual_length}`. What is lost is a third
party's change: a click that rewrites a secret field it did not aim at is invisible in the delta.

## Output flags

- **`inspect --urls`** — post-processes the snapshot text and resolves `href` on link nodes via `DOM.resolveNode` + `Runtime.callFunctionOn`.
- **`inspect --max-chars`/`--offset`** — char-based, UTF-8-safe paging via `inspect::paginate`. The full snapshot is still persisted for diff and uid lookups; only the printed window is capped. Truncated output appends the next `--offset`.
