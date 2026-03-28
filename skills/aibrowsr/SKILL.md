---
name: aibrowsr
description: Browser automation for AI agents. Use when the user asks to interact with websites, scrape data, fill forms, take screenshots, or automate any browser task. Triggers on "open a website", "go to", "scrape", "fill the form", "click", "take a screenshot", "read this page", "search on", "check this site".
metadata:
  author: sderosiaux
  version: "0.2.5"
  tags: ["browser", "automation", "scraping", "chrome", "cdp"]
---

# aibrowsr — Browser Automation

Use `aibrowsr` to control Chrome for the user. Single binary, zero dependencies, headless by default.

## Install Check

```bash
which aibrowsr || npm install -g aibrowsr
```

If install fails (no prebuilt binary), build from source:
```bash
cargo install aibrowsr
```

## Core Workflow

**inspect → read uids → act → inspect again**

```bash
# Navigate and see the page
aibrowsr goto https://example.com --inspect

# Click by uid from the inspect output
aibrowsr click n12 --inspect

# Fill a form field
aibrowsr fill --uid n20 "value"

# Or use CSS selectors when uids aren't practical
aibrowsr click --selector "button.submit"
aibrowsr fill --selector "input[name=email]" "hello@test.com"
```

## Content Extraction (choose the right tool)

| Tool | When | Tokens |
|------|------|--------|
| `aibrowsr read` | Articles, blog posts, product pages | ~200-500 |
| `aibrowsr extract` | Repeating data: product grids, news feeds, tables, search results | ~100-500 |
| `aibrowsr text --selector "main"` | Scoped visible text | ~500-1000 |
| `aibrowsr eval "JSON.stringify(...)"` | Structured data from DOM | Varies |
| `aibrowsr inspect --filter "link"` | Find interactive elements | ~50-200 |
| `aibrowsr text` | Full page text (last resort) | ~5000+ |

## Bot Protection

| Site protection | Solution |
|---|---|
| None | `aibrowsr goto ...` |
| Cloudflare/Turnstile | `aibrowsr --stealth goto ...` |
| Logged-in sites (X, Gmail, etc.) | `aibrowsr --stealth --copy-cookies goto ...` |
| DataDome/Kasada (Leboncoin, etc.) | Connect to real Chrome: see below |

For heavy protection:
```bash
# User must launch Chrome with debugging:
google-chrome --remote-debugging-port=9222 &
# Then connect:
aibrowsr --connect http://127.0.0.1:9222 goto https://protected-site.com --inspect
```

## Key Commands

```bash
# Navigation
aibrowsr goto <url> [--inspect] [--wait-for "selector"]
aibrowsr back
aibrowsr scroll down|up|<uid>

# Inspection
aibrowsr inspect [--max-depth N] [--filter "button,link"] [--uid nN]

# Actions (3 targeting modes: uid, --selector, --xy)
aibrowsr click <uid> [--inspect]
aibrowsr click --selector "css" [--inspect]
aibrowsr click --xy 100,200
aibrowsr fill --uid <uid> <value>
aibrowsr fill --selector "css" <value>
aibrowsr fill-form n20="a@b.com" n30="password"
aibrowsr type "text" [--selector "input.search"]
aibrowsr press Enter|Tab|Escape

# Content extraction
aibrowsr read [--truncate N]
aibrowsr extract [--selector "css"] [--limit N]   # auto-detect repeating data (no selectors needed)
aibrowsr extract --scroll                         # scroll first for lazy-loaded pages (YouTube, Pinterest)
aibrowsr text [--selector "main"] [--truncate N]
aibrowsr eval "expression" [--selector "css"]

# Network capture ("Readability for APIs" — extract API data, not DOM)
aibrowsr network [--filter "pattern"] [--body] [--limit N]  # already-loaded (stealth-safe)
aibrowsr network --live 5 --body --filter "graphql"          # capture live traffic

# Console + JS errors (stealth-safe)
aibrowsr console [--level error] [--clear]

# Pipe mode (persistent connection, 10x faster for multi-step workflows)
echo '{"cmd":"goto","url":"...","inspect":true}' | aibrowsr pipe

# Other
aibrowsr screenshot [--filename name]
aibrowsr wait text|url|selector "pattern" [--timeout N]
aibrowsr tabs
aibrowsr close [--purge]
```

## Global Flags

```bash
--stealth      # 7 anti-detection patches (Cloudflare/Turnstile)
--json         # Structured JSON output: {"ok":true,...} or {"ok":false,"error":"...","hint":"..."}
--page <name>  # Named tabs (keep multiple pages open)
--max-depth N  # Limit inspect tree depth
--headed       # Show browser window (default is headless)
--connect URL  # Use real Chrome (for DataDome/Kasada sites)
```

## Important Rules

1. **Always inspect before interacting** — UIDs change when the page mutates.
2. **After SPA navigation** (back, client-side routing), **re-inspect** — UIDs change on re-render.
3. **For SPA detail pages**, prefer `goto <direct-url>` over `click` — click may open a modal.
4. **Use `read` for articles**, `text --selector` for scoped extraction, `eval` for structured data.
5. **Prefer inspect over screenshot** — ~50 tokens vs ~100K tokens.
6. **UIDs are stable** (n47, n123) across inspects on the same page — based on backendNodeId.
7. **--json errors exit 0** — always parseable, check `ok` field.
8. **--max-depth works everywhere** — on standalone inspect AND on goto/click/fill --inspect.
9. **Use --filter** to find elements fast: `inspect --filter "button,link,textbox"`.
10. **close --purge** deletes browser profile (cookies, cache) when done.
11. **Parallel agents**: use `--browser <unique-name>` to isolate sessions. Without it, parallel agents share the same Chrome and corrupt each other's state.
