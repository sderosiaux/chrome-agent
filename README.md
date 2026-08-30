# chrome-agent

[![Crates.io](https://img.shields.io/crates/v/chrome-agent)](https://crates.io/crates/chrome-agent)
[![npm](https://img.shields.io/npm/v/chrome-agent)](https://www.npmjs.com/package/chrome-agent)
[![CI](https://github.com/sderosiaux/chrome-agent/actions/workflows/ci.yml/badge.svg)](https://github.com/sderosiaux/chrome-agent/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust 2024](https://img.shields.io/badge/Rust-2024_edition-orange)](https://doc.rust-lang.org/edition-guide/rust-2024/)

<p align="center">
  <img src="docs/hero-logo.png" alt="chrome-agent — web tasks that compile" width="500">
</p>

<p align="center">
  <a href="README.md">English</a> | <a href="README.cn.md">简体中文</a>
</p>

**Web tasks that compile.**

A browser doesn't report back. The click lands on a cookie banner, the form drops what you typed,
the page navigates away mid-action, and the tool returns success anyway. Everything the agent does
next is built on that.

chrome-agent reads the page back after every action and answers which one it was: the change held,
something else took the click, or nothing could be observed. One word, in JSON, to branch on.

One 3 MB Rust binary over CDP. No Node, no Playwright, no daemon.

> Independent project. Not affiliated with, endorsed by, or sponsored by Google or the Chrome team.

## Install

```bash
npx skills add sderosiaux/chrome-agent   # skill file + binary, for coding agents
npm install -g chrome-agent              # prebuilt binary
cargo install chrome-agent               # from source
```

Linux builds are static musl binaries, so they run on any distro without a glibc version to match.

## 60-second quickstart

```bash
# Navigate and read the page as an accessibility tree with stable uids
chrome-agent goto https://example.com --inspect
# uid=n9  heading "Example Domain" level=1
# uid=n12 link "More information..."

# Act by uid, by CSS selector, or by coordinates
chrome-agent click n12 --inspect
chrome-agent click --selector "button.submit"
chrome-agent click --xy 100,200

# Fill and check what the page kept
chrome-agent fill --uid n20 "user@test.com"
chrome-agent assert value --uid n20 --equals "user@test.com"

# Get content, not markup
chrome-agent read
chrome-agent extract --limit 30
chrome-agent text --selector "main" --truncate 500

# Everything is available as JSON
chrome-agent --json eval "document.title"
```

Chrome stays alive between invocations, so a command costs a connection, not a browser launch.
Use `--browser <name>` to give each parallel agent its own Chrome and its own session state.

## Commands

### Navigation and session

| Command | What it does |
|---|---|
| `goto <url> [--inspect] [--max-depth N] [--header "K: V"]` | Navigate. Auto-prefixes `https://`. Reports `landed` (see below). `--header` is repeatable. |
| `back [--inspect]` | History back. Answers `url` and `title`, or a message when there is nowhere to go. |
| `forward [--inspect]` | History forward. Same answer, opposite sign. |
| `history [--filter pattern]` | Pages this browser visited. |
| `tabs` | List open tabs. |
| `status` | Browsers the session store knows, their pids, and running ones no entry claims (`orphan=`). |
| `close [--purge] [--orphans]` | Stop the browser. `--purge` deletes cookies and profile. `--orphans` closes unclaimed browsers. |

### Reading the page

| Command | What it does |
|---|---|
| `inspect [--verbose] [--max-depth N] [--uid nN] [--filter "role,role"] [--scroll] [--limit N] [--urls] [--max-chars N] [--offset K]` | Accessibility tree with uids. `--urls` resolves link hrefs. Narrowing flags change what is PRINTED; the stored baseline is always the full tree. |
| `diff` | What changed since the last inspect, compared against that full baseline. |
| `text [uid] [--selector "css"] [--truncate N]` | Visible text of the page or one element. |
| `read [--html] [--truncate N]` | Article extraction via Mozilla Readability. |
| `extract [--selector "css"] [--limit N] [--scroll] [--a11y]` | Auto-detect repeating records (products, feeds, search results). No selectors needed. `--a11y` for React SPAs. |
| `eval <expression> [--selector "css"]` | JS in page context. `el` is the matched element. |
| `screenshot [--filename name] [--format jpeg\|png] [--quality N] [--max-width N] [--uid nN\|--selector "css"]` | Screenshot to a file path. `--uid`/`--selector` clip to one element. |
| `pdf [--filename name] [--landscape] [--background]` | Print the page to PDF. |
| `download <url> [--out path] [--timeout N] [--max-bytes N]` | Fetch in-page so cookies and auth carry over. Also `download --uid nN` / `--selector "css"` to click and capture a browser-native download. |

### Acting

| Command | What it does |
|---|---|
| `click <uid> [--selector "css"] [--xy X,Y] [--inspect]` | Click. Falls back to JS `.click()` when there is no box model. |
| `dblclick <uid>` | Double-click, same three targeting modes. |
| `fill --uid <uid> <value> [--secret] [--inspect]` | Fill an input. Also `--selector "css"`. Reports what the page kept; `--secret` reports only its length. |
| `fill-form <uid=val>...` | Fill several fields, one kept-value report per field. |
| `select --uid <uid> <value>` | Pick a `<select>` option by value or visible text. |
| `check <uid>` | Ensure a checkbox or radio is checked. Idempotent. |
| `uncheck <uid>` | Ensure a checkbox is unchecked. Idempotent. |
| `upload --uid <uid> <file>...` | Upload to a file input. Paths are validated first. |
| `drag <from-uid> <to-uid>` | Mouse-event drag. Does not work with the HTML5 Drag and Drop API. |
| `type <text> [--selector "css"] [--secret]` | Type into the focused element. `--secret` withholds the length too. |
| `press <key>` | Enter, Tab, Escape, and so on. |
| `scroll <down\|up\|uid>` | Scroll the page, or an element into view. |
| `hover <uid>` | Hover. |
| `wait <text\|url\|selector> <pattern>` | Wait for a condition. |
| `wait network-idle [--idle-ms N] [--timeout N]` | Wait until no request is in flight for `--idle-ms` (default 500). |

### Checking

| Command | What it does |
|---|---|
| `assert value (--selector "css"\|--uid nN) (--equals\|--contains\|--matches) <s>` | A form control's value. Secrets are compared, never printed. |
| `assert text (--contains\|--matches) <s> [--selector "css"\|--uid nN]` | Visible text of the page or one element. |
| `assert url (--equals\|--matches) <s>` | The current URL. |
| `assert state (--selector "css"\|--uid nN) (--checked\|--unchecked\|--selected <opt>\|--enabled\|--disabled\|--visible)` | Checked state, selected option, disabled, or rendered. |
| `assert exists --selector "css" [--count N\|--min N]` | How many elements match. `--count 0` asserts absence. |

### Monitoring and advanced

| Command | What it does |
|---|---|
| `network [--filter "pattern"] [--body] [--live N] [--abort "pattern"]` | Requests and API responses. `--abort` blocks matching requests for `--live N` seconds. |
| `console [--level error] [--clear]` | console.log/warn/error and JS exceptions. |
| `frame <selector\|main>` | Bind `eval`/`inspect` to an iframe. Only within one `pipe`/`batch` process. |
| `emulate device --width W --height H [--dpr N] [--mobile] [--touch] [--orientation portrait\|landscape] [--label name]` | Device metrics for one named page. Also `emulate status` and `emulate reset`. |
| `webmcp list` | Tools the page registered on `document.modelContext`. Also `webmcp call <name> --args '{"k":"v"}'`. |
| `macro record <name> --from-recording <file>` | Distil a recorded session into a guarded, parameterised path. Also `macro list`, `macro show`, `macro run`. |
| `replay <file>` | Re-run a `pipe --record` file command by command. |
| `batch` | Run a JSON array of commands from stdin. |
| `pipe` | Persistent JSON stdin/stdout connection. |

## Global flags

```
--browser <name>         Named browser profile (default: "default")
--page <name>            Named tab (default: "default")
--connect <auto|url>     Attach to a running Chrome (a value is required)
--proxy-server <url>     Proxy a managed Chrome (http(s), socks4/5; explicit port)
--headed                 Show the browser window (default: headless)
--stealth                Anti-detection CDP patches
--copy-cookies           Use cookies from your real Chrome profile
--chrome-arg <flag>      Extra flag for the Chrome that gets launched (repeatable)
--timeout <seconds>      Command timeout (default: 30)
--max-depth <N>          Limit inspect depth
--verdict <mode>         auto (default) reads the page back; off reports the action only
--budget <chars>         Cap the change report (default 1200; 0 = uncapped)
--on-intercept <mode>    dispatch (default), guard, or refuse
--ignore-https-errors    Accept self-signed certs
--dialog <mode>          JS dialog policy: accept (default), dismiss, or manual
--dialog-text <text>     Text submitted for prompt() dialogs under --dialog accept
--json                   Structured JSON output
```

Global flags parse on either side of the verb. `--timeout` and `--max-depth` are the exceptions:
commands that declare their own (`wait`, `download`, anything with `--inspect`) take them after the
verb, everywhere else they go before it. `--proxy-server` and `--chrome-arg` are launch-only and
fixed for the life of a named browser — a later command that omits them inherits them, one that
names different values is refused. Close or purge the browser to change them.

## Concepts

### uids

Element ids come from Chrome's `backendNodeId`, printed as `n82`. They stay valid across inspects
of the same page, so an agent can inspect once and act several times. A navigation reassigns them
all — re-inspect after `goto`, `back`, or a click that changes route. `goto` clears the uid map but
keeps the snapshot, so `diff` reports `document_changed` instead of erroring. Commands that resolve
a uid need one from a *stored* snapshot: inspect before `assert value --uid` or `download --uid`.

### Verdicts: did the page comply?

`ok:true` means the command ran, not that the page complied. Every mutating action carries a
`verdict`, a `verdict_reason` and a `next` — one token from a closed set of six, so a caller can
branch without parsing prose.

| `verdict` | `verdict_reason` | `next` | What it means |
|---|---|---|---|
| `changed` | `tree_delta`, `nodes_moved`, `focus_only` | `proceed` | The page moved; `delta` says how. On `focus_only` the only move was focus onto a real element, which may be a focusable ancestor of what you clicked. |
| `changed` | `value_kept` | `proceed` / `inspect` | The read-back on the element confirmed the write and the tree could not show it (a secret field renders as a fixed marker). `next` is `inspect` when the page read also failed. |
| `changed` | `values_lost` | `confirm` | It moved **and** emptied a field that held a value. `values_lost` names each one. A form that submitted and cleared itself looks the same as one that threw the input away. |
| `navigated` | `document_replaced` | `inspect` | New document. Every stored uid is dead. |
| `intercepted` | `hit_test_receiver`, `modal_dialog` | `dismiss` | Another element occupied the point and received the event. `intercepted_by` names it (tag, id, class, uid, z-index). Nothing is known about the target. |
| `not_kept` | `value_reverted`, `value_rewritten` | `stop` | The write reached the element and it does not hold it: empty, or rewritten by a mask. Read `value.actual`; a second fill gives the same answer. |
| `no_effect` | `delivered_no_change` | `confirm` | Delivery proven by hit test, and the tree stayed still inside `observed_after_ms`. |
| `unchanged` | `identical_tree` | `confirm` | The tree was identical while the tool watched. Delivery not proven. |
| `unknown` | `no_baseline`, `read_failed`, `identity_unreadable` | `inspect` | Nothing could be compared. Never "nothing happened". |
| `unknown` | `aim_point_off_target` | `inspect` | Nothing was dispatched and two readings of the aim point agreed, so a repeat misses identically. |
| `unknown` | `scroll_not_settled` | `retry` | Nothing was dispatched and the readings disagreed, so a repeat duplicates nothing. |
| `not_checked` | `reporting_disabled` | `proceed` | You passed `--verdict off`. |

Never repeat an action on `unknown`: the first one may already have landed.

Pointer actions also report `delivery` (`target_hit`, `intercepted`, `off_target`, `not_settled`,
`js`, `not_probed`) from a hit test at the coordinate about to be dispatched. `no_effect` is only
emitted behind `target_hit`. `--on-intercept` decides what happens when something else is in the
way:

| `--on-intercept` | Behaviour |
|---|---|
| `dispatch` (default) | Always sends the event through the receiver. |
| `guard` | Sends when the receiver is inert (no interactive tag or role, not focusable, no `cursor: pointer`); refuses when it could act, when it is an `<iframe>`, or when it cannot be identified. |
| `refuse` | Never sends. Returns `ok:false`, exit 1, with `delivery`, `intercepted_by`, `verdict`, `next` and `dispatched:false`. |

Two blind spots: the read-back window is a fixed 60 ms (reported as `observed_after_ms`, so a
validator firing at 400 ms is outside it — use `wait` then `assert value`), and canvas, WebGL and
CSS-only effects are invisible to the accessibility tree.

### `landed` and `serving`: where you ended up, and what answered

`goto` reports `landed{requested,final,redirected,http_status,serving}`. A fragment-only or
trailing-slash change is not a redirect. `http_status` is the final hop's status, read from the
Navigation Timing API so `--stealth` is untouched.

`serving` never changes `ok` or the exit code. Branch on `serving`, not on `ok`:

| `serving` | Meaning |
|---|---|
| `page` | Nothing measured contradicts the load. An absence of evidence, not a certificate — a paywall reads as `page` too. |
| `challenge` | An anti-bot vendor's frame or script and none of the site's own. `challenge_from` names the host. Use `--connect`, not `--stealth`. |
| `error` | The server answered 4xx/5xx. `http_status` says which. |
| `nothing_actionable` | No link, no form control, no script, almost no text. Also what a page that had not rendered yet looks like. Run `inspect` before giving up. |
| `unreadable` | The shape probe did not run. |

### Exit codes

`0` success · `1` error, including a bad flag · `2` a claim this tool made did not hold · `130`
Ctrl+C. `2` is an assertion, or a macro guard — the two things this tool promises about a page —
so CI can tell "the page is wrong" from "the tool broke".

```bash
chrome-agent fill --selector "#coupon" "SAVE10"
chrome-agent assert value --selector "#coupon" --equals "SAVE10"
chrome-agent assert state --selector "#terms" --checked
chrome-agent assert exists --selector ".result" --min 1
```

`assert` is a read: no change report, no verdict, and it never clicks. `--matches` is a Rust regex
(`\d`/`\w`/`\s` are ASCII-only, no `\p{...}`; `(?i)` works). Inside `batch` and `pipe` an assertion
has no exit code of its own — it is `ok:false` with an `assertion` object, and a `batch` that
stopped on it exits 1, not 2.

### Pipe and batch mode

One process, one connection, one JSON line per response, and uids stay stable across the whole
sequence — which is the reason to reach for it. The speed-up is real and small: pipe removes about
12 ms of per-command overhead, worth **1.5x on a stream of reads** (nine commands, 352 ms → 228 ms)
and **1.1x on a stream of fills and clicks** (2029 ms → 1908 ms), where the settle window and the
tree re-read pipe does not touch are most of the cost. Measured on 2026-08-30, M4 Max, Chrome 152,
median of 9 runs; reproduce with `./scripts/measure-pipe.sh`, record in
`docs/design/pipe-latency.md`.

```bash
echo '{"cmd":"goto","url":"https://example.com","inspect":true}
{"cmd":"click","uid":"n12","inspect":true}
{"cmd":"read"}' | chrome-agent pipe
```

`batch` takes a JSON array on stdin instead and is otherwise the same dispatcher, and answers with
one response object rather than one line per command. Pass `--json` to get that object as JSON;
without it the CLI prints one text line per entry.

The CLI `batch` process exits `1` when `--stop-on-error` cut the run short — never `2`, which stays
reserved for a claim that did not hold: the process is reporting that the batch stopped, not saying
anything about the page. Without `--stop-on-error` it ran every command it was
given and exits `0` even when one of them failed: read `ok`, on the batch and on each result.

### Iframes

`frame` binds `eval` and `inspect` to an iframe. The binding lives on the connection, so it only
survives inside one `pipe` or `batch` process:

```bash
printf '%s\n' \
  '{"cmd":"frame","target":"#payment-iframe"}' \
  '{"cmd":"inspect"}' \
  '{"cmd":"fill","uid":"n42","value":"4242424242424242"}' \
  '{"cmd":"frame","target":"main"}' | chrome-agent pipe
```

Name the iframe precisely (`iframe[src*="checkout"]`); a bare `iframe` matches the first one in DOM
order, often an empty ad slot. `frame` does not scope `--selector` targeting — inspect after the
switch and act by uid, which resolves across frames. The isolated world sees the frame's DOM but
not its main-world JS variables.

### `--stealth` vs `--connect`

`--stealth` applies 7 CDP-level patches (`Page.addScriptToEvaluateOnNewDocument`), not Chrome
flags: `navigator.webdriver`, `chrome.runtime`, the Permissions API, the WebGL renderer, the
User-Agent, an input coordinate leak, and never calling `Runtime.enable`.

| Protection | What works |
|---|---|
| None | `chrome-agent goto ...` |
| Cloudflare JS challenge ("Just a moment…") | `--stealth` clears it |
| Cloudflare managed Turnstile | `--stealth` does not help. Use `--connect`. |
| DataDome, Kasada | `--stealth` does not help. Use `--connect`. |
| Logged-in sites | `--copy-cookies`, optionally with `--stealth` |

Heavy protection fingerprints the Chromium binary itself, so the only route is a real installed
Chrome. `--copy-cookies` copies the cookie database from your Chrome profile; both instances use
the same Keychain, so encrypted cookies work, and your real Chrome is not affected.

```bash
google-chrome --remote-debugging-port=9222 &
chrome-agent --connect http://127.0.0.1:9222 goto https://www.leboncoin.fr --inspect
chrome-agent --stealth --copy-cookies goto x.com/home --inspect
chrome-agent --copy-cookies goto github.com/notifications --inspect
```

### Macros

A macro is a path that already worked once, kept under a name with the postconditions observed on
that success. `macro run` checks every step's guards and stops at the first that does not hold.
There is no repair and no retry.

```bash
chrome-agent macro record cancel --from-recording session.jsonl
chrome-agent macro run cancel --var email=ada@example.com
```

Guards are `delivery: target_hit`, the verdict word, `value.verbatim`, and a `url_matches` built
from the path — never the change counters, a uid, or a duration. A step aimed by uid is recorded by
role and accessible name, or refused. Secret fields become declared parameters, never file content.

A guard that was checked and did not hold exits **2**, the same code as a failed assertion: it is
the same kind of claim. The report carries `stopped_by: "guard"` along with which guard, what it
expected and what was there. A run that stopped for any other reason — the step itself failed, the
page could not be read, the macro file is missing — exits `1`, with `stopped_by: "error"`.

### Files on disk

Screenshots, PDFs and downloads land under `~/.chrome-agent/tmp` (or your `--out` path) with `0600`
permissions, and the path is printed on stdout. Binary bytes never reach stdout.

```bash
chrome-agent download https://app.com/reports/2024.csv --out ./2024.csv
chrome-agent download --selector "#export" --out ./export.csv
chrome-agent pdf --filename invoice.pdf --background
chrome-agent screenshot --uid n42
```

Read `downloaded`, not `ok`. A click that was delivered and produced no file answers `ok:true` with
`downloaded:false`: a click is not undoable, and an error there invites a second real one.
`--timeout` bounds the whole window; `--max-bytes` cancels a transfer that goes past it.

### Device emulation

```bash
chrome-agent --page mobile emulate device --label "checkout phone" \
  --width 412 --height 915 --dpr 2.625 --mobile --touch
chrome-agent --page mobile emulate status
chrome-agent --page mobile emulate reset
```

Metrics belong to one named page and are reapplied on each connection, because Chrome drops every
override when the CDP session that set it detaches. Under `--touch`, `click` and `check` dispatch
touch taps; `dblclick`, `hover` and `drag` stay mouse. Values are explicit: device preset catalogs
live in the DevTools frontend, not in CDP.

### WebMCP

Pages can register tools on `document.modelContext` (W3C WICG WebMCP). The protocol defines no
`outputSchema`, so `webmcp call` reports what the tool declared next to what the page measurably
did, using the same verdict machinery as any other action.

```bash
chrome-agent webmcp list
chrome-agent inspect
chrome-agent webmcp call add_to_cart --args '{"item":"Espresso Blend"}'
```

Most installed Chrome builds have no native WebMCP. Test against the real API with
`--chrome-arg --enable-features=WebMCP,WebMCPTesting`, or against a page shipping a polyfill. Under
a `frame` binding the response carries `frame_scoped: true`: a tool registered by the frame's own
main-world script is invisible from the isolated world.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `Element uid=nN not found` | The page navigated, or the snapshot was never stored. | Run `inspect` again. |
| Parallel agents corrupt each other | They share `--browser default`. | Give each one `--browser <unique>`. |
| A `frame` switch is lost | Each CLI call is a fresh connection. | Drive it through `pipe` or `batch`. |
| `select` throws "Element is not a `<select>`" | A custom React/MUI dropdown. | Click to open, then click the option. |
| Verdict is `unchanged` on a click that worked | Canvas, CSS-only or late effects are invisible to the tree. | `wait`, then `assert`. |
| 403 or a captcha with `--stealth` | Managed Turnstile, DataDome or Kasada. | `--connect` to a real Chrome. |
| `serving: nothing_actionable` on a page you know renders | The settle probe stopped before first paint. | Run `inspect`. |
| Blocked on a native `alert`/`confirm` | You passed `--dialog manual`. | Drop it, or answer the dialog. |
| `network --abort` catches nothing | It is blocking and runs for `--live N` seconds. | Start it before navigating. |

## Comparison

|  | chrome-agent | agent-browser (Vercel) | Playwright MCP |
|---|---|---|---|
| Language | Rust | Rust | TypeScript |
| Binary | 3 MB, zero runtime | 3 MB CLI + dashboard + cloud providers | Node + Playwright |
| Startup | 12 ms measured, one command on a running browser | daemon (fast after first) | cold start |
| UID stability | `backendNodeId`, stable across inspects | sequential `@e1`, reassigned per snapshot | N/A (selectors) |
| Action + observe | `--inspect` flag, one call | separate snapshot call | separate call |
| Compliance reporting | `verdict`/`next` on every action | no | no |
| Stealth | 7 CDP patches | delegated to cloud providers | none |
| Reader mode | `read` (Readability.js) | none | none |
| Record extraction | `extract`, structural, no LLM call | none | none |
| PDF export | `pdf` | none | none |
| MCP server | none | yes | yes |
| Cloud providers, iOS/Safari | none (`--connect` to anything) | yes | none |
| Codebase | ~28.2K lines of Rust in `src/` (blank and comment-only lines excluded; a test re-measures it) | ~40K lines (their figure, unverified here) | Playwright |

`extract` finds repeating records structurally with MDR/DEPTA-style heuristics (sibling similarity,
content heterogeneity, text-to-link ratio) instead of asking a model to read the DOM. On the Hacker
News front page it hands over 30 stories as records in 1,571 tokens against 5,652 for the
accessibility tree and 8,727 for raw HTML ([`scripts/measure.sh`](scripts/measure.sh)). The gap
depends on the page: on a blog archive that is nothing but a list, the two are close.

## When not to use it

- You need a test framework — use Playwright.
- You need an MCP server — there is none here.
- You need a browser fleet, a proxy pool or CAPTCHA solving — Browserbase, Steel, Browserless.
- You need Firefox or Safari — this speaks CDP, so Chrome only.
- You want a supported product — this is one person's project.

## Using it from an agent

`npx skills add sderosiaux/chrome-agent` installs a SKILL.md. Otherwise `chrome-agent --help`
embeds a full LLM usage guide, and every error carries a `hint` naming the next action. Claude Code
permissions: `{"permissions": {"allow": ["Bash(chrome-agent *)"]}}`.

```
chrome-agent (3 MB Rust binary, ~28.2K lines of Rust in src/)
    | CDP over WebSocket
    v
Chrome (headless by default, no Node.js, no runtime)
```

Design records live in [`docs/design/README.md`](docs/design/README.md).

## License

MIT
