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
---

# Getting content out of a page: read, extract, text, network, console

Moved out of `CLAUDE.md`'s **Key Design Decisions** — not rewritten and not summarised. The
words are the ones that were there, minus the factual corrections made in the same change (a
path that had stopped resolving, a count that had gone stale). What changed is *when* they
load: this file is pulled in when you read a file its `paths:` block names, and costs nothing
in a session that touches none of them.

- **Reader mode** — `read` injects Mozilla Readability.js for article extraction (~500 tokens vs ~15K)
- **`extract` command** — MDR/DEPTA-inspired heuristics: sibling structural similarity, content heterogeneity, text-to-link ratio, semantic class fast-pass, hidden element exclusion, tag-based merge for modifier classes. Covered by a jsdom unit suite (`tests/js/extract.*.test.js`) and a much smaller Rust E2E suite (`tests/extract_tests.rs`). The size is deliberately not quoted here — the number this line used to carry counted the whole repo's Rust tests as extract's own, and was wrong by a factor of about five the day it was written. It is guarded mechanically instead: `tests/js_suite_tests.rs` runs the jsdom suite inside `cargo test` and its `assert!(passed >= 100)` fails the build if the suite shrinks.
- **Network capture** — retroactive via Performance API (stealth-safe) or live via Network domain. `--body` fetches bodies at `Network.loadingFinished`, not `responseReceived` — at response time `getResponseBody` answers "No data found" for anything still in flight, which is most responses. A `--filter` overrides the textual-MIME allowlist (the allowlist protects an unfiltered `--body` from the page's images and fonts, not vetoes an explicit ask — issue #27's `application/yaml`); and `base64Encoded` from Chrome means "not on Chrome's own text list", not binary, so the decoded content decides: valid UTF-8 prints, raw bytes become `body_omitted` with the size and the `download` command
- **Console capture** — stealth-safe interceptor via addScriptToEvaluateOnNewDocument
- **`extract --scroll`** — scrolls page before extracting, uses `MutationObserver` to wait for lazy-loaded content. Uses `Math.max(body, documentElement)` for scroll height (YouTube fix). Max 10 iterations.
- **`wait network-idle`** — enables Network domain, tracks in-flight requestIds (`InFlightTracker`), resolves after `--idle-ms` at zero in-flight, bounded by `--timeout`. Opt-in, off the stealth hot path.
- **`network --abort`** — enables `Fetch` domain with URL pattern, intercepts `Fetch.requestPaused`, calls `Fetch.failRequest` with `BlockedByClient`
