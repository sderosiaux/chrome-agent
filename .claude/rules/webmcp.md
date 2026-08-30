---
paths:
  - "src/commands/webmcp.rs"
  - "tests/webmcp_tests.rs"
---

# WebMCP: a tool's declared result is not evidence

WebMCP (W3C WICG, `document.modelContext.getTools()`/`.executeTool()`) defines no `outputSchema`:
a tool's declared return is a freeform string with no contract to check it against.

## `webmcp call` is `mutates_page`, not a new verdict vocabulary

Measured against 17 real tools across 6 sites and reproduced on
`tests/fixtures/webmcp_honest_liar_partial.html`, where three tools return the SAME string
byte-for-byte: one adds a real cart line, one does nothing, one moves a counter with no backing
state.

`webmcp call` reports the tool's own `declared_result` next to the accessibility-tree delta the
shared hook already attaches to every mutating command. The three become distinguishable by
DEGREE — `added=3 removed=2 changed=1` for the honest tool, `changed=1` alone for the partial
one, "No changes detected" for the one that touched nothing — without a single new verdict reason.

That restraint is deliberate. The honest answer for the liar's call is
`unchanged / identical_tree`, the same rung every other command reaches when nothing moved, and
its gloss already says "not the same as the action having no effect" — a canvas repaint, a
CSS-only change and a handler firing after the window all look identical to this measurement.
Inventing a stronger word ("tool lied") would be a claim the accessibility tree cannot support;
`no_effect` needs delivery PROVEN, and a tool call has no hit test to prove it with.

`webmcp list` stays out of `mutates_page` — it is a read, like `assert` — and reports
`output_schema: null` per tool, not as a footnote: an agent needs that before it trusts a declared
result.

## Argument handling closes two spec traps

`executeTool` requires the actual `RegisteredTool` object (a bare name is
`TypeError: The provided value is not of type 'RegisteredTool'.`) and a JSON *string* second
argument (an object is "Failed to parse input arguments").

`webmcp call NAME` resolves the name against a fresh `getTools()` call and always hands
`executeTool` a string it validated first, so neither native error is reachable through this path.
Both still have a `hints.rs` branch for the one way in that bypasses it: raw `eval`.

## Under a `frame` binding

The isolated world `eval` already has — it shares the DOM, not main-world JS — applies here too,
and was MEASURED, not assumed. Bound into `webmcp_iframe_host.html`'s iframe, whose own
main-world script installs the fixture's polyfill on ITS `document.modelContext`,
`typeof document.modelContext` reads `"undefined"` even though `document.title` correctly reads
the iframe's own title.

`list_tools`/`call_tool` report `frame_scoped: true` whenever this happens, and the hint says
explicitly that absence is unproven, not "this frame has none". For a NATIVE (non-polyfilled)
`document.modelContext` the measurement supports neither claim; that case was not tested.
