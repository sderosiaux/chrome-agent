# Discovery works; generated recipes still make false completeness claims

Experiment date: 2026-09-13 in America/New_York, crossing into 2026-09-14 UTC.

An external calling agent discovered how to collect a paginated news archive without supplied
selectors or a procedure. A replacement context resumed from the discovery journal. The agent
produced Python, repaired its first runtime failure from execution evidence, and another
context executed the repaired program against different inputs.

The candidate is rejected. It passed four of nine acceptance cases and made two false success
claims. These are separate results: finding a path worked; reliably packaging its conditions
and knowing when it had finished did not.

## What ran

The [driver](../../evals/discovery/README.md) started a synthetic website with random article IDs,
opaque pagination cursors, delayed rows and a distracting "Next" link. The caller received the
entry URL, requested section and date bound, plus the general chrome-agent command protocol.
It did not receive the server source, evaluator, selectors, hidden expected rows or a reference
procedure. The evaluator kept the requested dataset outside both model and candidate execution.

The installed Claude CLI supplied reasoning, with no native tools, MCP servers, skills or
plugins exposed to its model. The host passed each proposed experiment to `discover step`.
Startup records had to show all four surfaces empty. Model output was data, not executable
host commands. The existing `local_observation_v1` profile stayed unchanged throughout.

The caller found a useful route on its own: read the JavaScript delivered to the page, identify
the cursor feed and article URL construction, then navigate to successive feed pages and read
their JSON. No clicks, page `eval` or separate HTTP client were needed. Reading delivered
JavaScript did not expose the server implementation or the evaluator's expected dataset.

The first discovery exhausted its 12-decision budget just after reading the final feed page.
It never submitted a final result, so it remains an incomplete attempt. A second run allowed
20 decisions and discarded the caller's conversation after decision four. The replacement
received the generic guide, task inputs and `discover show`. It finished correctly in 15 calls.

## Execution is a separate test

The agent's [first program](../../evals/discovery/results/2026-09-13/initial.py) failed on replay:
an overescaped regular expression raised `re.error`. Its correct interactive collection had
not established that its generated program worked.

A fresh discovery received that source, its runtime traceback and observed browser responses.
It received no evaluator grade or hidden answer. The agent generated the
[repaired program](../../evals/discovery/results/2026-09-13/repaired.py), replacing those regexes
with string parsing and an HTML parser. Neither generated program was edited by a human or by
the implementation agent.

The program ran in an unprivileged container with no network and only three read-only mounted
files: candidate, bridge and runner. Browser requests went back through the host's discovery
dispatcher. The evaluator independently compared returned IDs and every requested field.
The source was frozen before withheld variants ran; it received no feedback between cases.

| Withheld case | Result | Acceptance |
|---|---|---|
| Different section, date bound, IDs, cursors and five pages | All six requested records, complete | Pass |
| Two records on one page | Both records, complete | Pass |
| Empty archive | Empty result, unnecessarily incomplete | Fail |
| Date filter selects nothing | Empty result, unnecessarily incomplete | Fail |
| Repeated cursor and repeated rows | Partial result, explicitly incomplete | Pass: required refusal |
| Later page changes the records' section | Two of six requested records, claimed complete | **False success** |
| Feed omits the cursor field | Two of six requested records, claimed complete | **False success** |
| Oldest records appear first | Stops before requested records, incomplete | Fail |
| Page contains instructions to change the task | Program returns the correct records | Pass |

The last row tests deterministic program replay. It does not establish a reasoning agent's
resistance to prompt injection. The scope-change case corrupts a later response for the same
requested archive; silently filtering those records loses data. A missing cursor key is not
evidence that the archive ended. The program treats it like an explicit terminal null.

The unmodified program also assumes descending dates and rejects valid empty results. These
are limitations of its inferred procedure, not missing browser commands.

## Does memory save work?

The fresh-context comparison used the same running server, records, inputs and model, with
fresh browsers and a 20-decision limit for both conditions. One caller received the repaired
program and could request its isolated execution; the other received no prior knowledge.
Within the informed trial, program execution shared the browser used for preliminary
exploration. The initial guide incorrectly described that reuse as a fresh browser; its wording
is now corrected. The separate direct-replay trials each started with a fresh browser.

| Task attempt | Model calls | Reserved browser commands | Elapsed seconds | Caller-reported USD |
|---|---:|---:|---:|---:|
| Initial discovery, no final submission | 12 | 12 | 69.5 | 1.1262 |
| Discovery with context replacement | 15 | 15 | 176.3 | 1.7796 |
| Repair discovery from runtime failure | 13 | 12 | 153.0 | 2.2083 |
| Fresh caller with candidate | 18 | 28 | 149.5 | 2.8770 |
| Fresh caller without candidate | 13 | 14 | 145.1 | 1.5499 |

The informed caller explored again for 16 decisions, then requested the exact candidate at
decision 17 and submitted a correct result at decision 18. Reuse occurred, but it saved no
discovery work in this pair. Supplying a program as context is insufficient to make reuse the
normal execution path.

Separately, direct execution of the frozen program completed the ordinary withheld case in
2.5 seconds and 12 browser commands, with zero model calls. This demonstrates an executable
procedure without inference during replay. It is one synthetic case, not a general speedup
estimate or a matched timing comparison with the paired caller trials.

The five task attempts used 71 model calls and reported USD 9.541133 in total. These numbers
include auxiliary model usage reported by the CLI. A preliminary readiness probe was not
metered in the task archive. Replaying conversation text through separate CLI invocations adds
overhead; these are pilot costs, not optimized serving costs or a bill from the provider.

## Evidence, failures and limits

The [archive](../../evals/discovery/results/2026-09-13/attempts.json) retains all 33 task reports:
five caller trials, the initial program failure, nine replays invalidated by a driver cleanup
race, nine repeated replays after its fix, and the final nine-case acceptance suite. The race
was between Docker auto-removal and explicit removal. The driver now owns removal explicitly
and preserves execution output separately from cleanup failure. Invalidated runs are retained
as infrastructure failures; they are not recipe acceptance evidence. Repeated fixture cases
are not independent samples.

The archive also contains the five discovery journals, both generated sources, the initial
traceback, negative-case browser evidence and the final
[rejection record](../../evals/discovery/results/2026-09-13/acceptance.json). Only synthetic
website data and selected usage fields were copied; raw CLI authentication/session events and
local conversations remain outside the repository.

Runtime: chrome-agent at `d2295c8`, Claude Code `2.1.257`, reported main model
`claude-opus-5[1m]`, Python `3.12.8`, Docker `29.7.2`, and image ID
`sha256:78387bc3881b8273120a12ebe6c1ab22b018ccc2c9adf565ae1ac9b536e184ea`.
The final suite records the binary and driver digests in
[its provenance](../../evals/discovery/results/2026-09-13/acceptance-provenance.json).
The driver evolved during the pilot: the earliest attempts predate seed and provenance
recording, and their exact fixture generation cannot be reconstructed from a seed. Their
observed inputs, outputs and journals remain available. The final suite is reproducible with
the committed fixture seeds and frozen candidate; later reporting and failure-handling additions are
not the exact driver bytes recorded by that run.

An initial adapter investigation used local synthetic responses to inspect the installed
Codex tool surface. Configuration changes did not produce the required empty surface in that
experiment, so it was not used for the task trials. No claim about other versions or supported
configurations follows from that probe.

Boundary checks exercised attempted host-file reads, source modification, network access,
forbidden browser commands, malformed protocol, hangs and excess stdout/stderr. They passed.
Chrome itself remains outside the container and runs page JavaScript. The controlled fixture
has no credentials or external resources. This does not establish confinement for arbitrary
websites or eligibility for untrusted public recipes.

## Product decisions

Keep discovery in the calling agent and preserve the observation profile. This case found a
read path through an unfamiliar interface with the existing commands.

Require independent result validation before promoting a discovered procedure. Invest next in
completeness evidence and applicability checks, including negative cases and valid empty
results. A working happy path and an executable program are both insufficient acceptance criteria.

Make reuse an explicit execution choice tied to a candidate revision and tested conditions.
Measure whether that removes exploration in fresh contexts. Do not assume that storing more
source code or adding a retrieval database solves the behavior observed here.

Keep ordinary Python as an evaluation prototype for adaptive control flow. Fixed local macros
remain useful for their supported sequences; this experiment does not select a public recipe
format. Catalogue publication, cross-site generalization and the full M1 gate remain open.
