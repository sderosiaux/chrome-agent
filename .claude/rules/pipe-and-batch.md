---
paths:
  - "src/pipe.rs"
  - "src/pipe_dispatch.rs"
  - "src/pipe_dispatch_actions.rs"
  - "src/commands/batch.rs"
  - "src/commands/record.rs"
  - "src/macros_cmd.rs"
  - "tests/mode_parity_tests.rs"
  - "tests/one_dispatcher_tests.rs"
  - "tests/record_perms_tests.rs"
  - "tests/read_failure_tests.rs"
---

# One connection, many commands — pipe, batch, and recordings

## One dispatcher, and what it cost to have two

`pipe_dispatch::dispatch_single`. Pipe, pipe `batch` and CLI `batch` all reach it; `pipe.rs`
carried a second copy of the same ~36-arm match — same baseline capture, same `refusal_in`
unwrap, same `--verdict off` branch, same `attach_change_report` tail — and the two had drifted.
Only the pipe's copy had a `"batch"` arm, so `{"cmd":"batch","commands":[{"cmd":"batch",…}]}`
answered `Unknown command: batch` from inside a batch and ran from the pipe. Both copies also
spelled "uncheck is check with a field inserted" by cloning the command Value and inserting
`desired`; the verb now passes `desired` as an argument, so the decision is made once.

`batch` being a command makes `dispatch_single` → `dispatch_batch` → `run_batch` →
`dispatch_single` a cycle, which is the technical reason the copy existed: rustc cannot size a
future whose type contains itself. `dispatch_single` therefore returns
`Dispatched<'a> = Pin<Box<dyn Future<Output = Value> + Send + 'a>>` — an erased type breaks the
cycle where `Box::pin` on a named `async fn` future does not. `Send` is not decoration: the
clippy nursery enforces `future_not_send` crate-wide, and dropping it makes `run::run` non-`Send`.

`pipe::open_session` is the same story one level up: `run_pipe` and `run_replay` each kept their
own copy of the connect sequence its doc comment already claimed to be the only one of, and those
had drifted too (a different "no HTTP endpoint" message). Both now call it. Consequence, stated:
`replay`'s message for a browser with no HTTP endpoint is now the longer pipe one.

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
ever an assertion that did not hold. Without `--stop-on-error` the batch ran everything it was
asked to and exits 0 even when an entry failed; `ok` still carries that. The arm saves the session
before returning, because the error channel skips the save at the end of `run`.

`BatchStopped` prints nothing (unlike `assert::NotHeld`, which owns its own report): the arm has
already written the one response, and a second line would put two responses on stdout for one
invocation. That response is JSON only under `--json`, like every other CLI arm —
`run_helpers::batch_text_lines` renders one line per entry otherwise. Pipe mode is unaffected: it
is a JSON protocol and ignores `--json`.
