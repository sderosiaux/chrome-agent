# chrome-agent v0.16.0

Single Rust binary for browser automation via CDP, built for AI agents. 43 subcommands
(`chrome-agent --help`), 29.0K lines of Rust in `src/` across 84 files (measure: `tokei src/`),
one regex crate (`regex-lite`, for `assert --matches`), 3 MB binary.

## Architecture

```
CLI (clap) → CDP Client (WebSocket) → Chrome
```

| Module | Role |
|--------|------|
| `src/cli.rs`, `src/cli_actions.rs` | clap definition (`Cli`, `Command`, one variant per verb) and the per-verb subcommand enums (`MacroAction`, `EmulateAction`, `AssertWhat`, `WebmcpAction`, `DaemonAction`) |
| `src/main.rs`, `src/run.rs`, `src/run_helpers.rs`, `src/connect_cli.rs` | the binary entry point; CLI dispatch on `Command` — each arm builds typed args from clap, calls the SAME `pipe_dispatch::dispatch_*` pipe and batch call, and renders the answer; shared output/error handling and `connect_page` (8-attempt retry); resolving one invocation's browser + page connection |
| `src/page_ctx.rs` | `PageCtx`: the two clients, the store, the three names that locate a page in it, and the two global flags, in one struct — so a dispatcher takes three parameters instead of eleven |
| `src/render.rs` | text-mode renderer: the `value:` / `values lost:` / `verdict:` / `next:` lines, colour only on a tty |
| `src/pipe_command.rs` | the pipe/batch protocol as types: one `deny_unknown_fields` struct per verb, so a mistyped key is an error instead of a silently ignored one |
| `src/pipe.rs`, `src/pipe_command.rs`, `src/pipe_validate.rs`, `src/pipe_dispatch*.rs`, `src/pipe_report.rs`, `src/pipe_emulation.rs` | pipe mode (persistent connection, JSON stdin/stdout); typed per-verb protocol plus cross-field validation; the dispatchers shared by pipe, batch and CLI batch, plus `dispatch_assert` and `run_batch`; `mutates_page` + `attach_change_report`; strict JSON emulation parsing |
| `src/macros*.rs` | macro file format and store (`macros.rs`), session history → macro (`macros_record.rs`), guarded execution (`macros_run.rs`), the `macro list/show/record/run` surface (`macros_cmd.rs`) |
| `src/cdp/`, `src/setup.rs` | WebSocket transport, message correlation, CDP types, the input-event deadline (`send_input`), `ensure_foreground`; console interceptor injection and 7 stealth patches |
| `src/element.rs` | uid resolution, fill, type, press, the settle machinery, `js_exception`. Owns `SECRET_FIELD`, the one predicate deciding whether a value may be printed |
| `src/element_pointer.rs` | the pointer path: `PointerVerb`, `aim_and_dispatch` and the one aim rule both verbs share, native mouse/touch dispatch, the JS fallback when there is no box to aim at, `hover`. Split from element.rs for the 1000-line cap, re-exported via `pub use` — it is the half of `element` that moves when `hit_test` moves |
| `src/element_controls.rs` | select, check/uncheck, upload, drag. Owns `CHECKABLE_PROBE` and `SELECT_READ`, which `assert` reads through |
| `src/element_selector.rs`, `src/element_ref.rs`, `src/read_back.rs` | CSS-selector actions (click/dblclick/fill/focus); the `ElementRef` abstraction over CDP node identity; the `value:{requested,actual,verbatim}` object every read-back verb puts on its response |
| `src/hit_test.rs`, `src/hit_test_report.rs`, `src/geometry.rs` | where a mouse event will land (probe, settle loop, `Delivery` classifier, `--on-intercept`) and what the response says about it (`Dispatched`, `Unaimable`, `Refused`, carrying `intercepted_by`/`verdict`/`next`); box-model → screenshot clip math |
| `src/snapshot.rs`, `src/snapshot_render.rs`, `src/snapshot_secret.rs` | `take_snapshot` and `take_views` over `Accessibility.getFullAXTree`; the pure renderer (compact text, stable uids, role filter, depth limit, subtree focus); redaction of secret field values (`MARKER` = `<redacted>`), decided by asking the page since secret-ness is a property of the element |
| `src/verdict.rs`, `src/verdict_evidence.rs`, `src/verdict_words.rs` | pure classifier producing `verdict` + `verdict_reason` + hint; the evidence it reads (`Delivery`, `Postcondition`, each with a no-evidence floor variant); the `gloss`/`next_for`/`hint_for` tables |
| `src/landing.rs`, `src/serving.rs` | where a navigation ended up vs where it aimed (redirect rule, auth-wall guess, `host_and_path`/`origin_of`); and what answered: `challenge` / `error` / `nothing_actionable` / `unreadable` / `page` |
| `src/hints/` | error-recovery hints: one fact, one resolved command, an explicit refusal to retry when a retry is dangerous |
| `src/browser.rs`, `src/chrome_args.rs`, `src/daemon.rs` | Chrome launch, auto-discovery, stale DevToolsActivePort cleanup, profile management; `--chrome-arg` validation and the inherit-when-omitted merge; optional Unix micro-daemon with heartbeat and crash recovery |
| `src/session.rs`, `src/session_load.rs`, `src/session_save.rs`, `src/secure_fs.rs`, `src/profiles.rs`, `src/orphans.rs`, `src/kill.rs` | JSON session persistence (`~/.chrome-agent/sessions.json`, 0600) split by direction; shared 0600/0700 enforcement for files and directories; flock + read-merge-write and one-sided dead-pid prune. Plus the three-condition orphan-profile predicate; running browsers no session entry claims, recognised by `--user-data-dir`; `KillOutcome` |
| `src/emulation.rs` | page-scoped device metrics: validation, transactional CDP apply/reset, persistence, observed status |
| `src/base64.rs`, `src/truncate.rs` | RFC 4648 decoder for screenshot/pdf/download (no `base64` crate, keeps the musl graph pure-Rust); UTF-8 safe string truncation |
| `src/commands/` | 25 modules: goto, click, dblclick, fill, inspect, eval, text, read, extract, diff, network, console, wait, screenshot, pdf, download, download_click, tabs, frame, batch, assert, assert_args, record, history, webmcp. `select`/`check`/`upload`/`drag` had one too, each a single call into `element_controls` behind a message the dispatcher already built |
| `src/commands/assert.rs`, `assert_args.rs` | comparators (pure), page readers, the `NotHeld` carrier for exit 2; CLI/JSON → `Assertion` and the argument combinations it refuses |
| `src/commands/download_fetch.rs` | the download a URL produces: the base64 fetch through `Runtime.evaluate`, `MAX_FETCH_BYTES` derived from the transport ceiling with a `const` assertion tying them, and the filename derivation both paths share. Split from download.rs for the 1000-line cap, re-exported via `pub use` |
| `src/commands/download_click.rs` | the download a click produces: `Browser.setDownloadBehavior` arming, a subscription taken before anything can fire, bounded wait on `downloadWillBegin`/`downloadProgress`, `--max-bytes` cancel, 0600 move out of a private per-invocation directory, `collect_abandoned` deferred sweep |
| `src/commands/webmcp.rs` | `document.modelContext.getTools()`/`.executeTool()`; owns the thrown marker strings `src/hints/` matches on |
| `vendor/`, `npm/`, `skills/chrome-agent/SKILL.md` | Mozilla Readability (90 KB, MIT) and the MDR/DEPTA-inspired `extract.js`, both via `include_str!` and tested under jsdom; the npm wrapper whose postinstall downloads the native binary; the agent skill file |

## Build & Test

```bash
cargo build
cargo test
cargo clippy -- -D warnings  # zero warnings enforced in CI
```

`cargo test --test X` does not always rebuild the binary the tests exec. The integration suites run `target/debug/chrome-agent` as a subprocess and cargo may not refresh it for a single-suite run, so an A/B that edits `src/` and re-runs one suite can measure the previous build. Run `cargo build` between the two states.

**Every test owns its browser and its files.** Use `common::TestBrowser::new(label)` for a browser and `common::temp_path` for a file; both build names through `common::unique_name`, which carries the pid (separating concurrent `cargo test` processes) and a counter (separating tests on parallel threads inside one process). `TestBrowser` closes and purges on `Drop`, which a trailing `close` does not do when an assertion panics. A test may name, write and assert on what it owns and on nothing else — an assertion over a shared directory (`~/.chrome-agent/tmp/.incoming-*`) must filter on both the pid and the `TestBrowser` name. Three tests in `harness_tests.rs` scan the sources and fail on a hard-coded browser name, a second implementation of the rule, or a fixed temp path; a line that genuinely needs one says `isolation-exempt:` and why.

## Release

```bash
./scripts/release.sh 0.3.0
# → bumps Cargo.toml + npm/package.json, commits, tags, pushes
# → GitHub Actions: builds 5 platform binaries, creates release, publishes npm
# → Requires NPM_TOKEN in GitHub secrets
```

## Path-scoped rules

Facts true of one subsystem live in `.claude/rules/`, loaded when you `Read` a file the rule's `paths:` block matches. Each rule file's own `paths:` block is authoritative; the table below indexes it, and every rule also matches its own test suites.

| Rule file (`.claude/rules/`) | Loads on | Holds |
|---|---|---|
| `hit-test.md` | `src/hit_test*.rs`, `src/element_selector.rs` | the pre-dispatch probe, `--on-intercept` and its three modes, single-handle selector resolution |
| `verdict-and-diff.md` | `src/verdict*.rs`, `src/commands/diff.rs`, `src/pipe_report.rs`, `src/render.rs` | the verdict vocabulary, the two-group ladder, `next`, `values_lost`, focus and diff hygiene, document identity, the text renderer |
| `snapshot-and-inspect.md` | `src/snapshot*.rs`, `src/commands/inspect.rs`, `src/commands/diff.rs`, `src/session.rs`, `src/page_ctx.rs` | a display flag never narrows the baseline, `store_snapshot` as the one writer of it, secret-value redaction, noise filtering, `--urls`, output paging |
| `element-actions.md` | `src/element*.rs`, `src/read_back.rs`, `src/commands/{click,dblclick,fill}.rs` | the 60 ms observation window, what `fill`/`select`/`check` report back, `--secret`, `press`, the settle wait |
| `browser-lifecycle.md` | `src/browser.rs`, `src/session.rs`, `src/profiles.rs`, `src/orphans.rs`, `src/kill.rs`, `src/chrome_args.rs`, `src/connect_cli.rs`, `src/daemon.rs` | `--chrome-arg`, the three-condition profile predicate, orphan browsers, the spawn-to-persist window, the store's one-sided prune, `close`'s wait |
| `cli-parsing.md` | `src/cli.rs`, `src/cli_actions.rs`, `src/run.rs`, `src/run_helpers.rs` | a global flag parses on either side of the verb; an invocation is judged before a browser is; `run::run` renders a dispatcher's answer rather than reimplementing it, and the six verbs that could not |
| `hints.md` | `src/hints/**` | the three rules every error message holds to, and the navigation-failure branches |
| `macros.md` | `src/macros*.rs` | what becomes a guard and what is refused, locators, where a task begins, why the file is JSON |
| `navigation-and-serving.md` | `src/landing.rs`, `src/serving.rs`, `src/commands/goto.rs` | `landed`, the redirect rule, the five `serving` tokens and their thresholds, `--header`, `forward` |
| `files-on-disk.md` | `src/commands/download*.rs`, `src/commands/screenshot.rs`, `src/commands/pdf.rs`, `src/geometry.rs`, `src/base64.rs` | the click that produces a download, the deferred sweep, `downloaded` vs `ok`, clip math, 0600 |
| `cdp-transport.md` | `src/cdp/**`, `src/setup.rs` | every CDP call has a deadline, the input-event deadline, foreground for pointer events, `waited_ms`, dialogs, the seven stealth patches |
| `emulation.md` | `src/emulation.rs`, `src/pipe_emulation.rs` | Chrome keeps no override, so the store is the mechanism |
| `assert.md` | `src/commands/assert.rs`, `src/commands/assert_args.rs` | exit 0/1/2, reading through the action's own reader, `--matches` being a Rust regex |
| `content-extraction.md` | `src/commands/{read,extract,text,eval,network,console,wait}.rs`, `vendor/extract.js` | reader mode, the extraction heuristics, network and console capture, `--scroll`, `network-idle` |
| `webmcp.md` | `src/commands/webmcp.rs` | why a tool's declared result gets no new verdict word |
| `pipe-and-batch.md` | `src/pipe.rs`, `src/pipe_command.rs`, `src/pipe_dispatch*.rs`, `src/commands/batch.rs`, `src/commands/record.rs`, `src/macros_cmd.rs` | the typed protocol and what still takes a raw `Value`, one `history_step` behind `back`/`forward`, recordings are 0600 and refuse when unwritable, `stop_on_error`, a failed read is not a failed action |
| `frames.md` | `src/commands/frame.rs` | how an iframe is resolved and what the isolated world does not see |

The trigger is the `Read` tool; `rg`, `sed` and `wc` through Bash do not arm a path-scoped rule. Run `/context` to see what loaded. Longer records that no glob loads are indexed in `docs/design/README.md`.

## Conventions that hold everywhere

- **Headless by default** — `--headed` for debug. A mode mismatch auto-kills the old browser.
- **Static Linux binaries** — musl targets (`x86_64`/`aarch64-unknown-linux-musl`) via `cargo-zigbuild`, zero glibc dependency (fixes #3). This needs a pure-Rust dep graph: `ureq` runs with `default-features = false` (TLS off, it only hits Chrome's local `http://127.0.0.1`), dropping `ring`/`rustls`. CI guards the graph against C-linking crates.
- **`--connect` for heavy protection** — DataDome/Kasada detect bundled Chromium fingerprints. Connect to a real installed Chrome (`--connect http://127.0.0.1:9222`).
- **Stable UIDs** — `n{backendNodeId}`, not sequential `e1, e2`. They survive between inspects on the same page and change after SPA navigation.
- **3 targeting modes** — uid (from inspect), CSS selector (`--selector`), coordinates (`--xy`). Exactly one per invocation, declared as a clap `ArgGroup`, so two of them is a usage error before a browser is opened.
- **ElementRef abstraction** — the session stores `{"type":"backendNode","id":N}`, ready for BiDi.
- **`--json` mode** — errors exit 1 with `{"ok":false}` on stdout. Parse stdout for the error; the exit code only signals failure. The exception is a malformed invocation: clap answers that on stderr, with nothing on stdout.
- **Stdout delivery** — production output uses `out_line!`/`out!`, backed by one non-panicking writer. A closed consumer records a delivery failure, lets command/session cleanup finish, then exits 1; a source scanner forbids falling back to the standard print macros.
- **Content extraction hierarchy** — `read` (articles) > `extract` (repeating data) > `text --selector` (scoped) > `text` (full page) > `eval` (structured JS) > `network` (API responses).
- **Pipe mode** — `chrome-agent pipe` reads JSON from stdin and writes JSON to stdout. One connection, uids stable across the sequence — which is the reason to use it. It removes ~12 ms of per-command overhead and nothing else: measured 1.5x on a nine-command read stream, 1.1x on fills and clicks. A startup/final persistence failure is a terminal `{"ok":false,"terminal":true,...}` line and exit 1, not a command response. Record latency in `docs/design/pipe-latency.md`, re-measure with `./scripts/measure-pipe.sh`.
- **Command aliases** — navigate/open/go, snap/snapshot/tree, js/execute, capture, tap.
- **A source file stays under 1000 lines.** Measure with `find src -name '*.rs' | xargs wc -l | sort -rn | head`. A file that outgrows the cap is split and the new module re-exported via `pub use`, so no call site moves. Enforcement is a local PostToolUse hook (`~/.claude/hooks/file-size-guard.sh`) blocking a `Write`/`Edit` over the cap, not a CI gate, so any route that is not an edit can exceed it.

## Gotchas

- `assert` needs a browser like every other command, and `assert value|state --uid` needs a uid from a *stored* snapshot: `goto` clears the map, so inspect before asserting by uid. The `uid` an action echoes back is resolved live and is not enough on its own.
- Exit codes: `0` success, `1` error (including a bad flag — clap's usage exit moved off `2`), `2` a claim this tool made did not hold, `130` Ctrl+C. Two commands can return `2`: `assert`, and `macro run` when a guard was checked and the page disagreed (`stopped_by: "guard"`). A `macro run` that stopped for any other reason — the step never ran, a guard could not be evaluated, the macro file is unreadable — is `1` like everything else.
- `landed.serving` never changes `ok` or the exit code: a 403, a WAF refusal and a captcha are facts about the page, and the navigation still happened. Branch on `serving`, not on `ok`. `serving: "page"` is the absence of contradicting evidence, not a guarantee — a paywall, a cookie wall and a soft 404 all reach it.
- `serving` reads the document as it was when `goto`'s settle probe stopped. On a site whose first paint is empty (measured: `www.amazon.fr`, 1 run in 3) that reads `nothing_actionable`; `inspect` is the answer and the hint says so.
- `batch`/`pipe` have no exit code per command: a failed assertion is `ok:false` with an `assertion` object. The CLI `batch` process exits `1` when `--stop-on-error` cut the run short (`stopped_at` is set) — `1` and never `2`, because the process is reporting that the batch stopped, not making a claim about the page. Without `--stop-on-error` it ran everything it was asked to and exits `0` even when an entry failed: read `ok`. The response is printed once either way, and only `--json` makes it JSON — text mode gets one line per entry.
- CDP `rename_all = "camelCase"` fails on acronyms: use `#[serde(rename = "backendDOMNodeId")]`.
- Browser-level WebSocket only supports `Target.*`; page commands need the page WS from `/json/list`. `Accessibility.getFullAXTree` returns a flat list with parentId/childIds, not a tree.
- A CDP field this tool does not read is not declared in `src/cdp/types.rs` at all; serde ignores what it was not told about. A declared field that is not `Option<T>` + `#[serde(default)]` is one Chrome MUST send. Declare what something reads, and put the rest in the type's doc comment.
- `text --selector "main"` auto-falls back to `[role=main]` for ARIA compatibility. Readability.js can fail on non-article pages; it is wrapped in try-catch with a descriptive error.
- `--stealth` patches are CDP-level (`Page.addScriptToEvaluateOnNewDocument`), not Chrome flags. `--disable-blink-features=AutomationControlled` is a myth.
- DataDome/Kasada: `--stealth` is NOT enough. Use `--connect` to a real installed Chrome.
- `Runtime.evaluate` works WITHOUT `Runtime.enable`. Stealth mode skips it to avoid detection.
- After SPA navigation (a click that changes route) uids change — always re-inspect. For SPA product/detail pages prefer `goto <direct-url>` over `click <link-uid>`.
- `back`/`forward` are one `history_step` with a sign, in all three modes. Both read the boundary from `Page.getNavigationHistory` FIRST, so a step with nowhere to go is `{"ok":true,"title":"","message":"Already at first|last history entry"}` with no `url` — instantly, rather than after five seconds of waiting for a load event that cannot come. Both clear `uid_map` and keep `last_snapshot`, exactly as `goto` does.
- `history.back()` in pipe mode kills the WebSocket. Use `Page.navigateToHistoryEntry` instead.
- Parallel agents sharing `--browser default` corrupt each other's sessions. Use `--browser <unique>`.
- The console interceptor is guarded against re-injection (`__chrome-agent_console_installed`).
- `press Enter` needs `windowsVirtualKeyCode: 13` + `text: "\r"` for form submission.
- A pointer action brings its page to the foreground (once per connection); a keyboard action does not. On a browser with several pages this is a tab switch, and it is deliberate — a background page answers pointer events on a five-second timer.
- `goto`'s settle probe is bounded at both ends: the quiet window starts immediately (a static page pays no flat 3s) and a mutation never clears the ceiling, since `awaitPromise` has no deadline and `--timeout` does not reach it.
- Anything that both SHOWS a tree and STORES one goes through `snapshot::take_views`, never `take_snapshot` plus a narrowing flag. `take_snapshot` is for callers that only read (`diff`'s live side, `extract --a11y`).
- `goto` clears `uid_map` but keeps `last_snapshot`. Clearing the map stops a stale uid resolving to an unrelated node; keeping the snapshot lets `diff` report `document_changed` instead of erroring. The clear lives in `dispatch_goto`, so pipe and batch get it too — it used to be in the CLI arm only.
- Session saves only rewrite browsers this process actually modified. `load_from` reads every browser in the file, so writing them all back makes a reading agent clobber a concurrent agent's `uid_map`/`last_snapshot`.
- `drag` uses CDP mouse events (mousePressed/mouseMoved/mouseReleased). Works with mousedown-based DnD libs (Sortable.js, React DnD mouse backend). Does NOT work with the HTML5 Drag and Drop API, which needs dragstart/dragover/drop.
- `frame` only supports `<iframe>`, not legacy `<frameset>`/`<frame>`.
- `frame` binding lives on the connection, so it persists only within a single `pipe`/`batch` process. CLI single commands each open a fresh connection — use pipe mode for `frame → inspect → act` (issue #8).
- `frame <selector>` resolves the iframe *inside the currently bound frame*, so nesting works (`frame #outer` → `frame #inner`). To switch to a top-level sibling, reset with `frame main` first.
- `frame` scopes `eval` and `inspect`; it does NOT scope selector-based targeting (`click`/`fill --selector` still query the top document). Inspect after the switch to get iframe uids, then act by uid — backendNodeId is page-global and works cross-frame.
- `frame` uses an isolated world: `eval` sees the frame's DOM and `location` but NOT its main-world JS variables. `document.querySelector("iframe")` matches the *first* iframe in DOM order, which on ad-heavy pages is often an `about:blank` slot; pass a precise selector (e.g. `iframe[src*="…"]`).
- `webmcp list`/`call` under a `frame` binding hit the same isolated-world blindness as `eval`: a polyfilled `document.modelContext` reads as `undefined` even when the frame really has tools (measured, `tests/fixtures/webmcp_iframe_host.html`). The response carries `frame_scoped: true`, so an empty result means "unproven", not "none". Most Chrome installs have no native WebMCP — launch with `--chrome-arg --enable-features=WebMCP,WebMCPTesting`, or rely on a page's own polyfill.
- `batch` CLI mode: a new CDP connection means new backendNodeIds, so uids change between invocations. Use pipe mode for uid-stable multi-command flows.
- `select` on a non-`<select>` element throws "Element is not a \<select\>". Custom dropdowns (React, MUI) need a click + click approach.
- `network --abort` is blocking: it intercepts requests for `--live N` seconds, then disables the Fetch domain. Start it before navigating.
- `network --live` uses a Network-only event channel. Event loss keeps observed entries but answers `ok:false`, `complete:false`, `lostEvents:N`; `--limit` gives selected bodies 1 s to finish plus one bounded 1 s read window, never the rest of `--live`.
- `download` takes exactly one target: a URL, `--uid`, or `--selector`. Two is refused rather than ranked, by clap, before a browser is opened — as on every other verb that can be aimed more than one way. The URL path holds the file in memory as base64 during transfer (hence `--max-bytes`); the click path streams to disk through Chrome and is capped by cancelling the transfer.
- `download --uid` needs a uid from a *stored* snapshot: `goto` clears the map, so `inspect` first.
- A click-triggered download and the page's own reaction are not separable: `download --selector` clicks, and if the element navigates instead of downloading, that navigation happened. The response says no download began; it does not say the page is where it was.
- JS dialogs auto-accept by default. `beforeunload` under `accept` means "proceed". Use `--dialog manual` for the old blocking behaviour. The handler logs to stderr, never stdout, so `--json` stays clean.
- `wait network-idle` takes an empty pattern; other wait types still require one. In pipe mode use `{"cmd":"wait","what":"network-idle"}`.
- `Page.captureScreenshot` `clip.scale` downsamples without any image crate, which keeps the musl dep graph pure-Rust (issue #3). Element clip uses the border-box bounds of `DOM.getBoxModel`.

## Linting

Zero warnings enforced. Clippy pedantic + nursery with targeted suppressions in Cargo.toml.
CI runs `cargo clippy -- -D warnings`. Any warning is a build failure.
