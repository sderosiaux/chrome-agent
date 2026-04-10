---
name: chrome-agent
description: Browser automation for AI agents. Use when the user asks to interact with websites, scrape data, fill forms, take screenshots, or automate any browser task. Triggers on "open a website", "go to", "scrape", "fill the form", "click", "take a screenshot", "read this page", "search on", "check this site".
metadata:
  author: sderosiaux
  version: "0.4.0"
  tags: ["browser", "automation", "scraping", "chrome", "cdp"]
---

# chrome-agent — Browser Automation

Use `chrome-agent` to control Chrome for the user. Single binary, zero dependencies, headless by default.

## Install Check

```bash
which chrome-agent || npm install -g chrome-agent
```

If install fails (no prebuilt binary), build from source:
```bash
cargo install chrome-agent
```

## Core Workflow

**inspect → read uids → act → inspect again**

```bash
# Navigate and see the page
chrome-agent goto https://example.com --inspect

# Click by uid from the inspect output
chrome-agent click n12 --inspect

# Fill a form field
chrome-agent fill --uid n20 "value"

# Or use CSS selectors when uids aren't practical
chrome-agent click --selector "button.submit"
chrome-agent fill --selector "input[name=email]" "hello@test.com"
```

## Content Extraction (choose the right tool)

| Tool | When | Tokens |
|------|------|--------|
| `chrome-agent read` | Articles, blog posts, product pages | ~200-500 |
| `chrome-agent extract` | Repeating data: product grids, news feeds, tables, search results | ~100-500 |
| `chrome-agent text --selector "main"` | Scoped visible text | ~500-1000 |
| `chrome-agent eval "JSON.stringify(...)"` | Structured data from DOM | Varies |
| `chrome-agent inspect --filter "link" --urls` | Find links with their href URLs | ~50-200 |
| `chrome-agent text` | Full page text (last resort) | ~5000+ |

## Bot Protection

| Site protection | Solution |
|---|---|
| None | `chrome-agent goto ...` |
| Cloudflare/Turnstile | `chrome-agent --stealth goto ...` |
| Logged-in sites (X, Gmail, etc.) | `chrome-agent --stealth --copy-cookies goto ...` |
| DataDome/Kasada (Leboncoin, etc.) | Connect to real Chrome: see below |

For heavy protection:
```bash
# User must launch Chrome with debugging:
google-chrome --remote-debugging-port=9222 &
# Then connect:
chrome-agent --connect http://127.0.0.1:9222 goto https://protected-site.com --inspect
```

## Key Commands

```bash
# Navigation
chrome-agent goto <url> [--inspect] [--wait-for "selector"]
chrome-agent back
chrome-agent forward
chrome-agent scroll down|up|<uid>

# Inspection
chrome-agent inspect [--max-depth N] [--filter "button,link"] [--uid nN] [--urls]
chrome-agent inspect --filter "link" --urls              # links with resolved href URLs
chrome-agent inspect --filter "article" --scroll --limit 50  # collect from infinite scroll

# Click (3 targeting modes: uid, --selector, --xy)
chrome-agent click <uid> [--inspect]
chrome-agent click --selector "css" [--inspect]
chrome-agent click --xy 100,200
chrome-agent dblclick <uid> [--inspect]                  # double-click (also supports --selector, --xy)

# Fill & type
chrome-agent fill --uid <uid> <value>
chrome-agent fill --selector "css" <value>
chrome-agent fill-form n20="a@b.com" n30="password"
chrome-agent type "text" [--selector "input.search"]
chrome-agent press Enter|Tab|Escape

# Dropdowns
chrome-agent select --uid <uid> "Option text"            # matches by value or visible text
chrome-agent select --selector "#country" "France"

# Checkboxes & radios (idempotent — no-op if already in desired state)
chrome-agent check <uid>                                 # ensure checked
chrome-agent check --selector "input[name=agree]"        # by CSS selector
chrome-agent uncheck <uid>                               # ensure unchecked

# File upload
chrome-agent upload --uid <uid> /path/to/file.pdf        # single or multiple files
chrome-agent upload --selector "input[type=file]" /path/to/file.pdf

# Drag and drop
chrome-agent drag <from-uid> <to-uid>                    # mouse-event based drag

# Iframes
chrome-agent frame "#payment-iframe"                     # switch to iframe context
chrome-agent frame main                                  # switch back to main page

# Content extraction
chrome-agent read [--truncate N]
chrome-agent extract [--selector "css"] [--limit N]      # auto-detect repeating data
chrome-agent extract --scroll                            # scroll first for lazy-loaded pages
chrome-agent extract --a11y --scroll --limit 20          # React SPAs (X.com)
chrome-agent text [--selector "main"] [--truncate N]
chrome-agent eval "expression" [--selector "css"]

# Network
chrome-agent network [--filter "pattern"] [--body] [--limit N]  # already-loaded (stealth-safe)
chrome-agent network --live 5 --body --filter "graphql"          # capture live traffic
chrome-agent network --abort "*tracking*" --live 30              # block matching requests

# Console + JS errors (stealth-safe)
chrome-agent console [--level error] [--clear]

# Batch mode (execute multiple commands from JSON array on stdin)
echo '[{"cmd":"goto","url":"..."},{"cmd":"inspect"},{"cmd":"click","uid":"n12"}]' | chrome-agent batch

# Pipe mode (persistent connection, 10x faster for multi-step workflows)
echo '{"cmd":"goto","url":"...","inspect":true}' | chrome-agent pipe

# Other
chrome-agent screenshot [--filename name]
chrome-agent diff                                        # what changed since last inspect
chrome-agent wait text|url|selector "pattern" [--timeout N]
chrome-agent tabs
chrome-agent close [--purge]
```

## Global Flags

```bash
--stealth      # 7 anti-detection patches (Cloudflare/Turnstile)
--json         # Structured JSON output (see below)
--page <name>  # Named tabs (keep multiple pages open)
--max-depth N  # Limit inspect tree depth (saves tokens)
--headed       # Show browser window (default is headless)
--connect URL  # Use real Chrome (for DataDome/Kasada sites)
--copy-cookies # Use cookies from your real Chrome profile
```

## JSON Output Format

All commands with `--json` return objects on stdout. Errors exit 1 but JSON is still on stdout.

```
Success: {"ok":true, ...command-specific fields...}
Error:   {"ok":false, "error":"message", "hint":"what to do next"}
```

Per-command shapes:
- `goto --inspect` → `{"ok":true, "url":"...", "title":"...", "snapshot":"uid=n1..."}`
- `inspect` → `{"ok":true, "snapshot":"uid=n1 heading..."}`
- `click/fill/select/check --inspect` → `{"ok":true, "message":"Clicked...", "snapshot":"..."}`
- `click/fill/select/check` (no inspect) → `{"ok":true, "message":"Clicked uid=n12"}`
- `read` → `{"ok":true, "title":"...", "text":"article content..."}`
- `text` → `{"ok":true, "text":"visible text..."}`
- `eval` → `{"ok":true, "result": <any JSON value>}`
- `network` → `{"ok":true, "requests":[{"url":"...", "status":200, ...}]}`
- `console` → `{"ok":true, "messages":[{"level":"error", "message":"..."}]}`
- `batch` → `{"ok":true, "results":[...one result per command...]}`
- `screenshot` → `{"ok":true, "path":"/path/to/file.png"}`

## Token Budget

An inspect of a typical page is ~50-200 tokens. To stay lean:
- `--max-depth 2` for deep pages (limits tree to 2 levels)
- `--filter "button,link"` to see only interactive elements (~10-30 tokens)
- `--filter "link" --urls` when deciding which link to follow
- `read --truncate 1000` caps article extraction
- `text --selector "main" --truncate 500` for scoped visible text

## Important Rules

1. **Always inspect before interacting** — UIDs change when the page mutates.
2. **After SPA navigation** (back, forward, client-side routing), **re-inspect** — UIDs change on re-render.
3. **For SPA detail pages**, prefer `goto <direct-url>` over `click` — click may open a modal.
4. **Use `read` for articles**, `text --selector` for scoped extraction, `eval` for structured data.
5. **Prefer inspect over screenshot** — ~50 tokens vs ~100K tokens.
6. **UIDs are stable** (n47, n123) across inspects on the same page — based on backendNodeId.
7. **--json errors exit 1** with `{"ok":false}` on stdout — parseable, check `ok` field.
8. **--max-depth works everywhere** — on standalone inspect AND on goto/click/fill --inspect.
9. **Use --filter** to find elements fast: `inspect --filter "button,link,textbox"`.
10. **Use --urls** on inspect to get link destinations: `inspect --filter "link" --urls`.
11. **check/uncheck are idempotent** — "Already checked" if no change needed. Prefer over click for checkboxes.
12. **select works by value or text** — `select --uid n5 "Option 2"` tries `option.value` first, then `option.text`.
13. **frame before iframe interaction** — `frame "#iframe"` to enter, `frame main` to return. Re-inspect after switching.
14. **batch for multi-step sequences** — pipe JSON array to stdin. Faster than separate CLI calls. UIDs from inspect are valid within the same batch.
15. **close --purge** deletes browser profile (cookies, cache) when done.
16. **Parallel agents**: use `--browser <unique-name>` to isolate sessions.
