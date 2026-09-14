---
paths:
  - "src/discovery*.rs"
  - "tests/discovery_tests.rs"
  - "docs/discovery.md"
---

# Persistent caller-driven experiments

`discover` implements the continuation foundation of M1. The calling agent chooses experiments;
the CLI does not run a model. Scripted protocol tests do not establish autonomous discovery.

- Prepare actions through `Macro::prepare` and the shared typed command parser. Execute through
  `pipe::dispatch_on`. Do not duplicate browser command semantics.
- Hold a nonblocking lock across reservation, execution and outcome persistence. Reserve on
  disk before opening Chrome. A missing final outcome is uncertain; never redispatch an ID.
- Exact retries return recorded evidence. Stale revisions and changed proposals under an
  existing ID fail before browser work. New observations may continue after a crashed writer
  releases the lock; unresolved history stays visible.
- Charge the whole command/check reservation, even after failure or interruption. Use the
  persisted wall-clock deadline across processes. Do not imply control over caller inference.
- Preserve failed assertions separately from operational errors. An assertion that ran and did
  not hold exits 2; uncertainty exits 1. Never print success before its evidence is saved.
- The local observation profile limits explicit commands. Site JavaScript, network requests
  and redirects remain outside a confinement boundary. Do not advertise a public read sandbox.
- Export only explicitly selected successful paths, beginning with navigation and ending with
  an assertion. Refuse document uids and existing destinations. The output is a trusted local
  macro candidate; independent validation, immutable identity and publication are later gates.
- Local records contain raw private inputs and observations. Write atomically with 0600 on
  Unix, reject symlinks, cap size, and never remove a temporary file this writer did not create.
- Build the binary before running subprocess integration tests. Use the shared test helpers
  for owned browser names and files; test fresh-process continuation and new-input reuse.
