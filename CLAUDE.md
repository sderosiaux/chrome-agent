# aibrowsr

Single Rust binary for browser automation via CDP. Built for AI agents.
~5K lines Rust, zero runtime dependencies, 3.4 MB binary.

## Architecture

```
CLI (clap) → CDP Client (WebSocket) → Chrome
```

| Module | Role |
|--------|------|
| `src/cdp/` | WebSocket transport, message correlation, CDP types |
| `src/commands/` | 25 commands: goto, click, fill, inspect, eval, text, read, wait, screenshot, tabs, network, console... |
| `src/element.rs` | uid/selector/coordinate resolution → CDP input dispatch, JS click fallback |
| `src/element_ref.rs` | ElementRef abstraction (decouples from CDP internals) |
| `src/snapshot.rs` | Accessibility tree → compact text with stable uids (backendNodeId) |
| `src/session.rs` | JSON session persistence (~/.aibrowsr/sessions.json, 0600 perms) |
| `src/browser.rs` | Chrome launch, auto-discovery, profile management |
| `src/daemon.rs` | Optional micro-daemon (Unix only), heartbeat, crash recovery |
| `src/pipe.rs` | Pipe mode: persistent connection, JSON stdin/stdout protocol |
| `src/setup.rs` | Stealth patches extracted (shared by main.rs + pipe.rs) |
| `src/run_helpers.rs` | Shared output/error handling extracted from main.rs |
| `vendor/Readability.js` | Mozilla Readability (90KB, MIT) embedded via include_str! |
| `npm/` | npm distribution wrapper (postinstall downloads native binary) |

## Build & Test

```bash
cargo build
cargo test
cargo clippy -- -D warnings  # zero warnings enforced in CI
```

## Release

```bash
./scripts/release.sh 0.2.0
# → bumps Cargo.toml + npm/package.json
# → commits, tags v0.2.0, pushes
# → GitHub Actions: builds 5 platform binaries, creates release, publishes npm
# → Requires NPM_TOKEN in GitHub secrets
```

## Key Design Decisions

- **Headless by default** — `--headed` for debug. Mode mismatch auto-kills old browser.
- **`--stealth` mode** — 7 CDP patches: navigator.webdriver, chrome.runtime, WebGL, UA, Permissions, input screenX/pageX leak, Runtime.enable skipped. Bypasses Cloudflare/Turnstile.
- **`--connect` for heavy protection** — DataDome/Kasada detect bundled Chromium fingerprints. Connect to real installed Chrome instead (`--connect http://127.0.0.1:9222`).
- **Stable UIDs** — `n{backendNodeId}` instead of sequential `e1, e2`. Survive between inspects on same page. Change after SPA navigation (re-inspect needed).
- **3 targeting modes** — uid (from inspect), CSS selector (`--selector`), coordinates (`--xy`)
- **JS click fallback** — when a11y reports "disabled" but DOM isn't, click falls back to `.click()`
- **ElementRef abstraction** — session stores `{"type":"backendNode","id":N}`, ready for BiDi
- **Noise filtering** — StaticText/InlineTextBox stripped (66% token reduction), `--filter` by role
- **`--json` mode** — errors exit 0 with `{"ok":false}`. Agents parse stdout, not exit codes.
- **Self-healing errors** — every error includes a `hint` field suggesting the next action
- **Reader mode** — `read` injects Mozilla Readability.js for article extraction (~500 tokens vs ~15K)
- **Content extraction hierarchy** — `read` (articles) > `text --selector` (scoped) > `text` (full page) > `eval` (structured JS)
- **`--max-depth`** — accepted both as global flag and per-command (after `--inspect`)
- **`close --purge`** — removes browser profile to prevent orphan directory accumulation

## Gotchas

- CDP `rename_all = "camelCase"` fails on acronyms: use `#[serde(rename = "backendDOMNodeId")]`
- Browser-level WebSocket only supports `Target.*`. Page commands need page WS via `/json/list`.
- `Accessibility.getFullAXTree` returns a flat list with parentId/childIds, not a tree.
- Some AXRelatedNode fields may be missing — `Option<T>` + `#[serde(default)]` everywhere.
- `text --selector "main"` auto-falls back to `[role=main]` for ARIA compatibility.
- Readability.js can fail on non-article pages — wrapped in try-catch with descriptive error.
- `--stealth` patches are CDP-level (Page.addScriptToEvaluateOnNewDocument), not Chrome flags. `--disable-blink-features=AutomationControlled` is a myth — doesn't work on modern Chrome.
- After SPA navigation (`back`, `click` that triggers route change), UIDs change. Always re-inspect.
- For SPA product/detail pages, prefer `goto <direct-url>` over `click <link-uid>` — click may open a modal instead of navigating.
- DataDome/Kasada: `--stealth` is NOT enough. These detect Chromium binary fingerprints (canvas, audio, codecs). Use `--connect` to a real installed Chrome. Tested: Leboncoin passes with `--connect`, fails with `--stealth` alone.
- `Runtime.evaluate` works WITHOUT `Runtime.enable`. Stealth mode skips `Runtime.enable` to avoid the #1 CDP detection vector.

## Linting

Zero warnings enforced. Clippy pedantic + nursery enabled with targeted suppressions in Cargo.toml.
CI runs `cargo clippy -- -D warnings`. Any warning = build failure.
