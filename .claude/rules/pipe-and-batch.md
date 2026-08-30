---
paths:
  - "src/pipe.rs"
  - "src/pipe_command.rs"
  - "src/pipe_validate.rs"
  - "src/pipe_dispatch.rs"
  - "src/pipe_dispatch_actions.rs"
  - "tests/pipe_protocol_tests.rs"
  - "tests/pipe_refusal_shape_tests.rs"
  - "src/commands/batch.rs"
  - "src/commands/record.rs"
  - "src/macros_cmd.rs"
  - "tests/mode_parity_tests.rs"
  - "tests/one_dispatcher_tests.rs"
  - "tests/record_perms_tests.rs"
  - "tests/read_failure_tests.rs"
---

# One connection, many commands — pipe, batch, and recordings

## The protocol has a type

`src/pipe_command.rs`. One `#[serde(tag = "cmd")]` enum, one `deny_unknown_fields` struct per
verb, parsed once in `dispatch_one` before anything is dispatched. Thirty-six dispatchers used to
hand-decode `Value` (`cmd.get("value").and_then(Value::as_str).ok_or(…)`), and three had a typed
parser; the rest ignored every key they did not ask for. `{"cmd":"click","uidd":"n1"}` answered
`click: provide "uid", "selector", or "xy"` — a refusal naming a problem the caller did not have.
It now names `uidd`. `macros::{Step, Guards, Param}` already had this shape over the same objects.

**Aliases are `#[serde(alias)]`, not `|` arms**: `fill-form`/`fill_form`/`fillform`,
`navigate-and-read`, `fill-and-submit`, `webmcp-list`, `webmcp-call`. `PipeCommand::name()` gives
the canonical verb, and `mutates_page` reads that rather than the caller's string, so an alias
cannot fall out of the change-report allowlist.

**Error messages are serde's, mapped.** `missing field \`url\`` → `goto: missing "url"`, the exact
text the hand-decoded version produced (`pipe_refusal_shape_tests` pins nine of them). An unknown
variant becomes `Unknown command: X`; an absent `cmd` is caught before serde so it stays
`Missing "cmd" field`. For a wrong TYPE serde names neither the command nor the key, so
`blame_field` removes one key at a time and re-parses: the key whose removal changes the error is
the offender, and the answer is `fill: "value": invalid type: integer 42, expected a string`.
Only ever on the error path.

**What is still a raw `Value`, and why:**

- `_record` — a directive to the SESSION, not an argument to any verb. `pipe.rs::take_record_path` removes it before the protocol sees it, which also keeps it out of the recorded command, so replaying a recording no longer carries an instruction to record itself.
- `batch.commands` — each entry is a command, parsed by `dispatch_single` in turn.
- `webmcp_call.args` — the page's schema, not this one's.
- `fill` has its own `FillArgs` rather than sharing `ValueArgs` with `select`, because it takes `secret` and `select` cannot honour one (it reads secrecy off the element). A key that parses and does nothing is worse than a refusal naming it.
- `emulate`'s and `assert`'s VALUES. The keys are enumerated (a typo is refused by name); the values keep `pipe_emulation::parse_device_config` and `commands::assert::from_json`, whose messages name the field and the type it wanted (`emulate device: "dpr" must be a number`) where serde's would not, and which the CLI shares. `AssertArgs` serialises back to a `Value` for `from_json` rather than re-reading it, so there is still one `assert` parser.
- `EmulationRecovery::refusal_for`/`update_after` — they run BEFORE the parse, so a stored device configuration that no longer applies still refuses a command the protocol would also refuse.

**Consequences, stated:** four keys that were silently ignored are now refusals — `uncheck`'s
`desired` (only `check` may be talked out of the state its verb names), `emulate`'s
`deviceScaleFactor` (the pipe only ever read `dpr`), any per-command `verdict` (a session flag;
one inert `"verdict":"off"` in `slow_dispatch_tests` was removed as the dead key it was), and
anything else outside a verb's declared set. A `_record` that is present but not a string is
refused instead of ignored.

`pipe_validate.rs` enforces relationships serde cannot express. The pipe now refuses two target
forms where clap's `ArgGroup` permits exactly/at most one; an unknown `on_intercept`; a `wait`
that mixes `what`/`pattern` with a shorthand or supplies `pattern` alone; and device fields on
`emulate status|reset`. `pipe_command::tests::target_groups_are_exercised_through_both_parsers`
feeds the same accepted and rejected forms through `Cli::try_parse_from` and the pipe parser, so
the parity claim reads both implementations rather than testing only one and naming the other.

## One dispatcher, and what it cost to have two

`pipe_dispatch::dispatch_single`. Pipe, pipe `batch` and CLI `batch` all reach it; `pipe.rs`
carried a second copy of the same ~36-arm match — same baseline capture, same `refusal_in`
unwrap, same `--verdict off` branch, same `attach_change_report` tail — and the two had drifted.
Only the pipe's copy had a `"batch"` arm, so `{"cmd":"batch","commands":[{"cmd":"batch",…}]}`
answered `Unknown command: batch` from inside a batch and ran from the pipe. Both copies also
spelled "uncheck is check with a field inserted" by cloning the command Value and inserting
`desired`; the verb now passes `desired` as an argument, so the decision is made once.

Every dispatcher takes `&mut PageCtx` (`src/page_ctx.rs`) plus its own `cmd`. The eight values
in it — two clients, the store, browser/page/target_id, `--timeout`, `--max-depth`,
`ReportPolicy` — used to be threaded by hand through ~30 signatures, so a ninth meant editing
every function in between. A dispatcher that only reads the store takes `&PageCtx`, which is
still the read/write split the old `&SessionStore`/`&mut SessionStore` pair carried.
`emulation_recovery` stays a parameter: only the batch arm has one, and the CLI's other arms
would have to invent one to fill a field.

`batch` being a command makes `dispatch_single` → `dispatch_batch` → `run_batch` →
`dispatch_single` a cycle, which is the technical reason the copy existed: rustc cannot size a
future whose type contains itself. `dispatch_single` therefore returns
`Dispatched<'a> = Pin<Box<dyn Future<Output = Value> + Send + 'a>>` — an erased type breaks the
cycle where `Box::pin` on a named `async fn` future does not. `Send` is not decoration: the
clippy nursery enforces `future_not_send` crate-wide, and dropping it makes `run::run` non-`Send`.

`run::run`'s own arms are the same story a level out: each was a second implementation of the
dispatcher beside it, and six of them had drifted. `.claude/rules/cli-parsing.md` holds the list,
the six divergences, and the six verbs that deliberately did not collapse.

`pipe::open_session` is the same story one level up: `run_pipe` and `run_replay` each kept their
own copy of the connect sequence its doc comment already claimed to be the only one of, and those
had drifted too (a different "no HTTP endpoint" message). Both now call it. Consequence, stated:
`replay`'s message for a browser with no HTTP endpoint is now the longer pipe one.

## Session finalization is part of the protocol

Every response may have been delivered successfully and the final session-store write may still
fail. Pipe/replay therefore save after normal EOF and after processing or stdout failure. A
failure outside an individual command is emitted as one terminal JSON line:
`{"ok":false,"terminal":true,"phase":"startup|finalize","error":"…"}`, followed by process
exit 1. It is not a response to the preceding command. A broken stdout may prevent that line from
being delivered, but it no longer skips the save attempt.

## What one connection is actually worth

**A fixed ~12 ms per command, and nothing else.** That is the whole mechanism: process start, the
HTTP GET, two WebSocket handshakes, the CDP setup round trips and two session-store lock/merge/write
cycles, paid once instead of once per command. Everything a ratio does after that is arithmetic on
what the commands themselves cost — measured 1.5x on a nine-command stream of reads, 1.1x on the
same length of `fill`/`click`, because an action's observation window and tree re-read are ~230 ms
that one connection does not touch.

Do not restate those numbers here when they move; the table, the method and the raw spreads live in
`docs/design/pipe-latency.md`, and `./scripts/measure-pipe.sh` re-measures them in about a minute.

Six documents said "10x faster" for four releases. `git log -S"10x faster"` traces the sentence to
b80da9a, whose body measures a *binary size* (3.4 MB → 2.9 MB); there was never a latency number
behind it. The honest reason to reach for pipe is uid stability across the sequence and the `frame`
binding living on the connection — both correctness properties, neither of them speed.

## `back` and `forward` are one step with a sign

`pipe_dispatch::history_step(ctx, delta)`. The two dispatchers were the same twenty lines
twice over — same `Page.getNavigationHistory`, same pre-subscribe to `Page.loadEventFired`, same
`document.title` re-read — differing in `-1`/`+1` and in the boundary test, which only `forward`
had.

**`back` reported success for a navigation that did not happen.** The CLI arm fired
`history.back()` blind and waited on `Page.loadEventFired`; at the start of the stack that event
never comes, so it paid the full five seconds (measured: 5.04 s) and answered
`{"ok":true,"title":"New Tab"}` — byte-identical to a real back. The boundary is read before
anything is dispatched, in both directions, and both verbs report the `url` they landed on as
`goto` does (`back` carried none, so the only way to know where it went was to read the page
afterwards). At either end the answer is the same stated non-event it always was:
`{"ok":true,"title":"","message":"Already at first|last history entry"}`, no `url`, because
nothing was navigated to.

`run::run`'s `Command::Back`/`Command::Forward` arms call `history_step` too, so there is one
implementation for all three modes. Text mode prints `url — title`, or the `message` at a
boundary; `back` gaining a `url` in text output is the same fix as gaining one in JSON.

**The uid map is cleared by `history_step`, not by the CLI.** The clear lived in the CLI arm and
the dispatcher never had it, so a uid from before the step still resolved in pipe and batch — to
whatever node the new document happens to give that `backendNodeId`. It belongs with the
navigation, next to the `set_frame_context(None)` that is there for the same reason. `goto` had
the identical split and `dispatch_goto` now clears it too. `last_snapshot` survives both, so
`diff` still answers `document_changed` rather than erroring.

`back` also gained `--inspect`/`--max-depth`, which only `forward` carried: one step with a sign
should not have one set of display flags.

## A failed command does not hand its wait to the next one

`settle_wait` lives on the `CdpClient`, which pipe and batch reuse across commands. It is only
ever TAKEN in `pipe_report::attach_verdict_for`, and a command that returns `Err` never gets
there — so a `check` that clicked, waited for the navigation its handler triggered, and then
failed its read-back left the wait in the slot, and the next command reported it as its own
`waited_ms`. `mark_dispatch` clears it going in (which covers the keyboard verbs, which dispatch
nothing that waits); the error boundary of `dispatch_single` now takes and drops it going out.

Fixture `tests/fixtures/checkbox_navigates_away.html`: a checkbox whose click navigates, so the
read-back 60 ms later runs against a destroyed execution context. The next command in the test is
`scroll` — the one mutating command that settles a verdict without calling `mark_dispatch`, and
therefore the only one that could inherit another action's wait.

## A failed read is not a failed action

The post-action page read is best effort in all three modes. The CLI used to propagate it with
`?`, so a click that had already been delivered returned `ok:false`, and the natural response to
that is to click again — which is real. `pipe_dispatch` stated the opposite policy in a comment
and followed it; the CLI now matches.

The response is `ok:true` with `verdict: unknown / read_failed`. Reproduced with
`tests/fixtures/blocks_after_click.html`, which pins the main thread after the click returns so
CDP cannot answer within a short `--timeout`.

## Recordings are 0600

`commands::record::restrict`. A recording holds every command and response of the session,
including the values a fill put into the page — among them the ones redacted on stdout precisely
because they are secrets. It was created with whatever the umask allowed (typically 0644) while
screenshot/pdf/download/session all chmod 0600.

Applied on every write, not only at creation: the file may already exist, wider, from an earlier
run.

## A recording that cannot be written refuses the command

`pipe.rs`. `start_recording` and `log_entry` errors were both discarded with `let _ =`, so an
unwritable `_record` path produced `ok:true` responses indistinguishable from a session actually
being recorded, and the agent found out at `replay` time that there was nothing to replay.

- A failure to **open** refuses the command before it runs: the caller asked for a recorded action, and running it unrecorded is not that. Consequence: a bad path stops the session's work, not just its log — but per command and loudly, so it surfaces on the first line.
- A failure to **append** rides on the response as `recording_error` instead, because that command already ran and failing it invites a retry of real work.

## `batch`

CLI mode reads a JSON array from stdin and dispatches sequentially via
`pipe_dispatch::dispatch_single`. Pipe mode uses a `"commands"` array field. A `batch` inside a
`batch` is just another command and nests to any depth, in both front ends.

`stop_on_error` is opt-in (`pipe_dispatch_actions::run_batch`): `--stop-on-error` on the CLI,
`"stop_on_error": true` in pipe mode. The default is unchanged — every command runs. The response
adds `stopped_at` (index) and `skipped` (count) rather than leaving the caller to infer a short
array. Both front ends share one loop; they used to keep a copy each.

Where the two front ends stop being the same is the process boundary, and only the CLI has one.
`run::run`'s `Command::Batch` arm returns `run_helpers::BatchStopped` when `stopped_at` is set, so
the CLI **exits 1** — `stopped_at`/`skipped` in a payload that always exited 0 is a fact no shell
or CI step can branch on. `1` and never `2`: a batch that stopped is an error, and `2` is only
ever a claim this tool made that did not hold — an assertion, or a `macro run` guard. Without `--stop-on-error` the batch ran everything it was
asked to and exits 0 even when an entry failed; `ok` still carries that. The arm saves the session
before returning, because the error channel skips the save at the end of `run`.

`BatchStopped` prints nothing (unlike `assert::NotHeld`, which owns its own report): the arm has
already written the one response, and a second line would put two responses on stdout for one
invocation. That response is JSON only under `--json`, like every other CLI arm —
`run_helpers::batch_text_lines` renders one line per entry otherwise. Pipe mode is unaffected: it
is a JSON protocol and ignores `--json`.
