# dev-browser v2 — Implementation Plan

Reference: [Design Document](./2026-03-27-v2-design.md)

## Repo Structure (target)

```
dev-browser-v2/
├── Cargo.toml
├── src/
│   ├── main.rs                  # CLI entry, clap dispatch
│   ├── cdp/
│   │   ├── mod.rs
│   │   ├── client.rs            # WebSocket CDP client (connect, send, recv)
│   │   ├── transport.rs         # WebSocket frame handling (tungstenite)
│   │   └── types.rs             # CDP domain types (Target, Runtime, DOM, Accessibility, Input, Page)
│   ├── session.rs               # Session file read/write, stale cleanup
│   ├── daemon.rs                # Micro-daemon: persistent connection, heartbeat, IPC
│   ├── element_ref.rs           # ElementRef enum + resolve() abstraction
│   ├── browser.rs               # Launch Chromium, auto-discover, connect
│   ├── snapshot.rs              # AXTree → text format, uid↔ElementRef map
│   ├── element.rs               # uid → resolve ElementRef → dispatch click/fill/etc.
│   ├── commands/
│   │   ├── mod.rs
│   │   ├── goto.rs
│   │   ├── click.rs
│   │   ├── fill.rs
│   │   ├── snap.rs
│   │   ├── screenshot.rs
│   │   ├── eval.rs
│   │   ├── run.rs               # Tier 3 script orchestration
│   │   ├── tabs.rs
│   │   └── install.rs           # Download Chromium
│   └── helpers.rs               # include_str!("../../helpers/runtime.js")
├── helpers/
│   └── runtime.js               # ~200 lines, injected into Chrome pages
├── tests/
│   ├── cdp_client_test.rs
│   ├── snapshot_test.rs
│   ├── session_test.rs
│   └── integration/
│       ├── atomic_commands_test.rs
│       └── script_test.rs
└── README.md
```

## Dependencies (Cargo.toml)

```toml
[dependencies]
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = "0.24"
futures-util = "0.3"
dirs = "5"
```

No Node.js. No npm. No Playwright. No quickjs-emscripten.

---

## Milestones

### M1: CDP Client + WebSocket Transport

**Goal**: Connect to a Chrome instance via WebSocket, send CDP commands, receive responses and events.

**Files**: `src/cdp/mod.rs`, `src/cdp/client.rs`, `src/cdp/transport.rs`, `src/cdp/types.rs`

**What to build**:
- WebSocket connection to `ws://host:port/devtools/browser/...`
- JSON-RPC-style message correlation: send `{id, method, params}`, receive `{id, result}` or `{id, error}`
- Event subscription: receive `{method, params}` (no id) for CDP events
- Async: tokio runtime, channel-based message dispatch
- CDP types for the domains we use (structs for the ~20 methods, not the full protocol):
  - `Target.createTarget`, `Target.activateTarget`, `Target.closeTarget`, `Target.getTargets`
  - `Page.navigate`, `Page.captureScreenshot`, `Page.getFrameTree`
  - `Runtime.evaluate`, `Runtime.callFunctionOn`
  - `DOM.resolveNode`, `DOM.getBoxModel`, `DOM.describeNode`
  - `Input.dispatchMouseEvent`, `Input.dispatchKeyEvent`, `Input.insertText`
  - `Accessibility.getFullAXTree`
  - `Runtime.addBinding`, event `Runtime.bindingCalled` (critical for M7 script bridge)
  - `Page.frameNavigated`, `Page.loadEventFired` (events, for action stabilization)

**Acceptance**:
- Can connect to a Chrome launched with `--remote-debugging-port=9222`
- Can call `Runtime.evaluate({expression: "1+1"})` and get `{result: {type: "number", value: 2}}`
- Can subscribe to `Page.loadEventFired` and receive the event

**Estimated size**: ~800 lines

---

### M2: Browser Launcher + Auto-Discovery

**Goal**: Launch Chromium or find a running Chrome instance.

**Files**: `src/browser.rs`

**What to build**:
- Launch Chromium with `--remote-debugging-port=0` (auto-assign port), `--user-data-dir=~/.dev-browser/browsers/<name>/chromium-profile`, `--headless=new` (optional), `--ignore-certificate-errors` (optional)
- Read `DevToolsActivePort` file to get the assigned port and WebSocket path
- Auto-discover running Chrome: probe ports 9222-9229, read `DevToolsActivePort` from known Chrome profile paths per platform:
  - macOS: `~/Library/Application Support/Google/Chrome/DevToolsActivePort` (+ Canary, Chromium, Brave)
  - Linux: `~/.config/google-chrome/DevToolsActivePort` (+ chromium, beta, unstable, Brave)
  - Windows: `%LOCALAPPDATA%\Google\Chrome\User Data\DevToolsActivePort` (+ Beta, SxS, Chromium, Brave)
  - (Port from v1 `browser-manager.ts:493-576`)
- Fetch `http://host:port/json/version` to get `webSocketDebuggerUrl`
- Persistent profiles: `~/.dev-browser/browsers/<name>/chromium-profile` preserves cookies, localStorage, sessions
- `dev-browser install` command: download Chromium for Testing from Google's API

**Acceptance**:
- `dev-browser --headless` launches a Chromium instance and connects
- `dev-browser --connect` finds a running Chrome with debugging enabled
- `dev-browser --connect http://localhost:9222` connects to explicit endpoint
- Browser process survives CLI exit

**Depends on**: M1

**Estimated size**: ~500 lines (reuse logic from v1 `browser-manager.ts` and `connection.rs`)

---

### M3: Session Manager + Micro-Daemon

**Goal**: Persist browser connections and named pages across CLI invocations. Two modes: stateless (session file) and daemon (in-memory state).

**Files**: `src/session.rs`, `src/daemon.rs`

**What to build**:

**Session file (stateless fallback)**:
- Read/write `~/.dev-browser/sessions.json`
- Track per browser: `wsEndpoint`, `pid`, `headless`, `daemonPid`, named pages with `targetId` + `uidMap` (using `ElementRef` objects, not raw CDP ids)
- On each invocation: load session → try reconnect → if dead, clean up stale entry
- File locking (advisory) for concurrent CLI invocations

**Micro-daemon (primary mode for sessions)**:
- `dev-browser daemon start` — runs in background, same binary
- Listens on Unix socket (`~/.dev-browser/daemon.sock`) or named pipe (Windows)
- Holds persistent WebSocket to Chrome — reused across CLI invocations
- Heartbeat: ping Chrome every 2s via `Target.getTargets`. If dead: clean up session, log, auto-relaunch if configured.
- In-memory state: named pages, uidMaps, connection pool. No file-locking race conditions.
- Auto-start: first `goto`/`click`/`snap` starts the daemon if not running
- Auto-exit: after 5 min idle (no CLI invocation), daemon exits cleanly
- CLI detects daemon via socket existence → routes commands through daemon instead of direct WebSocket

**CLI dispatch logic**:
```
CLI invocation
  → daemon socket exists? → yes → send command via socket → done
                          → no  → one-shot commands (eval, tabs): stateless direct WebSocket
                                → session commands (goto, click, snap): auto-start daemon first
```

**Acceptance**:
- Run `dev-browser goto main https://example.com`, then `dev-browser eval main "document.title"` → returns "Example Domain"
- Stale sessions (dead PIDs) cleaned up automatically
- Two concurrent `dev-browser click main e1` don't corrupt state (daemon serializes)
- Chrome crash detected by heartbeat, session cleaned up, error reported
- Daemon auto-exits after 5 min idle

**Depends on**: M1, M2

**Estimated size**: ~600 lines (300 session + 300 daemon)

---

### M4: Snapshot Engine (snapshotForAI)

**Goal**: Generate token-optimized a11y tree snapshots with uid-based element targeting.

**Files**: `src/snapshot.rs`

**What to build**:
- Call `Accessibility.getFullAXTree({depth: -1})` via CDP
- Parse the AXNode tree: extract role, name, value, states (focused, disabled, expanded, selected), level (for headings)
- Assign sequential uids (`e1`, `e2`, ...) to each node
- Build `Map<uid, ElementRef>` for action resolution. `ElementRef` wraps the resolution strategy (currently `BackendNode { backend_node_id }`) so it can evolve without breaking session format
- Format as indented text:
  ```
  uid=e1 heading "Welcome" level=1
    uid=e2 textbox "Email" value="" focusable
    uid=e3 button "Submit" focusable disabled
  ```
- Filter noise: skip `none`/`ignored` roles by default, include them with `--verbose`
- Persist uidMap in session file for the page

**Acceptance**:
- `dev-browser snap main` on `https://example.com` returns a readable, compact snapshot
- uids are stable within a snapshot
- uidMap is persisted and usable by subsequent `click`/`fill` commands
- Verbose mode includes all nodes

**Depends on**: M1, M3

**Estimated size**: ~400 lines

---

### M5: Element Resolver + Input Actions

**Goal**: Resolve a uid to a DOM element and dispatch actions via CDP.

**Files**: `src/element.rs`

**What to build**:
- uid → `ElementRef` (from uidMap in session)
- `ElementRef::resolve()` → currently `DOM.resolveNode({backendNodeId})` → `objectId`. Abstracted so future resolution strategies (BiDi, objectId, selector fallback) only change the resolver, not the caller.
- `DOM.getBoxModel({backendNodeId})` → center coordinates for click
- Click: `Input.dispatchMouseEvent` (mousePressed + mouseReleased at center)
- Fill: `Runtime.callFunctionOn({objectId, functionDeclaration: "function(v) { this.focus(); this.value = v; this.dispatchEvent(new Event('input', {bubbles:true})); this.dispatchEvent(new Event('change', {bubbles:true})); }"})` with value as argument
- Fill for `<select>`: detect role from uidMap, match option by text (like chrome-devtools-mcp `selectOption`)
- Type: `Input.insertText({text})` or character-by-character `Input.dispatchKeyEvent`
- Press: `Input.dispatchKeyEvent` for special keys (Enter, Tab, Escape)
- Hover: `Input.dispatchMouseEvent` (mouseMoved)
- Action stabilization: after each action, wait up to 500ms for `Page.frameNavigated` or `Page.loadEventFired`. If navigation starts, wait for completion (up to `--timeout`). If no navigation, return immediately. Prevents snapping mid-transition state.
- Error handling — self-healing errors:
  - Detached node: `"Element uid=e12 no longer exists. The page may have changed. Run 'dev-browser snap main' to get fresh uids."`
  - No snapshot: `"No snapshot for page 'main'. Run 'dev-browser snap main' first to discover elements."`
  - Not interactable: `"Element uid=e5 is not visible or is covered. Try scrolling or waiting."`

**Acceptance**:
- `dev-browser click main e4` clicks the button from the snapshot
- `dev-browser fill main e2 "user@test.com"` fills the input and triggers events
- After a click that triggers navigation, the command waits for load before returning
- Detached node returns actionable error with fix suggestion

**Depends on**: M4

**Estimated size**: ~400 lines

---

### M6: Atomic CLI Commands

**Goal**: Wire up all Tier 1 commands through clap.

**Files**: `src/main.rs`, `src/commands/*.rs`

**What to build**:
- `goto <page> <url>` → connect, resolve page (create if new), `Page.navigate`, wait for load, print URL+title
- `click <page> <uid> [--snap]` → resolve element, dispatch click, wait for stabilization, optionally snap, print result
- `fill <page> <uid> <value> [--snap]` → resolve element, fill, wait for stabilization, optionally snap, print result
- `fill-form <page> <uid=value>... [--snap]` → batch fill multiple fields in one call, print result
- `snap <page> [--verbose]` → take snapshot, print formatted tree
- `screenshot <page> [filename]` → `Page.captureScreenshot`, save to `~/.dev-browser/tmp/`, print **file path** (not base64 — reference over value)
- `eval <page> <expr>` → `Runtime.evaluate`, print JSON result
- `tabs` → `Target.getTargets`, list pages with ids and titles
- `close <page>` → `Target.closeTarget`, update session
- `status` → print session info (browser PID, pages, uptime)
- `stop` → kill browser process, clean session
- `install` → download Chromium

Each command follows the pattern: load session → connect → resolve page → act → wait for stabilization → update session → print result → exit.

The `--snap` flag on action commands (click, fill, fill-form) takes a fresh snapshot after the action and appends it to the output. This eliminates one round-trip per interaction — the agent gets the updated page state in the same call.

**Acceptance**:
- All commands from the CLI surface section of the design doc work
- `--help` output includes LLM usage guide (like v1)
- Output is JSON-parseable where relevant, human-readable otherwise

**Depends on**: M1-M5

**Estimated size**: ~600 lines total across command files

---

### M7: Helper Runtime + Script Mode (Tier 3) — v2.1

**Deferred to v2.1.** Ship M1-M6 + M8 + M9 as v2.0 first. Tier 1 + `--snap` + Tier 2 `eval` covers 90%+ of agent interactions. Validate with real agents before committing to Tier 3 complexity.

**Why deferred**: The `Runtime.addBinding` bridge is architecturally complex — comparable to the QuickJS bridge it replaces. `goto()` destroys the JS context, requiring re-injection of helpers and promise resolution across page navigations. This is the same class of problem v1 solved with 69K lines of forked Playwright. Rushing it risks recreating the same complexity.

**Goal**: Execute multi-step scripts with batched actions in a single CLI call.

**Files**: `src/commands/run.rs`, `src/helpers.rs`, `helpers/runtime.js`

**Design (for v2.1 planning)**:
- `helpers/runtime.js` (~200 lines): `waitForSelector`, `click`, `fill`, `waitForURL`, `type`, `press`
- `Runtime.addBinding` bridge for CDP-level operations (`goto`, `snap`, `screenshot`):

```
Script: await goto("https://example.com")
  → calls __devBrowserGoto("https://example.com")
  → CDP event: Runtime.bindingCalled {name: "__devBrowserGoto", payload: "..."}
  → Rust/Daemon: Page.navigate + wait for load + re-inject helpers
  → Rust/Daemon: Runtime.evaluate("__resolveBinding('goto-1', {url, title})")
  → Script: goto() promise resolves
```

- Console capture via async IIFE wrapper
- Support `dev-browser run <page> <file>` and stdin

**Acceptance (v2.1)**:
- Multi-step scripts execute in a single CLI call
- `goto()` navigates without killing the script
- `snap()` returns fresh snapshot mid-script
- Timeout kills the script cleanly

**Depends on**: M1-M6, real-world validation of v2.0

**Estimated size**: ~500 lines Rust + ~200 lines JS

---

### M8: LLM Guide + Help Output

**Goal**: Make the CLI self-documenting for AI agents.

**Files**: `src/main.rs` (help text), `llm-guide.txt`

**What to build**:
- `--help` includes a full LLM usage guide (like v1) with:
  - Sandbox environment description (what's available, what's not)
  - Quick examples for each tier
  - Common patterns (snap → click → snap loop)
  - Tips for token efficiency
- `llm-guide.txt` embedded via `include_str!` in `after_long_help`

**Acceptance**:
- `dev-browser --help` output is sufficient for an agent to use the tool without any other documentation
- Examples cover all three tiers

**Depends on**: M6 (M7 examples added in v2.1 update)

**Estimated size**: ~200 lines

---

### M9: Integration Tests

**Goal**: Validate end-to-end behavior against a real Chrome instance.

**Files**: `tests/integration/*.rs`

**What to build**:
- Test harness: launch headless Chromium, run commands, assert output
- Test cases:
  - Atomic: goto → snap → click → fill → screenshot
  - Eval: expression evaluation, object return, error handling
  - Script: multi-step batch with goto + snap + click
  - Session: persistence across invocations (named pages survive)
  - Error: timeout, detached node, dead browser reconnect
  - Auto-discover: launch Chrome, verify `--connect` finds it
- CI-friendly: headless only, cleanup after each test

**Depends on**: M1-M8

**Estimated size**: ~500 lines

---

### M10: v1 Compatibility Shim (optional)

**Goal**: Let existing agents that use `dev-browser <<'EOF' ... browser.getPage("main") ...` migrate gradually.

**Files**: `src/commands/compat.rs`

**What to build**:
- Detect v1-style scripts (contain `browser.getPage`, `browser.listPages`, `browser.newPage`, `saveScreenshot`)
- Translate to v2 equivalents:
  - `browser.getPage("name")` → use the `--page` argument
  - `page.goto(url)` → `goto(url)`
  - `page.click(selector)` → `click(selector)` (CSS selector fallback via `document.querySelector`)
  - `page.snapshotForAI()` → `snap()`
  - `saveScreenshot(buf, name)` → `screenshot(name)`
  - `console.log(...)` → pass through
- Not a full Playwright API shim — just the ~17 methods from v1's LLM guide
- Print deprecation warning suggesting v2 syntax

**Acceptance**:
- v1-style scripts from the existing LLM guide work without modification
- Deprecation warning printed to stderr

**Depends on**: M7

**Estimated size**: ~200 lines

---

## Milestone Dependency Graph

## v2.0 Scope vs v2.1

```
v2.0 (ship first):  M1 → M2 → M3 → M4 → M5 → M6 → M8 → M9
v2.1 (after validation): M7 (Script Mode) → M10 (v1 Compat)
```

## Milestone Dependency Graph (v2.0)

```
M1 (CDP Client)
├── M2 (Browser Launcher)
│   └── M3 (Session Manager + Micro-Daemon)
│       ├── M4 (Snapshot Engine + ElementRef)
│       │   └── M5 (Element Resolver + Action Stabilization)
│       │       └── M6 (Atomic Commands + --snap + fill-form)
│       │           ├── M8 (LLM Guide)
│       │           └── M9 (Integration Tests)
│       └── M6 (partial — goto, eval, tabs don't need M4/M5)
```

v2.1 additions (after v2.0 validated with real agents):
```
M6 → M7 (Script Mode + Runtime.addBinding bridge)
M7 → M10 (v1 Compat Shim)
```

Critical path (v2.0): M1 → M2 → M3 → M4 → M5 → M6 → M9

Parallelizable:
- M4 (Snapshot) and M6-partial (goto/eval/tabs) can start once M3 is done
- M8 (LLM Guide) can be written in parallel with M9
- M9 (Tests) can be incrementally added alongside each milestone

## Size Summary

### v2.0 (ship first)

| Milestone | Estimated lines |
|-----------|----------------|
| M1 CDP Client | ~800 |
| M2 Browser Launcher | ~500 |
| M3 Session Manager + Micro-Daemon | ~600 |
| M4 Snapshot Engine + ElementRef | ~450 |
| M5 Element Resolver + Stabilization | ~500 |
| M6 Atomic Commands + --snap + fill-form | ~700 |
| M8 LLM Guide | ~200 |
| M9 Integration Tests | ~500 |
| **v2.0 Total** | **~4250 Rust** |

### v2.1 (after validation)

| Milestone | Estimated lines |
|-----------|----------------|
| M7 Script Mode + Binding Bridge | ~500 Rust + ~200 JS |
| M10 v1 Compat Shim | ~200 |
| **v2.1 Total** | **~700 Rust + ~200 JS** |

### Grand Total: ~4950 Rust + ~200 JS

Down from 76K lines (1K Rust + 75K TypeScript) in v1.
