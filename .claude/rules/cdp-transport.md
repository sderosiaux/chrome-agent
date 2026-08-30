---
paths:
  - "src/cdp/**"
  - "src/setup.rs"
  - "tests/cdp_timeout_tests.rs"
  - "tests/slow_dispatch_tests.rs"
---

# The wire to Chrome: deadlines, dialogs, foreground, stealth

## `--stealth`

7 CDP patches: `navigator.webdriver`, `chrome.runtime`, WebGL, UA, Permissions, the input
screenX/pageX leak, and skipping `Runtime.enable`.

**It clears a Cloudflare JS challenge and nothing else measured.** Three runs each, with and
without the flag:

| Site | Without | With | Reading |
|---|---|---|---|
| `shop.app` | 403, 6-node "Just a moment…" | 200, the real page | the patches satisfy the JS challenge |
| `nowsecure.nl` | 200, 10 nodes | 200, same 10 nodes | a managed Turnstile fingerprints the browser; no in-binary patch moves it |
| `leboncoin.fr` | 403 | 403 | the DataDome line |

The distinction is between two Cloudflare products, not between Cloudflare and someone else. The
docs used to say "Cloudflare/Turnstile", which sends an agent to spend `--stealth` on a Turnstile
and conclude the tool is broken. `--connect` to a real Chrome is the only route for the second
and third cases.

## Foreground for pointer events

`CdpClient::ensure_foreground`.

On a page that is not the active tab, `Input.dispatchMouseEvent` is answered on a fixed timer —
5007, 5004, 5023 ms across runs — while `Runtime.evaluate` on the same connection answers in
0–1 ms. The renderer's main thread is idle; the input pipeline is waiting for something a
backgrounded page never produces. `Page.bringToFront` costs 3 ms and takes the same events to
0–6 ms. This is Chrome's behaviour, not a bug here.

A page becomes hidden without anyone asking: opening a second page backgrounds the first
(`--page other` does exactly that), and Chrome's own `chrome://settings/help` update check did it
to a browser this tool had launched.

So the pointer paths bring their page forward once per connection, and only the pointer paths —
`Input.dispatchKeyEvent` answers in 1 ms on the same hidden page, so `press` and `type` would be
paying a state change for nothing. Consequence: with several pages in one browser, clicking on
one foregrounds it. Best effort — a target that refuses to come forward costs the latency this
exists to remove, not the click.

## An input event has its own deadline

`cdp::client::INPUT_ACK_DEADLINE`, 8 s.

Every CDP call used `--timeout` (30 s default), which is the caller's patience for the PAGE's
work. An input event is not that; Chrome acknowledges one in single-digit milliseconds when the
pipeline is healthy. 8 s clears the 5.00 s background-tab stall above, so a slow-but-delivered
click is never turned into an error, and it undercuts 30 s of silence.

What expires is the ACKNOWLEDGEMENT, never the event. The message says "was dispatched … The
event may already have reached the page", and `hints.rs` answers it under rule 3: do not repeat,
run `inspect` and read the state the action was supposed to produce. The generic "use --timeout N
for slow pages" advice would be wrong twice — the budget is not the caller's to raise, and "try
again with more patience" is the one thing that must not be said about an event the page may have
acted on.

## `waited_ms`

On every mutating response in all three modes, and only when the action actually waited for a
load. A field reading `waited_ms: 0` on every fast action is a field nobody reads on the one
action that took ten seconds.

Taken rather than read (`CdpClient::take_settle_wait_ms`) and cleared at `mark_dispatch`, because
pipe and batch reuse one connection and the wait belongs to the action that paid it.

Text mode prints it at 1 s and above (`render::waited_line`): one second is where a person starts
wondering whether the tool is stuck. `--json` carries it whatever its size.

## Every CDP call has a deadline

`CdpClient::call_timeout`, wired from `--timeout`, default 30 s.

`call` used to await its response channel with nothing behind it. Chrome answers promptly, but an
evaluation sent with `awaitPromise` only answers when the page's promise settles, so
`eval "new Promise(() => {})"` hung the command with no error, no output and no recovery — in
pipe mode for the rest of the session, socket still open and dispatcher still running.

The timed-out request is removed from `pending`: leaving it leaks a slot and would deliver a late
answer to a receiver nobody awaits.

`inspect --limit` was the reachable instance twice over. Its scroll probe re-armed a 400 ms
debounce on every mutation with no ceiling (now `snapshot::settle(400, 2000)`, whose hard timer
nothing clears), and its `limit * 3` bound counts iterations rather than time, so `--limit 500`
on a live page meant 1500 settle windows. The collection is now bounded by `--timeout` too, and
says so in its output rather than looking like a short page.

## `types.rs` declares what something reads, and nothing else

`src/cdp/types.rs`, `serde_proof`.

Serde ignores fields it was not told about, so an undeclared field cannot break a parse and
"deserialization completeness" is not a requirement. Where a field was declared but neither
`Option` nor `#[serde(default)]`, keeping it TIGHTENED what Chrome must send: nine such fields
were unread (`BoxModel` ×4, `ExceptionDetails` ×3, `AXValue::type`, `NavigateResult::frameId`),
so `DOM.getBoxModel` refused a reply omitting `margin` in order to not read it.

The module carries no `dead_code` allow, so a field added tomorrow and never read is a build
failure. Three justifications were audited away:

- **`pinned` was false** — the four `BoxModel` fields were named by one `#[cfg(test)]` struct literal read by no assertion. An initializer is not a dependency.
- **`shape` was documentation written in struct syntax** — prose on the type says the same thing without asking the compiler to carry it.
- **`envelope` was speculative** — a `sessionId` kept against the day this tool drives several sessions per connection.

Two unit tests pin the facts the module doc rests on: an undeclared field cannot break a parse,
and a declared non-`Option` field is a demand made of Chrome.

Cost: `client.rs`'s `an_unreadable_answer_fails_its_call_instead_of_timing_out` failed, because
its fixture was `{"id":7,"sessionId":42}` and with `sessionId` undeclared that message now parses
cleanly. The mechanism (`resolve_unreadable`) was untouched; the test's vector had been removed
out from under it. The class it tests is any field typed more narrowly than the protocol
guarantees, and that class shrinks with every unread field removed. What is left is `id`,
`method`, and `code`/`message` inside an `error`, all read, so none can be deleted away. The
fixture is now a string `error.code`.

## JS dialog auto-handling

`CdpClient::spawn_dialog_handler` runs a background task on every connection (CLI + pipe),
answering `Page.javascriptDialogOpening` via `Page.handleJavaScriptDialog` per `--dialog`
(`accept` default | `dismiss` | `manual`) and `--dialog-text`. The decision is pure, in
`setup::dialog_decision`.

Fire-and-forget request ids live in `[1_000_000_000, i32::MAX]` (wrapping): high enough never to
collide with the sequential request ids, still inside Chromium's accepted signed 32-bit range.
IDs beyond that (the old `1<<40`) are silently ignored by Chromium, so
`Page.handleJavaScriptDialog` never ran and the triggering command hung.
