# chrome-agent v0.14.0

Single Rust binary for browser automation via CDP. Built for AI agents.
At v0.14.0: 22.4K lines of Rust code, 26.1K physical (`tokei src/`). A size with no metric and no
version beside it rots silently — this line read `~13.3K` for three releases while the code grew past
22K, which is the shape of error the rest of this file exists to refuse. One dependency-free regex
crate (`regex-lite`, for `assert --matches`), 3 MB binary.

## Architecture

```
CLI (clap) → CDP Client (WebSocket) → Chrome
```

| Module | Role |
|--------|------|
| `src/cli.rs` | CLI definition: `Cli` struct, `Command` enum — one variant per verb, `assert` with its own `AssertWhat` subcommand (count them with `rg -c '^    [A-Z][A-Za-z]+( \{\|,\|$)' src/cli.rs`, the enum being the only 4-space block in the file shaped that way; a number written here goes stale the next time a verb lands, and this one did — it said 40 while `webmcp` and `macro` landed) |
| `src/cli_actions.rs` | The per-verb modes (`MacroAction`, `EmulateAction`, `AssertWhat`, `WebmcpAction`, `DaemonAction`) — split from cli.rs for the 1000-line cap, re-exported via `pub use` |
| `src/macros.rs` | The macro file: format, whitelist of guards (`deny_unknown_fields`), parameters and secrets, `{{var}}` substitution, the store under `~/.chrome-agent/macros` |
| `src/macros_record.rs` | Distillation: a session's history → a macro. The whitelist and the blacklist, the locator rules, and the refusals. Pure, no Chrome |
| `src/macros_run.rs` | Running one: the same dispatcher as pipe, one guard check per step, and a full stop at the first that does not hold |
| `src/macros_cmd.rs` | The `macro list/show/record/run` surface, CLI and pipe |
| `src/run.rs` | CLI command dispatch (match on Command enum) |
| `src/pipe.rs` | Pipe mode: persistent connection, JSON stdin/stdout |
| `src/pipe_dispatch.rs` | Pipe/batch command dispatchers (shared by pipe + batch + CLI batch) |
| `src/pipe_emulation.rs` | Strict JSON device-emulation parsing, recovery handling, and immediate pipe persistence |
| `src/cdp/` | WebSocket transport, message correlation, CDP types, the input-event deadline (`send_input`) and `ensure_foreground` |
| `src/commands/` | 29 command modules: goto, click, fill, inspect, eval, text, read, extract, diff, network, console, wait, screenshot, pdf, download, download_click, tabs, dblclick, select, check, upload, drag, frame, batch, assert, webmcp... |
| `src/element.rs` | uid/coordinate resolution → CDP input dispatch, click, fill, type, press, hover, dblclick |
| `src/element_controls.rs` | select, check/uncheck, upload, drag — split from element.rs for the 1000-line cap, re-exported via `pub use`. Owns the shared readers `CHECKABLE_PROBE` and `SELECT_READ` that `assert` reads through |
| `src/element.rs` (cont.) | also owns `SECRET_FIELD`, the one predicate deciding whether a value may be printed — was copy-pasted in four places, and it gates what reaches stdout, a transcript and any recording |
| `src/element_selector.rs` | CSS-selector actions (click/dblclick/fill/focus) — split from element.rs for the 1000-line cap, re-exported via `pub use` |
| `src/read_back.rs` | The `value:{requested,actual,verbatim}` object every read-back verb (fill, bulk fill, select, check/uncheck) puts on its response — split from run_helpers.rs for the 1000-line cap, re-exported via `pub use`. One key, because `pipe_report::postcondition_from_response` reads exactly one |
| `src/landing.rs` | Where a navigation ended up vs where it was aimed: the redirect rule, the auth-wall guess, and the two URL splitters (`host_and_path`, `origin_of`) `serving` and `hints` read through |
| `src/serving.rs` | What ANSWERED, as opposed to where it landed: the `serving` token, the anti-bot vendor origins, the false-positive guards and their measurements. Pure — `Landing` wires it into all three modes |
| `src/geometry.rs` | box-model → screenshot clip math (quad bounds, downscale factor), uid/selector clip resolution |
| `src/hit_test.rs` | Where a mouse event will land: the one-shot probe, the settle loop, the `Delivery` classifier over (`Probe`, `Settle`), `--on-intercept`, single-handle selector resolution |
| `src/hit_test_report.rs` | What the response says about all that: `Dispatched`, its `report()`/`refusal_message()`, `Unaimable`, and `Refused` — the refusal that travels through the error channel carrying `intercepted_by`/`verdict`/`next`. Split from hit_test.rs for the 1000-line cap, re-exported via `pub use` |
| `src/commands/download_click.rs` | The download a CLICK produces: `Browser.setDownloadBehavior` arming, the subscription taken before anything can fire, the bounded wait on `downloadWillBegin`/`downloadProgress`, the `--max-bytes` cancel, the 0600 move out of a private per-invocation directory, and `collect_abandoned` — the deferred collection of transfer directories whose process has exited, which is what converges where no sweep budget can |
| `src/base64.rs` | shared RFC 4648 decoder (screenshot/pdf/download) — no `base64` crate, keeps musl graph pure-Rust |
| `src/element_ref.rs` | ElementRef abstraction (decouples from CDP internals) |
| `src/snapshot.rs` | Reading the accessibility tree: `take_snapshot`, and `take_views` — one `getFullAXTree` rendered twice, the full pass persisted as the diff baseline and the caller's `--filter`/`--max-depth`/`--uid` pass printed |
| `src/snapshot_render.rs` | The pure half of the above: `AXNode` list → compact text with stable uids (backendNodeId), role filter + aliases, depth limit, subtree focus. Split from snapshot.rs for the 1000-line cap. Being pure is what lets one reading be rendered twice |
| `src/commands/assert.rs` | `assert` comparators (pure), page readers, the `NotHeld` carrier for exit 2 |
| `src/commands/assert_args.rs` | CLI/JSON → `Assertion`, and the argument combinations it refuses — split from assert.rs for the 1000-line cap |
| `src/pipe_dispatch_actions.rs` | composite/form-control dispatchers, `dispatch_assert`, and `run_batch` (the one batch loop, shared by CLI batch and pipe batch) |
| `src/verdict.rs` | What an action may claim about itself: pure classifier, `verdict` + `verdict_reason` + hint |
| `src/hints/` | Error-recovery hints — one fact, one resolved command, an explicit refusal to retry when a retry is dangerous. `mod.rs` holds the contract, the shared constructors and the corpus that scans `element.rs` for a branch nothing tests; `navigation.rs` the failures that happen before a document exists; `element.rs` the `error_hint` chain plus the six hints a SUCCESSFUL click carries. Split from run_helpers.rs, then from itself, for the 1000-line cap; re-exported via `pub use` so no call site moved |
| `src/pipe_report.rs` | `mutates_page` + `attach_change_report` — split from pipe_dispatch.rs for the 1000-line cap, re-exported via `pub use` |
| `src/commands/webmcp.rs` | `document.modelContext.getTools()`/`.executeTool()` — tool discovery, name→`RegisteredTool` resolution, JSON-string argument validation, `frame`-scoping detection. Owns `NO_MODEL_CONTEXT_MARKER`/`NO_MODEL_CONTEXT_FRAME_MARKER`/`UNKNOWN_TOOL_PREFIX`, the literal thrown texts `hints.rs` matches on |
| `src/truncate.rs` | UTF-8 safe string truncation (prevents panics on multi-byte chars) |
| `src/session.rs` | JSON session persistence (~/.chrome-agent/sessions.json, 0600 perms, flock + read-merge-write for parallel-safe saves) |
| `src/profiles.rs` | Three-condition predicate for removing orphaned browser profile directories, swept from the save path under the same lock |
| `src/orphans.rs` | Running browsers no session entry claims, recognised by the `--user-data-dir` they were launched with — the process half of what `profiles.rs` does for the disk. Backs the `orphan=` lines in `status` and `close --orphans` |
| `src/kill.rs` | Signalling a pid and saying truthfully what that did (`KillOutcome`, the browser classifier, the wording `close` prints). Split from run_helpers.rs for the 1000-line cap, re-exported via `pub use` |
| `src/browser.rs` | Chrome launch, auto-discovery, stale DevToolsActivePort cleanup, profile management |
| `src/chrome_args.rs` | `--chrome-arg` validation and the inherit-when-omitted merge — split from `browser.rs`/`session.rs` for the 1000-line cap, re-exported via `pub use` as `browser::normalized_chrome_args_option` and `session::ensure_chrome_args_compatible` |
| `src/connect_cli.rs` | Resolving a single CLI invocation's browser + page connection (load store, connect/launch, resolve target, connect page client) — split out of `run::run` for the 1000-line cap |
| `src/setup.rs` | 7 stealth patches (shared by run.rs + pipe.rs) |
| `src/emulation.rs` | Explicit page-scoped device metrics: validation, transactional CDP apply/reset, persistence, and observed status |
| `src/run_helpers.rs` | Shared output/error handling, connect_page with 8-attempt retry |
| `src/daemon.rs` | Optional micro-daemon (Unix only), heartbeat, crash recovery |
| `vendor/Readability.js` | Mozilla Readability (90KB, MIT) embedded via include_str! |
| `vendor/extract.js` | MDR/DEPTA-inspired data record extraction (standalone, tested via jsdom) |
| `npm/` | npm distribution wrapper (postinstall downloads native binary) |
| `skills/chrome-agent/SKILL.md` | Agent skill file — `npx skills add sderosiaux/chrome-agent` |

## Build & Test

```bash
cargo build
cargo test
cargo clippy -- -D warnings  # zero warnings enforced in CI
```

**`cargo test --test X` does not always rebuild the binary the tests exec.** The integration
suites run `target/debug/chrome-agent` as a subprocess, and cargo does not necessarily refresh
it for a single-suite run. An A/B that edits `src/`, re-runs one suite and reads the result is
then measuring the PREVIOUS build — which reads as a regression that is not there, and cost
three false failures once. Run `cargo build` between the two states.

**Every test owns its browser, and the name says so** (`tests/common/mod.rs`). Two `cargo test`
processes on one machine is the normal regime here, and a hard-coded `--browser` name means they
drive ONE browser: the first to finish `close --purge`s it under the second. Measured by running
the whole suite twice at once from two directories — `action_report_tests` died with
`transport: transport closed` on the browser named `pipe-bootstrap`, `proxy_tests` timed out on
`test-managed-proxy`, and a unit test in `src/browser.rs` read `None` because the other run had
deleted `/tmp/chrome-agent_test_devtools` between its write and its read. None of the three had
a bug; each had a name.

So there is ONE mechanism: `common::TestBrowser::new(label)` for a browser, `common::temp_path`
for a file, `common::unique_name` under both. The name carries the pid (which separates
concurrent processes) AND a counter (which separates tests inside one process, since the harness
runs them on parallel threads) — the pid half came from `hit_test_tests`, the counter half from
`device_emulation_tests`, and this is the two of them in one place instead of twenty-five
copies. `TestBrowser` closes and purges on `Drop`, which a `close` statement at the end of a
helper does not do when an assertion panics — that is how a Chrome and its ~14 MB profile leak
per failure. Three tests in `harness_tests.rs` scan the sources and fail on a hard-coded name, a
second implementation of the rule, or a fixed temp path; a line that genuinely needs one says
`isolation-exempt:` and why.

The scan earns its keep on contact with new code. Six suites merged after it was written had
each rewritten the rule by hand, and it caught all six on the first run — including a name bound
to a variable (`let browser = "test-webmcp-list";`) that the flag-shaped rule walked straight
past, which is the hole that second spelling now closes. One of the six, `download_click_tests`,
also showed the third form of the same defect, which no naming rule can catch: it ASSERTED on a
shared directory — the `.incoming-*` transfers under `~/.chrome-agent/tmp`, compared before and
after — so a sibling process's in-flight download read as this test's leak (measured:
`.incoming-76300-…` and `.incoming-76268-…`, neither pid a child of the test). Those directories
are named after the process that opened them, so the assertion filters on the pids this test
spawned. **The pid filter was half the rule** and the other half surfaced when the sweep fix
landed: the set of spawned pids is process-global, so a test also asserted on a sibling THREAD's
in-flight transfers — three tests failed at once on `.incoming-71509-…` and `.incoming-71525-…`,
opened by the slow-transfer test running beside them. The filter now keys on the pid *and* the
`TestBrowser` name, which is unique per test; the pid separates concurrent processes and the name
separates concurrent tests, the same two axes `unique_name` already carries. One rule behind all
three forms: a test may name, write and assert on what it owns, and on nothing else.

## Release

```bash
./scripts/release.sh 0.3.0
# → bumps Cargo.toml + npm/package.json, commits, tags, pushes
# → GitHub Actions: builds 5 platform binaries, creates release, publishes npm
# → Requires NPM_TOKEN in GitHub secrets
```

## Where the rest of this file lives

This file is loaded in full at the start of every session, by every agent. What an agent needs in
EVERY session is here: the map above, the build and test rules below, the conventions that hold
across the whole codebase, and the runtime traps under Gotchas. What is true of ONE part of the
codebase moved into `.claude/rules/`, where a `paths:` glob loads it when you read a file it
covers — same text, unabridged, at the moment it is useful instead of every time it is not.
Nothing was summarised and nothing was deleted; only the moment of loading changed.

Measured on 2026-08-30, in this repo, by reading `input_tokens + cache_creation + cache_read` off
the first request of a `claude -p` session's transcript: **73,789 tokens before, 43,391 after**
(43,423 on a second run) — about 30,400 tokens back per session, per agent, of which `CLAUDE.md`
itself is roughly 37,300 → 6,900 against a control session in an empty project. Re-measure rather
than trust that pair: like every other count, it is true of the day it was taken. An agent touching
`src/cli.rs` no longer pays for the hit test; an agent touching `src/hit_test.rs` gets it.

| Rule file (`.claude/rules/`) | Loads when you read | Holds |
|---|---|---|
| `hit-test.md` | `src/hit_test*.rs`, `src/element_selector.rs` | the pre-dispatch probe, `--on-intercept` and its three modes, single-handle selector resolution |
| `verdict-and-diff.md` | `src/verdict*.rs`, `src/commands/diff.rs`, `src/pipe_report.rs`, `src/render.rs` | the verdict vocabulary, the two-group ladder and its rungs, `next`, `values_lost`, focus and diff hygiene, document identity, the text renderer |
| `snapshot-and-inspect.md` | `src/snapshot*.rs`, `src/commands/inspect.rs` | a display flag never narrows the baseline, secret-value redaction in the tree, noise filtering, `--urls`, output paging |
| `element-actions.md` | `src/element.rs`, `src/element_controls.rs`, `src/read_back.rs`, and the pointer/form-control command modules | the 60 ms observation window, what `fill`/`select`/`check` report back, `press`, the settle wait that once cost ten seconds |
| `browser-lifecycle.md` | `src/browser.rs`, `src/session.rs`, `src/profiles.rs`, `src/orphans.rs`, `src/kill.rs`, `src/chrome_args.rs`, `src/connect_cli.rs` | `--chrome-arg`, the three-condition profile predicate, orphan browsers, the spawn-to-persist window, the store's one-sided prune, `close`'s wait |
| `cli-parsing.md` | `src/cli.rs`, `src/cli_actions.rs`, `src/run.rs`, `src/run_helpers.rs` | a global flag parses on either side of the verb; an invocation is judged before a browser is |
| `hints.md` | `src/hints/**` | the three rules every error message holds to, and the navigation-failure branches |
| `macros.md` | `src/macros*.rs` | what becomes a guard and what is refused, locators, where a task begins, why the file is JSON |
| `navigation-and-serving.md` | `src/landing.rs`, `src/serving.rs`, `src/commands/goto.rs` | `landed`, the redirect rule, the five `serving` tokens and every threshold in them, `--header`, `forward` |
| `files-on-disk.md` | `src/commands/download*.rs`, `src/commands/screenshot.rs`, `src/commands/pdf.rs`, `src/geometry.rs`, `src/base64.rs` | the click that produces a download, the deferred sweep, `downloaded` vs `ok`, clip math, 0600 |
| `cdp-transport.md` | `src/cdp/**`, `src/setup.rs` | every CDP call has a deadline, the input-event deadline, foreground for pointer events, `waited_ms`, dialogs, the seven stealth patches |
| `emulation.md` | `src/emulation.rs`, `src/pipe_emulation.rs` | Chrome keeps no override, so the store is the mechanism |
| `assert.md` | `src/commands/assert.rs`, `src/commands/assert_args.rs` | exit 0/1/2, reading through the action's own reader, `--matches` being a Rust regex |
| `content-extraction.md` | the `read` / `extract` / `text` / `eval` / `network` / `console` / `wait` modules under `src/commands/`, `vendor/extract.js` | reader mode, the extraction heuristics, network and console capture, `--scroll`, `network-idle` |
| `webmcp.md` | `src/commands/webmcp.rs` | why a tool's declared result gets no new verdict word |
| `pipe-and-batch.md` | `src/pipe.rs`, `src/pipe_dispatch*.rs`, `src/commands/batch.rs`, `src/commands/record.rs`, `src/macros_cmd.rs` | recordings are 0600 and refuse when unwritable, `stop_on_error`, a failed read is not a failed action |
| `frames.md` | `src/commands/frame.rs` | how an iframe is resolved and what the isolated world does not see |

Longer records that no glob loads — read them on purpose, not by accident — are indexed in
**`docs/design/README.md`**: the verdict-taxonomy design spec, the review findings, and the
display-flag baseline investigation.

Each rule file's own `paths:` block is authoritative; the middle column above is an index of it, and
a glob is added or corrected in `.claude/rules/*.md`. A glob that matches nothing is a rule that
never loads, which is the silent version of a dead pointer — `.claude/rules/hints.md` carries one on
purpose (`src/hints/**`, for a split in flight) and says so in an HTML comment, which is stripped
before the file is injected and therefore costs nothing.

**The trigger is the `Read` tool, not any access to the file** — measured: a session asked for
`src/hit_test.rs` answered with `wc -l` through Bash and the hit-test rule never loaded, while the
same question answered through `Read` loaded it. `rg`, `sed` and `wc` do not arm a path-scoped rule.
To see what actually loaded in a session, run `/context`, or add an `InstructionsLoaded` hook: it
fires once per instruction file with a `load_reason` of `session_start` or `path_glob_match`.

## Conventions that hold everywhere

Everything below is true of the whole codebase or of using the tool, so it is not
path-scoped. Per-subsystem decisions are in `.claude/rules/`, listed above.

- **Headless by default** — `--headed` for debug. Mode mismatch auto-kills old browser.
- **Static Linux binaries** — Linux releases target musl (`x86_64`/`aarch64-unknown-linux-musl`) via `cargo-zigbuild`, producing fully static binaries with **zero glibc dependency** → run on any distro (fixes #3: `GLIBC_2.39 not found` on Ubuntu 22.04). Enabled by a pure-Rust dep graph: `ureq` runs with `default-features = false` (TLS off) since it only hits Chrome's local `http://127.0.0.1` endpoint, dropping `ring`/`rustls`. CI guards the graph against C-linking crates.
- **`--connect` for heavy protection** — DataDome/Kasada detect bundled Chromium fingerprints. Connect to real installed Chrome instead (`--connect http://127.0.0.1:9222`).
- **Stable UIDs** — `n{backendNodeId}` instead of sequential `e1, e2`. Survive between inspects on same page. Change after SPA navigation (re-inspect needed).
- **3 targeting modes** — uid (from inspect), CSS selector (`--selector`), coordinates (`--xy`)
- **ElementRef abstraction** — session stores `{"type":"backendNode","id":N}`, ready for BiDi
- **`--json` mode** — errors exit 1 with `{"ok":false}` on stdout. Agents parse stdout for the error, exit code signals failure.
- **Content extraction hierarchy** — `read` (articles) > `extract` (repeating data) > `text --selector` (scoped) > `text` (full page) > `eval` (structured JS) > `network` (API responses)
- **Pipe mode** — `chrome-agent pipe` reads JSON from stdin, writes JSON to stdout. One connection, 10x faster.
- **Command aliases** — navigate/open/go, snap/snapshot/tree, js/execute, capture, tap
- **A source file stays under 1000 lines** — the cap is the rule; individual file sizes are not written down here, because four of them were and all four were stale (one by +62 %, on a file eighteen lines from the ceiling, which reads as room where there is none). Measure with `find src -name '*.rs' | xargs wc -l | sort -rn | head`. It is what forced most of the module list above; the modules it produced say so in their own row of that table ("split from X for the 1000-line cap", re-exported via `pub use` so no call site moved), which is one list rather than two that can disagree. Enforcement is a PostToolUse hook on the developer's machine (`~/.claude/hooks/file-size-guard.sh`, which blocks a `Write`/`Edit` that leaves a code file over the cap) and **not** a CI gate, so the cap can be exceeded by any route that is not an edit — which is how `src/hints.rs` reached 1194 lines and stayed there. State the exception rather than the absolute when there is one: "all files are under 1000 lines" was written here while that file was not. There is none today.

## Gotchas

- `assert` needs a browser like every other command, and `assert value|state --uid` needs a uid from a *stored* snapshot: `goto` clears the map, so inspect before asserting by uid. The `uid` an action echoes back is resolved live and is not enough on its own.
- Exit codes: `0` success, `1` error (including a bad flag — clap's usage exit moved off `2`), `2` an assertion did not hold, `130` Ctrl+C. Only `assert` ever returns `2`.
- `landed.serving` never changes `ok` or the exit code: a 403, a WAF refusal and a captcha are facts about the page, and the navigation still happened. Branch on `serving`, not on `ok`. `serving: "page"` is the absence of contradicting evidence and not a guarantee — a paywall, a cookie wall and a soft 404 all reach it.
- `serving` fires on the document as it was at the moment `goto`'s settle probe stopped. On a site whose first paint is empty (measured: `www.amazon.fr`, 1 run in 3) that reads `nothing_actionable`; `inspect` is the answer and the hint says so.
- `batch`/`pipe` have no exit code per command: a failed assertion is `ok:false` with an `assertion` object. The CLI `batch` process itself still exits 0 even when a command inside failed — read `ok`.
- CDP `rename_all = "camelCase"` fails on acronyms: use `#[serde(rename = "backendDOMNodeId")]`
- Browser-level WebSocket only supports `Target.*`. Page commands need page WS via `/json/list`.
- `Accessibility.getFullAXTree` returns a flat list with parentId/childIds, not a tree.
- A CDP field this tool does not read is not declared in `src/cdp/types.rs` at all — serde ignores what it was not told about, and a declared field that is not `Option<T>` + `#[serde(default)]` is a field Chrome MUST send. Declare what something reads; put the rest in the type's doc comment.
- `text --selector "main"` auto-falls back to `[role=main]` for ARIA compatibility.
- Readability.js can fail on non-article pages — wrapped in try-catch with descriptive error.
- `--stealth` patches are CDP-level (Page.addScriptToEvaluateOnNewDocument), not Chrome flags. `--disable-blink-features=AutomationControlled` is a myth.
- After SPA navigation (`back`, `click` that triggers route change), UIDs change. Always re-inspect.
- For SPA product/detail pages, prefer `goto <direct-url>` over `click <link-uid>`.
- DataDome/Kasada: `--stealth` is NOT enough. Use `--connect` to a real installed Chrome.
- `Runtime.evaluate` works WITHOUT `Runtime.enable`. Stealth mode skips it to avoid detection.
- `history.back()` in pipe mode kills WebSocket. Use `Page.navigateToHistoryEntry` instead.
- Parallel agents sharing `--browser default` corrupt each other's sessions. Use `--browser <unique>`.
- Console interceptor is guarded against re-injection (`__chrome-agent_console_installed`).
- `press Enter` needs `windowsVirtualKeyCode: 13` + `text: "\r"` for form submission.
- A pointer action brings its page to the foreground (once per connection); a keyboard action does not. On a browser with several pages this is a tab switch, and it is deliberate — a background page answers pointer events on a five-second timer.
- `goto`'s settle probe is bounded at both ends: the quiet window starts immediately (a static page no longer pays a flat 3s) and the ceiling is never cleared by a mutation (a continuously-mutating page used to hang forever, since `awaitPromise` has no deadline and `--timeout` does not reach it).
- Anything that both SHOWS a tree and STORES one goes through `snapshot::take_views`, never `take_snapshot` + a narrowing flag. `take_snapshot` is for callers that only read (`diff`'s live side, `extract --a11y`); reaching for it on a storing path is how the seven-path baseline bug happened, and there is nothing in the type to stop it happening again.
- `goto` clears `uid_map` but keeps `last_snapshot`. Clearing the map stops a stale uid resolving to an unrelated node on the new page; keeping the snapshot is what lets `diff` report `document_changed` instead of erroring.
- Session saves only rewrite browsers this process actually modified. `load_from` reads every browser in the file, so writing them all back made a reading agent clobber a concurrent agent's `uid_map`/`last_snapshot`.
- `drag` uses CDP mouse events (mousePressed/mouseMoved/mouseReleased). Works with mousedown-based DnD libs (Sortable.js, React DnD mouse backend). Does NOT work with HTML5 Drag and Drop API (requires dragstart/dragover/drop events).
- `frame` only supports `<iframe>`, not legacy `<frameset>`/`<frame>`. Error message is clear.
- `frame` binding only persists within a single `pipe`/`batch` process (state lives on the connection). CLI single commands each open a fresh connection, so `frame` can't carry over — use pipe mode for `frame → inspect → act` (issue #8).
- `frame <selector>` resolves the iframe *inside the currently bound frame's context*, so you can descend into nested iframes (`frame #outer` → `frame #inner`). Consequence: to switch to a **top-level sibling** iframe after binding, reset with `frame main` first, otherwise the sibling selector is searched inside the bound frame and won't be found.
- `frame` scopes `eval` and `inspect`; it does NOT scope selector-based targeting (`click`/`fill --selector` still query the top document). Use `inspect` after the switch to get iframe uids, then act by uid (backendNodeId is page-global, works cross-frame).
- `frame` uses an isolated world: `eval` sees the frame's DOM/`location` but NOT its main-world JS variables. `document.querySelector("iframe")` matches the *first* iframe in DOM order — on ad-heavy pages that's often an `about:blank` slot; pass a precise selector (e.g. `iframe[src*="…"]`) to hit the intended frame.
- `webmcp list`/`webmcp call` under a `frame` binding hit the same isolated-world blindness as `eval`: a polyfilled `document.modelContext` (installed by the frame's own main-world script) reads as `undefined` there even though the frame really has tools (measured, `tests/fixtures/webmcp_iframe_host.html`). The response carries `frame_scoped: true` so an empty result reads as "unproven", not "this frame has none". Most Chrome installs today have no native WebMCP; launch with `--chrome-arg --enable-features=WebMCP,WebMCPTesting` to test against the real API, or rely on a page's own polyfill (the fixture ships one).
- `batch` CLI mode: uids change between invocations (new CDP connection = new backendNodeIds). Use pipe mode for uid-stable multi-command flows.
- `select` on non-`<select>` element throws "Element is not a \<select\>". Custom dropdowns (React, MUI) need click + click approach.
- `network --abort` is blocking: it runs for `--live N` seconds intercepting requests, then disables Fetch domain. Start abort before navigating to the page.
- `download` takes exactly one target: a URL, `--uid`, or `--selector`. Two of them is refused rather than ranked. The URL path holds the file in memory as base64 during transfer (hence `--max-bytes`); the click path streams to disk through Chrome and is capped by cancelling the transfer.
- `download --uid` needs a uid from a *stored* snapshot, like every other uid: `goto` clears the map, so `inspect` before downloading by uid.
- A click-triggered download and the page's own reaction are not separable: `download --selector` clicks, and if the element navigates instead of downloading, that navigation happened. The response says no download began; it does not say the page is where it was.
- JS dialogs auto-accept by default. `beforeunload` under `accept` means "proceed" (the agent asked to navigate). Use `--dialog manual` to restore the old blocking behaviour. The handler logs to stderr, never stdout (safe for `--json`).
- `wait network-idle` takes an empty pattern; other wait types still require one. In pipe mode use `{"cmd":"wait","what":"network-idle"}`.
- `Page.captureScreenshot` `clip.scale` downsamples without any image crate — required to keep the musl dep graph pure-Rust (issue #3). Element clip uses the border-box bounds of `DOM.getBoxModel`.

## Linting

Zero warnings enforced. Clippy pedantic + nursery enabled with targeted suppressions in Cargo.toml.
CI runs `cargo clippy -- -D warnings`. Any warning = build failure.
