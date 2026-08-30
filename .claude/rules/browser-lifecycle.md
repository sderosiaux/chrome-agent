---
paths:
  - "src/browser.rs"
  - "src/session.rs"
  - "src/profiles.rs"
  - "src/orphans.rs"
  - "src/kill.rs"
  - "src/chrome_args.rs"
  - "src/connect_cli.rs"
  - "src/daemon.rs"
  - "tests/proxy_tests.rs"
  - "tests/profile_prune_tests.rs"
  - "tests/browser_timeout_tests.rs"
  - "tests/chrome_arg_tests.rs"
  - "tests/proxy_tests.rs"
---

# Launching, naming, reconnecting to and killing a browser

## `--chrome-arg`

`src/chrome_args.rs`. Passes extra flags to the Chrome chrome-agent launches. Repeatable,
`global = true`, so it parses on either side of the verb. No env var: nothing else in this
project is env-configured.

**Five flags are refused outright**, because chrome-agent depends on its own values to find and
reconnect to the browser it just launched:

| Flag | Why |
|---|---|
| `--user-data-dir` | `DevToolsActivePort` is read from the profile directory tracked per `--browser` name |
| `--remote-debugging-port` | chrome-agent launches with `=0` and reads the OS-assigned port back from that file |
| `--remote-debugging-pipe` | same; a pipe transport breaks the port read |
| `--proxy-server` | the dedicated flag already validates and persists it |
| `--headless` | the stored `headless` mode that detects a mismatch on reconnect would silently disagree with Chrome's actual mode |

Each refusal names the fact and the real value the caller passed, never a placeholder, and
suggests no rewrite — there is none for "don't send this flag" except dropping it.

Under `--connect` it is refused rather than silently ignored: that Chrome is already running and
reads no command line from this invocation. Mirrors the `--proxy-server` + `--connect` refusal.

Fixed for the life of a named browser, like `--proxy-server`. A follow-up command that omits it
inherits what the browser already runs with (`chrome_args::effective_chrome_args`). One naming
*different* flags is refused (`ensure_chrome_args_compatible`) rather than relaunched out from
under a session that may hold state — same tradeoff as `ensure_proxy_compatible`, and not the
auto-relaunch a headless/headed mismatch gets, because a proxy or a feature flag is a
security-relevant choice, not a display preference.

`src/connect_cli.rs` reads the resolved value for the CLI. `pipe.rs` keeps its own equivalent,
since it reads commands from stdin rather than dispatching on `cli.command`.

## `--copy-cookies`

Copies Cookies SQLite + Local State from the user's real Chrome profile, so logged-in sites
(X.com, Gmail) work without `--connect`. macOS Keychain decrypts the cookies.

## A profile directory is deleted only when three conditions agree

`src/profiles.rs`. A profile is created by `launch_browser` and removed only by `close --purge`,
so an agent that omits the flag or crashes leaves ~14 MB behind. Measured: `browsers/` held 1204
directories totalling 24.98 GB against 3 entries in the store.

"Delete every profile the store does not reference" is the wrong predicate — a concurrent agent
may have created its directory and not yet written its entry. A profile is removable only when:

1. No entry references it, read under the same exclusive `flock` the save path takes.
2. No artefact in it names a live holder. `SingletonLock` is a symlink whose target is `hostname-pid`, the only place a profile states its owner, resolved through `session::liveness`; `DevToolsActivePort` names a port, answered by asking whether anything still listens on it.
3. Nothing has touched it for 24 h. This window is what closes the create-then-write race without new coordination, so it is not optional: the launch path takes up to 10 s before the first save, and a day is ~8600x that.

Every condition fails towards keeping. An abandoned profile costs bytes; a deleted live one costs
whatever it was logged into. Validated against the real store: 333 of 1204 were removable
(5.56 GB), and it refused `test-tabs`, which had a four-day-old headless Chrome running in it and
no store entry.

Deliberately sacrificed: a named browser someone logged into by hand and expects to still be
logged in next week. The documented mechanisms do not work that way (`--copy-cookies` re-imports
on every fresh launch, `--connect` drives the real Chrome).

The sweep rides on the save path's existing lock, capped at 32 examinations and **one** removal
per invocation, so a read-only command never pays for the backlog (measured 0–80 ms).
`close --purge-orphans` applies the same predicate uncapped. `default` is exempt from the
automatic sweep — it is the profile every flagless invocation lands on.

`close --purge` used to claim the purge: it broke out of its retry loop on the first
`remove_dir_all` returning `Ok`, and a signalled-but-not-yet-exited Chrome wrote its state back
on the way down (measured: 235 files before the close, none immediately after, 22 a third of a
second later — 946 of those 1204 directories). The loop now ends when the directory is absent,
and a purge that never converges says so.

## A browser is not gone because the registry forgot it

`src/kill.rs`, `src/orphans.rs`. An entry leaves `sessions.json` for reasons unrelated to whether
the process is running: `close` removes it whether or not the kill landed, the relaunch path
removes it before spawning a replacement, the dead-entry prune drops it as soon as the pid reads
dead. Each leaves a Chrome that `status` cannot show and `close` cannot reach. Measured: two of
them 19 days old.

`orphans.rs` recognises them by the `--user-data-dir` they were launched with, which is true
independently of the registry. It rejects helper processes on `--type=` — Chrome hands the same
flag to every renderer, and matching the dir alone reported 39 browsers where 5 were running.
Matched by pid rather than name, so a relaunch leaves the previous process visible under a name
the registry still holds.

`kill_pid` used to return `()`, so `close` printed `Closed browser=…` in all three outcomes:
signalled, pid gone, pid reused. The reused case reached a user as `Closed browser=s9 (pid=80548)`
over a pid that by then belonged to `git fsmonitor--daemon`, which the guard had correctly left
alone. `KillOutcome` makes the act and the wording the same statement; `--json` carries
`signalled` (`ok` has always meant "the command ran").

## The spawn-to-persist window

`src/kill.rs`, `src/browser.rs`, `src/session.rs`. `cmd.spawn()` returns a live Chrome whose pid
is in memory only until `run.rs` reaches `save_session`, and in between sit two `?`
(`CdpClient::connect`, `resolve_page_target`) and every signal. `Child`'s drop does not kill it,
and the Ctrl+C handler reads the store, which does not know it yet. Measured A/B, cold start
interrupted at 250 ms, five runs each: HEAD leaked a running Chrome with no session entry 5/5,
the fix 0/5.

A pid is armed at spawn and disarmed **inside `save_session`** rather than at the call site: the
write that makes a browser reachable is the same event that ends the window, so a save path added
later inherits the discipline. `reap_unpersisted` runs on the two exits that would leak — the
interrupt handler and the error path out of `run` — and is a no-op once the pid is on disk, which
keeps the contract that a failed command leaves a usable browser behind. `SIGKILL` of
chrome-agent itself is still uncovered; `close --orphans` is the net.

## Every kill that precedes a relaunch waits for the exit it claims

`browser::kill_and_await_exit`. SIGTERM returns before Chrome exits and the dying instance keeps
answering `/json/version`, so a kill followed by any command on the same name reconnected to the
corpse through its stale `DevToolsActivePort` and failed the WebSocket handshake.

One helper for all of it: guarded kill through `kill::kill_pid` (liveness, then
`is_browser_process`, so a recycled pid is never signalled), then `kill::wait_until_gone`
(bounded, 5 s), then removal of the port file at `browsers_dir()/<name>/chromium-profile/`.
`close` uses it, and so do the headless/headed relaunch paths in both front ends — those killed
and relaunched immediately, which is how a store entry came to hold a dead ws endpoint with
`pid: None`. `close` on timeout says `still shutting down` instead of `Closed`; `--json` carries
`exited` beside `signalled`, only on the signalled case.

The path matters: `cmd_close` removed `browsers_dir()/<name>/DevToolsActivePort`, one directory
above the file Chrome writes, so the documented removal had never removed anything.

## `--headed` is read as intent, on both front ends

`connect_cli::want_headless`. `--headed` is a bare `SetTrue` flag, so `cli.headed == true` is
already "written on this command line": an explicit `--headed` overrules the stored mode and
relaunches, an omitted one lets the stored mode win. The CLI used to let the stored mode win in
both cases, which made the relaunch branch unreachable and dropped the flag in silence. The
converse regression is the one cf1e8a8 fixed and it stays fixed — a plain command must not kill
an existing headed browser.

Pipe still reads an omitted `--headed` as "I want headless" and kills a headed browser. That is
deliberate for now: with no `--headless` flag on the CLI, pipe's omission is the only way to move
a named browser back to headless. Adding `--headless` is what would let both sides read intent
symmetrically.

## Parallel agent isolation

`--browser <name>` per agent. Saves are parallel-safe via an exclusive `flock` on `sessions.lock`
plus read-merge-write: each save re-reads the on-disk store under the lock, deletes only the
browsers this process dropped since load, upserts its own, then atomically renames a per-PID temp
file into place.

## The store prunes dead browsers, one-sidedly

`session::prune_dead`, inside the existing read-merge-write. Nothing used to remove an entry
whose Chrome had exited, and each entry carries a `uid_map` and a `last_snapshot` per page.
Measured: 5,212,694 bytes, 2131 entries, 2123 naming pids the kernel reports as gone — parsed
*and* rewritten by every invocation, including read-only ones. After the prune: 7,827 bytes, 8
entries; `text --selector body` on a warm browser went from 0.38 s to under 0.01 s, and
`goto_settle_tests` stopped blowing its 2 s guard.

The predicate is not "is this browser reachable". An entry is dropped only when it carries a pid
AND `kill(pid, 0)` answers `ESRCH`. Everything else is kept, because a stale entry costs bytes
while a wrongly deleted one costs the caller its browser:

- `pid: None` is kept without a probe. Both `--connect` and a managed reconnect through `DevToolsActivePort` store no pid, so the absence carries no liveness information, and an HTTP probe per entry would put a round trip on every save.
- `EPERM` is kept: the process exists under another uid.
- A pid outside `pid_t` is kept: `kill` would read it as a process group.
- A recycled pid reads as alive under a name Chrome no longer holds, keeping a stale entry. Accepted, and the reason `is_process_alive` is now `liveness(pid) != Dead` rather than `== 0`.
- Non-Unix has no probe wired, so nothing is provably dead and the store grows as before.

It cannot delete another agent's live entry: the prune runs on the map re-read from disk *inside*
the exclusive lock and tests the pid about to be written. It runs after the upsert, so it also
covers a browser that died mid-command, and the dropped names are removed from the caller's
in-memory store or the next save's "untouched and absent from disk" branch would republish them.
No manual `prune` command: it could only reach the cases the predicate deliberately refuses to
judge.

## `connect_page` retry

Page-level CDP connection retries, up to 8 attempts, 500 ms / 300 ms backoff between tries.
