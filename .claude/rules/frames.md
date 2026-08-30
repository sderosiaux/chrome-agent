---
paths:
  - "src/commands/frame.rs"
  - "tests/frame_tests.rs"
---

# Binding to an iframe, and what the binding does not reach

Moved out of `CLAUDE.md`'s **Key Design Decisions** — not rewritten and not summarised. The
words are the ones that were there, minus the factual corrections made in the same change (a
path that had stopped resolving, a count that had gone stale). What changed is *when* they
load: this file is pulled in when you read a file its `paths:` block names, and costs nothing
in a session that touches none of them.

- **`frame`** — resolves the iframe via `document.querySelector` → `DOM.describeNode` (owner `frameId`, so it targets the *specific* iframe matched, not just the first child frame), then `Page.createIsolatedWorld` for its execution context. The `(frameId, contextId)` is stored on the `CdpClient` (`set_frame_context`) so subsequent `eval` (via `contextId`) and `inspect` (via `getFullAXTree` `frameId`) scope to that frame. Cleared on navigation (goto/back/forward/navigate_and_read) since the isolated world dies with it. `frame main` clears the binding. Only `<iframe>`, not `<frame>`/`<frameset>`.
