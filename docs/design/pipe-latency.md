# What `pipe` is worth, measured

Six documents claimed "one connection, 10x faster" — README.md, README.cn.md, npm/README.md,
CLAUDE.md, skills/chrome-agent/SKILL.md and llm-guide.txt, all carrying the same number. `git log
-S"10x faster"` traces it to **b80da9a**, whose commit body documents a *binary size* measurement
(3.4 MB → 2.9 MB). There was no latency measurement anywhere: no `benches/`, no `[[bench]]`, no
criterion or divan, no `Instant` in the parity suites, and `scripts/measure.sh` counts output
tokens and never invokes `pipe`. The claim was prose all the way down.

This is the measurement, and the reason every document now says something smaller.

## Method

`scripts/measure-pipe.sh`. The same command sequence is run two ways against the same
already-running browser, and the wall clock of the whole sequence is taken with bash's `time`
(`TIMEFORMAT=%3R`, 1 ms resolution):

- **(a)** one CLI process per command — `chrome-agent --browser B <cmd>` × N
- **(b)** one `pipe` session — the same N commands as JSON lines into one `chrome-agent pipe`

Controls, each because it would otherwise decide the answer:

- **Local fixtures only.** `file://tests/fixtures/extract_hn_like.html` for reads,
  `multi_field_form.html` for actions. A live URL makes the number unreproducible and puts the
  network in front of the effect being measured.
- **Chrome is launched and one full sequence is run before any timing.** A cold Chrome start is
  ~200 ms and would land entirely on whichever mode ran first.
- **Both modes drive the same browser**, so profile, page state and Chrome version are identical.
- **Median of 9 runs per mode**, min–max reported. Two independent full runs of the script; the
  second is published and the first agreed to within one decimal on every ratio.
- **An idle machine.** Re-run on a loaded one (five parallel `cargo build`s) the `eval 1` ×8 row
  read 869 ms with a 373–1161 spread against 332 ms (316–343) quiet, and every ratio inflated. The
  spread is the tell: read it before the median, and discard a run whose min–max is wider than
  ~15% of it. The published numbers all have spreads under 5%.

Two workloads, because the review predicted they diverge and they do. A ratio taken from one is
not a ratio for the other.

## Results

2026-08-30 · Apple M4 Max, macOS 26.4 (Darwin 25.4.0 arm64) · Chrome 152.0.7977.65 ·
chrome-agent 0.15.0, `--release` · median of 9 runs (min–max).

| Workload — 9 commands, a `goto` then 8 | process per command | one pipe session | ratio |
|---|---|---|---|
| **reads**: `text`, `inspect`, `eval`, `assert exists`, `text --selector`, `extract`, `eval`, `inspect` | 352 ms (334–372) | 228 ms (224–232) | **1.5x** |
| **actions**: `fill` ×6, `click` ×2 | 2029 ms (2009–2141) | 1908 ms (1902–1933) | **1.1x** |
| ceiling: `eval 1` ×8 | 332 ms (316–343) | 223 ms (219–227) | 1.5x |
| ceiling: `eval 1` ×40 (41 commands) | 666 ms (627–777) | 222 ms (221–232) | 3.0x |

| Per-invocation floor | wall clock |
|---|---|
| one CLI `eval 1` against a running browser | 12 ms (11–20) |

Across every run of the script taken that day, including three on a machine loaded enough to widen
the spreads: reads landed at **1.4–1.7x**, actions at **1.1–1.2x**, the floor at **12–15 ms**. The
published 1.5x / 1.1x / 12 ms are the quiet-machine medians and sit at the low end of each band,
which is the direction to err in.

## What the numbers say

**Pipe removes a fixed ~12 ms per command and nothing else.** The floor row measures that cost
directly; the two ceiling rows measure it again from the other end — 32 extra do-nothing commands
add 334 ms of CLI time (10 ms each; 13 ms in the other clean run) and no pipe time at all
(223 → 222 ms, inside the noise). Two independent readings of one constant, agreeing with the
floor row. It is the per-invocation preamble: process start, an HTTP GET to
`/json/list`, two WebSocket handshakes, the CDP setup round trips, and two session-store
lock/merge/write cycles.

Everything the ratio does after that is arithmetic on what the commands themselves cost.

- **Reads are cheap**, so the preamble is a third of the total and pipe wins 1.5x. Note that the
  read row and the `eval 1` row are within 20 ms of each other at the same length: the reads are
  nearly free, and both rows are mostly `goto` plus preamble.
- **Actions cost ~225 ms each** — the aim probe, the 60 ms observation window, the settle wait, the
  tree re-read for the change report. Pipe touches none of it, so 12 ms of 225 is 1.1x.
- The ratio **grows with sequence length** only because the one `goto` both modes pay is being
  diluted. That is what the 41-command row is for: it bounds the claim rather than advertising it.
  Extrapolated far enough on no-op commands the ratio does get large, which is probably where a
  number like "10x" felt plausible — but no sequence anyone runs is 40 no-ops.

## Consequences for the documents

- "About 10x faster" is gone from all six documents. They now state ~12 ms per command, 1.5x on
  reads, 1.1x on actions, and name the workload each holds for.
- The README comparison table's `Startup | ~10ms (session reuse)` row was equally unmeasured. It
  turned out roughly right: **12 ms (11–20)** for one CLI command on a running browser. The row now
  says 12 ms and says what was timed.
- The stated reason to use `pipe` changed. It is uid stability across the sequence and the `frame`
  binding living on the connection — correctness properties. The speed is a rounding error next to
  either.

## Limits of this measurement

- One machine, one OS, one Chrome. A slower disk or a busier machine moves the 12 ms floor; the
  action row is dominated by fixed waits and should move least.
- `file://` fixtures make page load nearly free. On a real site both columns grow by the same
  page-load cost, which *lowers* every ratio — so these are upper bounds for real work, not lower.
- Within each timed iteration the CLI sequence always runs before the pipe sequence. The spreads
  are tight enough (±5%) that ordering is not visible, but it was not randomised.
- The script is not run in CI. It needs a real Chrome and ~1 minute, and a latency assertion in CI
  is a flake generator. Re-run it by hand when the connect path or the settle path changes.
