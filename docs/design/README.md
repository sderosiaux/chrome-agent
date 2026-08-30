# docs/design — the record behind the rules

`CLAUDE.md` and the path-scoped rules under `.claude/rules/` state what holds and why. This
directory holds the longer records those statements were distilled from: design specs written
before the code, and investigations whose conclusion became a rule.

Nothing here is loaded into an agent's context automatically. Read a file when you are about to
change the thing it describes, or when a rule's reasoning is not enough to decide.

| File | What it is | Read it before |
|---|---|---|
| `verdict-taxonomy.md` | The design spec the verdict ladder was built against: nine verdicts, the R0–R8 classifier ladder, 30 stated limits, and a 113-fixture plan. Derived from 107 cases where a plausible signal reports a confident wrong answer. Its "Status against the code" section names what has since shipped; read the rest as a record of reasoning, not as a description of `src/verdict.rs`. | adding a verdict, a reason, or a rung to `src/verdict.rs` |
| `review-findings.md` | 52 findings across 7 reviewers, 40 verified. The fixed items are history; the "set aside deliberately" section is the live part — what was examined, not changed, and the measurement behind each decision. | proposing a cleanup in `pipe.rs`, `session.rs`, `resolve_uid`, `snapshot::settle`, or the test harness |
| `display-flag-baseline.md` | The investigation behind "a display flag narrows what is printed, never the baseline": seven broken paths with measured counts, the Wikipedia case where a display flag flipped the next action's `next` token, and the uid_map repro. | changing what `snapshot::take_views` persists, or adding a flag that reduces `inspect` output |
