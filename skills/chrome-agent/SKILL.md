---
name: chrome-agent
description: Local browser automation with structured, verified outcomes. Use for web navigation, scraping and extraction, form interaction, screenshots and downloads, network or console checks, responsive testing, or page-scoped device emulation.
metadata:
  author: sderosiaux
  version: "0.13.0"
  tags: ["browser", "automation", "scraping", "chrome", "cdp"]
---

# chrome-agent — Browser Automation

```bash
which chrome-agent || npm install -g chrome-agent   # or: cargo install chrome-agent
```

## What this tool guarantees

1. **Every mutating action states what it may claim about itself** — `verdict` + `verdict_reason` + `next`, always present, never four different silences collapsed into one empty report.
2. **A pointer-targeted action says who received the event** — `delivery` comes from a hit test at the coordinate about to be dispatched, and `intercepted_by` names the element that took it instead.
3. **A write says what the page kept** — `value.actual` beside `value.requested`, read back through a stated window (`observed_after_ms`). Same field and same window for `fill`, `select` and `check`/`uncheck`: they are one measurement on three kinds of control.

## Reading a response

`ok:true` means the command ran, not that the page complied. Read `verdict`, and read
`value.verbatim` whenever you filled something, before you tell anyone the action worked.

`next` is one token from a closed set of six — `proceed` `inspect` `retry` `confirm` `dismiss`
`stop` — so you can branch without parsing prose. **Branch on `next`, not on the verdict word.**

| `verdict` | `verdict_reason` | `next` | What to do |
|---|---|---|---|
| `changed` | `tree_delta` | proceed | The page moved; `delta` says how. |
| `changed` | `nodes_moved` | proceed | Same nodes, reordered (a drag landed). |
| `changed` | `focus_only` | proceed | Nothing moved but focus — the only sign the action arrived. |
| `changed` | `value_kept` | proceed / **inspect** | The element held what was asked of it when it was read back, and the tree could not show it — a secret field renders as a fixed marker, so a **refill** of one produces no delta, and on the first action of a session there is no tree to compare at all. `fill`, `select` and `check`/`uncheck` all reach this row, and the evidence is `value.verbatim` on each, not the delta. This is the one row whose `next` depends on more than the reason: if the PAGE read failed as well, the same verdict answers `inspect`, because the element is confirmed and nothing else on the page was seen. Branch on `next`. |
| `changed` | `values_lost` | **confirm** | It moved AND emptied a field that held a value. `values_lost:[{uid,role,name,was}]` names each. A form that submitted-and-cleared and a form that discarded your input look identical here. Confirm with `assert text --contains` on the page's own confirmation, or `network`, before re-filling — a re-submit may send the work twice. |
| `navigated` | `document_replaced` | inspect | New document. Every stored uid is dead. |
| `intercepted` | `hit_test_receiver` | dismiss | Another element occupied the aim point and got the event. `intercepted_by` names it (`tag`/`id`/`class`/`uid`/`z_index`/`text`/`modal`). Nothing is known about the target. |
| `intercepted` | `modal_dialog` | dismiss | A modal holds the top layer and receives everything outside itself. Press Escape or click its own dismiss control first. |
| `not_kept` | `value_reverted` | **stop** | The write reached the element and it held **nothing** afterwards. Do not fill again — the same write produces the same answer. Read `value.actual`. |
| `not_kept` | `value_rewritten` | **stop** | It holds something **else** — a mask, a trimmer, a normaliser. The write landed in the page's own shape. Read `value.actual` and decide whether that is the value you wanted. |
| `no_effect` | `delivered_no_change` | confirm | Delivery **proven** to the target, and the tree stayed still within `observed_after_ms`. The strongest word available. Repeating is the one thing that cannot help. |
| `unchanged` | `identical_tree` | confirm | The tree was identical while the tool watched. Delivery was **not** proven. |
| `unknown` | `no_baseline` | inspect | First action of the session on this page; nothing to compare against. |
| `unknown` | `read_failed` | inspect | The action ran; reading the page afterwards failed. |
| `unknown` | `identity_unreadable` | inspect | The two trees may not belong to the same document. |
| `unknown` | `aim_point_off_target` | inspect | Two readings of the aim point **agreed** and it still could not be aimed at — **nothing was dispatched**, and the miss is **stable**, so an identical retry misses identically. Two shapes, told apart by `aim`: a point on screen outside the element's own boxes (a wrapped inline box, a clipped container) → aim at a child that has a box of its own; a point **outside the viewport** (a `position: fixed` wall, a locked document scroll) → no scroll will move it, change the page's state first. |
| `unknown` | `scroll_not_settled` | **retry** | Two readings of the aim point **disagreed**: it was still moving — **nothing was dispatched**, and the miss is **transient**, so the retry is the fix: the movement ends and the next attempt aims at a settled box. `wait`, then repeat. |
| `not_checked` | `reporting_disabled` | proceed | You passed `--verdict off`. The silence is yours, not the page's. |

`delivery` rides on pointer-targeted actions (`click`, `dblclick`, and the `check`/`uncheck` click) and is what licenses the two strong words:

| `delivery` | Means | Licence |
|---|---|---|
| `target_hit` | The aim point resolved to the target, a descendant, its label's control, or its shadow host. | The only value that permits `no_effect`. |
| `intercepted` | The aim point belongs to an element outside the target's flat subtree. | → `verdict: intercepted`, `intercepted_by` names the receiver. |
| `off_target` | No point on the target could be aimed at, and two consecutive probes agreed: outside its own client rects, or outside the viewport and pinned there. | **Nothing was dispatched**, so repeating cannot double an action — but the miss is **stable**, so it cannot succeed either. Hence `inspect`. |
| `not_settled` | Two consecutive probes disagreed: the aim point was still moving when the settle budget ran out. | **Nothing was dispatched**, and the miss is **transient**, so repeating is both safe and the fix. Hence `retry`. |
| `js` | Went through a JS `click()`/`MouseEvent`, which performs no hit test. | Interception is inapplicable, not undetected. Licenses nothing. |
| `not_probed` | No hit test ran, or its answer does not cover the document that matters (a target inside an iframe). | Absence of evidence. Licenses **no** conclusion. |

`--on-intercept refuse` turns an interception into an error that dispatches nothing, instead of
the default `dispatch` (send it anyway — what a pointer does — and name the receiver). The
refusal is `ok:false` (CLI exit 1) and carries **the same fields as the dispatch** —
`delivery`, `intercepted_by`, `uid`, `aim`, `verdict`, `verdict_reason`, `next`, `verdict_hint`
and `hint` — plus `dispatched:false`. Branch on `next` there as everywhere else: it says
`dismiss`, and since nothing reached the page, dealing with the receiver and aiming again
duplicates nothing. `dispatched:false` appears on **every** response that aimed and sent
nothing, refusal or not.

## The rule: never report a success the tool did not confirm

`unchanged` means the tree was identical while the tool watched — which is also what a click
swallowed by an overlay looks like. Confirm with `assert` before repeating the action, because a
repeat is a second real click.

`unknown` always means the tool could not compare, never that nothing happened: run `inspect`
once and continue from what you see; do not re-send the action. The one exception is
`scroll_not_settled`, where `next` says `retry` because nothing was dispatched at all — which is
why you branch on `next` and not on the word `unknown`.

Two `unknown` rungs report that nothing was dispatched, and only one licenses a repeat. The
difference is whether the readings agreed, not whether an event was sent: `scroll_not_settled` is
**transient** (two probes 30 ms apart disagreed, so the retry aims at a settled box and succeeds),
`aim_point_off_target` is **stable** (the probes agreed and the point still could not be aimed at,
so an identical retry is an infinite loop dressed as a recovery — look at the page instead). A
consent wall in `position: fixed` whose control sits above the viewport, on a document whose
scroll is locked, is the second: `scroll` answers "Scrolled into view" and moves nothing, and the
aim point comes back identical to the pixel every time.

When `verdict` is `intercepted`, the element named in `intercepted_by` received your event and
nothing at all is known about the target — dismiss that element first, then repeat; when it is
`no_effect`, delivery was proven and the page stayed still, so repeating is the one thing that
cannot help.

**Latency has a field.** `waited_ms` appears on a mutating response when the action waited for
the page to load after it, and only then — so a ten-second command explains itself instead of
looking like a hung tool. A pointer event Chrome does not acknowledge within 8 s fails with
`ok:false` rather than waiting out `--timeout`, and its message says the event may already have
reached the page: **do not repeat it**, run `inspect` and read the state it was supposed to
produce. `click`, `dblclick`, `hover` and `drag` bring their page to the foreground first (a
background tab answers pointer events on a fixed five-second timer), so acting on one page of a
multi-page browser makes that page the active one.

**Two stated blind spots.** Neither can be fixed by retrying:

- **The observation window is fixed at 60 ms** (`observed_after_ms` reports it). It catches a
  revert on the microtask queue, in `setTimeout(0)` or in an animation frame. It does **not**
  catch a validator that clears the field at 400 ms. If persistence matters, `wait` then
  `assert value` — that is the measurement no single action can make.
- **Canvas, WebGL and CSS-only effects are invisible to the accessibility tree.** A class, an
  opacity, a transform, a repaint: all of them look like `unchanged` or `no_effect`. Confirm with
  `screenshot` or `eval`, never with a second click.

## Workflow: inspect → act → assert

```bash
chrome-agent goto https://app.com/login --inspect   # uids
chrome-agent fill --uid n52 "user@test.com"         # value: what the page kept
chrome-agent click n63                              # verdict + delivery + next
chrome-agent assert text --contains "Dashboard"     # exit 0 = evidence you may quote
```

Re-inspect when `verdict` is `navigated`, when `next` says `inspect`, and after any SPA route
change (`back`, `forward`, a click that re-renders) — uids are reassigned. For an SPA detail
page prefer `goto <direct-url>` over `click`, which may open a modal instead.

## `assert` is the guard, and the exit code is the answer

Prove the end state with `assert` before reporting a task complete — exit 0 is the only evidence
you may quote to the user, exit 2 is the page telling you what it actually holds, and exit 1
means nothing was compared, so retry.

```bash
chrome-agent assert value --selector "#email" --equals "ada@example.com"
chrome-agent assert text --selector "#status" --contains "Shipped"
chrome-agent assert url --matches "/orders/[0-9]+$"
chrome-agent assert state --selector "#terms" --checked        # native OR aria-checked
chrome-agent assert state --uid n15 --selected "California"    # a dropdown's current option
chrome-agent assert state --selector "#submit" --disabled      # :disabled OR aria-disabled
chrome-agent assert state --selector ".modal" --visible         # rendered, opaque, not hidden
chrome-agent assert exists --selector ".result" --count 10      # --min N, or --count 0 for absence
```

- **0** the claim held when we looked · **2** the page is not in that state (report or repair) ·
  **1** nothing was compared — no browser, a selector matching nothing, an unparseable regex, a
  CDP timeout (retry). No other command exits 2; a bad flag exits 1.
- `assert` is a read: no change report, no verdict, and it never clicks. Secrets are compared but
  never printed (the response gives both lengths).
- `--checked` reads the same classification `check`/`uncheck` apply, `--selected` the same reading
  `select` uses — an assertion cannot disagree with the action.
- `--matches` is a Rust regex: `\d` `\w` `\s` are ASCII-only, no `\p{...}`, no lookaround; `(?i)`
  for case-insensitive. `--contains` is a plain substring, case-sensitive.
- In `pipe`/`batch` there is no exit code: a failed assertion is `ok:false` with the same
  `assertion` object.

## Targeting

Three modes: **uid** (from `inspect`, e.g. `n47` — stable across inspects of the same page,
based on `backendNodeId`), **`--selector`** (CSS), **`--xy`** (coordinates).

Every targeted action returns the `uid` it actually resolved, even when you aimed with
`--selector` — cross-check it against the uids in `delta` to confirm the change you are reading
belongs to the element you meant.

`click --selector` is the same verb as `click <uid>`: both resolve the viewport centre and
dispatch native input — mouse events normally, or a touch tap when the page uses `--touch`.
So a `--selector` on a button behind a cookie banner clicks the banner — which is what a pointer
does, and what `intercepted_by` tells you.

## Commands

```bash
# Navigation
chrome-agent goto <url> [--inspect] [--wait-for "selector"] [--header "K: V"]
chrome-agent back
chrome-agent forward
chrome-agent scroll down|up|<uid>
chrome-agent wait network-idle [--idle-ms N] [--timeout N]   # deterministic SPA/XHR settle
chrome-agent wait text|url|selector "pattern" [--timeout N]

# Inspection
chrome-agent inspect [--max-depth N] [--filter "button,link"] [--uid nN] [--urls]
chrome-agent inspect --filter "link" --urls               # links with resolved href URLs
chrome-agent inspect --filter "article" --scroll --limit 50   # infinite scroll
chrome-agent inspect --max-chars 4000 [--offset 4000]     # cap/page huge trees
chrome-agent diff                                         # what changed since last inspect
# Every narrowing flag above (--filter/--max-depth/--uid/--limit/--urls/--max-chars) changes
# what is PRINTED only. `diff` always compares against the full tree, and every uid on the
# page stays actionable — so `inspect --filter button` then `diff` reports what really moved.

# Click / double-click (uid, --selector, --xy)
chrome-agent click <uid> [--inspect]
chrome-agent click --selector "css" [--inspect]
chrome-agent click --xy 100,200
chrome-agent dblclick <uid> [--inspect]

# Fill & type
chrome-agent fill --uid <uid> <value>
chrome-agent fill --selector "css" <value>
chrome-agent fill-form n20="a@b.com" n30="password"       # returns one value report per field
chrome-agent type "text" [--selector "input.search"]
chrome-agent press Enter|Tab|Escape                       # a single printable char types it

# Dropdowns, checkboxes, radios
chrome-agent select --uid <uid> "Option text"             # option.value first, then visible text
chrome-agent check <uid> | chrome-agent uncheck <uid>     # idempotent; refuse what they cannot check
chrome-agent upload --uid <uid> /path/to/file.pdf         # --selector too (file inputs hide from a11y)
chrome-agent drag <from-uid> <to-uid>                     # mouse events; NOT HTML5 DnD
chrome-agent hover <uid>

# Device metrics — explicit values, scoped to one named page, no preset catalog:
chrome-agent --page mobile emulate device --label "checkout phone" --width 412 --height 915 --dpr 2.625 --mobile --touch
chrome-agent --page mobile emulate status
chrome-agent --page mobile emulate reset
# Persisted per named page, reapplied at the start of each connection; restart discards it.
# Acting on an emulated page activates its tab (backgrounds siblings — a Chromium requirement
# for orientation). Under --touch, click/check tap; dblclick/hover/drag stay mouse events.

# WebMCP tools — document.modelContext.getTools()/.executeTool()
chrome-agent webmcp list                                          # name, description, inputSchema; no outputSchema
chrome-agent inspect                                              # baseline first, like any action
chrome-agent webmcp call add_to_cart --args '{"item":"Espresso Blend"}'
# Reported like any other action: verdict/delta/next come from the same accessibility-tree
# diff every mutating command gets — the protocol defines no outputSchema, so that diff is the
# only corroboration for what a tool DECLARED it did. A tool that declares success and moves
# nothing reads verdict=unchanged/identical_tree, never a stronger claim (canvas/CSS/late
# handlers all look the same to this measurement). Resolves the RegisteredTool object and
# validates --args as JSON itself, so the spec's own TypeError/SyntaxError traps don't reach you.
# Under `frame`, hits the same isolated-world blindness eval has — response carries
# frame_scoped:true when that applies; an empty list there is unproven, not "none".
# Most installed Chrome has no native WebMCP: `--chrome-arg --enable-features=WebMCP,WebMCPTesting`.

# Iframes — the frame switch lives on the connection, so use pipe/batch:
printf '%s\n' \
  '{"cmd":"frame","target":"iframe[src*=\"checkout\"]"}' \
  '{"cmd":"inspect"}' \
  '{"cmd":"fill","uid":"n42","value":"..."}' \
  '{"cmd":"frame","target":"main"}' | chrome-agent pipe
# Separate CLI calls (frame then inspect) do NOT work — each is a fresh connection.
# frame scopes eval+inspect, NOT --selector targeting. Act by uid after switching.

# Multi-step (pipe keeps one connection; uids stay valid across commands)
echo '[{"cmd":"goto","url":"..."},{"cmd":"inspect"},{"cmd":"click","uid":"n12"}]' | chrome-agent batch
echo '{"cmd":"goto","url":"...","inspect":true}' | chrome-agent pipe

# Network + console
chrome-agent network [--filter "pattern"] [--body] [--limit N]   # already loaded, stealth-safe
chrome-agent network --live 5 --body --filter "graphql"
# --body + --filter fetches every match whatever its MIME (the filter is the selection);
# without --filter only textual types. Binary: counted in body_omitted, never printed.
chrome-agent network --abort "*tracking*" --live 30              # blocking; start before navigating
chrome-agent console [--level error] [--clear]

# Files (written 0600 under ~/.chrome-agent/tmp; path on stdout, never base64)
chrome-agent screenshot [--format jpeg] [--quality N] [--max-width N] [--uid nN|--selector "css"]
chrome-agent pdf [--filename name] [--landscape] [--background]
chrome-agent download <url> [--out path] [--max-bytes N]        # in-page fetch, keeps the login
chrome-agent download <--uid nN|--selector "css"> [--out path]  # CLICK it, capture the download

chrome-agent tabs
chrome-agent close [--purge]
chrome-agent close --orphans                                     # close browsers no session claims
```

## Global flags

Accepted on **either side of the verb** — `fill --selector "#q" x --json` and `--json fill
--selector "#q" x` are the same command. Two exceptions: `--timeout` and `--max-depth`. A command
that declares its own takes it after the verb (`wait selector ".x" --timeout 5`, `click n1
--max-depth 2`); everywhere else those two must come **before** it. Passing one where it is not
declared is an exit-1 usage error that names the invocation which works.

```bash
--stealth          # 7 anti-detection CDP patches (Cloudflare/Turnstile)
--json             # structured JSON on stdout; errors exit 1 with {"ok":false,...} still on stdout
--browser <name>   # isolate parallel agents — sharing "default" corrupts sessions
--page <name>      # named tabs
--max-depth N      # limit inspect tree depth
--verdict MODE     # auto (default): report what changed. off: report the action only
--budget N         # cap the change report, in chars (default 1200; 0 = uncapped)
--on-intercept M   # dispatch (default) | refuse
--timeout N        # deadline on every CDP call (default 30s)
--headed           # show the window
--connect URL      # attach to a real Chrome (DataDome/Kasada)
--proxy-server URL # http(s)/socks4/5://host:port; launch-only
--copy-cookies     # use cookies from your real Chrome profile
--chrome-arg FLAG  # extra flag for the launched Chrome (repeatable); launch-only, see below
--dialog MODE      # accept (default) | dismiss | manual · --dialog-text for prompt()
```

`--chrome-arg` passes an extra flag straight to the Chrome chrome-agent launches, e.g. the flag
that unlocks an experimental feature:

```bash
chrome-agent --chrome-arg --enable-features=WebMCP,WebMCPTesting goto https://example.com
```

Like `--proxy-server`, it applies only when chrome-agent launches Chrome (no effect, and refused
rather than silently ignored, under `--connect`), and is fixed for the life of a named browser: a
follow-up command that omits it inherits whatever the browser already runs with, and one naming
different flags is refused — close or purge the browser to change them. Refused outright:
`--user-data-dir`, `--remote-debugging-port`, `--remote-debugging-pipe`, `--proxy-server` (use
`--proxy-server` above instead), `--headless` (use `--headed` instead) — chrome-agent depends on
its own values for these to find and reconnect to the browser it launched.

Exit codes: **0** success · **1** error (including a bad flag) · **2** an assertion did not hold ·
**130** Ctrl+C. JS dialogs auto-accept so the page never hangs.

### Bot protection

| Site protection | Solution |
|---|---|
| None | `chrome-agent goto ...` |
| Cloudflare/Turnstile | `chrome-agent --stealth goto ...` |
| Logged-in sites (X, Gmail) | `chrome-agent --stealth --copy-cookies goto ...` |
| DataDome/Kasada | `google-chrome --remote-debugging-port=9222 &` then `chrome-agent --connect http://127.0.0.1:9222 goto ...` |

## Response shapes (`--json`)

```
Success: {"ok":true, ...}
Error:   {"ok":false, "error":"message", "hint":"what to do next"}
```

- `goto` → `{"url","title","landed":{"requested","final","redirected","http_status"}}`. Check
  `redirected` before trusting the page: an expired session bouncing to a login wall otherwise
  reads as a successful load. A fragment-only or trailing-slash difference is not a redirect.
  `http_status` is the final hop as the browser reported it, **absent** (never 0) when there is
  none. A redirect onto `/login`, `/signin`, `/auth`, `/sso` adds a `hint` — a guess from the
  URL, so `inspect` to confirm. `goto` carries no verdict: you navigated on purpose.
- `click`/`fill`/`select`/`check`/… → `{"message", "uid", "verdict", "verdict_reason", "next",
  "verdict_hint"?, "changed":{"added","removed","changed","unchanged","moved","anonymous",
  "document_changed","identity_known"}, "delta":"+ uid=n88 heading \"Saved\""}`, plus
  `"focus":{"from","to"}` when focus moved (deliberately not counted as a content change).
- `fill` also → `"value":{"requested","actual","verbatim","observed_after_ms","caveat"?}`.
  A secret field (`type=password`, or `autocomplete` naming a password/card/CVC/one-time code)
  reports `{"redacted":true,"requested_length","actual_length","verbatim"}` instead — the
  response reaches stdout, your transcript and any recording. `caveat` when the write exceeded
  `maxlength`: verbatim, and about to be rejected by the form.
- `fill-form` / `fill_and_submit` → `"values":[{"uid"|"selector","value":{...}}]`, one per field,
  redacted the same way. On `fill_and_submit` this is the **only** witness: the change report
  runs after the submit, by which time the form has moved on.
- `select` → `"value":{"requested","actual","verbatim"}` with the option TEXT, plus
  `observed_after_ms` at the top level (the window covers setting the option and looking again).
  A selection the page reverted inside the window is refused with a non-zero exit, so a response
  you receive describes a selection the page held.
- `check`/`uncheck` → `"value":{"requested","actual","verbatim"}` with `checked`/`unchecked`
  (`indeterminate` for a mixed native box), plus `observed_after_ms`. Both are **absent** when the
  element already held the state: nothing was dispatched, so there was no post-action moment and
  no write of yours to have been kept — that response answers `no_baseline`/`identical_tree`, not
  `value_kept`, and it is not evidence that this action changed anything.
- pointer-targeted actions → `"delivery"`, `"aim":[x,y]`, and `"intercepted_by":{...}` when intercepted.
- `inspect --max-chars` → `{"total_chars","truncated","next_offset"}`.
- `read`/`text` → `{"text"}` · `eval` → `{"result"}` · `network` → `{"requests":[...]}` ·
  `console` → `{"messages":[...]}` · `batch` → `{"results":[...]}` ·
  `screenshot`/`pdf` → `{"path"}` · `download <url>` → `{"via":"fetch","downloaded","path","bytes","mime"}`.
- `download --uid/--selector` clicks and captures the browser-native download — the only route to
  a file the page builds itself (`Blob`) or serves from a POST no anchor names. It runs the same
  click as `click`, so `delivery`/`intercepted_by` ride along and `--on-intercept refuse` works.
  **Branch on `downloaded`, never on `ok`.** A click that landed and produced no file answers
  `ok:true` with `downloaded:false`, a `message`, and a hint that forbids the retry — an error
  there would invite a second real click, and the page cannot tell that from a second deliberate
  action. `--timeout` bounds the whole window (begin AND finish); `--max-bytes` cancels a
  transfer past it and removes the partial file. `download` carries no verdict and no change
  report: `downloaded`/`path` are the result, the way `landed` is for `goto`. Exactly one target
  — a URL, `--uid`, or `--selector`. Naming none or two is a clap usage error on stderr with
  exit 1, before any browser is resolved, so it arrives even from a shell that has never run
  this tool; under `--json` there is no `{"ok":false}` on stdout for it, as with any malformed
  invocation.

## Cheap reads: choose the right one

Reach for `extract` first on anything that looks like a list. Figures are measured on the Hacker
News front page with `./scripts/measure.sh`; they scale with the page, so treat them as relative.

| Tool | When | Tokens on HN |
|------|------|--------|
| `extract` | Repeating records: grids, feeds, tables, results. No selectors. | ~1,570 for all 30 |
| `read` | Articles, blog posts, product pages (Mozilla Readability) | ~950 |
| `text --selector "main"` | Scoped visible text | ~1,040 unscoped |
| `network --filter "api"` | The page fetched its data over XHR — take the payload | often the cleanest |
| `eval "JSON.stringify(...)"` | Attributes a record view doesn't carry | varies |
| `inspect --filter "button,link"` | Finding what to act on, not reading content | ~1,735 |
| `inspect` | Full tree with uids | ~5,650 |

`extract --scroll` scrolls and waits on a `MutationObserver` for lazy content; `extract --a11y`
works on React SPAs (X.com) where the DOM carries no stable structure.

## Token budget

- Prefer `inspect` over `screenshot`: a screenshot gives you no uids, so you pay for the image
  and then inspect anyway.
- `--filter "button,link"` (~10-30 tokens on a small page) when you only need what to act on.
- `--max-depth 2` on deep pages · `read --truncate 1000` · `text --selector "main" --truncate 500`.
- `--budget N` caps the change report; `--verdict off` removes the post-action read entirely and
  costs you every guarantee at the top of this file.
- `pipe`/`batch` for multi-step flows: one connection, ~10x faster, and uids stay valid.
