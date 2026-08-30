---
paths:
  - "src/pipe.rs"
  - "src/pipe_dispatch.rs"
  - "src/pipe_dispatch_actions.rs"
  - "src/commands/batch.rs"
  - "src/commands/record.rs"
  - "src/macros_cmd.rs"
  - "tests/mode_parity_tests.rs"
  - "tests/record_perms_tests.rs"
  - "tests/read_failure_tests.rs"
---

# One connection, many commands — pipe, batch, and recordings

Moved out of `CLAUDE.md`'s **Key Design Decisions** — not rewritten and not summarised. The
words are the ones that were there, minus the factual corrections made in the same change (a
path that had stopped resolving, a count that had gone stale). What changed is *when* they
load: this file is pulled in when you read a file its `paths:` block names, and costs nothing
in a session that touches none of them.

- **A failed read is not a failed action** — the post-action page read is best effort in all three modes. The CLI used to propagate it with `?`, so a click that had already been delivered returned `ok:false`, and the natural response to that is to click again — which is real. `pipe_dispatch` stated the opposite policy in a comment and followed it; the CLI now matches. The response is `ok:true` with `verdict: unknown / read_failed`. Reproduced with `tests/fixtures/blocks_after_click.html`, which pins the main thread after the click returns so CDP cannot answer within a short `--timeout`.
- **Recordings are 0600** (`commands::record::restrict`) — a recording holds every command and response of the session, including the values a fill put into the page, among them the ones redacted on stdout precisely because they are secrets. It was created with whatever the umask allowed (typically 0644) while screenshot/pdf/download/session all chmod 0600. Applied on every write, not only at creation: the file may already exist, wider, from an earlier run.
- **A recording that cannot be written refuses the command** (`pipe.rs`) — `start_recording` and `log_entry` errors were both discarded with `let _ =`, so an unwritable `_record` path produced `ok:true` responses indistinguishable from a session actually being recorded, and the agent found out at `replay` time that there was nothing to replay. A failure to *open* now refuses the command before it runs: the caller asked for a recorded action, and running it unrecorded is not that. Consequence, deliberate: a bad path stops the session's work, not just its log — but per command and loudly, so it surfaces on the first line rather than at the end. A failure to *append* rides on the response as `recording_error` instead, because that command already ran and failing it invites a retry of real work.
- **`batch` gained opt-in `stop_on_error`** (`pipe_dispatch_actions::run_batch`) — `--stop-on-error` on the CLI, `"stop_on_error": true` in pipe mode, default unchanged (every command runs). The response adds `stopped_at` (index) and `skipped` (count) rather than leaving the caller to infer a short array. Both front ends now share one loop: they kept a copy each, so the flag would otherwise have been implemented twice and kept in step by hand.
- **`batch`** — CLI reads JSON array from stdin, dispatches sequentially via `pipe_dispatch::dispatch_single`. Pipe mode uses `"commands"` array field.
