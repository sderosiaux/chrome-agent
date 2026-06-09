# chrome-agent v0.4.0

Single Rust binary for browser automation via CDP. Built for AI agents.
~7.3K lines Rust, zero runtime dependencies, 3 MB binary.

## Architecture

```
CLI (clap) → CDP Client (WebSocket) → Chrome
```

| Module | Role |
|--------|------|
| `src/cli.rs` | CLI definition: `Cli` struct, `Command` enum (37 commands) |
| `src/run.rs` | CLI command dispatch (match on Command enum) |
| `src/pipe.rs` | Pipe mode: persistent connection, JSON stdin/stdout |
| `src/pipe_dispatch.rs` | Pipe/batch command dispatchers (shared by pipe + batch + CLI batch) |
| `src/cdp/` | WebSocket transport, message correlation, CDP types |
| `src/commands/` | 23 command modules: goto, click, fill, inspect, eval, text, read, extract, diff, network, console, wait, screenshot, tabs, dblclick, select, check, upload, drag, frame, batch... |
| `src/element.rs` | uid/selector/coordinate resolution → CDP input dispatch, JS click fallback, dblclick, select, check, upload, drag |
| `src/element_ref.rs` | ElementRef abstraction (decouples from CDP internals) |
| `src/snapshot.rs` | Accessibility tree → compact text with stable uids (backendNodeId), role filter + aliases |
| `src/truncate.rs` | UTF-8 safe string truncation (prevents panics on multi-byte chars) |
| `src/session.rs` | JSON session persistence (~/.chrome-agent/sessions.json, 0600 perms, conflict detection) |
| `src/browser.rs` | Chrome launch, auto-discovery, stale DevToolsActivePort cleanup, profile management |
| `src/setup.rs` | 7 stealth patches (shared by run.rs + pipe.rs) |
| `src/run_helpers.rs` | Shared output/error handling, connect_page with 5x retry |
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
- **ElementRef abstraction** — session stores `{"type":"backendNode","id":N}`, ready for BiDi
- **Noise filtering** — StaticText/InlineTextBox stripped (66% token reduction), `--filter` by role with aliases (textbox→searchbox+combobox, input→all input roles, button→menuitem)
- **`--json` mode** — errors exit 1 with `{"ok":false}` on stdout. Agents parse stdout for the error, exit code signals failure.
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
- **Parallel agent isolation** — `--browser <name>` per agent. Session conflict detection via mtime.
- **connect_page with 5x retry** — page-level CDP connection retries with 300ms backoff
- **`forward`** — symmetric to `back`, uses `Page.getNavigationHistory` + `Page.navigateToHistoryEntry`
- **`dblclick`** — 4 mouse events (pressed/released x2 with click_count 1 then 2), JS fallback via `dblclick` MouseEvent
- **`select`** — matches by `option.value` first, then by `option.text.trim()`. Dispatches `change` event.
- **`check`/`uncheck`** — idempotent: queries `this.checked` via callFunctionOn, clicks only if state differs
- **`upload`** — validates file paths exist before CDP call. Uses `DOM.setFileInputFiles` with backendNodeId (uid) or nodeId (selector)
- **`drag`** — 5-step linear interpolation between source/destination centers, 16ms between moves for realism
- **`batch`** — CLI reads JSON array from stdin, dispatches sequentially via `pipe_dispatch::dispatch_single`. Pipe mode uses `"commands"` array field.
- **`frame`** — uses `Page.getFrameTree` to find child frames, `Page.createIsolatedWorld` to get execution context. Only `<iframe>`, not `<frame>`/`<frameset>`.
- **`inspect --urls`** — post-processes snapshot text, resolves href on link nodes via `DOM.resolveNode` + `Runtime.callFunctionOn`
- **`network --abort`** — enables `Fetch` domain with URL pattern, intercepts `Fetch.requestPaused`, calls `Fetch.failRequest` with `BlockedByClient`
- **File split** — main.rs (72 lines) → cli.rs (450), run.rs (745), pipe_dispatch.rs (608). All files under 1000 lines (hook-enforced).

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
- `drag` uses CDP mouse events (mousePressed/mouseMoved/mouseReleased). Works with mousedown-based DnD libs (Sortable.js, React DnD mouse backend). Does NOT work with HTML5 Drag and Drop API (requires dragstart/dragover/drop events).
- `frame` only supports `<iframe>`, not legacy `<frameset>`/`<frame>`. Error message is clear.
- `batch` CLI mode: uids change between invocations (new CDP connection = new backendNodeIds). Use pipe mode for uid-stable multi-command flows.
- `select` on non-`<select>` element throws "Element is not a \<select\>". Custom dropdowns (React, MUI) need click + click approach.
- `network --abort` is blocking: it runs for `--live N` seconds intercepting requests, then disables Fetch domain. Start abort before navigating to the page.

## Linting

Zero warnings enforced. Clippy pedantic + nursery enabled with targeted suppressions in Cargo.toml.
CI runs `cargo clippy -- -D warnings`. Any warning = build failure.
