---
name: chrome-agent
description: Pull structured records out of a web page (product grids, search results, feeds, tables) with no CSS selectors and no model call spent reading HTML. Drives Chrome for everything else too: navigate, click, fill forms, screenshot, print to PDF, download files that sit behind a login, and get past bot detection. Runs locally as one binary, no API key, no cloud. Use when the user says scrape, extract the list of, get the data from, fill this form, log in to, click, take a screenshot, read this page, check this site, or when a site has no API.
metadata:
  author: sderosiaux
  version: "0.9.0"
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

**inspect → read uids → act**

An action reports what it changed on the page, so you don't need a second call to find out
whether it landed. Re-inspect when you need fresh uids after a navigation, or when the
report says `document_changed`.

```bash
# Navigate and see the page
chrome-agent goto https://example.com --inspect

# Click by uid; the response says what changed
chrome-agent click n12

# Fill a form field
chrome-agent fill --uid n20 "value"

# Or use CSS selectors when uids aren't practical
chrome-agent click --selector "button.submit"
chrome-agent fill --selector "input[name=email]" "hello@test.com"
```

## Content Extraction (choose the right tool)

Reach for `extract` first on anything that looks like a list. Token figures below are
measured on the Hacker News front page with `./scripts/measure.sh`; they scale with the
page, so treat them as relative cost, not as constants.

| Tool | When | Tokens on HN |
|------|------|--------|
| `chrome-agent extract` | Repeating records: product grids, feeds, tables, search results. No selectors needed. | ~1,570 for all 30 records |
| `chrome-agent read` | Articles, blog posts, product pages (Mozilla Readability) | ~950 |
| `chrome-agent text --selector "main"` | Scoped visible text | ~1,040 unscoped |
| `chrome-agent network --filter "api"` | The page fetched its data over XHR — take the payload | varies, often the cleanest |
| `chrome-agent eval "JSON.stringify(...)"` | Specific attributes a record view doesn't carry | varies |
| `chrome-agent inspect --filter "button,link"` | Finding what to act on, not reading content | ~1,735 |
| `chrome-agent inspect` | Full accessibility tree with uids | ~5,650 |

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
chrome-agent goto <url> [--inspect] [--wait-for "selector"] [--header "K: V"]
chrome-agent back
chrome-agent forward
chrome-agent scroll down|up|<uid>
chrome-agent wait network-idle [--idle-ms N] [--timeout N]   # wait for XHR/SPA to settle

# Inspection
chrome-agent inspect [--max-depth N] [--filter "button,link"] [--uid nN] [--urls]
chrome-agent inspect --filter "link" --urls              # links with resolved href URLs
chrome-agent inspect --filter "article" --scroll --limit 50  # collect from infinite scroll
chrome-agent inspect --max-chars 4000 [--offset 4000]    # cap/page huge trees (tail shows next --offset)

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

# Iframes — the frame switch persists only inside ONE process, so use pipe/batch:
printf '%s\n' \
  '{"cmd":"frame","target":"iframe[src*=\"checkout\"]"}' \
  '{"cmd":"inspect"}' \
  '{"cmd":"fill","uid":"n42","value":"..."}' \
  '{"cmd":"frame","target":"main"}' | chrome-agent pipe
# Separate CLI calls (frame then inspect) do NOT work — each is a fresh connection.

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

# Files: screenshots, PDF, downloads (saved to ~/.chrome-agent/tmp; path on stdout)
chrome-agent screenshot [--filename name] [--format jpeg] [--quality N] [--max-width N] [--uid nN|--selector "css"]
chrome-agent pdf [--filename name] [--landscape] [--background]   # current page to PDF
chrome-agent download <url> [--out path] [--max-bytes N]          # auth-preserving (in-page fetch); rejects over 64 MiB by default

# Other
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
--verdict MODE # auto (default): an action reports what changed. off: report the action only
--budget N     # Cap that change report, in characters (default 1200; 0 = uncapped)
--headed       # Show browser window (default is headless)
--connect URL  # Use real Chrome (for DataDome/Kasada sites)
--proxy-server URL # Route a managed browser via a proxy (http(s)/socks4/5://host:port); launch-only
--copy-cookies # Use cookies from your real Chrome profile
--dialog MODE  # JS dialog policy: accept (default) | dismiss | manual
--dialog-text  # Text submitted for prompt() dialogs under --dialog accept
```

JS dialogs (`alert`/`confirm`/`prompt`/`beforeunload`) are auto-accepted by default so the page never hangs. Use `--dialog dismiss` to cancel them.

## JSON Output Format

All commands with `--json` return objects on stdout. Errors exit 1 but JSON is still on stdout.

```
Success: {"ok":true, ...command-specific fields...}
Error:   {"ok":false, "error":"message", "hint":"what to do next"}
```

Per-command shapes:
- `goto --inspect` → `{"ok":true, "url":"...", "title":"...", "snapshot":"uid=n1..."}`
- `inspect` → `{"ok":true, "snapshot":"uid=n1 heading..."}`
- `click/fill/select/check` → `{"ok":true, "message":"Clicked uid=n12", "changed":{"added":1,"removed":0,"changed":0,"unchanged":42,"moved":0,"anonymous":0,"document_changed":false,"identity_known":true}, "delta":"+ uid=n88 heading \"Saved\""}`
- `fill` also returns `"value":{"requested":"...","actual":"...","verbatim":true|false}` — what you asked for and what the page kept. `verbatim:false` means a mask, a controlled component or a constraint rewrote it; read `actual` before assuming the form holds your value.
- `focus` appears as `{"from":"n11","to":"n15"}` when focus moved. It is deliberately not counted as a content change.
- `click/fill/select/check --inspect` → the same, plus `"snapshot"` with the whole tree
- `click/fill/select/check --verdict off` → `{"ok":true, "message":"Clicked uid=n12", "verdict":"not_checked", "verdict_reason":"reporting_disabled"}`
- every mutating command also carries `verdict` + `verdict_reason`, so silence is never ambiguous:
  - `changed` (`tree_delta`, `nodes_moved`, `focus_only`) — the page moved; `delta` says how
  - `navigated` (`document_replaced`) — new document, every stored uid is dead, re-inspect
  - `no_delta` (`identical_tree`) — the tree was identical in the observation window. NOT proof the action did nothing: it may have hit an overlay, changed only a canvas or styling, or landed slower than the window
  - `unknown` (`no_baseline`, `read_failed`, `identity_unreadable`) — nothing could be compared; `verdict_hint` says how to find out
  - `not_checked` (`reporting_disabled`) — you passed `--verdict off`
- `read` → `{"ok":true, "title":"...", "text":"article content..."}`
- `text` → `{"ok":true, "text":"visible text..."}`
- `eval` → `{"ok":true, "result": <any JSON value>}`
- `network` → `{"ok":true, "requests":[{"url":"...", "status":200, ...}]}`
- `console` → `{"ok":true, "messages":[{"level":"error", "message":"..."}]}`
- `batch` → `{"ok":true, "results":[...one result per command...]}`
- `screenshot`/`pdf` → `{"ok":true, "path":"/path/to/file"}`
- `download` → `{"ok":true, "path":"...", "bytes":N, "mime":"..."}`
- `inspect --max-chars` → adds `{"total_chars":N, "truncated":bool, "next_offset":N|null}`

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
5. **Prefer inspect over screenshot** — a screenshot gives you no uids to act on, so you pay for the image and then inspect anyway.
6. **UIDs are stable** (n47, n123) across inspects on the same page — based on backendNodeId.
7. **--json errors exit 1** with `{"ok":false}` on stdout — parseable, check `ok` field.
8. **--max-depth works everywhere** — on standalone inspect AND on goto/click/fill --inspect.
9. **Use --filter** to find elements fast: `inspect --filter "button,link,textbox"`.
10. **Use --urls** on inspect to get link destinations: `inspect --filter "link" --urls`.
11. **check/uncheck are idempotent** — "Already checked" if no change needed. Prefer over click for checkboxes.
12. **check/uncheck refuse what they cannot check** — a text input, a custom dropdown, or unchecking a radio all return `ok:false` with the reason, instead of reporting success. They also read the state back after clicking.
13. **fill tells you what the page kept** — check `value.verbatim`. A phone mask, a currency field or a `maxlength` will change what you wrote, and the form may reject it.
14. **press types single characters** — `press a` inserts "a". An unknown key name is refused rather than silently doing nothing.
15. **select works by value or text** — `select --uid n5 "Option 2"` tries `option.value` first, then `option.text`.
13. **frame is pipe/batch-only** — `frame` scopes `eval`+`inspect` to the iframe, but the binding lives on the connection, so it only persists inside one `pipe`/`batch` process (not across separate CLI calls). After switching, `inspect` for iframe uids then act by uid; `--selector` still hits the top document. `frame main` returns.
14. **batch for multi-step sequences** — pipe JSON array to stdin. Faster than separate CLI calls. UIDs from inspect are valid within the same batch.
15. **close --purge** deletes browser profile (cookies, cache) when done.
16. **Parallel agents**: use `--browser <unique-name>` to isolate sessions.
17. **download is auth-preserving** — it fetches inside the page so logins carry over. It can't capture a click-triggered browser download; get the href with `inspect --urls` and pass the URL.
18. **wait network-idle over sleeps** — for SPA/XHR settle, `wait network-idle` is deterministic; avoid guessing fixed timeouts.
19. **screenshots can be large** — prefer `--format jpeg --max-width 1024` or `--uid`/`--selector` to keep files (and any re-read tokens) small.
