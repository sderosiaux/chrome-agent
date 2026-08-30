---
paths:
  - "src/commands/frame.rs"
  - "tests/frame_tests.rs"
---

# Binding to an iframe, and what the binding does not reach

`frame` resolves the iframe via `document.querySelector` → `DOM.describeNode` (owner `frameId`,
so it targets the *specific* iframe matched, not just the first child frame), then
`Page.createIsolatedWorld` for its execution context.

The `querySelector` half is the shared prelude `element_selector::bind_element_js`, so a selector
that cannot parse is reported as a selector here in the same words `fill --selector` and
`click --selector` use, and a selector that matches nothing keeps its own separate message. The
thrown text is read by `element::js_exception` like everywhere else: first line, no stack.

The `(frameId, contextId)` is stored on the `CdpClient` (`set_frame_context`), so subsequent
`eval` (via `contextId`) and `inspect` (via `getFullAXTree` `frameId`) scope to that frame.

- Cleared on navigation (`goto`/`back`/`forward`/`navigate_and_read`): the isolated world dies with it.
- `frame main` clears the binding.
- Only `<iframe>`, not `<frame>`/`<frameset>`.
