# aibrowsr

Browser automation for AI agents. Single Rust binary, zero runtime dependencies, talks CDP directly to Chrome.

## Why

Existing tools (Playwright, Puppeteer, Selenium) carry heavy runtimes and weren't designed for agents. Agents need:
- **Minimum tokens** — a11y tree snapshots instead of raw HTML (~50 tokens vs ~2000)
- **Minimum round-trips** — `--inspect` returns updated page state with every action
- **Zero setup** — single binary, headless by default, no npm/Node required
- **Persistent sessions** — login once, stay logged in across invocations
- **Stable UIDs** — element identifiers based on `backendNodeId`, survive between inspects
- **3 targeting modes** — uid from accessibility tree, CSS selectors, or coordinates

## Install

```bash
# npm (recommended — downloads prebuilt binary)
npm install -g aibrowsr

# or with npx (no install)
npx aibrowsr --help

# or with Cargo (builds from source)
cargo install aibrowsr
```

## Quick Start

```bash
# Navigate and inspect the page in one call
aibrowsr goto https://example.com --inspect
# → https://example.com — Example Domain
# → uid=n1 RootWebArea "Example Domain"
# →   uid=n9 heading "Example Domain" level=1
# →   uid=n10 paragraph "This domain is for..."
# →   uid=n12 link "Learn more"

# Click by uid, get updated page state
aibrowsr click n12 --inspect

# Fill a form field
aibrowsr fill --uid n20 "user@test.com"

# Or target by CSS selector (when uids aren't practical)
aibrowsr click --selector "button.submit"
aibrowsr fill --selector "input[name=email]" "hello@test.com"

# Extract article content (Mozilla Readability — reader mode)
aibrowsr read

# Extract full visible text (use --selector to scope, --truncate to cap)
aibrowsr text --selector "main" --truncate 500

# Evaluate JavaScript
aibrowsr eval "document.title"

# Screenshot (returns file path, not binary data)
aibrowsr screenshot
```

## How It Works

```
aibrowsr (Rust, ~4K lines)
    │
    │ WebSocket (Chrome DevTools Protocol)
    ▼
Chrome / Chromium (headless by default)
```

No Node.js. No Playwright. No daemon required. Headless by default — `--headed` for debugging.

UIDs are stable across inspects (based on Chrome's `backendNodeId`). The agent inspects, picks a uid, acts — even minutes later. When a11y tree isn't practical, CSS selectors and coordinates work as fallbacks. Click auto-falls back to JS `.click()` when the element has no box model.

## Commands

| Command | Description |
|---------|------------|
| `goto <url> [--inspect] [--max-depth N]` | Navigate to URL |
| `inspect [--verbose] [--max-depth N] [--uid nN] [--filter "role,role"]` | Accessibility tree with stable uids |
| `click <uid> [--inspect] [--max-depth N]` | Click by uid (JS fallback if no box model) |
| `click --selector "css" [--inspect]` | Click by CSS selector |
| `click --xy 100,200` | Click by coordinates |
| `fill --uid <uid> <value> [--inspect]` | Fill input by uid |
| `fill --selector "css" <value>` | Fill by CSS selector |
| `fill-form <uid=val>...` | Batch fill multiple fields |
| `read [--html] [--truncate N]` | Extract main content (Mozilla Readability) |
| `text [uid] [--selector "css"] [--truncate N]` | Extract visible text (page or element) |
| `eval <expression>` | Run JS in page context |
| `wait <text\|url\|selector> <pattern>` | Wait for condition |
| `type <text> [--selector "css"]` | Type into focused/selected element |
| `press <key>` | Press Enter, Tab, Escape, etc. |
| `scroll <down\|up\|uid>` | Scroll page or element into view |
| `hover <uid>` | Hover over element |
| `back` | Navigate back in history |
| `screenshot [--filename name]` | Capture screenshot → file path |
| `tabs` | List open browser tabs |
| `close [--purge]` | Close browser (--purge deletes profile/cookies) |
| `status` | Show session info |
| `stop` | Stop background daemon |

## Global Flags

```
--browser <name>         Named browser profile (default: "default")
--page <name>            Named page/tab (default: "default")
--connect [url]          Connect to running Chrome (auto or explicit)
--headed                 Show browser window (default is headless)
--timeout <seconds>      Command timeout (default: 30)
--max-depth <N>          Limit inspect tree depth (works with --inspect on any command)
--ignore-https-errors    Accept self-signed certificates
--json                   Structured JSON output for all commands
```

## The Inspect → Act → Inspect Loop

```bash
# 1. Navigate and inspect
aibrowsr goto https://app.com/login --inspect
# → uid=n47 heading "Login" level=1
#   uid=n52 textbox "Email" focusable
#   uid=n58 textbox "Password" focusable
#   uid=n63 button "Sign In" focusable

# 2. Act
aibrowsr fill --uid n52 "user@test.com"
aibrowsr fill --uid n58 "password123"

# 3. Click with --inspect to get result + new state in one call
aibrowsr click n63 --inspect
# → Clicked uid=n63
# → uid=n101 heading "Dashboard" level=1
# → uid=n105 navigation "Main menu"
```

UIDs (n47, n52, etc.) are stable — they won't change between inspects as long as the DOM node exists.

## Content Extraction

```bash
# Article content (Readability — like Firefox Reader Mode)
aibrowsr read
# → # Article Title
# → Clean article text without nav, footer, sidebar...

# Full page text (scoped by selector)
aibrowsr text --selector "[role=main]" --truncate 1000

# Structured data via JS
aibrowsr eval "JSON.stringify([...document.querySelectorAll('h2')].map(e => e.textContent))"
```

## JSON Mode

```bash
aibrowsr --json goto https://example.com --inspect
# → {"ok":true,"url":"...","title":"...","snapshot":"uid=n1 heading..."}

aibrowsr --json eval "1+1"
# → {"ok":true,"result":2}

aibrowsr --json read
# → {"ok":true,"title":"...","text":"...","excerpt":"...","byline":"..."}

# Errors also structured (exit 0 for agent parsing):
aibrowsr --json click n99
# → {"ok":false,"error":"Element uid=n99 not found.","hint":"Run 'aibrowsr inspect'"}
```

## Multi-Tab

```bash
aibrowsr --page main goto https://app.com
aibrowsr --page docs goto https://docs.app.com
aibrowsr --page main eval "document.title"   # → "App"
aibrowsr --page docs eval "document.title"   # → "Docs"
```

## Using with AI Agents

Tell your agent to run `aibrowsr --help` — the help output includes a complete LLM usage guide. No plugin needed.

### Claude Code

```json
{
  "permissions": {
    "allow": ["Bash(aibrowsr *)"]
  }
}
```

### Connect to Your Browser

```bash
aibrowsr --connect inspect    # auto-discover Chrome with debugging
google-chrome --remote-debugging-port=9222  # or launch manually
```

## Comparison

| | aibrowsr | Playwright MCP | dev-browser v1 | chrome-devtools-mcp |
|---|---|---|---|---|
| Runtime deps | none | Node.js | Node.js + npm | Node.js |
| Binary size | ~3 MB | ~200 MB | ~200 MB | ~200 MB |
| Startup | ~10ms | ~500ms | ~500ms | ~500ms |
| Element targeting | uid + selector + xy | CSS selectors | CSS selectors | uid |
| UID stability | backendNodeId (stable) | — | — | sequential (unstable) |
| Action + observe | `--inspect` flag | 2 calls | 1 script | 2 calls |
| Reader mode | `read` (Readability) | — | — | — |
| Code | ~4K Rust | Playwright | 76K (69K fork) | ~12K TS |

## License

MIT
