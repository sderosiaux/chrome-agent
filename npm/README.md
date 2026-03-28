# aibrowsr

Browser automation for AI agents. Single Rust binary, zero runtime dependencies, talks CDP directly to Chrome.

## Why

Existing browser automation tools (Playwright, Puppeteer, Selenium) carry heavy runtimes (Node.js, Python) and weren't designed for AI agents. Agents need:
- **Minimum tokens** — a11y tree snapshots instead of raw HTML (~50 tokens vs ~2000)
- **Minimum round-trips** — `--inspect` flag returns updated page state with every action
- **Zero setup** — single binary, no npm install, no runtime dependencies
- **Persistent sessions** — login once, stay logged in across invocations

aibrowsr is a ~3K line Rust binary that replaces the entire Playwright/Puppeteer stack for agent use cases.

## Install

```bash
cargo install aibrowsr
```

Or build from source:

```bash
git clone https://github.com/sderosiaux/aibrowsr.git
cd aibrowsr
cargo build --release
```

## Quick Start

```bash
# Navigate to a page
aibrowsr goto https://example.com

# Inspect the page (accessibility tree, token-optimized)
aibrowsr inspect
# → uid=e1 heading "Example Domain" level=1
#   uid=e2 link "More information..." focusable

# Click an element by uid
aibrowsr click e2 --inspect

# Fill a form field
aibrowsr fill e5 "user@test.com"

# Evaluate JavaScript
aibrowsr eval "document.title"
# → "Example Domain"

# Screenshot (returns file path)
aibrowsr screenshot
# → ~/.aibrowsr/tmp/screenshot-1711540200.png
```

## How It Works

```
aibrowsr (Rust, ~3K lines)
    │
    │ WebSocket (CDP protocol)
    ▼
Chrome / Chromium
```

No Node.js. No Playwright. No QuickJS sandbox. No daemon required.

aibrowsr talks Chrome DevTools Protocol directly. The a11y tree snapshot assigns a unique `uid` to each element. The agent reads the snapshot, picks a uid, sends the action. No CSS selector guessing.

## Commands

| Command | Description |
|---------|------------|
| `goto <url>` | Navigate to URL |
| `inspect [--verbose]` | Inspect page accessibility tree with uids |
| `click <uid> [--inspect]` | Click element by uid |
| `fill <uid> <value> [--inspect]` | Fill input by uid |
| `fill-form <uid=val>... [--inspect]` | Batch fill multiple fields |
| `eval <expression>` | Evaluate JS in page context |
| `screenshot [--filename]` | Capture screenshot, return file path |
| `tabs` | List open browser tabs |
| `close` | Close managed browser |
| `status` | Show session info |
| `stop` | Stop background daemon |

## Global Flags

```
--browser <name>         Named browser profile (default: "default")
--connect [url]          Connect to running Chrome (auto-discover or explicit)
--headed               Show browser window (default is headless)
--timeout <seconds>      Command timeout (default: 30)
--ignore-https-errors    Accept self-signed certificates
```

## The Inspect → Act → Inspect Loop

The core workflow for agents:

```bash
# 1. Inspect to discover the page
aibrowsr inspect
# → uid=e1 heading "Login" level=1
#   uid=e2 textbox "Email" value="" focusable
#   uid=e3 textbox "Password" value="" focusable
#   uid=e4 button "Sign In" focusable

# 2. Act using uids from the snapshot
aibrowsr fill e2 "user@test.com"
aibrowsr fill e3 "password123"

# 3. Click with --inspect to get updated state in one call
aibrowsr click e4 --inspect
# → Clicked uid=e4
# → uid=e1 heading "Dashboard" level=1
#   uid=e2 navigation "Main menu"
#   ...
```

The `--inspect` flag eliminates one round-trip per interaction — the agent gets the action result and updated page state in a single call.

## Using with AI Agents

Tell your agent to run `aibrowsr --help` — the help output includes a complete LLM usage guide with examples and patterns. No plugin or skill installation needed.

### Allowing in Claude Code

```json
{
  "permissions": {
    "allow": ["Bash(aibrowsr *)"]
  }
}
```

### Connect to Your Browser

```bash
# Auto-discover Chrome with debugging enabled
aibrowsr --connect snap

# Or launch Chrome manually with:
google-chrome --remote-debugging-port=9222
```

## Architecture

- **CDP Client** — async WebSocket transport with request/response correlation and event subscription
- **Browser Launcher** — auto-discover running Chrome or launch managed Chromium with persistent profiles
- **Session Manager** — JSON file tracks browser connections, named pages, uid maps across invocations
- **Micro-Daemon** (optional) — persistent connection, Chrome health heartbeat, crash recovery
- **Snapshot Engine** — `Accessibility.getFullAXTree` → compact text with uid identifiers
- **Element Resolver** — uid → `ElementRef` → `DOM.resolveNode` → CDP input dispatch with action stabilization
- **ElementRef abstraction** — decouples uid resolution from CDP internals, ready for WebDriver BiDi

## Comparison

| | aibrowsr | Playwright MCP | dev-browser v1 | chrome-devtools-mcp |
|---|---|---|---|---|
| Runtime deps | none | Node.js | Node.js + npm | Node.js |
| Binary size | ~10 MB | ~200 MB | ~200 MB | ~200 MB |
| Startup | ~10ms | ~500ms | ~500ms | ~500ms |
| Element targeting | uid (a11y tree) | CSS selectors | CSS selectors | uid (a11y tree) |
| Batching | --inspect flag | 1 action/call | script mode | 1 action/call |
| Code | ~3K Rust | Playwright | 76K (69K fork) | ~12K TS |

## License

MIT
