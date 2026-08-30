# docs/design — the record behind the rules

`CLAUDE.md` and the path-scoped rules under `.claude/rules/` state what holds and why. This
directory holds the longer records those statements were distilled from: design specs written
before the code, and investigations whose conclusion became a rule.

Nothing here is loaded into an agent's context automatically. Read a file when you are about to
change the thing it describes, or when a rule's reasoning is not enough to decide.

| File | What it is | Read it before |
|---|---|---|
| `verdict-taxonomy.md` | The design spec the verdict ladder was built against — 113 fixtures, 107 cases where a plausible signal reports a confident wrong answer. Its own inventory of "what is built" describes an earlier state and carries `AMENDED IN IMPLEMENTATION` notes rather than edits; read it as a record of reasoning, not as a description of `src/verdict.rs`. | adding a verdict, a reason, or a rung to `src/verdict.rs` |
| `review-findings.md` | Review findings rescued out of `/tmp` alongside the spec (`5c3fd10`). | — |
| `display-flag-baseline.md` | The investigation behind "a display flag narrows what is printed, never the baseline": seven broken paths with their measured counts, the Wikipedia case where a display flag flipped the next action's `next` token, and the uid_map repro. | changing what `snapshot::take_views` persists, or adding a flag that reduces `inspect` output |
