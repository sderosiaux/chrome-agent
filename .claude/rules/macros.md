---
paths:
  - "src/macros*.rs"
  - "tests/macro_tests.rs"
---

# Distilling a session that worked into a macro, and replaying it

A macro is a path that worked once, and a guard is what makes replaying it worth anything.
`batch` had a list of commands with no name, `replay` had a file with no distillation, and the
verdict was per action and never per scenario; a macro is the three in one artefact.

`macro record` distils a session that succeeded into named, parameterised steps whose every
expectation is what was OBSERVED then. `macro run` dispatches each step through the same
dispatcher pipe and batch use, checks its guards, and stops at the first one that does not hold.

**There is no repair, no retry and no branch.** That is the line between this and a compiler this
project has not built.

## What becomes a guard

`macros_record.rs`. An observation becomes an expectation only if it would still be true tomorrow
on the same task.

**Kept:**

- `delivery: target_hit` — binary, measured before the dispatch, the strongest thing this tool knows. Only `target_hit`; the other readings describe a step that did not do what it was asked.
- The verdict WORD, never the reason. `tree_delta` today and `value_kept` tomorrow for the same successful fill is in this repo's own history.
- `value.verbatim: true` — for a secret field this is the guard, while the value stays out of the file.
- `url_matches`, derived from the PATH only, escaped for `regex-lite`.

**Refused, and not written anywhere:**

- The `changed` counters. `added: 450` was measured as a scrim closing; a node count is not an intention, and it is the most tempting field on the response.
- uids (numbered per document).
- Any duration (`observed_after_ms`, `waited_ms`) — a slower machine is not a failure.
- `verdict_reason`, and the `delta` prose.

`text_contains` and `exists` are in the format and deliberately NOT derived: which of the strings
a page gained means success, rather than a date or an order id, is a judgement the agent that just
did the task can make and a heuristic cannot.

## Locators

`macros_record::locate`. A macro carries no uid, so a step either has a durable locator or is
refused. Order of preference:

1. the CSS selector the agent already used;
2. the accessible role and name the response reports;
3. a refusal that says so.

`--xy` is never recordable: a coordinate names no element.

A refused step is reported apart from a dropped one. A dropped step was exploration; a refused
step ACTED and could not be written down, so the macro is shorter than the task.

At run time a role+name step resolves against a fresh snapshot: one match is the target, none is a
page that no longer has the control, and several are an ambiguity a macro may not settle by
picking one.

## A verdict guard has to be paid for

`macros_run`. A verdict is a comparison and a comparison needs a baseline, which the recording
session had (the agent inspected while exploring) and which distillation drops. Measured on the
first end-to-end run: every macro's first guarded step answered `unknown / no_baseline`.

The runner takes one snapshot before the first step that promises a verdict, and one after each
navigation — the same cost the session it was recorded from paid.

## Where the task begins

Chosen with hindsight, and printed. An explicit marker has to be planted BEFORE the task, and the
premise is that the agent finds out afterwards that it worked.

So `macro record --from N` names the first step retrospectively, the default is the last
successful navigation, and the response says which entry it started at and whether that was the
caller's choice (`started_at`, `started_by`).

## A step that promises nothing says so

`Step::unguarded`. A step whose observation produced nothing from the whitelist carries the reason
(`--verdict off`, an `unknown` verdict, or no delivery and no read-back at all). Both
`macro record` and `macro run` report how many there are. An unguarded step that looks like a
guarded one is trusted like one.

## Why the file is JSON

YAML earns its keep when a human writes the file, and the premise here is that the agent writes
it. Against that it costs a dependency (`serde_yaml` is unmaintained, and the crate graph is
CI-guarded), while every other artefact this tool reads or writes is JSON.

A step's `do` is EXACTLY the command object `pipe`/`batch` take, so `macro run` reuses the
execution semantics instead of inventing a second set.
