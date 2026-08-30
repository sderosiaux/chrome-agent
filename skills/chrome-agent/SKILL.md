---
name: chrome-agent
description: Local browser automation with structured, verified outcomes. Use for web navigation, scraping and extraction, form interaction, screenshots and downloads, network or console checks, responsive testing, or page-scoped device emulation.
metadata:
  author: sderosiaux
  version: "0.15.0"
  tags: ["browser", "automation", "scraping", "chrome", "cdp"]
---

# chrome-agent — Browser Automation

```bash
which chrome-agent || npm install -g chrome-agent   # or: cargo install chrome-agent
```

## Reading a response

Every mutating action carries `verdict` + `verdict_reason` + `next`. Every pointer-targeted action
also carries `delivery` (a hit test at the coordinate about to be dispatched) and `intercepted_by`
when another element took the event. Every write also carries `value.actual` beside
`value.requested`, read back after `observed_after_ms` — same field and window for `fill`,
`select`, `check`/`uncheck`.

`ok:true` means the command ran, not that the page complied. Read `verdict`, and read
`value.verbatim` after any write, before reporting success. **Branch on `next`, not on the verdict
word.** `next` is one token from a closed set of six: `proceed` `inspect` `retry` `confirm`
`dismiss` `stop`.

| `verdict` | `verdict_reason` | `next` | What to do |
|---|---|---|---|
| `changed` | `tree_delta` | proceed | The page moved; `delta` says how. |
| `changed` | `nodes_moved` | proceed | Same nodes, reordered (a drag landed). |
| `changed` | `focus_only` | proceed | Only focus moved, onto a real element — on a path with no hit test that is the sole sign the action arrived. `focus.to` is often a focusable **ancestor** of what you clicked; read `uid` for the element you aimed at. Focus onto the **document** answers `identical_tree` instead. |
| `changed` | `value_kept` | proceed / **inspect** | The element held it when read back, and no delta could show that (a secret renders as a fixed marker; a first action has no tree to compare). `fill`, `select` and `check`/`uncheck` all reach this row; the evidence is `value.verbatim`. The one row whose `next` depends on more than the reason: if the PAGE read also failed it answers `inspect` — the field is confirmed, the page around it was never seen. |
| `changed` | `values_lost` | **confirm** | It moved AND emptied a field that held a value; `values_lost:[{uid,role,name,was}]` names each. Submitted-and-cleared and discarded-without-sending look identical. Confirm with `assert text --contains` or `network` before re-filling — a re-submit may send the work twice. |
| `navigated` | `document_replaced` | inspect | New document. Every stored uid is dead. |
| `intercepted` | `hit_test_receiver` | dismiss | Another element occupied the aim point and got the event; `intercepted_by` names it (`tag`/`id`/`class`/`uid`/`z_index`/`text`/`modal`). Nothing is known about the target. This is the row you will actually get: 8 interceptions over 61 real sites, all of them this one. Read `tag`/`id`/`class`, not `z_index` — it was `auto` on 7 of the 8. |
| `intercepted` | `modal_dialog` | dismiss | A `<dialog>` opened with `showModal()` (or a fullscreen element) holds the top layer and receives everything outside itself. Press Escape first. Rare — 0 over 61 real sites: `modal` is `el.matches(':modal')`, so the `<div role="dialog">` overlays most sites ship arrive as `hit_test_receiver` instead. |
| `not_kept` | `value_reverted` | **stop** | The write reached the element and it held **nothing** afterwards. Read `value.actual`. Do not fill again — the same write produces the same answer. |
| `not_kept` | `value_rewritten` | **stop** | It holds something **else** — a mask, a trimmer, a normaliser. Read `value.actual` and decide whether that is the value you wanted. |
| `no_effect` | `delivered_no_change` | confirm | Delivery **proven** to the target, and the tree stayed still within `observed_after_ms`. The strongest word available. Repeating is the one thing that cannot help. |
| `unchanged` | `identical_tree` | confirm | The tree was identical while the tool watched. Delivery was **not** proven. |
| `unknown` | `no_baseline` | inspect | First action on this page; nothing to compare against. |
| `unknown` | `read_failed` | inspect | The action ran; reading the page afterwards failed. |
| `unknown` | `identity_unreadable` | inspect | The two trees may not belong to the same document. |
| `unknown` | `aim_point_off_target` | inspect | Two readings **agreed** and the point still could not be aimed at — **nothing was dispatched**, the miss is **stable**, an identical retry misses identically. `aim` tells the two shapes apart: on screen but outside the element's own boxes (wrapped inline box, clipped container) → aim at a child with a box; outside the viewport (a fixed wall, a locked scroll) → change the page's state, no scroll will move it. |
| `unknown` | `scroll_not_settled` | **retry** | Two readings of the aim point **disagreed**: it was still moving. **Nothing was dispatched**, the miss is **transient**, and the retry is the fix. `wait`, then repeat. |
| `not_checked` | `reporting_disabled` | proceed | You passed `--verdict off`. The silence is yours, not the page's. |

`delivery` rides on pointer-targeted actions (`click`, `dblclick`, the `check`/`uncheck` click) and
is what licenses the two strong words:

| `delivery` | Means | Licence |
|---|---|---|
| `target_hit` | The aim point resolved to the target, a descendant, its label's control, or its shadow host. | The only value that permits `no_effect`. |
| `intercepted` | The aim point belongs to an element outside the target's flat subtree. | → `verdict: intercepted`; `intercepted_by` names the receiver. |
| `off_target` | No point could be aimed at and two probes agreed: outside its own client rects, or outside the viewport and pinned there. | Nothing dispatched, so a repeat doubles nothing — but the miss is stable, so it cannot succeed either. Hence `inspect`. |
| `not_settled` | Two probes disagreed: the aim point was still moving when the settle budget ran out. | Nothing dispatched, miss is transient, so repeating is safe and is the fix. Hence `retry`. |
| `js` | Went through a JS `click()`/`MouseEvent`, which performs no hit test. | Interception is inapplicable, not undetected. Licenses nothing. |
| `not_probed` | No hit test ran, or its answer does not cover the document that matters (a target inside an iframe). | Absence of evidence. Licenses no conclusion. |

`--on-intercept` has three modes. `dispatch` (default) sends through any receiver, which is what a
pointer does. `refuse` errors on every interception. `guard` sends through an inert receiver and
refuses one that looks like a control, reading `intercepted_by.actionable` from the same probe
call: a native interactive tag, an ARIA interactive role, `tabIndex >= 0`, or `cursor: pointer`.
An `<iframe>` receiver always refuses under `guard` — its content is opaque, so "inert" could only
be assumed. Any refusal is `ok:false` (exit 1) with the same fields as a dispatch plus
`dispatched:false`, which appears on every response that aimed and sent nothing; `next` says
`dismiss`, and re-aiming duplicates nothing.

**Never report a success the tool did not confirm.** `unchanged` is also what a click swallowed by
an overlay looks like: confirm with `assert` before repeating, because a repeat is a second real
click. `unknown` means the tool could not compare, never "nothing happened" — run `inspect` once
and continue from what you see; the one exception is `scroll_not_settled`, where nothing was
dispatched. `waited_ms` appears only when the action waited for a page load after it. A pointer
event Chrome does not acknowledge within 8 s fails with `ok:false` rather than waiting out
`--timeout`; it may already have reached the page, so run `inspect` instead of repeating. `click`,
`dblclick`, `hover` and `drag` bring their page to the foreground first (a background tab answers
pointer events on a 5 s timer).

**Two blind spots, neither fixable by retrying.** The observation window is fixed at 60 ms
(`observed_after_ms`): it catches a revert on the microtask queue, in `setTimeout(0)` or in an
animation frame, not a validator that clears the field at 400 ms — for that, `wait` then
`assert value`. Canvas, WebGL and CSS-only effects are invisible to the accessibility tree: a
class, an opacity, a transform, a repaint all look like `unchanged` or `no_effect`, so confirm
with `screenshot` or `eval`, never with a second click.

## Workflow: inspect → act → assert

```bash
chrome-agent goto https://app.com/login --inspect   # uids
chrome-agent fill --uid n52 "user@test.com"         # value: what the page kept
chrome-agent click n63                              # verdict + delivery + next
chrome-agent assert text --contains "Dashboard"     # exit 0 = evidence you may quote
```

Re-inspect when `verdict` is `navigated`, when `next` says `inspect`, and after any SPA route
change (`back`, `forward`, a click that re-renders) — uids are reassigned. For an SPA detail page
prefer `goto <direct-url>` over `click`, which may open a modal.

## `assert`: the exit code is the answer

Prove the end state before reporting a task complete. Exit 0 is the only evidence you may quote.

```bash
chrome-agent assert value --selector "#email" --equals "ada@example.com"
chrome-agent assert text --selector "#status" --contains "Shipped"
chrome-agent assert url --matches "/orders/[0-9]+$"
chrome-agent assert state --selector "#terms" --checked        # native OR aria-checked
chrome-agent assert state --uid n15 --selected "California"    # a dropdown's current option
chrome-agent assert state --selector "#submit" --disabled      # :disabled OR aria-disabled
chrome-agent assert state --selector ".modal" --visible        # rendered, opaque, not hidden
chrome-agent assert exists --selector ".result" --count 10     # --min N, or --count 0 for absence
```

- **0** it held · **2** the page is not in that state (report or repair) · **1** nothing was
  compared: no browser, a selector matching nothing, an unparseable regex, a CDP timeout (retry).
  The only other thing that exits 2 is a `macro run` guard that was checked and did not hold — the
  same kind of claim. A bad flag exits 1. Inside `pipe`/`batch` an assertion has no exit
  code of its own: it is `ok:false` with the same `assertion` object, and a `batch` that stopped
  on it exits 1, not 2.
- `assert` is a read: no change report, no verdict, and it never clicks. Secrets are compared but
  never printed (the response gives both lengths). `--checked` reads the same classification
  `check`/`uncheck` apply and `--selected` the same reading `select` uses, so an assertion cannot
  disagree with the action.
- `--matches` is a Rust regex: `\d` `\w` `\s` are ASCII-only, no `\p{...}`, no lookaround; `(?i)`
  for case-insensitive. `--contains` is a plain case-sensitive substring.

**Targeting** has three modes: **uid** (from `inspect`, e.g. `n47`, stable across inspects of the
same page), **`--selector`** (CSS), **`--xy`** (coordinates). Every targeted action returns the
`uid` it actually resolved, even when you aimed with `--selector` — cross-check it against `delta`.
`click --selector` is the same verb as `click <uid>`: both resolve the viewport centre and dispatch
native input, so a `--selector` on a button behind a cookie banner clicks the banner. That is what
a pointer does, and what `intercepted_by` tells you.

## Commands

```bash
# Navigation
chrome-agent goto <url> [--inspect] [--wait-for "selector"] [--header "K: V"]
chrome-agent back [--inspect]                                # answers {url, title}
chrome-agent forward [--inspect]                             # at either end: a message, no url
chrome-agent scroll down|up|<uid>
chrome-agent wait network-idle [--idle-ms N] [--timeout N]   # deterministic SPA/XHR settle
chrome-agent wait text|url|selector "pattern" [--timeout N]
chrome-agent history [--filter "pattern"] [--limit N]
# Inspection — narrowing flags change what is PRINTED only; `diff` always compares the full tree
chrome-agent inspect [--max-depth N] [--filter "button,link"] [--uid nN] [--urls]
chrome-agent inspect --filter "article" --scroll --limit 50   # infinite-scroll feeds
chrome-agent inspect --max-chars 4000 [--offset 4000]         # cap/page huge trees
chrome-agent diff                                             # what changed since last inspect
# Pointer actions — uid, --selector or --xy
chrome-agent click <uid> [--inspect]
chrome-agent click --xy 100,200                           # or --selector "css"
chrome-agent dblclick <uid> [--inspect]
chrome-agent hover <uid>
chrome-agent drag <from-uid> <to-uid>                     # mouse events; NOT HTML5 DnD
# Writes
chrome-agent fill --uid <uid> <value> [--secret]           # or --selector "css"
chrome-agent fill-form n20="a@b.com" n30="password"       # one value report per field
chrome-agent type "text" [--selector "input.search"] [--secret]
# --secret reports lengths instead of the value. It only ADDS to what the element declares;
# there is no flag that turns redaction off.
chrome-agent press Enter|Tab|Escape                       # a single printable char types it
chrome-agent select --uid <uid> "Option text"             # option.value first, then visible text
chrome-agent check <uid> | chrome-agent uncheck <uid>     # idempotent; refuse what they cannot check
chrome-agent upload --uid <uid> /path/to/file.pdf         # --selector too (file inputs hide from a11y)
# Reads
chrome-agent read [--truncate N]                          # articles, via Readability
chrome-agent text [--selector "main"] [--truncate N]      # visible text
chrome-agent extract [--selector "css"] [--limit N] [--scroll] [--a11y]
chrome-agent eval "JSON.stringify(...)" [--selector "h1"]
chrome-agent network [--filter "pattern"] [--body] [--limit N]   # already loaded, stealth-safe
chrome-agent network --live 5 --body --filter "graphql"          # capture live traffic
chrome-agent network --abort "*tracking*" --live 30              # blocking; start before navigating
chrome-agent console [--level error] [--clear]
# --body with --filter fetches every match whatever its MIME; without --filter, textual types only
# Files — written 0600 under ~/.chrome-agent/tmp; path on stdout, never base64
chrome-agent screenshot [--format jpeg] [--quality N] [--max-width N] [--uid nN|--selector "css"]
chrome-agent pdf [--filename name] [--landscape] [--background]
chrome-agent download <url> [--out path] [--max-bytes N]        # in-page fetch, keeps the login
chrome-agent download <--uid nN|--selector "css"> [--out path]  # CLICK it, capture the download
# Device metrics — explicit values, scoped to one named page, no preset catalog
chrome-agent --page mobile emulate device --label "checkout phone" --width 412 --height 915 --dpr 2.625 --mobile --touch
chrome-agent --page mobile emulate status
chrome-agent --page mobile emulate reset
# Persisted per named page, reapplied on each connection; a Chrome restart discards it. Acting on
# an emulated page activates its tab and backgrounds siblings. Under --touch, click/check tap.
# WebMCP tools — document.modelContext.getTools()/.executeTool()
chrome-agent webmcp list                                          # name, description, inputSchema
chrome-agent inspect                                              # baseline first, like any action
chrome-agent webmcp call add_to_cart --args '{"item":"Espresso Blend"}'
# The protocol defines no outputSchema, so the accessibility-tree diff is the only corroboration
# for what a tool DECLARED: one that declares success and moves nothing reads
# unchanged/identical_tree. Under `frame` the response carries frame_scoped:true — an empty list
# there is unproven, not "none". Most installs lack native WebMCP: use --chrome-arg above.
# Multi-step — pipe keeps one connection, so uids stay valid across commands
echo '[{"cmd":"goto","url":"..."},{"cmd":"inspect"},{"cmd":"click","uid":"n12"}]' | chrome-agent --json batch
echo '{"cmd":"goto","url":"...","inspect":true}' | chrome-agent pipe
# Iframes — the frame switch lives on the connection, so use pipe/batch
printf '%s\n' \
  '{"cmd":"frame","target":"iframe[src*=\"checkout\"]"}' \
  '{"cmd":"inspect"}' \
  '{"cmd":"fill","uid":"n42","value":"..."}' \
  '{"cmd":"frame","target":"main"}' | chrome-agent pipe
# Separate CLI calls (frame then inspect) do NOT work — each is a fresh connection.
# frame scopes eval + inspect, NOT --selector targeting. Act by uid after switching.
# Session, tabs, and paths that already worked
chrome-agent status                                              # browsers, pids, orphan= lines
chrome-agent tabs
chrome-agent macro list                                          # named paths, with their guards
chrome-agent macro run checkout --var email=ada@example.com      # stops at the first failed guard
chrome-agent replay recording.jsonl                              # a pipe --record file, no guards
chrome-agent close [--purge] [--orphans]                         # --orphans: browsers no session claims
```

**Macros.** `macro record <name> --from-recording <file>` (or
`{"cmd":"macro","action":"record","name":"x"}` inside the pipe session that just did it) distils
the steps that changed the page and keeps only what survives tomorrow: `delivery: target_hit`, the
verdict **word**, `value.verbatim`, a `url_matches` on the path — never counters, uids or
durations. A step aimed by uid is recorded by role + accessible name, or refused. A secret field
becomes a declared parameter and is never stored, so `macro run` refuses without `--var`. A guard
that does not hold **stops** the run, naming the step index, the guard, what was observed and the
action's own `next`, and exits **2** with `stopped_by: "guard"` — the same code as a failed
assertion, because it is the same kind of claim. A run that stopped for any other reason (the step
failed, the page could not be read, no such macro) exits **1** with `stopped_by: "error"`. Steps
that could promise nothing are marked `unguarded` and counted in both reports — read that number
before trusting a green run.

## Global flags

Accepted on **either side of the verb**. Two exceptions: `--timeout` and `--max-depth`. A command
that declares its own takes it after the verb (`wait selector ".x" --timeout 5`); everywhere else
they must come before it. Passing one where it is not declared is an exit-1 usage error naming
the invocation that works.

```bash
--stealth          # 7 anti-detection CDP patches
--json             # structured JSON on stdout; errors exit 1 with {"ok":false,...} still on stdout
--browser <name>   # isolate parallel agents — sharing "default" corrupts sessions
--page <name>      # named tabs
--max-depth N      # limit inspect tree depth
--verdict MODE     # auto (default): report what changed. off: report the action only
--budget N         # cap the change report, in chars (default 1200; 0 = uncapped)
--on-intercept M   # dispatch (default) | guard | refuse
--timeout N        # deadline on every CDP call (default 30s)
--headed           # show the window
--connect URL      # attach to a real Chrome (DataDome/Kasada)
--proxy-server URL # http(s)/socks4/5://host:port; launch-only
--copy-cookies     # use cookies from your real Chrome profile
--chrome-arg FLAG  # extra flag for the launched Chrome (repeatable); launch-only
--dialog MODE      # accept (default) | dismiss | manual · --dialog-text for prompt()
```

`--chrome-arg` applies only when chrome-agent launches Chrome (refused, not ignored, under
`--connect`) and is fixed for the life of a named browser — close or purge it to change them.
Refused outright: `--user-data-dir`, `--remote-debugging-port`, `--remote-debugging-pipe`,
`--proxy-server` (use the global one), `--headless` (use `--headed`).

Exit codes: **0** success · **1** error (including a bad flag, and a `batch --stop-on-error` that
stopped) · **2** a claim this tool made did not hold — an assertion, or a `macro run` guard ·
**130** Ctrl+C. JS dialogs auto-accept so the page never hangs.

### Bot protection

| Site protection | Solution | Measured |
|---|---|---|
| None | `chrome-agent goto ...` | — |
| Cloudflare **JS challenge** ("Just a moment…") | `chrome-agent --stealth goto ...` | `shop.app`: 403 / 6 nodes without, 200 / real page with |
| Cloudflare **managed Turnstile** | `--stealth` does **not** get through | `nowsecure.nl`: 10 nodes with and without |
| Logged-in sites (X, Gmail) | `chrome-agent --stealth --copy-cookies goto ...` | — |
| DataDome/Kasada | `google-chrome --remote-debugging-port=9222 &` then `chrome-agent --connect http://127.0.0.1:9222 goto ...` | `leboncoin.fr`: 403 with `--stealth` and without |

The two Cloudflare rows are different products: "Just a moment…" is a JS challenge the seven
patches satisfy, while a managed Turnstile fingerprints the browser and no in-binary patch moves
it. There, and for DataDome, `--connect` to a real Chrome is the only route. Read the page after
navigating rather than assuming the flag worked.

## Response shapes (`--json`)

```
Success: {"ok":true, ...}
Error:   {"ok":false, "error":"message", "hint":"what to do next"}
```

- `goto` → `{"url","title","landed":{"requested","final","redirected","http_status","serving","challenge_from"?}}`.
  Check `redirected`: an expired session bouncing to a login wall otherwise reads as a successful
  load. Fragment-only and trailing-slash differences are not redirects. `http_status` is the final
  hop's, absent (never 0) when the browser reports none. A redirect onto `/login`, `/signin`,
  `/auth`, `/sso` adds a `hint` guessed from the URL. `goto` carries no verdict.
- **`serving` is what answered, and it is the field to branch on.**

  | `serving` | means | do |
  |---|---|---|
  | `page` | nothing measured contradicts the load | proceed |
  | `challenge` | an anti-bot vendor's frame or script is here and the site has no form of its own; `challenge_from` names the vendor host | `--connect` to a real Chrome — `--stealth` does not defeat these |
  | `error` | the server answered 4xx/5xx; read `http_status` | fix the URL (404), authenticate (401/403), or wait (429/5xx) |
  | `nothing_actionable` | no link, no form control, no script, almost no text | `inspect` before giving up |
  | `unreadable` | the shape probe did not run | `inspect` |

  `page` is the absence of evidence, not a certificate: a paywall, a cookie wall and an unknown
  captcha vendor all read as `page`. `nothing_actionable` is a measurement, not a claim you were
  blocked — an edge-served refusal and a page that had not finished rendering look identical.
- `click`/`fill`/`select`/`check`/… → `{"message", "uid", "verdict", "verdict_reason", "next",
  "verdict_hint"?, "changed":{"added","removed","changed","unchanged","moved","anonymous",
  "document_changed","identity_known"}, "delta":"+ uid=n88 heading \"Saved\""}`, plus
  `"focus":{"from","to"}` when focus moved (deliberately not counted as a content change).
- `fill` also → `"value":{"requested","actual","verbatim","observed_after_ms","caveat"?}`. A secret
  field (`type=password`, or `autocomplete` naming a password/card/CVC/one-time code) reports
  `{"redacted":true,"requested_length","actual_length","verbatim"}` instead. `caveat` when the
  write exceeded `maxlength`. `fill-form`/`fill_and_submit` → `"values":[...]`, one per field,
  redacted the same way; on `fill_and_submit` it is the only witness, since the change report runs
  after the form has moved on.
- `select` → `"value"` with the option TEXT; `check`/`uncheck` → `"value"` with
  `checked`/`unchecked` (`indeterminate` for a mixed native box). Both put `observed_after_ms` at
  the top level and both REFUSE (non-zero exit) when the page reverts inside the window. `value` is
  **absent** when the element already held the state — nothing was dispatched, so that response
  answers `no_baseline`/`identical_tree` and is no evidence this action changed anything.
- pointer-targeted actions → `"delivery"`, `"aim":[x,y]`, `"intercepted_by":{...}` when intercepted.
- `inspect --max-chars` → `{"total_chars","truncated","next_offset"}` · `read`/`text` → `{"text"}` ·
  `eval` → `{"result"}` · `network` → `{"requests":[...]}` · `console` → `{"messages":[...]}` ·
  `batch` → `{"results":[...]}` · `screenshot`/`pdf` → `{"path"}` ·
  `download <url>` → `{"via":"fetch","downloaded","path","bytes","mime"}`.
- `download --uid/--selector` clicks and captures the browser-native download — the only route to a
  file the page builds itself (`Blob`) or serves from a POST no anchor names. **Branch on
  `downloaded`, never on `ok`**: a click that landed and produced no file answers `ok:true` with
  `downloaded:false` and a hint forbidding the retry. `--timeout` bounds the whole window (begin
  AND finish); `--max-bytes` cancels past it and removes the partial file. Exactly one target — a
  URL, `--uid` or `--selector`; none or two is a usage error on stderr, exit 1, before a browser
  is resolved.

## Cheap reads: choose the right one

Reach for `extract` first on anything that looks like a list. Figures measured on the Hacker News
front page; they scale with the page, so treat them as relative.

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

Budget: prefer `inspect` over `screenshot` (a screenshot gives no uids, so you inspect anyway);
narrow with `--filter "button,link"`, `--max-depth 2`, `read --truncate 1000`,
`text --selector "main" --truncate 500`; `--budget N` caps the change report and `--verdict off`
removes the post-action read entirely, and every guarantee at the top of this file with it; use
`pipe`/`batch` for multi-step flows — one connection, uids stay valid, and about 12 ms of
per-command overhead goes away (measured 1.5x on a read stream, 1.1x on fills and clicks). Reach
for it for the stable uids, not for the speed.
