# aibrowsr — LLM-Friendly CLI Improvement Plan

## Audit Against Best Practices

Sources: chrome-devtools-mcp design principles, dev-browser v1 LLM guide,
MCP tool design patterns, agentic context engineering.

### What we do well

| Principle | Status |
|---|---|
| Token-optimized output (snap vs raw HTML) | Done |
| Small deterministic blocks (one command = one action) | Done |
| Reference over value (screenshot → file path) | Done |
| Self-documenting (`--help` includes LLM guide) | Done |
| Progressive complexity (simple by default, `--verbose`/`--snap` for more) | Done |
| Persistent sessions (login once, stay logged in) | Done |

### What's missing or weak

| # | Gap | Impact | Source |
|---|---|---|---|
| 1 | **Structured JSON output mode** — agents parse text, but JSON is safer | High | MCP patterns, chrome-devtools-mcp |
| 2 | **`goto --snap`** doesn't auto-snap — must call snap separately after goto | High | Round-trip waste |
| 3 | **No `--page` flag** — all commands operate on "default" page only | Medium | Multi-tab workflows blocked |
| 4 | **Error output not structured** — `error: ...` text, not JSON | Medium | Self-healing errors need parseable format |
| 5 | **No `wait` command** — can't wait for text/selector/URL to appear | Medium | Dynamic SPAs need explicit waits |
| 6 | **Snap is noisy** — StaticText/InlineTextBox clutter the output for agents | High | Token waste — agents don't need inline text boxes |
| 7 | **No `--output json` on snap** — only text format, no machine-parseable tree | Medium | Some agents prefer JSON |
| 8 | **`goto` doesn't return snap** — agent must always do 2 calls (goto + snap) | High | v1's biggest workflow friction |
| 9 | **No scroll command** — can't interact with elements below the fold | Medium | Real pages need scrolling |
| 10 | **No type/press commands exposed** — element.rs has them, CLI doesn't | Low | Missing for form completion |
| 11 | **`--help` LLM guide too long** — 78 lines of text, burns tokens if agent reads it | Medium | Should be concise |
| 12 | **No `--quiet` mode** — suppresses non-essential output (warnings) | Low | Agent doesn't need cargo warnings |

---

## Improvement Plan

### P1: Reduce snap noise (filter InlineTextBox/StaticText) [High, token impact]

**Problem:** A simple page like example.com produces 12 nodes, but only 5 are meaningful. InlineTextBox and StaticText are child nodes that repeat the parent's name — pure noise for agents.

**Current:**
```
uid=e2 heading "Example Domain" level=1
  uid=e3 StaticText "Example Domain"
    uid=e4 InlineTextBox "Example Domain"
```

**After:**
```
uid=e2 heading "Example Domain" level=1
```

**Fix:** Filter `StaticText` and `InlineTextBox` roles by default. Keep with `--verbose`.

**Token impact:** ~60% reduction on typical pages. Conduktor.io pricing page goes from ~200 nodes to ~80.

### P2: `goto --snap` auto-snaps [High, round-trip impact]

**Problem:** Every navigation requires 2 commands: `goto` then `snap`. The `--snap` flag exists on `click`/`fill` but not on `goto`.

**Fix:** Add `--snap` to `goto`. When set, automatically take snapshot after navigation and print it. This makes the most common workflow (navigate + discover page) a single call.

**Before:** 2 calls
```bash
aibrowsr goto https://example.com
aibrowsr snap
```

**After:** 1 call
```bash
aibrowsr goto https://example.com --snap
```

### P3: Structured JSON output (`--json` flag) [High, agent parsing]

**Problem:** Agents parse text output with regex/heuristics. Fragile. MCP pattern: return structured data.

**Fix:** Global `--json` flag. When set, all commands return JSON on stdout instead of text.

```bash
aibrowsr --json goto https://example.com
→ {"ok":true,"url":"https://example.com","title":"Example Domain"}

aibrowsr --json snap
→ {"ok":true,"nodes":[{"uid":"e1","role":"heading","name":"Example Domain","level":1,"children":[...]}]}

aibrowsr --json click e4
→ {"ok":true,"message":"Clicked uid=e4"}

aibrowsr --json click e4 --snap
→ {"ok":true,"message":"Clicked uid=e4","snapshot":{...}}

# Errors also structured:
aibrowsr --json click e99
→ {"ok":false,"error":"Element uid=e99 not found.","hint":"Run 'aibrowsr snap' to get fresh uids."}
```

### P4: `--page` flag for multi-tab [Medium]

**Problem:** All commands operate on the "default" page. No way to manage multiple tabs.

**Fix:** Global `--page <name>` flag (default: "default"). Each named page is tracked independently in the session.

```bash
aibrowsr --page login goto https://app.com/login
aibrowsr --page dashboard goto https://app.com/dashboard
aibrowsr --page login snap
aibrowsr --page dashboard snap
```

### P5: Self-healing error format [Medium]

**Problem:** Errors are plain text. Agents can't parse the suggested fix.

**Current:** `error: Element uid=e12 no longer exists. Run 'aibrowsr snap' to get fresh uids.`

**Fix:** Structured errors with `hint` field suggesting the next action:

```
error: Element uid=e12 not found
hint: Run 'aibrowsr snap' to get fresh uids
```

In JSON mode:
```json
{"ok":false,"error":"Element uid=e12 not found","hint":"Run 'aibrowsr snap' to get fresh uids"}
```

### P6: `wait` command [Medium]

**Problem:** SPAs load content asynchronously. No way to wait for specific content.

**Fix:** `aibrowsr wait <text|selector|url> <pattern> [--timeout 10]`

```bash
aibrowsr wait text "Welcome back"
aibrowsr wait url "**/dashboard"
aibrowsr wait selector ".results-loaded"
```

### P7: Expose type/press/scroll commands [Medium]

**Problem:** element.rs implements `type_text`, `press_key`, `hover` but they're not wired to CLI.

**Fix:**
```bash
aibrowsr type "Hello world"              # Type into focused element
aibrowsr press Enter                     # Press key
aibrowsr scroll down                     # Scroll page down
aibrowsr scroll uid e15                  # Scroll element into view
aibrowsr hover e10                       # Hover over element
```

### P8: Compact LLM guide [Medium, token impact]

**Problem:** The LLM guide in `--help` is 78 lines. If an agent reads `--help`, that's ~500 tokens burned just on documentation.

**Fix:** Two tiers:
- `-h` (short help): commands list only (~15 lines)
- `--help` (long help): includes compact LLM guide (~30 lines instead of 78)
- `aibrowsr guide`: dedicated command that prints the full guide (for first-time setup)

Compact guide targets ~200 tokens (half of current).

### P9: `goto` returns URL+title+snap in one response [High]

**Problem:** After `goto`, the agent needs both the page state AND the snapshot. Currently requires 2 calls even with `--snap` on goto (from P2), because goto returns url+title and snap returns tree separately.

**Fix:** `goto --snap` returns everything in one structured response:
```
https://example.com — Example Domain
uid=e1 heading "Example Domain" level=1
uid=e2 link "More information..." focusable
```

---

## Priority Order

```
P1 (snap noise filter)     → biggest token savings, trivial to implement
P2 (goto --snap)           → biggest round-trip savings, trivial
P3 (--json output)         → agent reliability, moderate effort
P5 (error format)          → agent error recovery, small effort
P8 (compact LLM guide)     → first-impression token savings, small effort
P4 (--page flag)           → multi-tab workflows, moderate effort
P6 (wait command)          → SPA support, moderate effort
P7 (type/press/scroll)     → form completion, small effort per command
P9 (goto combined output)  → follows from P2+P3
```

P1 and P2 are the highest impact with lowest effort. They should be done first.
