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

**Every instance, refused by the parser:**

- `download`'s target is an `ArgGroup` (`required`, non-`multiple`, over `url`/`uid`/`selector`), the mechanism `assert` already uses for its comparators.
- The same group on the nine other verbs that take more than one way to name a target: `click`, `dblclick`, `fill`, `select`, `check`, `uncheck`, `upload` (all `required`), and `text`, `screenshot` (exclusive but optional — both act on the whole page with no target). These nine used to be hand-rolled `provided == 0` / `provided > 1` checks inside `run::run`'s second match, i.e. AFTER `resolve_cli_connection`, so `chrome-agent click --selector a --xy 1,2` launched a Chrome before refusing.
- `--xy` takes a `value_parser` returning `[f64; 2]` rather than `Vec<f64>` plus a length check in the arm. `num_args = 2` cannot express it: the two numbers arrive as one comma-separated token, so clap counts them as one value.
- `--dialog` and `screenshot --format` gain the `value_parser` value list that `--verdict` and `--on-intercept` always had, with `ignore_case` so the spellings `DialogPolicy::parse` and `ImgFormat::parse` accept (`Accept`, `DISMISS`, `PNG`, `JPG`) still parse.
- `goto --header` gains a `value_parser` that calls `commands::goto::parse_header` itself. That one used to launch a full Chrome and then reject the header.

**Deliberately NOT moved:** those parsers remain the ONE definition of each rule, because
pipe/batch reach them from JSON where clap never runs. The clap declaration is a second reader of
the same rule, accepted only because a group and a value list are declarative — they can go
stale, they cannot drift into a *different* rule the way a re-implemented predicate can.
`--header`'s gate calls the shared function rather than restating it, at the cost of parsing each
header twice. The pipe-side `else { return Err("click: provide …") }` branches stay for the same
reason: clap never runs there.

Stated rather than hidden: a malformed CLI invocation does not produce `{"ok":false}` on stdout
under `--json`; it is a clap usage error on stderr, exit 1. That was already true of every missing
positional and of every `assert` group, and is now true of the nine target rules too — the
wording is clap's (`the argument '[UID]' cannot be used with '--selector <SELECTOR>'`) rather
than the sentence the seven siblings used to share.

`hints::usage_error` is untouched: it rewrites the one error clap gets wrong, and clap's own
`<UID|--selector <SELECTOR>|--xy <XY>>` states the ways to name a target.

The guard is one test walking every case under an empty `HOME`
(`cli_tests::an_invalid_invocation_is_refused_before_a_browser_is_resolved`): exit 1, the refusal
never says `browser session`, stdout is empty, and no profile directory appears. What is left in
`tests/cli_contract_tests.rs` is the fact the refusal exists for — a refused invocation must not
have acted on the target it discarded — which needs a real browser and a real page to check.

## `run::run` renders; it does not act

Every verb that also exists in pipe mode calls the SAME `pipe_dispatch::dispatch_*`, builds its
typed args (`src/pipe_command.rs`) from clap and renders the response. `run.rs` went from 952
lines to 677 doing it.

What each arm had been was a second implementation of the dispatcher beside it, and several had
already drifted:

- `goto` cleared `uid_map`; `dispatch_goto` never did, so pipe and batch carried uids from the previous document across every navigation.
- `back` fired `history.back()` blind, waited 5 s for a `Page.loadEventFired` that a boundary never sends, and answered `{"ok":true,"title":"New Tab"}` — byte-identical to a real back. Measured at 5.04 s.
- `fill-form` answered `Filled 3 fields: uid=n1, uid=n7` on the CLI and `Filled 3 fields` in pipe.
- `check --selector` reported `role`/`name` on the CLI and only `uid` in pipe.
- `type` built `Typed {text.len()} chars` in both, discarding the message `element::type_text_with` returns — the one that withholds the length on a secret field. The redaction was inert on every path.
- `fill_form --inspect` stored a `--max-depth`-truncated uid map as the baseline in pipe, the seven-path bug in its original shape.

**Two things stay in `run.rs`, and the reason is the same both times:** text mode renders from a
Rust value the JSON has already flattened, so there is nothing to render *from*.

| Verb | What text mode needs that the response does not carry |
|---|---|
| `goto` | `landing::Landing::text_line` — a sentence built from the struct, not from the `landed` object |
| `extract`, `console`, `tabs`, `history`, `network` | `format_text(&T)` over the record structs; the shared half is the collection (`extract::collect`, `network::collect`) and the JSON shape (`console::to_json`, `history::to_json`, `network::Capture::to_json`) |
| `eval` | `eval::run` returns a DISPLAY string and `run_raw` a `Value`; they differ on a result with no value (`undefined`, not `null`). The expression the two evaluate is one function, `eval::scoped_expression` |
| `emulate` | clap has already parsed the values; the pipe's parser exists to turn untyped JSON into the same ones, and going through it would re-serialise a parse that succeeded |
| `webmcp call` | `--args` is a raw JSON string here and a parsed object there. `webmcp::call_report` is the shared half |
| `download` | shares `download::dispatch` and `Outcome::to_json`/`print_text` already; only the `Request` is built twice, from different defaults (`--timeout` is `download`'s own 30 s, pipe's falls back to the session's) |

`run::output_dispatched` is what makes the rest work: it takes a dispatcher's object, lifts
`message` out of it and hands the remainder to `run_helpers::output_action_with` as details. The
CLI never passes `inspect: true` to a dispatcher — `output_action_with` takes ONE reading and
renders it twice (baseline at full depth, display at `--max-depth`), where the dispatcher's own
`inspect` would take a second one.
