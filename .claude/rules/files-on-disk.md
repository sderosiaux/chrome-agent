---
paths:
  - "src/commands/download.rs"
  - "src/commands/download_click.rs"
  - "src/commands/screenshot.rs"
  - "src/commands/pdf.rs"
  - "src/geometry.rs"
  - "src/base64.rs"
  - "tests/download_click_tests.rs"
  - "tests/download_limit_tests.rs"
---

# Every file this tool writes, and how it gets there

Every file is written 0600.

- **`screenshot`** — `--format jpeg`/`--quality`, `--max-width` (downscale via CDP `clip.scale`, no image crate), `--uid`/`--selector` clip via `DOM.getBoxModel` (`geometry::clip_for_*`). Never emits base64 on stdout.
- **`pdf`** — `Page.printToPDF` (`transferMode: ReturnAsBase64`) → `base64::decode` → file. Mirrors screenshot.
- **`download <url>`** — in-page `fetch(url,{credentials:'include'})` → base64 in page → `base64::decode` → file. Auth-preserving. Filename from Content-Disposition (including RFC 5987 `filename*`), then the URL.

## `download --uid` / `--selector`: the download a click produces

`src/commands/download_click.rs`, `commands::download::Target`.

`download <url>` only reaches a file with an address, which leaves out bytes made client-side
(`URL.createObjectURL`, where `inspect --urls` returns `blob:null/…`) or served by a POST no
anchor names. The click path runs the click that already exists — `element::click` /
`click_selector`, same hit test, same `--on-intercept`, same refusal messages — and arms
`Browser.setDownloadBehavior {behavior:"allowAndName", eventsEnabled:true}` around it.

Three facts were measured before the shape was chosen:

1. `Browser.*` is accepted on the PAGE websocket this tool already holds, and `Browser.downloadWillBegin`/`downloadProgress` are delivered on it. No second browser-level connection is needed.
2. The override does NOT outlive the CDP session that set it — a fresh connection clicking the same link with nothing armed produced no file. Same rule `emulation.rs` documents for `Emulation.*`. The arming must happen on the connection that clicks, which is why this is a flag on `download` and not a `wait --download` verb: a separate verb works in pipe mode and silently captures nothing from the CLI, where each invocation opens its own connection.
3. `Browser.cancelDownload` is implemented, so `--max-bytes` keeps one meaning across both halves of the verb.

### Read `downloaded`, not `ok`

A click that was delivered is never an error, whatever it failed to produce. The only recovery an
error invites is a second click, and the page cannot tell that from a second deliberate action —
on an export or a purchase that is two of them. A click that landed and produced no file answers
`ok:true`, `downloaded:false`, the window it waited (`observed_after_ms`) and a hint that forbids
the retry in words, exactly as `fill` answers `ok:true` beside `verbatim:false`.

The one branch that permits a retry is the one where NOTHING was dispatched
(`--on-intercept refuse`, an aim point that never settled). Its hint says so.

### No verdict

`download` stays out of `mutates_page`, for `goto`'s reason: `downloaded`/`path` are
self-describing, and a `no_effect` verdict on a command whose purpose is to produce a file would
be worse than nothing, since a click that downloads usually leaves the accessibility tree
identical — a `delivered_no_change` reported beside a 48 KB file on disk.

The click's own evidence (`delivery`, `aim`, `uid`, `intercepted_by`) still rides on the
response. `verdict_hint` is the one field stripped, because it names a vocabulary this response
does not carry.

### Four outcomes, four recoveries

Completed (the file, moved to `--out` or to the server's suggested name); never began; cancelled
(by `--max-bytes`, or by Chrome); still running when the window closed. The last two write
nothing, because a prefix of a file is not the file.

Chrome writes into a private `~/.chrome-agent/tmp/.incoming-<pid>-<nanos>/`, so `allowAndName`'s
guid-named files cannot collide with a concurrent agent's. The cleanup RETRIES (5 × 30 ms):
measured on the cancel path, Chrome answers `canceled`, we return, and it then finalises,
recreating the directory and a zero-byte stub behind us. Same lesson as `close --purge`.

One `--timeout` bounds both questions — did anything begin, did it finish — because the scale of
the first is not knowable in advance: a blob begins in ~100 ms, an attachment begins when the
server's response headers arrive, and a fixed short window would report "nothing began" for every
slow server.

## The deferred sweep

`download_click::collect_abandoned`.

The 5 × 30 ms retry is not enough on the path a transfer is still running when `--timeout`
expires. Measured with the shipped budget over eight downloads in a row: three directories were
back on disk the moment the last invocation returned, and **all eight** were there fifteen
seconds later, each holding the zero-byte stub `allowAndName` names after the guid.

This is not a slow finalisation to wait out. **Chrome keeps the download after chrome-agent is
gone**, so the only window wide enough is the length of the transfer, which is exactly the bound
`--timeout` declined to be. `disarm` does not help: it is already called before every `clean_up`,
and returning the default behaviour to Chrome says nothing about a transfer that has already
chosen its path. Widening the budget only moves the failure onto a slower runner.

So the tool waits for a fact instead. The directory is named after the process that armed it,
only Chrome-on-behalf-of-that-invocation writes there, and the override dies with that CDP
session — so a directory whose pid the OS no longer knows cannot gain another byte, and any later
invocation may take it. `arm()` collects them before creating its own, through
`session::liveness`, the same probe `profiles.rs` and the store prune read. Verified: the
eight-of-eight scenario drains to zero on the next `download`.

**Every unresolved case keeps the directory** — `Unknown` (a pid under another uid; and every
non-Unix platform, where no probe is wired and this is a no-op), a name whose pid does not parse,
and anything in `~/.chrome-agent/tmp` that is not a transfer directory, which matters because
`screenshot`, `pdf` and `download <url>` put unnamed output in that same directory. A concurrent
agent is safe by the predicate: its directory carries ITS pid, which reads `Alive`. A recycled
pid reads `Alive` too and keeps a directory that could have gone — the harmless direction.

**Arming is the collection point**, because it is the only moment anyone has reason to care.
Measured cost: 227 µs with nothing to collect, 241 µs with 500 unrelated files beside it (the
prefix filter answers before any pid is probed), 6.1 ms to drain a full 64-entry window. The save
path where `profiles.rs` sweeps was rejected: it runs on every command, including read-only ones,
for a directory exactly one verb creates.

Stated rather than hidden: a caller who abandons a download and never runs another keeps the
crumbs, bounded at one partial file each. The 5 × 30 ms sweep stays as the fast path, not the
guarantee — a completed download clears on the first attempt and never reaches the collector.

### What the test asserts

It runs an arming invocation and asserts nothing survives it: no partial file on the caller's own
`--out` path, and nothing accumulating between invocations. The race itself is deliberately NOT
reproduced — eight out of eight on a slow transfer, zero out of fifteen on a blob that finalises
in milliseconds, so a test built on it is green on one machine and red on another. The state the
race leaves behind is planted instead, and one test pins that arming is what collects it;
verified by neutering the predicate, where seven of eight tests still passed and that one did not.

The suite's `ours()` filters on the pid *and* the `TestBrowser` name. Pid alone is process-global,
so three tests once failed at once on directories a SIBLING THREAD's slow transfer was still
being written into.
