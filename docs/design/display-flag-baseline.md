# A display flag narrowed the baseline — the investigation

The RULE lives in `.claude/rules/snapshot-and-inspect.md` and loads whenever `src/snapshot*.rs` or
`src/commands/inspect.rs` is read. This is the measurement record behind it.

The named tests and the Wikipedia measurement also live in `tests/snapshot_baseline_tests.rs`:
the before/after table as a module doc at :17-34, `a_display_flag_does_not_narrow_the_uid_map` at
:283, `a_display_flag_does_not_flip_the_verdict_of_the_next_action` at :332.

## The bug

"The baseline snapshot is always taken at full depth" was implemented on ONE path and stated as if
it held everywhere. `inspect --filter "heading,button,link"` persisted the FILTERED rendering as
`last_snapshot`, so the next `diff` compared the whole page against an amputated copy and reported
every node the filter had dropped as an ADDITION.

Measured on `https://webmcp-coffee.jilles.fyi`: `added=22` of nodes that never moved —
RootWebArea, `main "Coffee collection"`, contentinfo, the `Roast` combobox — where the same
sequence with a plain `inspect` answered `added=0 removed=30 changed=3`, which is the truth. A
false positive produced by the tool whose reason to exist is removing false positives, and an
agent's recovery from a phantom addition is to act on a node that was always there.

## Seven paths, not one

Measured on `tests/fixtures/snapshot_filter_baseline.html` (13 nodes, one button injected, so the
honest answer is `added=1 removed=0 changed=0`):

| Path | Reported `added` |
|---|---|
| `--filter` | 13 |
| `--max-depth` | 10 |
| `--uid` (subtree focus) | 5 |
| `--limit` with a filter | 13 |
| `goto --inspect --max-depth`, in CLI *and* pipe | 10 |
| a pipe action's `inspect:true` + `max_depth` under `--verdict off` | 10 |
| `--urls` | inflated `changed` rather than `added`: it appends `url="…"` to link lines and the next read renders them bare, so every link read as rewritten |

**`--urls` is the worst of the seven and was found independently**, by an audit sweeping fifty real
sites with no knowledge of this work. On Wikipedia the same click on the same element answered
`changed: 2656` / `next: proceed` after an `inspect --urls`, and `changed: 0` / `next: confirm`
after a plain `inspect`. That is not a wrong count, it is a wrong INSTRUCTION: `proceed` and
`confirm` are opposite branches of the closed set of six, so an agent did two different things
because of a display flag used several steps earlier.

Reproduced in miniature on the fixture (`changed / tree_delta / proceed` against
`no_effect / delivered_no_change / confirm`, on a button with no handler) and pinned by
`a_display_flag_does_not_flip_the_verdict_of_the_next_action`, which asserts the two responses
agree on `verdict`, `verdict_reason` AND `next`. Agreement alone is satisfiable by both sides
being wrong, so it also refuses `proceed` outright for a click on an inert button.

The two paths the change report covered were invisible because `attach_change_report` overwrote
the truncated snapshot with a full read moments later. They surface where nothing overwrites it:
`goto` (deliberately outside `mutates_page`) and any action with the report off.

## The uid map, measured

On `snapshot_filter_baseline.html`, `inspect --max-depth 1` left a map of four uids on a page of
thirteen, and `click n26` on the button below the limit answered `Element uid=n26 not found. Run
'chrome-agent inspect' to get fresh uids.` about a node that was plainly on the page and plainly
clickable. It now answers `{"ok":true,"message":"Clicked uid=n26"}`. Same for `inspect --uid
<main>` followed by a click on a heading OUTSIDE that subtree. The error message compounded the bug
by advising a re-inspect that would only have worked if the caller happened to drop the flag.

Why that is a behaviour change and not a safety regression is stated in the rule file, and it is
kept there deliberately: without it, someone can reintroduce the narrow map believing they are
restoring a protection.
