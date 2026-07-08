# chrome-agent

[![Crates.io](https://img.shields.io/crates/v/chrome-agent)](https://crates.io/crates/chrome-agent)
[![npm](https://img.shields.io/npm/v/chrome-agent)](https://www.npmjs.com/package/chrome-agent)
[![CI](https://github.com/sderosiaux/chrome-agent/actions/workflows/ci.yml/badge.svg)](https://github.com/sderosiaux/chrome-agent/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust 2024](https://img.shields.io/badge/Rust-2024_edition-orange)](https://doc.rust-lang.org/edition-guide/rust-2024/)

<p align="center">
  <img src="docs/hero-logo.png" alt="chrome-agent — Browser automation for AI agents" width="500">
</p>

<p align="center">
  <strong>Browser automation that speaks LLM.</strong>
</p>

<p align="center">
  <a href="README.md">English</a> | <a href="README.cn.md">简体中文</a>
</p>

> **Disclaimer:** This is an independent, community-driven project. It is not affiliated with, endorsed by, or sponsored by Google or the Chrome team.

> You're not the user. Your LLM is.
>
> You don't need to read this README. Your agent does. Install it, run `chrome-agent --help`, and let the LLM figure it out. The CLI embeds its own usage guide, every error comes with a hint for the next action, and `--json` mode outputs structured data an agent can parse without you writing a single adapter. This page is here because GitHub expects one.

## How is this different from agent-browser?

[agent-browser](https://github.com/vercel-labs/agent-browser) (Vercel) is a feature-complete browser automation platform: dashboard, cloud providers, annotated screenshots, iOS support, AI chat, auth vault, 40K lines of Rust. It's excellent.

chrome-agent is the opposite bet. Instead of adding features, it removes tokens.

| | chrome-agent | agent-browser |
|---|---|---|
| **Page snapshot** | ~50 tokens (a11y noise stripped, 66% reduction) | ~200 tokens (full a11y tree) |
| **Element IDs** | `backendNodeId` — stable across inspects | Sequential `@e1, @e2` — reassigned every snapshot |
| **Action + observe** | `click n12 --inspect` (1 call) | `click @e1` then `snapshot` (2 calls) |
| **Stealth** | 7 native CDP patches (incl. `Runtime.enable` skip) | Delegated to cloud providers |
| **Content extraction** | `read` (articles), `extract` (auto-detect lists/tables) | None built-in |
| **Binary** | 3 MB, zero runtime | 3 MB + Next.js dashboard + cloud SDKs |
| **Codebase** | 7K lines | 40K lines |

agent-browser gives you a platform with monitoring, cloud browsers, and visual debugging. chrome-agent gives your LLM the smallest possible representation of a webpage and gets out of the way. If your agent needs a dashboard, use agent-browser. If your agent needs to spend tokens on reasoning instead of page parsing, use this.

## Philosophy

Every token your agent spends understanding a page is a token it doesn't spend reasoning about the task. chrome-agent is built around one idea: **minimize the tokens between "what does this page look like?" and "what should I do next?"**

This means:

- **Accessibility tree over DOM.** Playwright returns ~2,000 tokens of raw HTML. chrome-agent returns ~50 tokens of a11y tree with stable element IDs. No CSS selectors to write, no DOM to parse.
- **One binary, zero runtime.** 3 MB Rust binary. No Node.js, no npm, no Playwright runtime. `npx chrome-agent` just works. Linux builds are fully static (musl) — no glibc dependency, runs on any distro.
- **Action + observation in one call.** `--inspect` on any action command returns the page state after the action. One round-trip instead of two.
- **Errors are instructions.** Every error includes a `hint` field telling the agent what to do next. `{"ok":false, "error":"...", "hint":"run inspect"}`.
- **Stealth by default intent.** 7 CDP patches including the detection vector nobody talks about (`Runtime.enable`). Connect to real Chrome for the hardest protections.
- **Content extraction without selectors.** `read` for articles, `extract` for repeating data, `network` for API payloads. The agent never writes CSS selectors.

This is not a general-purpose browser testing framework. It's a tool that makes an LLM effective at browsing the web.

```bash
chrome-agent goto news.ycombinator.com --inspect

# ~50 tokens instead of ~2,000:
uid=n1 RootWebArea "Hacker News"
  uid=n50 heading "Hacker News" level=1
  uid=n82 link "Show HN: A New Browser Tool"
  uid=n97 link "Rust 2025 Edition Announced"
  ...

# Click + see the new page in one call:
chrome-agent click n82 --inspect
```

UIDs are based on Chrome's `backendNodeId`. They don't change between inspects. Click `n82` now or five minutes from now.

```
chrome-agent (3 MB Rust binary)
    | CDP over WebSocket
    v
Chrome (headless, no Node.js, no runtime)
```

### Why this exists

| If you've hit this... | chrome-agent does this instead |
|---|---|
| Playwright snapshots burn 2K tokens | a11y tree: ~50 tokens. 40x less context spent on page state. |
| CSS selectors break after every deploy | UIDs from Chrome's `backendNodeId`. Stable as long as the DOM node exists. |
| Click then inspect = 2 round-trips | `--inspect` on any command. One call, action + observation. |
| 200MB of Node + npm + Playwright | 3 MB binary. `npx chrome-agent` works out of the box. |
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
npx skills add sderosiaux/chrome-agent

# Or just the binary
npm install -g chrome-agent    # prebuilt
npx chrome-agent --help        # no install
cargo install chrome-agent     # from source
```

## Quick start

```bash
# Navigate and see the page
chrome-agent goto https://example.com --inspect

# Click by uid
chrome-agent click n12 --inspect

# Fill a form
chrome-agent fill --uid n20 "user@test.com"

# CSS selectors work too
chrome-agent click --selector "button.submit"
chrome-agent fill --selector "input[name=email]" "hello@test.com"

# Article content (Readability -- like Firefox Reader Mode)
chrome-agent read

# Visible text, scoped and capped
chrome-agent text --selector "main" --truncate 500

# Run JS
chrome-agent eval "document.title"

# Screenshot (returns a file path, not binary)
chrome-agent screenshot
```

## Commands

### Navigation

| Command | What it does |
|---------|------------|
| `goto <url> [--inspect] [--max-depth N] [--header "K: V"]` | Navigate. Auto-prefixes `https://`. `--header` (repeatable) sends extra HTTP headers. |
| `back` | History back. |
| `forward` | History forward. |
| `close [--purge]` | Stop browser. `--purge` deletes cookies/profile. |

### Inspection

| Command | What it does |
|---------|------------|
| `inspect [--verbose] [--max-depth N] [--uid nN] [--filter "role,role"] [--scroll] [--limit N] [--urls] [--max-chars N] [--offset K]` | a11y tree with UIDs. `--scroll --limit` for infinite scroll. `--urls` resolves href on links. `--max-chars`/`--offset` cap and page the output. |
| `diff` | What changed since last inspect. |
| `screenshot [--filename name] [--format jpeg\|png] [--quality N] [--max-width N] [--uid nN\|--selector "css"]` | Screenshot to file. JPEG/quality/max-width shrink it; `--uid`/`--selector` clip to one element. |
| `pdf [--out name] [--landscape] [--background]` | Print the current page to a PDF file. |
| `tabs` | List open tabs. |

### Interaction

| Command | What it does |
|---------|------------|
| `click <uid> [--inspect]` | Click by uid. Falls back to JS `.click()` when no box model. |
| `click --selector "css" [--inspect]` | Click by CSS selector. |
| `click --xy 100,200` | Click by coordinates. |
| `dblclick <uid> [--inspect]` | Double-click by uid, `--selector`, or `--xy`. |
| `fill --uid <uid> <value> [--inspect]` | Fill input by uid. |
| `fill --selector "css" <value>` | Fill by selector. |
| `fill-form <uid=val>...` | Batch fill. |
| `select --uid <uid> <value>` | Select dropdown option by value or visible text. |
| `select --selector "css" <value>` | Select by CSS selector. |
| `check <uid>` | Ensure checkbox/radio is checked. Idempotent. |
| `uncheck <uid>` | Ensure checkbox/radio is unchecked. Idempotent. |
| `upload --uid <uid> <file>...` | Upload file(s) to a file input. |
| `upload --selector "css" <file>...` | Upload by CSS selector. |
| `drag <from-uid> <to-uid>` | Drag element to another element. |
| `type <text> [--selector "css"]` | Type into focused element. |
| `press <key>` | Enter, Tab, Escape, etc. |
| `scroll <down\|up\|uid>` | Scroll page or element into view. |
| `hover <uid>` | Hover. |
| `wait <text\|url\|selector> <pattern>` | Wait for a condition. |
| `wait network-idle [--idle-ms N] [--timeout N]` | Wait until the network is quiet for `--idle-ms` (default 500). Beats fixed sleeps for SPA/XHR settle. |

### Content extraction

| Command | What it does |
|---------|------------|
| `read [--html] [--truncate N]` | Article extraction via Mozilla Readability. |
| `text [uid] [--selector "css"] [--truncate N]` | Visible text from page or element. |
| `eval <expression> [--selector "css"]` | JS in page context. `el` = matched element. |
| `extract [--selector "css"] [--limit N] [--scroll] [--a11y]` | Auto-detect repeating data. `--a11y` for React SPAs (X.com). |
| `download <url> [--out path] [--timeout N]` | Download a URL fetched in-page, so cookies/auth carry over (login-gated files). Returns `{path,bytes,mime}`. |

### Monitoring

| Command | What it does |
|---------|------------|
| `network [--filter "pattern"] [--body] [--live N] [--abort "pattern"]` | Network requests and API responses. `--abort` blocks matching requests. |
| `console [--level error] [--clear]` | console.log/warn/error + JS exceptions. |

### Advanced

| Command | What it does |
|---------|------------|
| `frame <selector\|main>` | Switch execution context to an iframe (or back to main). |
| `batch` | Execute multiple commands from a JSON array on stdin. |
| `pipe` | Persistent JSON stdin/stdout connection. |

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
--dialog <mode>          JS dialog policy: accept (default), dismiss, or manual
--dialog-text <text>     Text to submit for prompt() dialogs when --dialog accept
```

JS dialogs (`alert`/`confirm`/`prompt`/`beforeunload`) are auto-answered by default (`--dialog accept`) — a native dialog otherwise blocks the page with no DOM signal and the agent's next command hangs. Use `--dialog dismiss` to cancel them, or `--dialog manual` to opt out.

## The loop: inspect, act, inspect

```bash
chrome-agent goto https://app.com/login --inspect
# uid=n52 textbox "Email" focusable
# uid=n58 textbox "Password" focusable
# uid=n63 button "Sign In" focusable

chrome-agent fill --uid n52 "user@test.com"
chrome-agent fill --uid n58 "password123"
chrome-agent click n63 --inspect
# uid=n101 heading "Dashboard" level=1
```

UIDs stay the same between inspects as long as the DOM node exists.

## Content extraction

From least to most tokens:

```bash
# Articles (Readability, like Firefox Reader Mode)
chrome-agent read

# Repeating data -- products, search results, feeds. No selectors.
chrome-agent extract
# Uses MDR/DEPTA heuristics. Finds the pattern automatically.

# React SPAs (X.com, etc.) -- uses a11y tree instead of DOM
chrome-agent extract --a11y --scroll --limit 20

# Scoped visible text
chrome-agent text --selector "[role=main]" --truncate 1000

# API responses -- skip the DOM
chrome-agent network --filter "api" --body
```

## Forms: dropdowns, checkboxes, file uploads

```bash
# Select dropdown by value or visible text
chrome-agent select --uid n15 "California"

# Idempotent checkbox control
chrome-agent check n20     # no-op if already checked
chrome-agent uncheck n20   # no-op if already unchecked

# File upload
chrome-agent upload --uid n30 /path/to/document.pdf

# Double-click (text selection, special controls)
chrome-agent dblclick n42
```

## Iframes

```bash
# Switch into an iframe
chrome-agent frame "#payment-iframe"
chrome-agent inspect    # see iframe content
chrome-agent fill --selector "input[name=card]" "4242424242424242"

# Switch back to main page
chrome-agent frame main
```

## Batch mode

Execute a sequence of commands from stdin without per-command process startup:

```bash
echo '[
  {"cmd":"goto","url":"https://example.com"},
  {"cmd":"inspect","filter":"button"},
  {"cmd":"click","uid":"n42"}
]' | chrome-agent batch
```

Each command produces one JSON line. About 10x faster than spawning a process per command.

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
chrome-agent --connect http://127.0.0.1:9222 goto https://www.leboncoin.fr --inspect
```

| Protection | Solution |
|---|---|
| None | `chrome-agent goto ...` |
| Cloudflare/Turnstile | `chrome-agent --stealth goto ...` |
| Logged-in sites | `chrome-agent --stealth --copy-cookies goto ...` |
| DataDome/Kasada | `chrome-agent --connect` to real Chrome |

## Logged-in sites

`--copy-cookies` copies the cookie database from your Chrome profile. Both Chrome instances use the same macOS Keychain, so encrypted cookies just work.

```bash
chrome-agent --stealth --copy-cookies goto x.com/home --inspect
# Your timeline. Your DMs. No login flow.

chrome-agent --copy-cookies goto mail.google.com --inspect
chrome-agent --copy-cookies goto github.com/notifications --inspect
```

Your real Chrome is not affected.

## Network capture and blocking

```bash
# Resources already loaded (stealth-safe, uses Performance API)
chrome-agent network --filter "api"

# Live traffic with response bodies
chrome-agent network --live 5 --body --filter "graphql"

# Block tracking/ads (uses Fetch domain interception)
chrome-agent network --abort "*tracking*" --live 30

# Console output
chrome-agent console --level error    # errors + exceptions only
```

Console capture uses an injected interceptor, not `Runtime.enable`.

## Downloads, PDF, and token-safe screenshots

Files are written under `~/.chrome-agent/tmp` (or your `--out` path) with `0600` perms; the path is printed on stdout. Binary bytes never hit stdout.

```bash
# Download a file, fetched inside the page so cookies/auth carry over.
# Ideal for login-gated exports (invoices, CSVs, PDFs behind an auth wall).
chrome-agent download https://app.com/reports/2024.csv --out ./2024.csv
# {"ok":true,"path":"./2024.csv","bytes":48213,"mime":"text/csv"}

# Print the current page to PDF.
chrome-agent pdf --out invoice.pdf --background

# Screenshots that don't blow up your context window.
chrome-agent screenshot --format jpeg --quality 60 --max-width 1024
chrome-agent screenshot --uid n42            # capture a single element (or --selector "css")
```

`download` uses an in-page `fetch` with `credentials:'include'`, so the request inherits the page's session. Click-triggered browser-native downloads are not handled — resolve the target href (`inspect --urls`) and download it directly.

## Waiting for the network to settle

```bash
# Resolve once no requests are in flight for 500ms (tunable), bounded by --timeout.
# Replaces fragile fixed sleeps on SPAs / XHR-heavy pages.
chrome-agent wait network-idle
chrome-agent wait network-idle --idle-ms 800 --timeout 20
```

Opt-in (enables the Network domain), so it stays off the stealth hot path.

## Pipe mode

For agents that send many commands in sequence, pipe mode keeps a single connection open:

```bash
echo '{"cmd":"goto","url":"https://example.com","inspect":true}
{"cmd":"click","uid":"n12","inspect":true}
{"cmd":"read"}' | chrome-agent pipe
```

One JSON line per response. About 10x faster than spawning a process per command.

## JSON mode

```bash
chrome-agent --json goto https://example.com --inspect
# {"ok":true,"url":"...","title":"...","snapshot":"uid=n1 heading..."}

chrome-agent --json eval "1+1"
# {"ok":true,"result":2}

# Errors exit 1 but JSON is still on stdout (parseable):
chrome-agent --json click n99
# {"ok":false,"error":"Element uid=n99 not found.","hint":"Run 'chrome-agent inspect'"}
```

## Inspect with link URLs

When deciding which link to click, the agent often needs the URL, not just the text:

```bash
chrome-agent inspect --urls --filter link
# uid=n82 link "Pricing" url="https://example.com/pricing"
# uid=n97 link "Docs" url="https://docs.example.com"
```

## Multi-tab and parallel agents

```bash
# Multiple tabs in one browser
chrome-agent --page main goto https://app.com
chrome-agent --page docs goto https://docs.app.com
chrome-agent --page main eval "document.title"   # "App"

# Multiple agents, each with their own Chrome
chrome-agent --browser agent1 goto https://example.com
chrome-agent --browser agent2 goto https://other.com
```

## Using with AI agents

```bash
# Install the skill (Claude Code, Cursor, Copilot, etc.)
npx skills add sderosiaux/chrome-agent

# Or tell your agent to run:
chrome-agent --help
# The help output includes a full LLM usage guide.
```

Claude Code permissions:

```json
{
  "permissions": {
    "allow": ["Bash(chrome-agent *)"]
  }
}
```

## Comparison

|  | chrome-agent | agent-browser (Vercel) | Playwright MCP |
|---|---|---|---|
| Language | Rust | Rust | TypeScript |
| Binary | 3 MB, zero runtime | 3 MB CLI + dashboard + cloud providers | Node + Playwright |
| Startup | ~10ms (session reuse) | daemon (fast after first) | cold start |
| Token efficiency | ~50 tokens/page (a11y noise filtering) | ~200 tokens/page (a11y tree) | ~2,000 tokens (HTML) |
| UID stability | `backendNodeId` (stable across inspects) | sequential `@e1, @e2` (reassigned per snapshot) | N/A (selectors) |
| Action + observe | `--inspect` flag (1 call) | separate snapshot call | separate call |
| Stealth | 7 native CDP patches | delegated to cloud providers | none |
| Reader mode | `read` (Readability.js) | none | none |
| Data extraction | `extract` (auto-detect repeating data) | none | none |
| Link URL resolution | `inspect --urls` | `snapshot -u` | N/A |
| Dropdowns | `select` | `select` | via selectors |
| Checkboxes | `check`/`uncheck` (idempotent) | `check`/`uncheck` | via selectors |
| File upload | `upload` | `upload` | via selectors |
| Drag and drop | `drag` | `drag` | via selectors |
| Annotated screenshots | not yet | `screenshot --annotate` | not yet |
| Element/token-safe screenshots | `screenshot --uid/--selector`, `--format jpeg`, `--max-width` | via options | via options |
| PDF export | `pdf` (`Page.printToPDF`) | none | none |
| File download | `download` (in-page fetch, auth-preserving) | `download` | via events |
| Extra request headers | `goto --header` | yes | via context |
| Network-idle wait | `wait network-idle` | yes | `browser_wait_for` |
| JS dialog handling | auto (`--dialog accept/dismiss/manual`) | yes | `browser_handle_dialog` |
| Live dashboard | no (lean) | yes (Next.js) | no |
| Cloud providers | no (`--connect` to anything) | 5 built-in | no |
| iOS/Safari | no | yes (WebDriver) | no |
| Network blocking | `network --abort` | `network route --abort` | no |
| Iframe switching | `frame` | `frame` | via selectors |
| Batch execution | `batch` (JSON stdin) | `batch` (JSON or quoted) | N/A |
| AI chat built-in | no (the agent IS the LLM) | yes (AI Gateway) | N/A |
| Codebase | ~7.3K lines | ~40K lines | Playwright |
| Design goal | minimal tokens, maximal autonomy | feature-complete platform | browser testing |

## License

MIT
