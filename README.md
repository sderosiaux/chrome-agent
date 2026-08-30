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
  <strong>Turn a web page into records your agent can use, from one 3 MB binary.</strong>
</p>

<p align="center">
  <a href="README.md">English</a> | <a href="README.cn.md">简体中文</a>
</p>

> **Disclaimer:** This is an independent, community-driven project. It is not affiliated with, endorsed by, or sponsored by Google or the Chrome team.

> You're not the user. Your LLM is.
>
> You don't need to read this README. Your agent does. Install it, run `chrome-agent --help`, and let the LLM figure it out. The CLI embeds its own usage guide, every error comes with a hint for the next action, and `--json` gives an agent structured data without you writing an adapter. This page is here because GitHub expects one.

## The one thing it does that others don't

Every browser tool can hand a page to a model. The question is what shape it arrives in.

```bash
chrome-agent goto news.ycombinator.com
chrome-agent --json extract --limit 30
```

```json
{"ok":true,"count":30,"pattern":"TR.athing.submission","items":[
  {"title":"PGSimCity - How PostgreSQL Works",
   "url":"https://nikolays.github.io/PGSimCity/",
   "fields":["PGSimCity - How PostgreSQL Works (nikolays.github.io)"]},
  ...
]}
```

No selectors. No model call to find the rows. The pattern is detected structurally, with
MDR/DEPTA-style heuristics that score sibling similarity, content heterogeneity and
text-to-link ratio.

Measured on that page with [`scripts/measure.sh`](scripts/measure.sh), which you can run yourself:

| what you hand the model | tokens | what it gets |
|---|---|---|
| `extract --limit 30` | **1,571** | 30 stories as records, with URLs |
| `inspect` (accessibility tree) | 5,652 | the tree, stories mixed into the surrounding page |
| raw HTML | 8,727 | everything, including markup nobody reads |

All three contain the same 30 stories. Only the first hands them over as records. The
others hand over the page and leave the model to find the stories in it, which you pay for
twice: once in input tokens, again in the reasoning to parse them.

How wide that gap is depends on the page. On a blog archive that is nothing but a list,
`extract` returns ~12,500 tokens against ~16,100 for the tree, because there is little
surrounding markup to strip. The win comes from pages where the records sit inside a lot of other page furniture.

Every rival makes an agent either write per-site selectors, which break on the next deploy,
or pay a model to read the DOM, which is the recurring cost this tooling exists to avoid.

## How is this different from agent-browser?

[agent-browser](https://github.com/vercel-labs/agent-browser) (Vercel) is the closest thing
to this, it is also a Rust CLI built for agents, and it is well ahead: more features, more
users, near-daily releases. If you want a platform, use it. Two honest differences:

| | chrome-agent | agent-browser |
|---|---|---|
| **Repeating-record extraction** | `extract`, structural, no LLM call | not built in; `read` returns readable text |
| **Bot detection** | 7 in-binary CDP patches, `--connect` to real Chrome | no stealth in core; delegated to paid cloud providers |
| **Process model** | one command, one connection, exits | background daemon |
| **Element IDs** | `backendNodeId`, still valid on the next inspect of the same page | sequential `@e1`, reassigned on every snapshot |
| **Browser-native downloads** | `download --uid/--selector` clicks and captures the file, and says so when none began | supported |
| **MCP server** | none | yes |

Where it is genuinely behind: agent-browser has an encrypted credential vault, cloud
provider integrations and an MCP mode. It also reuses your real
Chrome profile, same as `--copy-cookies` here, so logged-in access is not a differentiator
for either of us.

## Why you shouldn't use chrome-agent

Borrowed from [ripgrep](https://github.com/BurntSushi/ripgrep), because the fastest way to
waste your afternoon is a README that only lists strengths.

- **You need a test framework.** Use Playwright. Assertions, retries, trace viewer, a
  test runner, and Microsoft maintaining it.
- **You want a supported product.** This is one person's project. No SLA, no roadmap
  promises, no enterprise support.
- **You need MCP.** There is no MCP server here. If your coding agent can't shell out, this is
  the wrong tool.
- **You need a browser fleet.** No cloud, no proxy pool, no CAPTCHA solving. Browserbase,
  Steel and Browserless do that.
- **Your target is behind DataDome or Kasada.** `--stealth` won't get you through. You'll
  need `--connect` to a real Chrome, and even then, no promises.
- **You need Firefox or Safari.** This speaks CDP. Chrome only.

## What it's built on

- **One binary, zero runtime.** No Node, no npm, no Playwright download. Linux builds are
  static (musl), so they run on any distro without a glibc version to match.
- **Errors are instructions.** Every failure carries a `hint` for the next action:
  `{"ok":false,"error":"...","hint":"run inspect"}`.
- **Stable element IDs.** uids come from Chrome's `backendNodeId`, so `n82` still points at
  the same node on the next inspect. They do change after a navigation, and `diff` will
  tell you when that happened instead of pretending to compare two different pages.
- **Sessions persist.** Chrome stays alive between calls, so a command costs a connection,
  not a browser launch.
- **Parallel agents don't collide.** `--browser agent1`, `--browser agent2`, separate Chrome
  instances and separate session state.

```
chrome-agent (3 MB Rust binary)
    | CDP over WebSocket
    v
Chrome (headless, no Node.js, no runtime)
```

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
| `close --orphans` | Close every running browser no session entry claims. `status` lists them; the profiles they leave are what `--purge-orphans` sweeps. |

### Inspection

| Command | What it does |
|---------|------------|
| `inspect [--verbose] [--max-depth N] [--uid nN] [--filter "role,role"] [--scroll] [--limit N] [--urls] [--max-chars N] [--offset K]` | a11y tree with UIDs. `--scroll --limit` for infinite scroll. `--urls` resolves href on links. `--max-chars`/`--offset` cap and page the output. Every one of these narrows what is PRINTED only: the snapshot stored for `diff` is the full tree, and every uid on the page stays actionable. |
| `diff` | What changed since last inspect. Compared against the FULL tree, whatever narrowing flags the inspect printed with — a `--filter`ed view is not a baseline, and using one reports every node the filter hid as an addition. |
| `screenshot [--filename name] [--format jpeg\|png] [--quality N] [--max-width N] [--uid nN\|--selector "css"]` | Screenshot to file. JPEG/quality/max-width shrink it; `--uid`/`--selector` clip to one element. |
| `pdf [--filename name] [--landscape] [--background]` | Print the current page to a PDF file. |
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

### Assertions

The exit code is the answer: **0** the claim held, **2** it did not, **1** it could not be checked
(no browser, a selector matching nothing, an unparseable regex, a CDP timeout). No other command
returns 2, and a bad flag exits 1 — so a CI job can tell "the page is wrong" from "the tool broke".

| Command | What it does |
|---------|------------|
| `assert value (--selector "css"\|--uid <uid>) (--equals\|--contains\|--matches) <s>` | A form control's value. Secret fields are compared but never printed. |
| `assert text (--contains\|--matches) <s> [--selector "css"\|--uid <uid>]` | Visible text of the page, or of one element. |
| `assert url (--equals\|--matches) <s>` | The current URL. |
| `assert state (--selector "css"\|--uid <uid>) (--checked\|--unchecked\|--selected <opt>\|--enabled\|--disabled\|--visible)` | Checked (native or `aria-checked`), the `<select>`'s option, disabled (`:disabled` or `aria-disabled`), or rendered. |
| `assert exists --selector "css" [--count N\|--min N]` | How many elements match. `--count 0` asserts absence. |

```bash
chrome-agent fill --selector "#coupon" "SAVE10"
chrome-agent assert value --selector "#coupon" --equals "SAVE10"; echo $?
# 0 if the field holds it. 2 if a mask or a controlled component rewrote it — and the
# response then names what the page kept: {"actual":"","held":false}

chrome-agent assert state --selector "#terms" --checked      # reads the same classification check/uncheck apply
chrome-agent assert exists --selector ".result" --min 1
```

`assert` is a read: no change report, no verdict, and it never clicks. `--matches` is a Rust
regex (`\d`/`\w`/`\s` are ASCII-only, no `\p{...}`; `(?i)` works). In `batch`/`pipe` there is no
exit code — a failed assertion is `ok:false` with the same `assertion` object.

### Content extraction

| Command | What it does |
|---------|------------|
| `read [--html] [--truncate N]` | Article extraction via Mozilla Readability. |
| `text [uid] [--selector "css"] [--truncate N]` | Visible text from page or element. |
| `eval <expression> [--selector "css"]` | JS in page context. `el` = matched element. |
| `extract [--selector "css"] [--limit N] [--scroll] [--a11y]` | Auto-detect repeating data. `--a11y` for React SPAs (X.com). |
| `download <url> [--out path] [--timeout N] [--max-bytes N]` | Download a URL fetched in-page, so cookies/auth carry over (login-gated files). Rejects responses over 64 MiB by default. Returns `{path,bytes,mime}`. |
| `download --uid n47 \| --selector "css"` | Click the element and capture the browser-native download it triggers — the only route to a file built in the page (`Blob`) or served by a POST no anchor names. Returns `{path,bytes,suggested_filename,source_url,delivery}`. A click that landed and produced nothing answers `ok:true` with `downloaded:false`. |

### Monitoring

| Command | What it does |
|---------|------------|
| `network [--filter "pattern"] [--body] [--live N] [--abort "pattern"]` | Network requests and API responses. `--abort` blocks matching requests. |
| `console [--level error] [--clear]` | console.log/warn/error + JS exceptions. |

### Advanced

| Command | What it does |
|---------|------------|
| `frame <selector\|main>` | Switch `eval`/`inspect` into an iframe (or back to main). Persists only within a `pipe`/`batch` process. |
| `emulate device --width W --height H [--dpr N] [--mobile] [--touch] [--orientation portrait\|landscape] [--label name]` | Apply explicit device metrics to the current named page. |
| `emulate status\|reset` | Report requested and page-observed metrics, or clear that page's overrides. |
| `webmcp list` | List tools a page registered on `document.modelContext`. No `outputSchema` — the protocol defines none. |
| `webmcp call <name> [--args '{"k":"v"}'] [--inspect]` | Call a tool by name and report what it declared next to what the page's accessibility tree measurably did. |
| `batch` | Execute multiple commands from a JSON array on stdin. |
| `pipe` | Persistent JSON stdin/stdout connection. |
| `status` | Which browsers the session store knows, their pids, and the running ones no entry claims (`orphan=` lines). |
| `history` | The pages this browser visited. |
| `replay <file>` | Re-run a `pipe --record` file command by command. `macro` is the guarded, parameterised form of the same idea — see below. |

## Global flags

These are accepted on either side of the verb: `fill --selector "#q" x --json` works as well as
`--json fill --selector "#q" x`. The exceptions are `--timeout` and `--max-depth`. A command that
declares its own takes it after the verb (`wait selector ".x" --timeout 5` keeps `wait`'s 10 s
default, not the global 30 s); everywhere else those two must come before it, and passing one
where it is not declared is a usage error naming the invocation which works.

```
--browser <name>         Named browser profile (default: "default")
--page <name>            Named tab (default: "default")
--connect <auto|url>     Attach to a running Chrome (a value is required: "auto", or a
                         ws:// or http:// URL)
--proxy-server <url>     Proxy a managed Chrome (http(s), socks4/5; explicit port required)
--headed                 Show browser window (default: headless)
--stealth                Anti-detection patches. Clears a Cloudflare JS challenge
                         ("Just a moment..."); does NOT clear a managed Turnstile or
                         DataDome -- see the protection table below
--copy-cookies           Use cookies from your real Chrome profile
--chrome-arg <flag>      Extra flag for the Chrome chrome-agent launches (repeatable)
--timeout <seconds>      Command timeout (default: 30)
--max-depth <N>          Limit inspect depth
--verdict <mode>         auto (default): an action reports what changed. off: report the action only
--budget <chars>         Cap that change report (default 1200; 0 = uncapped)
--on-intercept <mode>    dispatch (default), guard, or refuse — see the table below
--ignore-https-errors    Accept self-signed certs
--json                   Structured JSON output
--dialog <mode>          JS dialog policy: accept (default), dismiss, or manual
--dialog-text <text>     Text to submit for prompt() dialogs when --dialog accept
```

`--proxy-server` is launch-only and is persisted with the named browser session. Close or purge a
running named browser before changing its proxy. Attached browsers (`--connect`) must be configured
with their proxy before ChromeAgent attaches. Proxy URLs containing credentials are rejected.

`--chrome-arg` passes an extra flag straight to the Chrome chrome-agent launches, repeatable:

```bash
chrome-agent --chrome-arg --enable-features=WebMCP,WebMCPTesting goto https://example.com
```

Like `--proxy-server`, it is launch-only (no effect under `--connect` — that Chrome is already
running) and fixed for the life of a named browser: a follow-up command that omits it inherits
whatever flags the browser already runs with, and one that names different flags is refused.
Close or purge the browser to change them. A handful of flags are refused outright because
chrome-agent depends on its own values for them to find and reconnect to the browser it launched:
`--user-data-dir`, `--remote-debugging-port`, `--remote-debugging-pipe`, `--proxy-server` (use the
dedicated flag above), and `--headless` (use `--headed`).

JS dialogs (`alert`/`confirm`/`prompt`/`beforeunload`) are auto-answered by default (`--dialog accept`). A native dialog otherwise blocks the page with no DOM signal and the agent's next command hangs. Use `--dialog dismiss` to cancel them, or `--dialog manual` to opt out.

## The loop: inspect, act

```bash
chrome-agent goto https://app.com/login --inspect
# uid=n52 textbox "Email" focusable
# uid=n58 textbox "Password" focusable
# uid=n63 button "Sign In" focusable

chrome-agent fill --uid n52 "user@test.com"
# ~ uid=n52 textbox "Email" focusable value="" -> value="user@test.com"

chrome-agent fill --uid n58 "password123"
chrome-agent click n63
# Page navigated — previous uids are gone. New page:
# uid=n101 heading "Dashboard" level=1
```

An action says what it changed, so there is no second call to find out whether it landed.
That is the whole point: an agent loop pays for turns, not just for tokens.

UIDs stay the same between inspects as long as the DOM node exists. After a navigation they
are all reassigned, which is why the click above reports the page rather than a diff.

## What a response tells you

`ok:true` means the command ran, not that the page complied. Every mutating action carries a
`verdict`, a `verdict_reason` and a `next` — one token from a closed set of six, so an empty
change report is never ambiguous and a caller can branch without parsing prose.

| `verdict` | `verdict_reason` | `next` | What it means |
|---|---|---|---|
| `changed` | `tree_delta`, `nodes_moved`, `focus_only` | `proceed` | The page moved; `delta` says how. On `focus_only` the move was focus alone, onto a real **element** — `focus.to` names what RECEIVED focus, which is often a focusable **ancestor** of the node clicked (a span inside a link focuses the link). Focus landing on the **document** is not counted: it is what a click on nothing focusable leaves behind, so it answers `identical_tree` instead. |
| `changed` | `value_kept` | `proceed` / `inspect` | The state was confirmed by the read-back on the element itself, and the tree could not show it — a secret field renders as a fixed marker, so re-filling one leaves no delta to point at, and on a fresh session there is no tree to compare at all. `fill`, `select` and `check`/`uncheck` all reach it, through the same `value.verbatim`. The only row whose `next` is not a function of the reason alone: when the page read failed too, the verdict still stands (it is about the element) and `next` becomes `inspect` (nothing else was seen). |
| `changed` | `values_lost` | `confirm` | It moved **and** emptied a field that held a value — `values_lost` names each one. A form that submitted and cleared itself looks identical to one that threw the input away. |
| `navigated` | `document_replaced` | `inspect` | New document; every stored uid is dead. |
| `intercepted` | `hit_test_receiver`, `modal_dialog` | `dismiss` | Another element occupied the point aimed at and received the event. **`intercepted_by`** names it (tag, id, class, uid, z-index, whether it is a modal). Nothing is known about the target. `hit_test_receiver` is the one that fires in practice — 8 interceptions over 61 real sites, all of them it. `modal_dialog` needs the **top layer** (`:modal`: a `<dialog>` opened with `showModal()`, or fullscreen), is covered by a fixture and has **not** been seen in the wild: the `<div role="dialog">` overlays most sites ship never enter the top layer. Read `tag`/`id`/`class` rather than `z_index`, which was `auto` on 7 of those 8 and on both fixtures — scrims usually stack by DOM order and `position`. |
| `not_kept` | `value_reverted`, `value_rewritten` | `stop` | The write reached the element and it does not hold it: empty on the first, rewritten by a mask or normaliser on the second. Read `value.actual`; a second fill produces the same answer. |
| `no_effect` | `delivered_no_change` | `confirm` | Delivery **proven** by a hit test and the tree stayed still inside `observed_after_ms`. |
| `unchanged` | `identical_tree` | `confirm` | The tree was identical while the tool watched — delivery not proven. |
| `unknown` | `no_baseline`, `read_failed`, `identity_unreadable` | `inspect` | Nothing could be compared. Never "nothing happened". |
| `unknown` | `aim_point_off_target` | `inspect` | Nothing was dispatched, and the readings agreed — so a repeat aims at the same point and refuses again. |
| `unknown` | `scroll_not_settled` | `retry` | Nothing was dispatched at all and the readings disagreed, so a repeat duplicates nothing and can succeed. |
| `not_checked` | `reporting_disabled` | `proceed` | You passed `--verdict off`. |

Two of those rungs report that nothing was dispatched and only one asks for a retry, because what
separates them is whether two readings of the aim point agreed: `scroll_not_settled` is transient
(they disagreed — the point was still moving, so the next attempt aims at a settled box and
works), while `aim_point_off_target` is stable (they agreed and the point still could not be aimed
at, so an identical retry misses identically). The stable rung covers two shapes, and `aim` tells
them apart: a coordinate on screen means the element has no box a pointer can reach (a wrapped
inline box, a clipped container), and a coordinate outside the viewport means the page is holding
it there — a consent wall in `position: fixed` over a document whose scroll is locked, where
`scroll` reports success and moves nothing.

`unchanged` means the page did not change while the tool watched — not "the action had no
effect", which it cannot know: an unchanged tree is also what a click swallowed by an overlay
looks like. That is why pointer-targeted actions also report `delivery` (`target_hit`, `intercepted`,
`off_target`, `not_settled`, `js`, `not_probed`) from a hit test at the coordinate about to be
dispatched: `no_effect` is only ever emitted behind `target_hit`, and `not_settled`/`off_target`
mean nothing was sent. `--on-intercept refuse` turns an interception into an error instead of
sending the event anyway — `ok:false`, exit 1, and the same fields the dispatch would have
carried (`delivery`, `intercepted_by`, `verdict`, `next`, `verdict_hint`, `hint`) plus
`dispatched:false`, because the mode that refuses to act is the one whose caller has the most
re-planning to do.

| `--on-intercept` | Sends through the receiver when it looks... | Refuses when it looks... |
|---|---|---|
| `dispatch` (default) | anything | never — always sends |
| `guard` | inert (no interactive tag/role, not focusable, no `cursor: pointer`) | actionable, or an `<iframe>` (opaque — see below), or unidentified |
| `refuse` | never — always refuses | anything |

`guard` is the middle ground: neither extreme was right for an interception measured during a
site audit, where a click aimed at unrelated navigation landed on a consent wall's own "accept"
button and `dispatch` sent the click through it, accepting the wall on the caller's behalf. Five
of eight interceptions measured that day were inert (a `HEADER`, plain text, an image, a search
`<iframe>`) and would have been wrongly refused by a blanket `refuse`; three could act (that
consent button, a CMP `<iframe>`, a country-selector cell) and were wrongly sent through by
`dispatch`. `guard` reads `intercepted_by.actionable` — a native interactive tag, an ARIA
interactive role, explicit keyboard focusability, or a `cursor: pointer` computed style,
whichever the receiver has — computed inside the same probe call, so it costs no extra round
trip. It is deliberately **not** a keyword match against the receiver's text or class name
("accept", "agree", "j'accepte", a CMP vendor's own class fragment): those carried the
information to a person reading the field data, but a wordlist is never complete and never will
be, in every language a consent wall might use. An `<iframe>` receiver refuses under `guard`
regardless — its content is opaque from outside, so "inert" would be assumed, not measured, and
between a false refusal on an inert search box and a false dispatch into an unseen consent wall
this project accepts the former. The default stays `dispatch`: `guard`'s predicate has not been
measured at the scale that justifies `dispatch`'s own default (12/12 on the design fixtures, and
separately checked across dozens of real sites), and flipping the default would change outcomes
for every overlapping-button case, not only consent walls. A refusal under `guard` carries the
exact same payload a refusal under `refuse` does (`delivery`, `intercepted_by` — now with
`actionable` — `verdict`, `next`, `verdict_hint`, `hint`, `dispatched:false`); the two differ
only in the words explaining *why* nothing was dispatched.

`waited_ms` rides on a mutating response when the action waited for the page to load after it,
and only then — a click that navigates carries it, a click that did not does not, and it is what
answers "why did that take so long" without the caller guessing. A pointer event that Chrome does
not acknowledge within 8 s fails instead of waiting out `--timeout`, and says the event may
already have reached the page: the one thing not to do there is send it again. Pointer actions
also bring their page to the foreground first, because a background tab answers them on a fixed
five-second timer — with several pages open in one browser, clicking on one makes it the active
one, which is what clicking means.

### Macros

A macro is a path that already worked once, kept under a name with the postconditions observed
on that success. `macro record` distils a recorded session — the exploration and the dead ends
do not survive — and `macro run` replays it, checking every step's guards and stopping at the
first that does not hold.

```bash
chrome-agent macro record cancel --from-recording session.jsonl
chrome-agent macro run cancel --var email=ada@example.com
```

What becomes a guard is the whole point: `delivery: target_hit`, the verdict WORD (never the
reason), `value.verbatim`, and a `url_matches` built from the path. Never the `changed` counters,
never a uid, never a duration — a macro that pins those breaks on a page that still works. A step
aimed by a uid is recorded by role and accessible name or refused outright; a secret field becomes
a declared parameter and is never written to the file. A guard that does not hold stops the run
and reports the step, the guard, what was observed and the action's own `next`. There is no
repair and no retry: that line is deliberate.

Two blind spots, both stated rather than papered over: the read-back window is a fixed 60 ms
(reported as `observed_after_ms`, so a validator firing at 400 ms is outside it — `wait` then
`assert value`), and canvas, WebGL and CSS-only effects are invisible to the accessibility tree.

`--verdict off` restores the older behaviour: the action is reported, the page is not read
back. Faster, quieter, and you find out what happened on your next call — the response says
`not_checked` so you know that is what happened.

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

## Device emulation

```bash
chrome-agent --page mobile emulate device --label "checkout phone" \
  --width 412 --height 915 --dpr 2.625 --mobile --touch
chrome-agent --page mobile emulate status
chrome-agent --page mobile emulate reset
```

Metrics are attached to one named page. Chrome reverts every override the moment the CDP session
that set it detaches, so the configuration is persisted and reapplied at the start of each
connection — which also means that between commands on a headed or `--connect` browser the page
briefly shows its real metrics. Sibling pages keep their own metrics, with one visibility caveat:
Chromium exposes an orientation override only for its active target, so commands on an emulated
page activate that tab first, backgrounding its siblings the way switching tabs does. Closing or
restarting Chrome discards the configuration. Values are explicit because device preset catalogs
belong to the DevTools frontend, not CDP; the binary does not ship a copy that can drift from
Chromium. Under `--touch`, `click` and `check` dispatch touch taps instead of mouse events
(`dblclick`, `hover` and `drag` stay mouse — pages that only listen to touch will not see them);
Chrome's own mouse-to-touch conversion was measured to leave `Input.dispatchMouseEvent`
unanswered, which is why the taps are synthesized. `screenshot --max-width` counts CSS pixels, so
a `--dpr 2.625` capture is that factor larger. If Chromium rejects one of the override calls, the
command attempts every cleanup call and does not persist the incomplete configuration.

## Iframes

The `frame` switch binds `eval` and `inspect` to the iframe, but **only within one process**, so drive it through `pipe` (or `batch`), never as separate CLI calls:

```bash
printf '%s\n' \
  '{"cmd":"frame","target":"#payment-iframe"}' \
  '{"cmd":"inspect"}' \
  '{"cmd":"fill","uid":"n42","value":"4242424242424242"}' \
  '{"cmd":"frame","target":"main"}' | chrome-agent pipe
```

- Target the intended iframe precisely (e.g. `iframe[src*="checkout"]`); a bare `iframe` matches the first one in DOM order, often an ad `about:blank` slot.
- `frame` scopes `eval`/`inspect`; it does **not** scope `--selector` targeting. Run `inspect` after the switch to get iframe uids, then act by uid (uids resolve across frames).
- Each standalone `chrome-agent <cmd>` opens a fresh connection, so `chrome-agent frame …` followed by a separate `chrome-agent inspect` loses the switch. Use `pipe`/`batch`.

## WebMCP tools

Pages can register tools on `document.modelContext` (W3C WICG WebMCP). `webmcp list` discovers
them; `webmcp call` calls one and reports what it *declared* next to what the page *measurably
did* — because the protocol defines no `outputSchema`. A tool's return is a freeform string with
no contract to check it against, so `{"success":true}` and nothing having moved is not a
protocol violation, it looks exactly like a correct call:

```bash
chrome-agent webmcp list
# tool=add_to_cart "Add a product to the cart."
# note: no tool here carries an outputSchema — the protocol defines none...

chrome-agent inspect   # establish a baseline first, like any other action
chrome-agent webmcp call add_to_cart --args '{"item":"Espresso Blend"}'
# declared_result: {"success":true,"item":"Espresso Blend","price":"$18.00"}
# verdict: changed (tree_delta) — added=3 removed=2 changed=1: a real cart line appeared

chrome-agent webmcp call add_to_cart_broken --args '{"item":"Espresso Blend"}'
# declared_result: {"success":true,"item":"Espresso Blend","price":"$18.00"}   <- byte-identical
# verdict: unchanged (identical_tree) — nothing moved. Not "no effect": a canvas
# repaint, a CSS-only change, or a handler that runs after the window all look like this too.
```

`webmcp call` is reported exactly like any other action command — `verdict`, `delta`, `next` —
because that machinery is the only corroboration the protocol leaves available. It never invents
a stronger claim than the tree can support: a call that declares success and moves nothing reads
as `unchanged`, not as a lie, for the same reason `no_effect` elsewhere requires proof of
delivery before it is used.

Two of the spec's own sharp edges are handled rather than surfaced: `executeTool` requires the
actual `RegisteredTool` object from `getTools()` (a bare name throws
`TypeError: The provided value is not of type 'RegisteredTool'.`) and a JSON *string* second
argument (an object throws `Failed to parse input arguments`) — `webmcp call <name>` resolves the
tool and validates `--args` as JSON before either can happen.

Under a `frame` binding, `webmcp` hits the same isolated-world blindness `eval` already has: a
tool a frame's own main-world script registered is invisible from there (measured, not assumed —
see `tests/fixtures/webmcp_iframe_host.html`). The response carries `frame_scoped: true` so an
empty list reads as unproven, not as "this frame has none".

Most installed Chrome builds have no native WebMCP yet. Test against the real API with
`--chrome-arg --enable-features=WebMCP,WebMCPTesting` (see `--chrome-arg` above), or against a
page that ships its own polyfill.

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

| Protection | Solution | Measured |
|---|---|---|
| None | `chrome-agent goto ...` | — |
| Cloudflare **JS challenge** ("Just a moment…") | `chrome-agent --stealth goto ...` | `shop.app`: 403 and 6 nodes without it, 200 and the real page with it |
| Cloudflare **managed Turnstile** | `--stealth` does **not** help | `nowsecure.nl`: 10 nodes with it and without it, no difference |
| Logged-in sites | `chrome-agent --stealth --copy-cookies goto ...` | — |
| DataDome/Kasada | `chrome-agent --connect` to real Chrome | `leboncoin.fr`: 403 with `--stealth` and without |

The two Cloudflare rows are not the same product. The interstitial that says "Just a moment…"
runs a JS challenge that the seven patches satisfy; a managed Turnstile widget fingerprints
the browser itself, and no in-binary patch moves it. `--connect` to a real Chrome is the only
route this tool has for the second row and for DataDome. Each measurement above was taken
three times, with and without the flag, on the dates in the commit.

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
chrome-agent pdf --filename invoice.pdf --background

# Screenshots that don't blow up your context window.
chrome-agent screenshot --format jpeg --quality 60 --max-width 1024
chrome-agent screenshot --uid n42            # capture a single element (or --selector "css")
```

```bash
# Click-triggered downloads: an "Export CSV" button that builds the file in the page has no
# URL to fetch, so the only way to the bytes is to click it.
chrome-agent download --selector "#export" --out ./export.csv
# {"ok":true,"downloaded":true,"via":"click","path":"./export.csv","bytes":48213,
#  "suggested_filename":"report.csv","delivery":"target_hit","uid":"n42"}

# The click landed and nothing downloaded. Exit 0, because the click is not undoable and an
# error here invites a second real one.
chrome-agent download --selector "#maybe" --timeout 5 --json
# {"ok":true,"downloaded":false,"observed_after_ms":5002,
#  "message":"Clicked selector '#maybe' and no download began in the 5s that followed…",
#  "hint":"…Do not click again: the first click reached the page…"}
```

Two mechanisms, one verb, one contract: a file at a named path, 0600, with its size and the
name the server proposed. `download <url>` uses an in-page `fetch` with `credentials:'include'`,
so the request inherits the page's session. `download --uid/--selector` runs the same click as
`click` — same hit test, same `--on-intercept`, so a covered button reports its receiver — and
arms `Browser.setDownloadBehavior` around it.

**Read `downloaded`, not `ok`.** A click that was delivered is never an error, whatever it
failed to produce, for the reason a lost connection is never a retry: the page cannot tell a
second click from a second deliberate action. `--timeout` bounds the whole window (the transfer
must begin *and* finish inside it) and `--max-bytes` cancels a transfer that goes past it,
removing the partial file.

The arming lives on the connection that clicks because Chrome's download override does not
outlive the CDP session that set it — measured: a fresh connection clicking the same link with
nothing armed produces no file. That is why this is a flag on `download` and not a separate
`wait --download` verb, which would work in pipe mode and silently capture nothing from the CLI.

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
# {"ok":true,"url":"...","title":"...","landed":{"requested":"...","final":"...","redirected":false,"http_status":200,"serving":"page"},"snapshot":"uid=n1 heading..."}

# `landed` is where you aimed vs where you ended up. An expired session redirecting to a
# login wall used to be indistinguishable from a successful load:
chrome-agent --json goto https://app.example.com/orders
# {"ok":true,"url":"https://app.example.com/login?next=/orders","title":"Sign in",
#  "landed":{"requested":"https://app.example.com/orders",
#            "final":"https://app.example.com/login?next=/orders",
#            "redirected":true,"http_status":200,"serving":"page"},
#  "hint":"The redirect landed on a path containing 'login', which often means the session expired. ..."}
# A fragment-only or trailing-slash change is not a redirect. `http_status` is the final
# hop's status (a followed 302 reports 200), read from the Navigation Timing API so
# --stealth is untouched, and absent — never 0 — when the page reports none.

# `serving` is what answered. `ok:true` only says the navigation happened — a WAF refusal
# served with a 200, a captcha interstitial and a 403 all used to look like a load:
chrome-agent --json goto https://www.leboncoin.fr
# {"ok":true,"title":"leboncoin",
#  "landed":{...,"http_status":403,"serving":"challenge",
#            "challenge_from":"geo.captcha-delivery.com"},
#  "hint":"A challenge frame from geo.captcha-delivery.com is the only thing here to act on..."}
#
#   page                nothing measured contradicts the load — an absence of evidence,
#                       not a certificate (a paywall reads as `page` too)
#   challenge           an anti-bot vendor's frame or script, and no form of the site's own;
#                       `challenge_from` names the host. Use --connect, not --stealth
#   error               the server answered 4xx/5xx — `http_status` says which
#   nothing_actionable  no link, no form control, no script, almost no text. What an
#                       edge-served refusal looks like, and what a page that had not
#                       rendered yet looks like. Run `inspect` before giving up
#   unreadable          the shape probe did not run

chrome-agent --json eval "1+1"
# {"ok":true,"result":2}

# Errors exit 1 but JSON is still on stdout (parseable):
chrome-agent --json click n99
# {"ok":false,"error":"Element uid=n99 not found.","hint":"Run 'chrome-agent inspect'"}

# An assertion that did not hold exits 2 — a page fact, not a tool failure:
chrome-agent --json assert value --selector "#coupon" --equals "SAVE10"
# {"ok":false,"assertion":{"kind":"value","expected":"SAVE10","actual":"","held":false},"hint":"..."}
```

Exit codes: `0` success · `1` error (including a bad flag) · `2` an assertion did not hold · `130` Ctrl+C.

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
| Page cost, HN front page | 5,652 tokens (`inspect`), 1,571 (`extract`, all 30 records) | not measured here | not measured here |
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
| File download | `download <url>` (in-page fetch, auth-preserving) and `download --uid/--selector` (click-triggered) | `download` | via events |
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
| Codebase | ~22.2K lines of Rust in `src/` (blank and comment-only lines excluded; a test re-measures it) | ~40K lines (their figure, unverified here) | Playwright |
| Design goal | minimal tokens, maximal autonomy | feature-complete platform | browser testing |

## License

MIT
