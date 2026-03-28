# aibrowsr

[![Crates.io](https://img.shields.io/crates/v/aibrowsr)](https://crates.io/crates/aibrowsr)
[![npm](https://img.shields.io/npm/v/aibrowsr)](https://www.npmjs.com/package/aibrowsr)
[![CI](https://github.com/sderosiaux/aibrowsr/actions/workflows/ci.yml/badge.svg)](https://github.com/sderosiaux/aibrowsr/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust 2024](https://img.shields.io/badge/Rust-2024_edition-orange)](https://doc.rust-lang.org/edition-guide/rust-2024/)

<p align="center">
  <img src="docs/hero.png" alt="aibrowsr — AI agent looking through a browser's accessibility tree" width="700">
</p>

**Browser automation that speaks LLM.**

Playwright returns 2,000 tokens of raw HTML. aibrowsr returns 50 tokens of accessibility tree with stable element IDs. No CSS selectors to write, no DOM to parse.

```bash
aibrowsr goto news.ycombinator.com --inspect

# ~50 tokens instead of ~2,000:
uid=n1 RootWebArea "Hacker News"
  uid=n50 heading "Hacker News" level=1
  uid=n82 link "Show HN: A New Browser Tool"
  uid=n97 link "Rust 2025 Edition Announced"
  ...

# Click + see the new page in one call:
aibrowsr click n82 --inspect
```

UIDs are based on Chrome's `backendNodeId`. They don't change between inspects. Click `n82` now or five minutes from now.

```
aibrowsr (3 MB Rust binary)
    | CDP over WebSocket
    v
Chrome (headless, no Node.js, no runtime)
```

### Why this exists

| If you've hit this... | aibrowsr does this instead |
|---|---|
| Playwright snapshots burn 2K tokens | a11y tree: ~50 tokens. 40x less context spent on page state. |
| CSS selectors break after every deploy | UIDs from Chrome's `backendNodeId`. Stable as long as the DOM node exists. |
| Click then inspect = 2 round-trips | `--inspect` on any command. One call, action + observation. |
| 200MB of Node + npm + Playwright | 3 MB binary. `npx aibrowsr` works out of the box. |
| Cloudflare blocks your headless Chrome | 7 CDP patches. `Runtime.enable` never called (the detection vector nobody talks about). |
| Writing per-site scraping selectors | `read` for articles, `extract` for lists/tables/cards, `network` for API payloads. No selectors. |
| Errors are stack traces | `{"ok":false, "error":"...", "hint":"run inspect"}` -- parseable, actionable. |
| Each command launches a fresh browser | Sessions persist. Chrome stays alive between calls. ~10ms startup. |
| Agent can't access your logged-in accounts | `--copy-cookies` grabs cookies from your real Chrome. Works with X.com, Gmail, dashboards. |
| Infinite scroll shows 10 items | `inspect --scroll --limit 50` scrolls and collects. Tested on X.com: 50 tweets from a live timeline. |
| Two agents sharing one browser = chaos | `--browser agent1`, `--browser agent2`. Separate Chrome instances. |

## Install

```bash
# For AI agents -- installs a SKILL.md your agent reads automatically
npx skills add sderosiaux/aibrowsr

# Or just the binary
npm install -g aibrowsr    # prebuilt
npx aibrowsr --help        # no install
cargo install aibrowsr     # from source
```

## Quick start

```bash
# Navigate and see the page
aibrowsr goto https://example.com --inspect

# Click by uid
aibrowsr click n12 --inspect

# Fill a form
aibrowsr fill --uid n20 "user@test.com"

# CSS selectors work too
aibrowsr click --selector "button.submit"
aibrowsr fill --selector "input[name=email]" "hello@test.com"

# Article content (Readability -- like Firefox Reader Mode)
aibrowsr read

# Visible text, scoped and capped
aibrowsr text --selector "main" --truncate 500

# Run JS
aibrowsr eval "document.title"

# Screenshot (returns a file path, not binary)
aibrowsr screenshot
```

## Commands

| Command | What it does |
|---------|------------|
| `goto <url> [--inspect] [--max-depth N]` | Navigate. Auto-prefixes `https://` if missing. |
| `inspect [--verbose] [--max-depth N] [--uid nN] [--filter "role,role"] [--scroll] [--limit N]` | a11y tree with UIDs. `--scroll --limit` for infinite scroll. |
| `click <uid> [--inspect]` | Click by uid. Falls back to JS `.click()` when no box model. |
| `click --selector "css" [--inspect]` | Click by CSS selector. |
| `click --xy 100,200` | Click by coordinates. |
| `fill --uid <uid> <value> [--inspect]` | Fill input by uid. |
| `fill --selector "css" <value>` | Fill by selector. |
| `fill-form <uid=val>...` | Batch fill. |
| `read [--html] [--truncate N]` | Article extraction via Mozilla Readability. |
| `text [uid] [--selector "css"] [--truncate N]` | Visible text from page or element. |
| `eval <expression> [--selector "css"]` | JS in page context. `el` = matched element. |
| `extract [--selector "css"] [--limit N] [--scroll]` | Auto-detect repeating data. `--scroll` for lazy-loaded pages. |
| `network [--filter "pattern"] [--body] [--live N]` | Network requests and API responses. |
| `console [--level error] [--clear]` | console.log/warn/error + JS exceptions. |
| `pipe` | Persistent JSON stdin/stdout connection. |
| `wait <text\|url\|selector> <pattern>` | Wait for a condition. |
| `type <text> [--selector "css"]` | Type into focused element. |
| `press <key>` | Enter, Tab, Escape, etc. |
| `scroll <down\|up\|uid>` | Scroll page or element into view. |
| `hover <uid>` | Hover. |
| `back` | History back. |
| `screenshot [--filename name]` | Screenshot to file. |
| `tabs` | List open tabs. |
| `diff` | What changed since last inspect. |
| `close [--purge]` | Stop browser. `--purge` deletes cookies/profile. |
| `status` | Session info. |
| `stop` | Stop daemon. |

## Global flags

```
--browser <name>         Named browser profile (default: "default")
--page <name>            Named tab (default: "default")
--connect [url]          Attach to a running Chrome
--headed                 Show browser window (default: headless)
--stealth                Anti-detection patches (Cloudflare, Turnstile)
--copy-cookies           Use cookies from your real Chrome profile
--timeout <seconds>      Command timeout (default: 30)
--max-depth <N>          Limit inspect depth
--ignore-https-errors    Accept self-signed certs
--json                   Structured JSON output
```

## The loop: inspect, act, inspect

```bash
aibrowsr goto https://app.com/login --inspect
# uid=n52 textbox "Email" focusable
# uid=n58 textbox "Password" focusable
# uid=n63 button "Sign In" focusable

aibrowsr fill --uid n52 "user@test.com"
aibrowsr fill --uid n58 "password123"
aibrowsr click n63 --inspect
# uid=n101 heading "Dashboard" level=1
```

UIDs stay the same between inspects as long as the DOM node exists.

## Content extraction

From least to most tokens:

```bash
# Articles (Readability, like Firefox Reader Mode)
aibrowsr read

# Repeating data -- products, search results, feeds. No selectors.
aibrowsr extract
# Uses MDR/DEPTA heuristics. Finds the pattern automatically.

# Scoped visible text
aibrowsr text --selector "[role=main]" --truncate 1000

# API responses -- skip the DOM
aibrowsr network --filter "api" --body
```

## Stealth

`--stealth` patches 7 automation fingerprints via CDP:

- `navigator.webdriver` set to `undefined`
- `chrome.runtime` mocked
- Permissions API fixed
- WebGL renderer masked
- User-Agent cleaned
- Input coordinate leak patched
- `Runtime.enable` never called

These are CDP-level patches (`Page.addScriptToEvaluateOnNewDocument`), not Chrome flags.

For sites with heavier protection (DataDome, Kasada) that fingerprint the Chromium binary itself, connect to your real Chrome:

```bash
google-chrome --remote-debugging-port=9222 &
aibrowsr --connect http://127.0.0.1:9222 goto https://www.leboncoin.fr --inspect
```

| Protection | Solution |
|---|---|
| None | `aibrowsr goto ...` |
| Cloudflare/Turnstile | `aibrowsr --stealth goto ...` |
| Logged-in sites | `aibrowsr --stealth --copy-cookies goto ...` |
| DataDome/Kasada | `aibrowsr --connect` to real Chrome |

## Logged-in sites

`--copy-cookies` copies the cookie database from your Chrome profile. Both Chrome instances use the same macOS Keychain, so encrypted cookies just work.

```bash
aibrowsr --stealth --copy-cookies goto x.com/home --inspect
# Your timeline. Your DMs. No login flow.

aibrowsr --copy-cookies goto mail.google.com --inspect
aibrowsr --copy-cookies goto github.com/notifications --inspect
```

Your real Chrome is not affected.

## Network and console capture

```bash
# Resources already loaded (stealth-safe, uses Performance API)
aibrowsr network --filter "api"

# Live traffic with response bodies
aibrowsr network --live 5 --body --filter "graphql"

# Console output
aibrowsr console --level error    # errors + exceptions only
```

Console capture uses an injected interceptor, not `Runtime.enable`.

## Pipe mode

For agents that send many commands in sequence, pipe mode keeps a single connection open:

```bash
echo '{"cmd":"goto","url":"https://example.com","inspect":true}
{"cmd":"click","uid":"n12","inspect":true}
{"cmd":"read"}' | aibrowsr pipe
```

One JSON line per response. About 10x faster than spawning a process per command.

## JSON mode

```bash
aibrowsr --json goto https://example.com --inspect
# {"ok":true,"url":"...","title":"...","snapshot":"uid=n1 heading..."}

aibrowsr --json eval "1+1"
# {"ok":true,"result":2}

# Errors exit 0 so agents can always parse stdout:
aibrowsr --json click n99
# {"ok":false,"error":"Element uid=n99 not found.","hint":"Run 'aibrowsr inspect'"}
```

## Multi-tab and parallel agents

```bash
# Multiple tabs in one browser
aibrowsr --page main goto https://app.com
aibrowsr --page docs goto https://docs.app.com
aibrowsr --page main eval "document.title"   # "App"

# Multiple agents, each with their own Chrome
aibrowsr --browser agent1 goto https://example.com
aibrowsr --browser agent2 goto https://other.com
```

## Using with AI agents

```bash
# Install the skill (Claude Code, Cursor, Copilot, etc.)
npx skills add sderosiaux/aibrowsr

# Or tell your agent to run:
aibrowsr --help
# The help output includes a full LLM usage guide.
```

Claude Code permissions:

```json
{
  "permissions": {
    "allow": ["Bash(aibrowsr *)"]
  }
}
```

## Comparison

| | aibrowsr | dev-browser | chrome-devtools-mcp | Playwright MCP |
|---|---|---|---|---|
| Language | Rust | Rust + Node.js | TypeScript | TypeScript |
| Runtime deps | none | Node + npm + Playwright + QuickJS | Node + Puppeteer | Node + Playwright |
| Binary size | 3 MB | 3 MB CLI + 200 MB deps | npm package | npm package |
| CLI startup | ~10ms (session reuse) | ~500ms | N/A (MCP) | N/A (MCP) |
| Element targeting | uid + selector + coords | selectors + snapshotForAI | uid (sequential) | selectors |
| UID stability | backendNodeId (stable) | N/A | sequential (reassigned) | N/A |
| Action + observe | `--inspect` (1 call) | batched script | 1 call per action | 1 call per action |
| Stealth | 7 CDP patches | No | No | No |
| Reader mode | `read` (Readability) | No | No | No |
| Network capture | retroactive + live | No | No | metadata only |
| Data extraction | `extract` (auto-detect) | No | No | No |
| Console capture | stealth-safe | No | yes | No |
| Pipe mode | yes | No | No | No |
| Code | ~6.2K lines | ~76K lines | ~12K lines | Playwright |

## License

MIT
