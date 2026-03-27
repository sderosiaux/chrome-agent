# aibrowsr

Single-binary Rust CLI for browser automation via CDP. Built for AI agents.

## Docs

- Design: `docs/plans/2026-03-27-v2-design.md`
- Plan: `docs/plans/2026-03-27-v2-implementation-plan.md`

## Build

```bash
cargo build
```

## Architecture

Rust CLI talks CDP directly to Chrome via WebSocket. No Node.js, no Playwright, no QuickJS.
Optional micro-daemon for persistent connections and crash recovery.

## Validation

```bash
cargo build
cargo test
```
