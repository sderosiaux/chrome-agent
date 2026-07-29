# chrome-agent v0.10.0

Single Rust binary for browser automation via CDP. Built for AI agents.
~11.5K lines Rust, zero runtime dependencies, 3 MB binary.

## Architecture

```
CLI (clap) → CDP Client (WebSocket) → Chrome
```

| Module | Role |
|--------|------|
| `src/cli.rs` | CLI definition: `Cli` struct, `Command` enum (38 commands) |
| `src/run.rs` | CLI command dispatch (match on Command enum) |
| `src/pipe.rs` | Pipe mode: persistent connection, JSON stdin/stdout |
| `src/pipe_dispatch.rs` | Pipe/batch command dispatchers (shared by pipe + batch + CLI batch) |
| `src/cdp/` | WebSocket transport, message correlation, CDP types |
| `src/commands/` | 25 command modules: goto, click, fill, inspect, eval, text, read, extract, diff, network, console, wait, screenshot, pdf, download, tabs, dblclick, select, check, upload, drag, frame, batch... |
| `src/element.rs` | uid/coordinate resolution → CDP input dispatch, click, fill, type, press, hover, dblclick |
| `src/element_controls.rs` | select, check/uncheck, upload, drag — split from element.rs for the 1000-line cap, re-exported via `pub use` |
| `src/element_selector.rs` | CSS-selector actions (click/dblclick/fill/focus) — split from element.rs for the 1000-line cap, re-exported via `pub use` |
| `src/geometry.rs` | box-model → screenshot clip math (quad bounds, downscale factor), uid/selector clip resolution |
| `src/base64.rs` | shared RFC 4648 decoder (screenshot/pdf/download) — no `base64` crate, keeps musl graph pure-Rust |
| `src/element_ref.rs` | ElementRef abstraction (decouples from CDP internals) |
| `src/snapshot.rs` | Accessibility tree → compact text with stable uids (backendNodeId), role filter + aliases |
| `src/verdict.rs` | What an action may claim about itself: pure classifier, `verdict` + `verdict_reason` + hint |
| `src/pipe_report.rs` | `mutates_page` + `attach_change_report` — split from pipe_dispatch.rs for the 1000-line cap, re-exported via `pub use` |
| `src/truncate.rs` | UTF-8 safe string truncation (prevents panics on multi-byte chars) |
| `src/session.rs` | JSON session persistence (~/.chrome-agent/sessions.json, 0600 perms, flock + read-merge-write for parallel-safe saves) |
| `src/browser.rs` | Chrome launch, auto-discovery, stale DevToolsActivePort cleanup, profile management |
| `src/setup.rs` | 7 stealth patches (shared by run.rs + pipe.rs) |
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

## Release

```bash
./scripts/release.sh 0.3.0
# → bumps Cargo.toml + npm/package.json, commits, tags, pushes
# → GitHub Actions: builds 5 platform binaries, creates release, publishes npm
# → Requires NPM_TOKEN in GitHub secrets
```

## Key Design Decisions

- **Headless by default** — `--headed` for debug. Mode mismatch auto-kills old browser.
- **Static Linux binaries** — Linux releases target musl (`x86_64`/`aarch64-unknown-linux-musl`) via `cargo-zigbuild`, producing fully static binaries with **zero glibc dependency** → run on any distro (fixes #3: `GLIBC_2.39 not found` on Ubuntu 22.04). Enabled by a pure-Rust dep graph: `ureq` runs with `default-features = false` (TLS off) since it only hits Chrome's local `http://127.0.0.1` endpoint, dropping `ring`/`rustls`. CI guards the graph against C-linking crates.
- **`--stealth` mode** — 7 CDP patches: navigator.webdriver, chrome.runtime, WebGL, UA, Permissions, input screenX/pageX leak, Runtime.enable skipped. Bypasses Cloudflare/Turnstile.
- **`--connect` for heavy protection** — DataDome/Kasada detect bundled Chromium fingerprints. Connect to real installed Chrome instead (`--connect http://127.0.0.1:9222`).
- **Stable UIDs** — `n{backendNodeId}` instead of sequential `e1, e2`. Survive between inspects on same page. Change after SPA navigation (re-inspect needed).
- **3 targeting modes** — uid (from inspect), CSS selector (`--selector`), coordinates (`--xy`)
- **JS click fallback** — when a11y reports "disabled" but DOM isn't, click falls back to `.click()`
- **`click --selector` is the same verb as `click <uid>`** — both resolve the element's viewport centre and dispatch native CDP mouse events (`element_selector::click_selector`, mirroring `dblclick_selector`). It used to call `el.click()`, which fires the handler whatever is stacked above the node: a click on a button under a modal scrim reported success with the same shape as a click a user could have made, and nothing in the response distinguished the two spellings. Consequence, deliberate: a covered element now hands the click to whatever covers it — `--selector` on a button behind a cookie banner clicks the banner, which is what a pointer does. The JS `click()` survives only as the zero-size fallback, where there is no point to aim at.
- **ElementRef abstraction** — session stores `{"type":"backendNode","id":N}`, ready for BiDi
- **Noise filtering** — StaticText/InlineTextBox stripped (66% token reduction), `--filter` by role with aliases (textbox→searchbox+combobox, input→all input roles, button→menuitem)
- **`--json` mode** — errors exit 1 with `{"ok":false}` on stdout. Agents parse stdout for the error, exit code signals failure.
- **Actions report what changed (default on, all three modes)** — after a mutating command the page is re-read and the response carries `changed{added,removed,changed,unchanged,moved,anonymous,document_changed,identity_known}` + `delta`, plus `focus{from,to}` when focus moved. `goto` is deliberately excluded (`pipe_dispatch::mutates_page`): the caller navigated on purpose, and a truncated slice of the destination is neither a delta nor a usable snapshot — `--inspect` is for that. The baseline snapshot is always taken at full depth; applying `--max-depth` to it made every node past the limit reappear as an addition on the next comparison. Bounded by `--budget` (1200 chars, 0 = uncapped); `--verdict off` skips the read entirely and restores the pre-0.8 output and latency. Costs one `getFullAXTree` per action. CLI goes through `run_helpers::output_action`; pipe and batch go through a central hook in `pipe::dispatch` / `pipe_dispatch::dispatch_single` (`mutates_page` + `attach_change_report`, both now in `pipe_report.rs`) rather than through each dispatcher, so adding a command means adding it to `mutates_page` and nothing else.
- **Every action states a verdict** (`src/verdict.rs`) — `verdict` + `verdict_reason` ride on every mutating response in all three modes, because the absence of `changed`/`delta` used to mean four different things: `--verdict off`, no baseline yet, the post-action read failed, or the page genuinely did not move. Three of those are "I don't know" and one is an assertion. The vocabulary is `changed` (`tree_delta` / `nodes_moved` / `focus_only`), `navigated` (`document_replaced`), `unchanged` (`identical_tree`), `unknown` (`no_baseline` / `read_failed` / `identity_unreadable`) and `not_checked` (`reporting_disabled`); the uncertain ones carry a `verdict_hint` naming the next action. `unchanged` says the tree was identical while we watched — deliberately not "no effect", which would be a claim about the action rather than the observation. An identical tree is also what an overlay swallowing the click, a canvas repaint, or a handler firing after the window all look like; answering "no effect" there makes the agent retry, and the retry is a second real click. The full taxonomy (`docs/design/verdict-taxonomy.md`) does define `no_effect`, but only behind proof of delivery (a hit test, or a postcondition read on the acted-on handle) — neither is built, and when the hit test lands, `unchanged` splits into `no_effect` and `intercepted`. `focus_only` is a `changed`: focus churn is subtracted from the delta counts (every click focuses something) but it is the closest thing to proof of delivery available without a hit test, and calling it `unchanged` would throw away the signal that separates "landed on an inert element" from "never arrived". The classifier is pure and unit-tested; the wiring is `run_helpers::attach_verdict`.
- **Document identity** — `(frameId, loaderId)` from `Page.getFrameTree`, stored as `PageSession.last_snapshot_frame/_loader`. `diff::Identity` is tri-state (`Same`/`Different`/`Unknown`) and `compare` diffs only on `Same`. A URL is the wrong signal both ways: it moves on a fragment jump and on `pushState` where every uid survives, and stays put across a reload or a form GET to the same address where none does. `Unknown` returns the page rather than guessing — the previous `_ => false` took the confident branch. Measured on a real navigation before the fix: 328 bogus `~` lines, 18,764 tokens, versus 16,148 for re-inspecting the destination. Polling, not events: `CdpClient::events()` only delivers post-subscribe messages and several commands never subscribe.
- **Diff hygiene** — focus churn is reported as `focus: from -> to` and kept out of `changed` (every click focuses something, so it was the loudest line in most reports). `e{n}` uids (the fallback for nodes with no `backendDOMNodeId`, `snapshot.rs`) are renumbered per snapshot and never matched, only counted as `anonymous`. A reorder is counted as `moved`: pairing by uid alone left every uid present and reported "No changes detected" for a drag-and-drop.
- **One observation window, stated** (`element::READ_BACK_MS`, 60 ms) — every read-back waits the same window and reports `observed_after_ms` beside the value. The four paths used to disagree: `fill` read synchronously (0 ms), so a value reverted one microtask later was reported as kept — `verbatim:true` on a field the page had already emptied (`tests/fixtures/form_value_microtask_revert.html`); `check --selector` waited 60 ms; `check <uid>` waited however long a CDP round trip happened to cost. 60 ms catches a revert on the microtask queue, in `setTimeout(0)` or in an animation frame, and does NOT catch a validator firing at 400 ms (`form_value_late_revert.html`) — no fixed window could, which is why the window is reported rather than persistence asserted. `check`'s `observed_after_ms` is absent when the element already held the state: nothing was dispatched, so there was no post-action moment (`CheckOutcome`). `select` was the fourth path and the last to join: it returned the option text from the same synchronous script that set `selectedIndex`, so a controlled component that snapped the selection back was still reported as selected.
- **`fill` reports what the page kept** — `value: {requested, actual, verbatim, observed_after_ms, caveat?}`. Secret fields (`type=password`, or `autocomplete` naming a password/card/CVC/one-time code) report `{redacted:true, requested_length, actual_length, verbatim}` instead: the response reaches stdout, the agent transcript and any `--record` file. Masks reformat, controlled components rewrite, number inputs discard. Both strings are returned rather than reduced to one word: a currency mask turns `1000` into `10.00` while a digits-only comparison insists the content was preserved. Refuses on `:disabled` (catches an ancestor `<fieldset disabled>`, which `el.disabled` does not) and `readOnly`. Caveat when the write exceeded `maxlength` — that constrains the editing pipeline, not the value setter, so the field now holds something the form will reject.
- **A bulk fill reports each field** (`run_helpers::bulk_fill_report`) — `fill-form` and `fill_and_submit` filled through the same code as `fill` and dropped every `FillOutcome`, answering "Filled 3 fields": right about the count, silent about the mask that reformatted one of them. Both now return `values: [{uid|selector, value:{...}}]`, redacted the same way a single fill is. For `fill_and_submit` this is the *only* witness: the change report runs after the submit, by which time the form has moved on.
- **`check`/`uncheck` classify before acting** — native `<input type=checkbox|radio>` read through `.checked` (`indeterminate` as mixed), an element with `aria-checked` or a checkable role through that attribute, anything else refused. `!!el.checked` was wrong in both directions: absent on a `<div role=checkbox>` (so a checked box was clicked OFF while reporting success) and present-but-meaningless on any other input. Unchecking a radio is refused. The state is read back after the click.
- **`press`** — a single printable character is sent with `text` so it actually types; unmapped names are refused rather than dispatched with virtual key code 0.
- **A targeted action names the node it hit** (`element_selector::selector_uid` → `run_helpers::target_details`) — every click/dblclick/fill/select/check/uncheck/upload response carries `uid`, whether the caller aimed by selector or by uid. `--selector` resolves in the page, the message quoted the selector back and the change report named uids, with nothing tying the two together: an agent could not check that the node the delta describes is the node it aimed at, and a selector matching several elements gave no clue which one was used. Resolved BEFORE the action (`Runtime.evaluate` → `DOM.describeNode` → `n{backendNodeId}`) — afterwards the element may be detached and the answer would describe a different page. Costs one CDP round trip on the selector path.
- **A failed read is not a failed action** — the post-action page read is best effort in all three modes. The CLI used to propagate it with `?`, so a click that had already been delivered returned `ok:false`, and the natural response to that is to click again — which is real. `pipe_dispatch` stated the opposite policy in a comment and followed it; the CLI now matches. The response is `ok:true` with `verdict: unknown / read_failed`. Reproduced with `tests/fixtures/blocks_after_click.html`, which pins the main thread after the click returns so CDP cannot answer within a short `--timeout`.
- **Recordings are 0600** (`commands::record::restrict`) — a recording holds every command and response of the session, including the values a fill put into the page, among them the ones redacted on stdout precisely because they are secrets. It was created with whatever the umask allowed (typically 0644) while screenshot/pdf/download/session all chmod 0600. Applied on every write, not only at creation: the file may already exist, wider, from an earlier run.
- **A recording that cannot be written refuses the command** (`pipe.rs`) — `start_recording` and `log_entry` errors were both discarded with `let _ =`, so an unwritable `_record` path produced `ok:true` responses indistinguishable from a session actually being recorded, and the agent found out at `replay` time that there was nothing to replay. A failure to *open* now refuses the command before it runs: the caller asked for a recorded action, and running it unrecorded is not that. Consequence, deliberate: a bad path stops the session's work, not just its log — but per command and loudly, so it surfaces on the first line rather than at the end. A failure to *append* rides on the response as `recording_error` instead, because that command already ran and failing it invites a retry of real work.
- **Self-healing errors** — every error includes a `hint` field suggesting the next action
- **Reader mode** — `read` injects Mozilla Readability.js for article extraction (~500 tokens vs ~15K)
- **Content extraction hierarchy** — `read` (articles) > `extract` (repeating data) > `text --selector` (scoped) > `text` (full page) > `eval` (structured JS) > `network` (API responses)
- **`extract` command** — MDR/DEPTA-inspired heuristics: sibling structural similarity, content heterogeneity, text-to-link ratio, semantic class fast-pass, hidden element exclusion, tag-based merge for modifier classes. 187 tests (117 JS unit via jsdom + 70 Rust E2E).
- **Pipe mode** — `chrome-agent pipe` reads JSON from stdin, writes JSON to stdout. One connection, 10x faster.
- **Network capture** — retroactive via Performance API (stealth-safe) or live via Network domain
- **Console capture** — stealth-safe interceptor via addScriptToEvaluateOnNewDocument
- **Command aliases** — navigate/open/go, snap/snapshot/tree, js/execute, capture, tap
- **`--copy-cookies`** — copies Cookies SQLite + Local State from user's real Chrome profile. Enables access to logged-in sites (X.com, Gmail) without `--connect`. macOS Keychain decrypts the cookies.
- **`extract --scroll`** — scrolls page before extracting, uses `MutationObserver` to wait for lazy-loaded content. Uses `Math.max(body, documentElement)` for scroll height (YouTube fix). Max 10 iterations.
- **Parallel agent isolation** — `--browser <name>` per agent. Saves are parallel-safe via an exclusive `flock` on `sessions.lock` + read-merge-write: each save re-reads the on-disk store under the lock, deletes only the browsers this process dropped since load, upserts its own, then atomically renames a per-PID temp file into place.
- **connect_page with 8-attempt retry** — page-level CDP connection retries (up to 8 attempts) with 500ms/300ms backoff between tries
- **`forward`** — symmetric to `back`, uses `Page.getNavigationHistory` + `Page.navigateToHistoryEntry`
- **`dblclick`** — 4 mouse events (pressed/released x2 with click_count 1 then 2), JS fallback via `dblclick` MouseEvent. `--selector` resolves the element's viewport-center coords then runs the native CDP double-click (`dblclick_selector`); it is a real double-click, not a single `click_selector`.
- **`select`** — matches by `option.value` first, then by `option.text.trim()`. Dispatches `change`, then reads the selection back through `READ_BACK_MS` and reports `observed_after_ms` like `fill` and `check`. A selection the page reverted inside the window is refused (exit 1, naming the option actually held) rather than reported as made: an agent that submits a form believing a different option is chosen cannot recover from that answer. Both the uid and the selector path run one in-page script that sets, dispatches, waits and re-reads, bound to the same node throughout.
- **`check`/`uncheck`** — idempotent: queries `this.checked` via callFunctionOn, clicks only if state differs
- **`upload`** — validates file paths exist before CDP call. Uses `DOM.setFileInputFiles` with backendNodeId (uid) or nodeId (selector)
- **`drag`** — 5-step linear interpolation between source/destination centers, 16ms between moves for realism
- **`batch`** — CLI reads JSON array from stdin, dispatches sequentially via `pipe_dispatch::dispatch_single`. Pipe mode uses `"commands"` array field.
- **`frame`** — resolves the iframe via `document.querySelector` → `DOM.describeNode` (owner `frameId`, so it targets the *specific* iframe matched, not just the first child frame), then `Page.createIsolatedWorld` for its execution context. The `(frameId, contextId)` is stored on the `CdpClient` (`set_frame_context`) so subsequent `eval` (via `contextId`) and `inspect` (via `getFullAXTree` `frameId`) scope to that frame. Cleared on navigation (goto/back/forward/navigate_and_read) since the isolated world dies with it. `frame main` clears the binding. Only `<iframe>`, not `<frame>`/`<frameset>`.
- **`inspect --urls`** — post-processes snapshot text, resolves href on link nodes via `DOM.resolveNode` + `Runtime.callFunctionOn`
- **`inspect --max-chars`/`--offset`** — char-based, UTF-8-safe output paging via `inspect::paginate`. Full snapshot still persisted for diff/uid lookups; only the printed window is capped. Truncated output appends the next `--offset`.
- **`goto --header`** — repeatable `"Name: Value"` (split on first colon) applied via `Network.setExtraHTTPHeaders` before navigate. `--post` intentionally not implemented (fragile over `Page.navigate`).
- **`wait network-idle`** — enables Network domain, tracks in-flight requestIds (`InFlightTracker`), resolves after `--idle-ms` at zero in-flight, bounded by `--timeout`. Opt-in, off the stealth hot path.
- **screenshot flags** — `--format jpeg`/`--quality`, `--max-width` (downscale via CDP `clip.scale`, no image crate), `--uid`/`--selector` clip via `DOM.getBoxModel` (`geometry::clip_for_*`). Never emits base64 on stdout.
- **`pdf`** — `Page.printToPDF` (`transferMode: ReturnAsBase64`) → shared `base64::decode` → 0600 file, mirrors screenshot.
- **`download <url>`** — in-page `fetch(url,{credentials:'include'})` → base64 in page → `base64::decode` → 0600 file. Auth-preserving. Filename from Content-Disposition (incl. RFC 5987 `filename*`) then URL. Click-triggered browser-native downloads NOT handled (resolve href + download).
- **Every CDP call has a deadline** (`CdpClient::call_timeout`, wired from `--timeout`, default 30s) — `call` awaited its response channel with nothing behind it. Chrome answers promptly, but an evaluation sent with `awaitPromise` only answers when the page's promise settles, so `eval "new Promise(() => {})"` hung the command with no error, no output and no recovery — in pipe mode for the rest of the session, with the socket still open and the dispatcher still running. The timed-out request is removed from `pending` (leaving it leaks a slot and would deliver a late answer to a receiver nobody awaits). `inspect --limit` was the reachable instance twice over: its scroll probe re-armed a 400ms debounce on every mutation with no ceiling (now `snapshot::settle(400, 2000)`, whose hard timer nothing clears), and its `limit * 3` bound counts iterations rather than time, so `--limit 500` on a live page meant 1500 settle windows — the collection is now bounded by `--timeout` too, and says so in its output rather than looking like a short page.
- **JS dialog auto-handling** — `CdpClient::spawn_dialog_handler` runs a background task on every connection (CLI + pipe) answering `Page.javascriptDialogOpening` via `Page.handleJavaScriptDialog` per `--dialog` (accept default | dismiss | manual) + `--dialog-text`. Pure decision in `setup::dialog_decision`; fire-and-forget request ids live in `[1_000_000_000, i32::MAX]` (wrapping) — high enough to never collide with the sequential request ids, but still inside Chromium's accepted signed 32-bit range. IDs beyond that (the old `1<<40`) are silently ignored by Chromium, so `Page.handleJavaScriptDialog` never ran and the triggering command hung.
- **`network --abort`** — enables `Fetch` domain with URL pattern, intercepts `Fetch.requestPaused`, calls `Fetch.failRequest` with `BlockedByClient`
- **File split** — main.rs (72 lines) → cli.rs (450), run.rs (745), pipe_dispatch.rs (608). All files under 1000 lines (hook-enforced) — `element.rs` box-model helpers were split into `geometry.rs`, and its CSS-selector actions into `element_selector.rs`, for this reason.

## Gotchas

- CDP `rename_all = "camelCase"` fails on acronyms: use `#[serde(rename = "backendDOMNodeId")]`
- Browser-level WebSocket only supports `Target.*`. Page commands need page WS via `/json/list`.
- `Accessibility.getFullAXTree` returns a flat list with parentId/childIds, not a tree.
- Some AXRelatedNode fields may be missing — `Option<T>` + `#[serde(default)]` everywhere.
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
- `goto`'s settle probe is bounded at both ends: the quiet window starts immediately (a static page no longer pays a flat 3s) and the ceiling is never cleared by a mutation (a continuously-mutating page used to hang forever, since `awaitPromise` has no deadline and `--timeout` does not reach it).
- `goto` clears `uid_map` but keeps `last_snapshot`/`last_snapshot_url`. Clearing the map stops a stale uid resolving to an unrelated node on the new page; keeping the snapshot is what lets `diff` report `document_changed` instead of erroring.
- Session saves only rewrite browsers this process actually modified. `load_from` reads every browser in the file, so writing them all back made a reading agent clobber a concurrent agent's `uid_map`/`last_snapshot`.
- `drag` uses CDP mouse events (mousePressed/mouseMoved/mouseReleased). Works with mousedown-based DnD libs (Sortable.js, React DnD mouse backend). Does NOT work with HTML5 Drag and Drop API (requires dragstart/dragover/drop events).
- `frame` only supports `<iframe>`, not legacy `<frameset>`/`<frame>`. Error message is clear.
- `frame` binding only persists within a single `pipe`/`batch` process (state lives on the connection). CLI single commands each open a fresh connection, so `frame` can't carry over — use pipe mode for `frame → inspect → act` (issue #8).
- `frame <selector>` resolves the iframe *inside the currently bound frame's context*, so you can descend into nested iframes (`frame #outer` → `frame #inner`). Consequence: to switch to a **top-level sibling** iframe after binding, reset with `frame main` first, otherwise the sibling selector is searched inside the bound frame and won't be found.
- `frame` scopes `eval` and `inspect`; it does NOT scope selector-based targeting (`click`/`fill --selector` still query the top document). Use `inspect` after the switch to get iframe uids, then act by uid (backendNodeId is page-global, works cross-frame).
- `frame` uses an isolated world: `eval` sees the frame's DOM/`location` but NOT its main-world JS variables. `document.querySelector("iframe")` matches the *first* iframe in DOM order — on ad-heavy pages that's often an `about:blank` slot; pass a precise selector (e.g. `iframe[src*="…"]`) to hit the intended frame.
- `batch` CLI mode: uids change between invocations (new CDP connection = new backendNodeIds). Use pipe mode for uid-stable multi-command flows.
- `select` on non-`<select>` element throws "Element is not a \<select\>". Custom dropdowns (React, MUI) need click + click approach.
- `network --abort` is blocking: it runs for `--live N` seconds intercepting requests, then disables Fetch domain. Start abort before navigating to the page.
- `download` is `--url`-only (in-page fetch). It cannot capture a click-initiated browser-native download — resolve the href (`inspect --urls`) and pass the URL. Large files are held in memory as base64 during transfer.
- JS dialogs auto-accept by default. `beforeunload` under `accept` means "proceed" (the agent asked to navigate). Use `--dialog manual` to restore the old blocking behaviour. The handler logs to stderr, never stdout (safe for `--json`).
- `wait network-idle` takes an empty pattern; other wait types still require one. In pipe mode use `{"cmd":"wait","what":"network-idle"}`.
- `Page.captureScreenshot` `clip.scale` downsamples without any image crate — required to keep the musl dep graph pure-Rust (issue #3). Element clip uses the border-box bounds of `DOM.getBoxModel`.

## Linting

Zero warnings enforced. Clippy pedantic + nursery enabled with targeted suppressions in Cargo.toml.
CI runs `cargo clippy -- -D warnings`. Any warning = build failure.
