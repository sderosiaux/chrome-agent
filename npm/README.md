# chrome-agent

**Web tasks that compile.**

A browser doesn't report back. The click lands on a cookie banner, the form drops what you typed,
the page navigates away mid-action, and the tool returns success anyway. Everything the agent does
next is built on that.

chrome-agent reads the page back after every action and answers which one it was: the change held,
something else took the click, or nothing could be observed. One word, in JSON, to branch on.

One 3 MB Rust binary over CDP. No Node runtime, no Playwright, no daemon.

chrome-agent v0.16.0 (~28.2K lines of Rust in `src/`, blank and comment-only lines excluded; 3 MB binary)

Full documentation: [github.com/sderosiaux/chrome-agent](https://github.com/sderosiaux/chrome-agent).

## Install

```bash
npx skills add sderosiaux/chrome-agent   # skill file + binary, for coding agents
npm install -g chrome-agent              # prebuilt binary
npx chrome-agent --help                  # no install
cargo install chrome-agent               # from source
```

## Quickstart

```bash
# Navigate and read the page as an accessibility tree with stable uids
chrome-agent goto https://example.com --inspect
# uid=n9  heading "Example Domain" level=1
# uid=n12 link "More information..."

# Act by uid, by CSS selector, or by coordinates
chrome-agent click n12 --inspect
chrome-agent click --selector "button.submit"
chrome-agent click --xy 100,200

# Fill, then check what the page kept
chrome-agent fill --uid n20 "user@test.com"
chrome-agent assert value --uid n20 --equals "user@test.com"

# Content, not markup
chrome-agent read
chrome-agent extract --limit 30
chrome-agent text --selector "main" --truncate 500

# JSON for everything
chrome-agent --json eval "document.title"
chrome-agent screenshot --format jpeg --quality 60 --max-width 1024
```

Chrome stays alive between invocations, so a command costs a connection, not a browser launch. Give
each parallel agent its own `--browser <name>`, or they corrupt each other's session state.

## Commands

| Command | What it does |
|---|---|
| `goto <url> [--inspect] [--header "K: V"]` | Navigate. Reports where you landed and what answered. |
| `inspect [--filter "role,role"] [--uid nN] [--urls] [--max-chars N] [--offset K]` | Accessibility tree with stable uids. |
| `diff` | What changed since the last inspect. |
| `click <uid> [--selector "css"] [--xy X,Y] [--inspect]` | Click. JS fallback when there is no box model. |
| `dblclick <uid>` | Double-click, same targeting modes. |
| `fill --uid <uid> <value>` | Fill an input. Reports the value the page kept. |
| `fill-form <uid=val>...` | Fill several fields at once. |
| `select --uid <uid> <value>` | Pick a `<select>` option by value or visible text. |
| `check <uid>` / `uncheck <uid>` | Idempotent checkbox and radio control. |
| `upload --uid <uid> <file>...` | Upload to a file input. |
| `drag <from-uid> <to-uid>` | Mouse-event drag. |
| `type <text>` / `press <key>` / `hover <uid>` / `scroll <down\|up\|uid>` | Keyboard and pointer primitives. |
| `wait <text\|url\|selector> <pattern>` | Wait for a condition. `wait network-idle` for SPA settle. |
| `assert value\|text\|url\|state\|exists ...` | Check a page fact. Exit 2 when it does not hold. |
| `read [--html] [--truncate N]` | Article extraction via Mozilla Readability. |
| `text [--selector "css"] [--truncate N]` | Visible text of the page or one element. |
| `extract [--limit N] [--scroll] [--a11y]` | Auto-detect repeating records. No selectors needed. |
| `eval <expression> [--selector "css"]` | JS in page context. |
| `screenshot [--format jpeg\|png] [--quality N] [--max-width N] [--uid nN]` | Screenshot to a file path. |
| `pdf [--filename name] [--landscape] [--background]` | Print the page to PDF. |
| `download <url> [--out path]` | Fetch in-page so cookies and auth carry over. |
| `network [--filter "pattern"] [--body] [--live N] [--abort "pattern"]` | Requests and API responses. |
| `console [--level error] [--clear]` | console.log/warn/error and JS exceptions. |
| `frame <selector\|main>` | Bind `eval`/`inspect` to an iframe, inside a `pipe`/`batch` process. |
| `emulate device --width W --height H` | Device metrics for one named page. |
| `pipe` / `batch` | Persistent JSON stdin/stdout, or a JSON array on stdin. |
| `tabs` / `status` / `history` / `close [--purge]` | Session management. |

## Global flags

```
--browser <name>         Named browser profile (default: "default")
--page <name>            Named tab (default: "default")
--connect <auto|url>     Attach to a running Chrome (a value is required)
--headed                 Show the browser window (default: headless)
--stealth                Anti-detection CDP patches
--copy-cookies           Use cookies from your real Chrome profile
--timeout <seconds>      Command timeout (default: 30)
--max-depth <N>          Limit inspect depth
--verdict <mode>         auto (default) reads the page back; off reports the action only
--dialog <mode>          JS dialog policy: accept (default), dismiss, or manual
--ignore-https-errors    Accept self-signed certificates
--json                   Structured JSON output
```

## What a response tells you

`ok:true` means the command ran, not that the page complied. Every mutating action carries a
`verdict`, a `verdict_reason` and a `next` — one token from `proceed`, `inspect`, `retry`,
`confirm`, `dismiss`, `stop` — so an agent branches without parsing prose.

| `verdict` | Means |
|---|---|
| `changed` | The page moved; `delta` says how. |
| `navigated` | New document. Every stored uid is dead. |
| `intercepted` | Another element received the event; `intercepted_by` names it. |
| `not_kept` | The write reached the element and it does not hold it. Read `value.actual`. |
| `no_effect` | Delivery proven by hit test, and the tree stayed still. |
| `unchanged` | The tree was identical while the tool watched. Delivery not proven. |
| `unknown` | Nothing could be compared. Never repeat the action — it may already have landed. |

Exit codes: `0` success, `1` error, `2` a claim this tool made did not hold, `130` Ctrl+C. `2` is
a failed `assert`, or a `macro run` guard that was checked and did not hold — nothing else.

## uids

Element ids come from Chrome's `backendNodeId`, printed as `n82`. They stay valid across inspects
of the same page. A navigation reassigns them all, so re-inspect after `goto`, `back`, or a click
that changes route. CSS selectors and coordinates work where a uid is impractical.

## Pipe mode

One process, one connection, one JSON line per response, and uids stay stable across the whole
sequence — which is the reason to reach for it. The speed-up is real and small: pipe removes about
12 ms of per-command overhead, worth 1.5x on a stream of reads (nine commands, 352 ms → 228 ms) and
1.1x on a stream of fills and clicks (2029 ms → 1908 ms), where the settle window and the tree
re-read pipe does not touch are most of the cost. Measured on 2026-08-30, M4 Max, Chrome 152,
median of 9 runs (`scripts/measure-pipe.sh` in the repo).

```bash
echo '{"cmd":"goto","url":"https://example.com","inspect":true}
{"cmd":"click","uid":"n12","inspect":true}
{"cmd":"read"}' | chrome-agent pipe
```

## Bot detection

`--stealth` applies 7 CDP-level patches: `navigator.webdriver`, `chrome.runtime`, the Permissions
API, the WebGL renderer, the User-Agent, an input coordinate leak, and never calling
`Runtime.enable`.

| Protection | What works |
|---|---|
| None | `chrome-agent goto ...` |
| Cloudflare JS challenge | `--stealth` clears it |
| Cloudflare managed Turnstile, DataDome, Kasada | `--stealth` does not help. Use `--connect`. |
| Logged-in sites | `--copy-cookies`, optionally with `--stealth` |

Heavy protection fingerprints the Chromium binary itself, so the only route is a real installed
Chrome. `--copy-cookies` copies the cookie database from your Chrome profile and leaves your real
Chrome untouched.

```bash
google-chrome --remote-debugging-port=9222 &
chrome-agent --connect http://127.0.0.1:9222 goto https://www.leboncoin.fr --inspect
chrome-agent --stealth --copy-cookies goto x.com/home --inspect
```

## Comparison

| | chrome-agent | agent-browser (Vercel) | Playwright MCP |
|---|---|---|---|
| Language | Rust | Rust | TypeScript |
| Runtime deps | none | none (CLI) | Node + Playwright |
| Startup | 12 ms measured, one command on a running browser | daemon | cold start |
| UID stability | `backendNodeId`, stable across inspects | sequential, reassigned per snapshot | N/A |
| Compliance reporting | `verdict`/`next` on every action | no | no |
| Stealth | 7 CDP patches | delegated to cloud providers | none |
| Reader mode | `read` (Readability.js) | none | none |
| Record extraction | `extract`, structural, no LLM call | none | none |
| MCP server | none | yes | yes |
| Code | ~28.2K lines of Rust in `src/` (blank and comment-only lines excluded; a test re-measures it) | ~40K lines (their figure, unverified here) | Playwright |

## Using it from an agent

`npx skills add sderosiaux/chrome-agent` installs a SKILL.md. Otherwise `chrome-agent --help`
embeds a full LLM usage guide, and every error carries a `hint` naming the next action. Claude Code
permissions: `{"permissions": {"allow": ["Bash(chrome-agent *)"]}}`.

## License

MIT
