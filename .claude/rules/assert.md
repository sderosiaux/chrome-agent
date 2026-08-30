---
paths:
  - "src/commands/assert.rs"
  - "src/commands/assert_args.rs"
  - "tests/assert_tests.rs"
  - "tests/text_exception_tests.rs"
---

# The command whose result is its exit code

## Exit codes

`assert value|text|url|state|exists` is the command whose *result* is the exit status:
**0 held, 2 did not hold, 1 could not be checked.**

The third code is the reason the scheme exists. Collapsing "the form kept a different value" into
the same `1` as "Chrome never started" makes the two indistinguishable to a CI job, and the
recoveries are opposites (report/repair vs retry).

- `2` travels as `commands::assert::NotHeld` through the error channel, so no caller threads a second return type through `run::run`. `main` recognises it before its generic handler.
- Clap's usage errors moved from `2` to `1` (`main` uses `try_parse`): a wrong flag is the caller's mistake, not a fact about the page, and `2` may mean exactly one thing.
- A selector that matches nothing is a `1`, not a `2`. "The field holds X" is unanswerable when there is no field, and answering `false` would let a typo read as a statement about the page. `assert exists` is where presence itself becomes the claim, and there `--count 0` is a legitimate absence assertion.

**`2` is a claim class, not a command.** `EXIT_NOT_HELD` was written as "only `assert` returns
`2`", and a macro guard then exited `1` — which put "the guarantee this macro was recorded with
failed" back in the same bucket as "the browser never started", the exact conflation the code
exists to remove. A macro guard IS an assertion: it ran, the page disagreed, and the recovery is
report/repair rather than retry. `macros_run::exit_code` returns `EXIT_NOT_HELD` too, off the
report's own `stopped_by: "guard"`. A macro guard that could not be EVALUATED (`stopped_by:
"error"`, e.g. the page could not be read) is a `1`, the same rule as a selector matching nothing.

`assert` is NOT in `mutates_page`: it is a read, so no change report and no verdict ride on the
response (a verdict is an action's vocabulary).

In pipe/batch an assertion has no exit code of its own, so `held` rides on `ok` and the response
carries the same `assertion` object; an operational failure carries `error` instead, which is how
the two are told apart. A CLI `batch --stop-on-error` that stopped on a failed assertion exits
`1`, not `2`: the process is reporting that the batch stopped, not making a claim about the page. `--json` puts a failed assertion on stdout; text mode puts it on stderr with stdout empty,
so a shell pipeline can use the exit code alone.

## An assertion reads through the action's own reader

Two implementations that agree today drift. An assertion that read `el.checked` on a
`<div role=checkbox>` would report a checked box as unchecked — the exact bug `check` was fixed
for — and an agent trusting it would click the box OFF.

- `assert state --checked` calls `element_controls::CHECKABLE_PROBE`, the classification `check`/`uncheck` apply before clicking.
- `assert state --selected` calls `element_controls::SELECT_READ`, which `select`'s read-back uses too (`SELECT_APPLY` became `select_apply()` so it can embed it).
- `--disabled` reads `:disabled` (what `fill` refuses on, so it catches an ancestor `<fieldset disabled>`) *plus* `aria-disabled`, reported as `enabled` / `disabled` / `aria-disabled`. A `<div role=button aria-disabled=true>` is inert to everything that reads the page, and only the CSS pseudo-class disagrees.
- `--visible` means rendered, opaque and not `visibility:hidden`, and says so in a `means` field. It is NOT "in the viewport" and NOT "nothing on top of it", which would need a hit test this command does not do.
- `assert value` refuses an element with no `value` property (naming `assert text` instead) and redacts secret fields on the same test `fill` uses, reporting lengths only.

## `--matches` is a Rust regex, not a JS one

`regex-lite`, chosen over `regex` because it has zero transitive dependencies (the musl graph
stays pure Rust, which the CI guard depends on) and none of the ~1 MB of Unicode tables, for a
comparator that runs once per assertion.

The cost is documented in `llm-guide.txt`: `\d`, `\w` and `\s` are ASCII-only (`^\w+$` does not
match `Jean-Sébastien`), and there is no `\p{…}` or lookaround. `(?i)` works.

Evaluating a JS `RegExp` in the page was rejected: every comparator test would then need Chrome,
and `assert url --matches` would become a page evaluation.

A malformed pattern is exit 1 — nothing was compared.

`assert text --equals` is refused outright: equality against `innerText` breaks on a cosmetic
whitespace edit, and an assertion that brittle reports a working page as broken.
