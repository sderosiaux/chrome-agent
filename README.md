# aibrowsr

[![Crates.io](https://img.shields.io/crates/v/aibrowsr)](https://crates.io/crates/aibrowsr)
[![npm](https://img.shields.io/npm/v/aibrowsr)](https://www.npmjs.com/package/aibrowsr)
[![CI](https://github.com/sderosiaux/aibrowsr/actions/workflows/ci.yml/badge.svg)](https://github.com/sderosiaux/aibrowsr/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust 2024](https://img.shields.io/badge/Rust-2024_edition-orange)](https://doc.rust-lang.org/edition-guide/rust-2024/)

<p align="center">
  <img src="docs/hero.png" alt="aibrowsr — AI agent looking through a browser's accessibility tree" width="700">
</p>

**The browser tool that thinks like your agent does.**

Playwright, Puppeteer, Selenium — they were built for humans writing test scripts. aibrowsr was built for LLMs issuing commands. That distinction changes everything.

```bash
# Your agent runs this:
aibrowsr goto news.ycombinator.com --inspect

# Gets back ~50 tokens instead of ~2,000:
uid=n1 RootWebArea "Hacker News"
  uid=n50 heading "Hacker News" level=1
  uid=n82 link "Show HN: A New Browser Tool"
  uid=n97 link "Rust 2025 Edition Announced"
  ...

# Clicks a link, gets the new page state in the same call:
aibrowsr click n82 --inspect
```

No CSS selectors to guess, no DOM to parse, no flaky locators. The agent reads UIDs, acts on them. They're stable across inspects — click `n82` now or 5 minutes from now.

```
aibrowsr (single 3 MB Rust binary, Rust 2024)
    │ CDP over WebSocket
    ▼
Chrome (headless, no Node.js, no runtime deps)
```

### What makes it different

| Problem with existing tools | aibrowsr's answer |
|---|---|
| Playwright returns ~2K tokens of raw HTML | **40x fewer tokens** — a11y tree snapshots. Agents read what matters. |
| Elements identified by CSS selectors that break | **Stable UIDs** — based on Chrome's `backendNodeId`, survive across inspects. |
| Action then observe = 2 round-trips | **1 call** — every command accepts `--inspect`. Click + see result together. |
| Need Node.js, npm, Playwright, 200MB runtime | **Single 3 MB binary**. `npx aibrowsr` works immediately. No deps. |
| Headless detected by Cloudflare, Turnstile | **7 CDP stealth patches** built-in. `Runtime.enable` never called. |
| Scraping requires writing selectors per site | **Smart extraction** — `read` (articles), `extract` (repeating data), `network` (APIs). |
| Errors are stack traces agents can't parse | **Agent-native errors** — `{"ok":false, "error":"...", "hint":"try this"}` |
| Each command cold-boots a browser | **10ms startup** — persistent sessions. Chrome stays alive between calls. |
| Can't access logged-in sites | **`--copy-cookies`** — reuse your real Chrome sessions. X.com, Gmail, dashboards. |
| Infinite scroll loads 10 items | **Scroll-collect** — `inspect --scroll --limit 50` collects from virtualized lists. X.com: 50 tweets. |
| Parallel agents corrupt shared state | **`--browser <name>`** — isolated Chrome instances per agent. |

## Install

### For AI agents (recommended)

```bash
# Install the skill — your agent learns aibrowsr automatically
npx skills add sderosiaux/aibrowsr
```

This installs a `SKILL.md` that teaches your agent (Claude Code, Cursor, Copilot, etc.) how to use aibrowsr, including the workflow, commands, and best practices.

### CLI binary

```bash
# npm (downloads prebuilt binary)
npm install -g aibrowsr

# or with npx (no install needed)
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
| `eval <expression> [--selector "css"]` | Run JS in page context (`el` = matched element) |
| `network [--filter "pattern"] [--body] [--live N]` | Capture network requests / API responses |
| `console [--level error] [--clear]` | Show captured console.log/warn/error + JS exceptions |
| `pipe` | Persistent connection: JSON stdin → JSON stdout |
| `wait <text\|url\|selector> <pattern>` | Wait for condition |
| `type <text> [--selector "css"]` | Type into focused/selected element |
| `press <key>` | Press Enter, Tab, Escape, etc. |
| `scroll <down\|up\|uid>` | Scroll page or element into view |
| `hover <uid>` | Hover over element |
| `back` | Navigate back in history |
| `screenshot [--filename name]` | Capture screenshot → file path |
| `tabs` | List open browser tabs |
| `extract [--selector "css"] [--limit N] [--scroll]` | Auto-detect repeating data (--scroll for lazy-loaded pages) |
| `diff` | Compare current page state to last inspect snapshot |
| `close [--purge]` | Close browser (--purge deletes profile/cookies) |
| `status` | Show session info |
| `stop` | Stop background daemon |

## Global Flags

```
--browser <name>         Named browser profile (default: "default")
--page <name>            Named page/tab (default: "default")
--connect [url]          Connect to running Chrome (auto or explicit)
--headed                 Show browser window (default is headless)
--stealth                Bypass bot detection (Cloudflare, Turnstile)
--timeout <seconds>      Command timeout (default: 30)
--max-depth <N>          Limit inspect tree depth (works with --inspect on any command)
--copy-cookies           Copy cookies from your real Chrome (access logged-in sites)
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

## Network Capture

Extract API data directly instead of DOM scraping:

```bash
# Show resources loaded by the page (stealth-safe, uses Performance API)
aibrowsr network --filter "api"

# Capture live traffic with response bodies (5 seconds)
aibrowsr network --live 5 --body --filter "graphql"

# JSON output for structured extraction
aibrowsr --json network --body --filter "api" --limit 10
```

## Console Capture

See what the page logs — useful for debugging and error detection:

```bash
aibrowsr console                    # all messages
aibrowsr console --level error      # errors + exceptions only
aibrowsr console --clear            # read and clear buffer
```

Stealth-safe: uses injected interceptor, not `Runtime.enable`.

## Pipe Mode

Persistent connection for high-performance agent workflows:

```bash
# Start pipe (one connection, reads JSON from stdin)
echo '{"cmd":"goto","url":"https://example.com","inspect":true}
{"cmd":"click","uid":"n12","inspect":true}
{"cmd":"read"}' | aibrowsr pipe
```

Each command returns one JSON line: `{"ok":true,...}` or `{"ok":false,"error":"..."}`. 10x faster than spawning aibrowsr per command.

## Content Extraction — 4 levels

Pick the right tool for the job. Each returns structured data, not raw DOM.

```bash
# 1. Articles — Mozilla Readability (like Firefox Reader Mode)
aibrowsr read
# → ~300 tokens of clean markdown, no nav/footer/sidebar

# 2. Repeating data — products, news items, search results (no selectors needed)
aibrowsr extract
# → Found 30 items (pattern: TR.athing.submission)
# → 1. Title: "Show HN: ..." | URL: https://... | Price: "$99"
# Uses MDR/DEPTA heuristics: sibling similarity, content heterogeneity, nav filtering

# 3. Scoped text — visible text from a specific section
aibrowsr text --selector "[role=main]" --truncate 1000

# 4. API responses — skip the DOM entirely
aibrowsr network --filter "api" --body
```

## Stealth Mode

Many sites (Cloudflare, Turnstile) block headless Chrome. `--stealth` patches 7 automation fingerprints via CDP:

```bash
aibrowsr --stealth goto https://protected-site.com --inspect
```

What it patches:
- `navigator.webdriver` → `undefined`
- `chrome.runtime` → mocked (headless doesn't have it)
- Permissions API → consistent with real browser
- WebGL renderer → masks ANGLE/headless fingerprint
- User-Agent → removes "HeadlessChrome"
- Input `screenX`/`pageX` leak → random offset added
- `Runtime.enable` → skipped (the #1 CDP detection vector)

All patches are CDP-level (`Page.addScriptToEvaluateOnNewDocument`). No fake Chrome flags.

### Heavy bot protection (DataDome, Kasada)

Some sites (Leboncoin, etc.) use advanced fingerprinting that detects bundled Chromium regardless of CDP patches. For these, connect to your real installed Chrome instead:

```bash
# Launch your real Chrome with debugging enabled
google-chrome --remote-debugging-port=9222 &

# Connect aibrowsr to it
aibrowsr --connect http://127.0.0.1:9222 goto https://www.leboncoin.fr --inspect
```

Real Chrome has genuine canvas/audio/codec fingerprints that Chromium lacks.

| Protection Level | Solution |
|---|---|
| None | `aibrowsr goto ...` |
| Cloudflare/Turnstile | `aibrowsr --stealth goto ...` |
| Logged-in sites | `aibrowsr --stealth --copy-cookies goto ...` |
| DataDome/Kasada | `aibrowsr --connect` to real Chrome |

## Logged-in Sessions

Access sites where you're already logged in — no manual login needed:

```bash
# Copy cookies from your real Chrome profile
aibrowsr --stealth --copy-cookies goto x.com/home --inspect
# → Logged in as you. Sees your timeline, DMs, notifications.

aibrowsr --copy-cookies goto mail.google.com --inspect
# → Your Gmail inbox.

aibrowsr --copy-cookies goto github.com/notifications --inspect
# → Your GitHub notifications.
```

`--copy-cookies` copies the Cookies database from your Chrome Default profile into the aibrowsr browser profile. Works because both use the same macOS Keychain for cookie decryption. Your real Chrome stays untouched.

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

### Parallel Agents

Multiple agents sharing the same browser corrupt each other's sessions. Isolate with `--browser`:

```bash
# Agent 1
aibrowsr --browser agent1 goto https://example.com

# Agent 2 (separate Chrome instance)
aibrowsr --browser agent2 goto https://other.com
```

## Using with AI Agents

### Skill (recommended)

```bash
npx skills add sderosiaux/aibrowsr
```

This installs a SKILL.md that teaches your agent the full aibrowsr workflow, commands, and tips. Works with Claude Code, Cursor, Copilot, and any agent that reads skill files.

### Manual

Tell your agent to run `aibrowsr --help` — the help output includes a complete LLM usage guide.

### Claude Code permissions

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

| | aibrowsr | dev-browser | chrome-devtools-mcp | Playwright MCP |
|---|---|---|---|---|
| Language | Rust | Rust + Node.js | TypeScript | TypeScript |
| Runtime deps | none | Node.js + npm + Playwright + QuickJS | Node.js + Puppeteer | Node.js + Playwright |
| Binary size | ~3 MB | ~3 MB (CLI) + ~200 MB (daemon + deps) | npm package | npm package |
| CLI startup (reuse session) | ~10ms | ~500ms (daemon check) | N/A (MCP server) | N/A (MCP server) |
| Element targeting | uid + CSS selector + coordinates | CSS selectors + snapshotForAI | uid (sequential) | CSS selectors |
| UID stability | backendNodeId (stable across inspects) | N/A | sequential (reassigned each snapshot) | N/A |
| Action + observe | `--inspect` flag (1 call) | 1 script (batched) | 1 MCP call per action | 1 MCP call per action |
| Script batching | No (atomic commands + eval) | Full JS scripts in QuickJS sandbox | No | No |
| Stealth mode | 7 CDP patches + Runtime.enable skip | No | No | No |
| Reader mode | `read` (Mozilla Readability) | No | No | No |
| Sandbox | Chrome sandbox | QuickJS WASM sandbox | Chrome sandbox | No |
| Network capture | Retroactive + live | No | No | Metadata only (no bodies) |
| Console capture | Stealth-safe interceptor | No | Console messages | No |
| Pipe mode | JSON stdin/stdout | No | No | No |
| Data extraction | `extract` (auto-detect repeating patterns) | No | No | No |
| Code | ~6.2K lines | ~76K lines (69K Playwright fork) | ~12K lines | Playwright |

## License

MIT
