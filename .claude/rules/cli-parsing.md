---
paths:
  - "src/cli.rs"
  - "src/cli_actions.rs"
  - "src/run.rs"
  - "src/run_helpers.rs"
  - "tests/cli_tests.rs"
  - "tests/guide_examples_tests.rs"
---

# What the parser refuses before a browser exists

## A global flag parses on either side of the verb

`cli.rs`, `global = true`. All fifteen flags on `Cli` are global (`--chrome-arg` joined later, for
the same reason: Chrome can be launched from any command). Requiring a global flag to precede the
subcommand is the opposite of the reflex a shell teaches.

**Two exceptions, forced rather than chosen:** `--timeout` (redeclared by `wait` and `download`
with their own meaning) and `--max-depth` (redeclared by the twelve action commands that take
`--inspect`). A global arg propagates into every subcommand, so sharing an id with one of them is
a duplicate-argument panic at startup, not a parse error. Both keep the `local.or(global)` rule
in `run.rs`.

Unifying them was refused: `wait`'s own default is 10 s against the global 30 s, so folding them
would make every `wait` that gives up today hang three times longer.

Instead the failure teaches (`hints::usage_error`, wired into `main`'s parse-error path). Clap
answers `chrome-agent click n1 --timeout 5` with `tip: to pass '--timeout' as a value, use
'-- --timeout'`, which is advice for escaping a literal string nobody meant to pass. That tip is
replaced under the `hints.rs` contract — one fact, one imperative command with real values:

```
hint: --timeout is read before the verb: `chrome-agent --timeout 5 click n1`. Same values,
same command — only the flag moves. (`wait` and `download` declare their own --timeout with
different defaults, so this one is not global.)
```

The command is the caller's own argv with the flag and its value moved ahead of everything else,
so `--timeout=5` and any flag already before the verb survive. A test runs the suggested
invocation through `CHROME_AGENT_PARSE_ONLY` and asserts it parses, because a hint naming an
invocation the parser also rejects is worse than no hint. Every other clap error is returned
untouched.

Consequence: global flags also appear in each subcommand's `--help`, which is longer and more
discoverable.

## An invocation is judged before a browser is

`cli.rs`, `ArgGroup` + `value_parser`.

`run::run` resolves the store, connects to or launches the named browser and builds the CDP
client BEFORE its second `match`, so every argument check living inside an arm answers a question
about the machine rather than about the arguments.

`commands::download::Target::parse` sat in the `Command::Download` arm, so `chrome-agent download`
with no target answered ``No browser session 'default'. Run `chrome-agent --browser default goto
<url>` first.`` — true, about a problem the caller does not have, sending them to launch a
browser for an invocation that could never run. It also made a test pass everywhere except where
it mattered: the assertion held on any machine that had used the tool and failed on a fresh CI
runner.

**Four instances, all now refused by the parser:**

- `download`'s target is an `ArgGroup` (`required`, non-`multiple`, over `url`/`uid`/`selector`), the mechanism `assert` already uses for its comparators.
- `--dialog` and `screenshot --format` gain the `value_parser` value list that `--verdict` and `--on-intercept` always had, with `ignore_case` so the spellings `DialogPolicy::parse` and `ImgFormat::parse` accept (`Accept`, `DISMISS`, `PNG`, `JPG`) still parse.
- `goto --header` gains a `value_parser` that calls `commands::goto::parse_header` itself. That one used to launch a full Chrome and then reject the header.

**Deliberately NOT moved:** those four parsers remain the ONE definition of each rule, because
pipe/batch reach them from JSON where clap never runs. The clap declaration is a second reader of
the same rule, accepted only because a group and a value list are declarative — they can go
stale, they cannot drift into a *different* rule the way a re-implemented predicate can.
`--header`'s gate calls the shared function rather than restating it, at the cost of parsing each
header twice.

Stated rather than hidden: a malformed CLI invocation no longer produces `{"ok":false}` on stdout
under `--json`; it is a clap usage error on stderr, exit 1. That was already true of every
missing positional and of every `assert` group.

`hints::usage_error` is untouched: it rewrites the one error clap gets wrong, and clap's own
`<URL|--uid <UID>|--selector <SELECTOR>>` states the three ways to name a target.

**Still judged after, and deliberately:** the "only one way to name a target" rule for
`click`/`fill`/`select`/`dblclick`/`check`/`uncheck`/`upload` lives in each arm of `run::run`,
not in clap. `check`, `uncheck` and `upload` were missing it — they guarded only "neither was
given" and then took the selector branch whenever a selector was present, so
`chrome-agent check n47 --selector "#other"` checked `#other` and never mentioned that `n47` was
discarded. They now refuse in the wording their four siblings already use. Kept in `run.rs` rather
than moved to an `ArgGroup` so all seven state the rule the same way and in one place; the price
is that these refusals cost a browser launch, and `CHROME_AGENT_PARSE_ONLY` returns before
reaching them, so `tests/cli_contract_tests.rs` owns a real `TestBrowser`. Moving all seven to
`ArgGroup` at once would be the better end state — moving three would just split the family.

The guard is a test asserting the refusal never says `browser session` (`tests/cli_tests.rs`,
`tests/download_click_tests.rs`), run under an empty `HOME`, plus an assertion that no profile
directory appears there.
