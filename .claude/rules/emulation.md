---
paths:
  - "src/emulation.rs"
  - "src/pipe_emulation.rs"
  - "tests/device_emulation_tests.rs"
---

# Device emulation is persistence plus reapply

`emulate device/status/reset` attaches explicit metrics
(`width`/`height`/`dpr`/`mobile`/`touch`/`orientation`) to one named page. No preset catalog:
that belongs to the DevTools frontend, and a bundled copy drifts from Chromium.

**Chrome keeps nothing.** Measured: every `Emulation.*` override is reverted the moment the CDP
session that set it detaches — apply via the CLI, let the process exit, read `devicePixelRatio`
back: 1.

So the stored config is the mechanism, reapplied at the start of every connection to that page,
and there is deliberately NO inter-process coordination on top: an exited process's overrides are
already gone, a still-open pipe's die with it, and `close` has nothing to clear. A first version
shipped a `closing` marker, per-browser client pids probed via `ps`, a 25 ms store poller in every
pipe and a three-way session merge — all guarding against overrides outliving a process, which
cannot happen. Concurrent writers get the store's existing contract: last writer wins, one
`--browser` per agent.

Three consequences, stated rather than hidden:

1. `Target.activateTarget` runs before touching an emulated page, because Chromium reports the Screen Orientation of the ACTIVE target only. Without it, `emulate status` on the mobile page right after a sibling was created read `landscape` off the sibling (e2e-pinned). Price: siblings background like a tab switch.
2. Under `--touch`, `click` and `check` dispatch synthesized touch taps. Chrome's own `Emulation.setEmitTouchEventsForMouse` was measured to convert the event AND leave `Input.dispatchMouseEvent` unanswered forever, so the conversion is done tool-side. `dblclick`/`hover`/`drag` stay mouse events.
3. On a headed or `--connect` browser the page shows its real metrics between commands — the override only exists while a session holds it.

`apply_and_store` is transactional: the store commits only after CDP accepted the full override
set and the page reported effective values. On failure it attempts every cleanup call and
restores the previous config.

A stored config that no longer applies blocks ordinary commands with the one command that repairs
it (`EmulationRecovery` in pipe/replay/batch, the same refusal worded for the CLI). Acting under
silently-wrong metrics reports measurements from a viewport the caller did not ask for.
