---
paths:
  - "src/commands/read.rs"
  - "src/commands/extract.rs"
  - "src/commands/text.rs"
  - "src/commands/eval.rs"
  - "src/commands/network.rs"
  - "src/commands/console.rs"
  - "src/commands/wait.rs"
  - "vendor/extract.js"
  - "tests/js/**"
  - "tests/extract_tests.rs"
  - "tests/network_body_tests.rs"
  - "tests/js_suite_tests.rs"
  - "tests/console_bounds_tests.rs"
---

# Getting content out of a page: read, extract, text, network, console

- **Reader mode** — `read` injects Mozilla Readability.js for article extraction (~500 tokens vs ~15K).
- **`extract`** — MDR/DEPTA-inspired heuristics: sibling structural similarity, content heterogeneity, text-to-link ratio, semantic class fast-pass, hidden element exclusion, tag-based merge for modifier classes. Covered by a jsdom unit suite (`tests/js/extract.*.test.js`) and a much smaller Rust E2E suite (`tests/extract_tests.rs`). Suite size is guarded mechanically rather than quoted: `tests/js_suite_tests.rs` runs the jsdom suite inside `cargo test` and its `assert!(passed >= 140)` fails the build if the suite shrinks.
- **`extract --a11y`** — reads the accessibility tree ONCE, filtered to `article`/`listitem`/`row`/`treeitem` at once, then partitions that one string with `snapshot_render::apply_role_filter` (pure, no CDP) and returns the first pattern with ≥3 rows. A read per candidate role cost up to four `getFullAXTree` transfers and four `--scroll` loops, and was also wrong: the four reads saw four page states, so which pattern "won" depended on when the page settled. Record text comes from `snapshot_render::name_in`, so an escaped name is decoded rather than cut at its first quote.
- **`extract --scroll`** — scrolls before extracting and uses a `MutationObserver` to wait for lazy-loaded content. Scroll height is `Math.max(body, documentElement)` (YouTube fix). Max 10 iterations.
- **Console capture** — stealth-safe interceptor via `addScriptToEvaluateOnNewDocument`. Bounded in both dimensions, by ONE `__push` helper the three producers share: 200 entries (oldest shifted out) and 2000 characters per message, the overflow reported as `… (+N chars)`. The cap used to live in the `console[level]` wrapper only, so the `error` and `unhandledrejection` listeners below it grew the in-page array without bound — and `console::run` pulls the whole thing back in one `JSON.stringify`. `tests/fixtures/console_flood.html` throws 400 times and logs 50 KB. The same helper try/catches `JSON.stringify`, which throws on a circular object and used to break the page's own `console.log`.
- **`wait network-idle`** — enables the Network domain, tracks in-flight requestIds (`InFlightTracker`), resolves after `--idle-ms` at zero in-flight, bounded by `--timeout`. Opt-in, off the stealth hot path.
- **`network --abort`** — enables the `Fetch` domain with a URL pattern, intercepts `Fetch.requestPaused`, calls `Fetch.failRequest` with `BlockedByClient`.

## Network capture

Retroactive via the Performance API (stealth-safe), or live via the Network domain.

`--body` fetches bodies at `Network.loadingFinished`, not `responseReceived`: at response time
`getResponseBody` answers "No data found" for anything still in flight, which is most responses.

- A `--filter` overrides the textual-MIME allowlist. The allowlist protects an unfiltered `--body` from the page's images and fonts; it does not veto an explicit ask (issue #27's `application/yaml`).
- `base64Encoded` from Chrome means "not on Chrome's own text list", not "binary", so the decoded content decides: valid UTF-8 prints, raw bytes become `body_omitted` with the size and the `download` command.
