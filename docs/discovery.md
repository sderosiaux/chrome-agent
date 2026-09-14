# Caller-driven discovery

`discover` persists the calling agent's objective, experiments and observations. A new process
can read the record and continue from its current revision. The agent chooses the next
experiment; chrome-agent executes it through the existing browser dispatcher. No model runs
inside the CLI.

This is the first part of [M1](roadmap.md#m1-discover-a-capability-without-a-supplied-procedure).
The protocol supports continuation and local candidate export. An
[isolated evaluation](experiments/discovery-2026-09-13.md) now exercises autonomous discovery,
handoff and program replay on one synthetic application. The generated candidate fails negative
cases; independent product acceptance, site-change repair and catalogue publication remain open.

## Start, observe, continue

The parent directory of the state file must exist. Choose a browser name and page for the
discovery; subsequent experiments must use those same names. Use a dedicated browser so other
callers do not change its state during an experiment.

The URLs and selectors below illustrate a news site; the calling agent must discover the
selectors on the actual site. An assertion states a fact to check, not a general task oracle.

```bash
chrome-agent --json --browser news discover start news.json --goal "Collect this edition's headlines" --url https://news.example/today --inputs '{"url":"https://news.example/today","edition":"Daily news"}' --max-commands 20 --within 900
chrome-agent --json --browser news discover step news.json --proposal '{"id":"open","revision":0,"reason":"Observe the entry page","command":{"cmd":"goto","url":"{{url}}"},"checks":[{"cmd":"assert","what":"url","equals":"{{url}}"}],"unknowns":["Where are the headlines?"]}'
chrome-agent --json discover show news.json
chrome-agent --json --browser news discover step news.json --proposal '{"id":"headlines","revision":2,"reason":"Read the discovered headline list","command":{"cmd":"text","selector":"main ul"},"checks":[{"cmd":"assert","what":"text","selector":"h1","contains":"{{edition}}"}],"hypotheses":["This list belongs to the requested edition"],"unknowns":["Other editions remain untested"]}'
```

`start` and `show` never open Chrome. `step` prepares the command and every check before
connecting. Its command objects use the same JSON vocabulary and validators as `pipe`.
Parameters are string values supplied in `--inputs`; `{{name}}` substitution runs once through
the macro preparation code. Unknown keys, undeclared parameters, invalid assertions and
commands outside the profile fail before reserving budget or opening Chrome.

Each proposal contains an ID, the current revision, a reason and one command. It may include
up to eight assertion checks, sixteen hypotheses and sixteen unknowns. IDs use 1–64 ASCII
letters, digits, `_` or `-`. Hypotheses and unknowns remain caller-authored claims.
The journal retains them with the experiment that introduced them; they do not change policy.

Use the revision returned by `show` or `step`, rather than calculating it. Reserving an
experiment advances it once; writing its outcome advances it again. A stale proposal cannot
start work. Reusing an ID with different content is refused. Repeating the same complete
proposal returns its stored receipt with `replayed:true`, without dispatching commands or
charging the budget again. That receipt describes the earlier observation, not the current page.

## What the record establishes

| State | Meaning | Step exit |
|---|---|---|
| `observed` | The command and its declared checks returned success; their responses were saved. | 0 |
| `not_held` | An assertion was evaluated and did not hold. Its expected and observed values remain in the response. | 2 |
| `error` | A command failed, or the page left the permitted origin. Inspect the retained responses. | 1 |
| `uncertain` | Completion could not be established, for example after a timeout or oversized response. A command may have run. | 1 |
| `pending` | A durable reservation has no durable outcome. The process may still be running or may have died. | An identical retry reports `uncertain`, exit 1. |

Responses preserve the existing command fields, including partial-read indicators. URL
observations before and after successful dispatches record navigation context; they do not
establish account identity or the absence of external effects. Declared checks must express the
requested result precisely enough for the calling agent to assess it.

The file is atomically replaced under a nonblocking OS lock. Only one writer can work on a
record at a time; `show` can read its last durable revision while a writer runs. An experiment
is reserved on disk before any browser connection. Responses are saved together at the end.
A killed process can therefore leave a `pending` record even if some commands completed.
After its lock is released, a new observation may use the current revision to inspect the
browser. The old experiment stays unresolved and cannot be exported. There is no automatic
retry or inference that an interrupted operation had no effect.

Always use `--json` for callers: reports are JSON, and the flag also makes operational errors
before dispatch JSON. Clap usage errors still go to stderr. A write failure cannot produce a
successful experiment receipt; read the file again to determine which revision survived.

## Limits and local scope

`--max-commands` reserves one unit for the proposed command and one for each check. Failed and
interrupted attempts keep their reservation, including checks that never ran. The default is
50 and the maximum is 100. This counts dispatcher commands, not internal CDP calls or assertion
polls. `--within` sets a wall-clock lifetime, including time between processes: 1–86,400 seconds,
default 1,800. The remaining lifetime also bounds the asynchronous browser operation. This
depends on the system clock and is not a hard real-time guarantee for blocking filesystem work.
`show` and retrieval of an existing receipt remain available after expiry. Model usage is
reported as `null`; inference limits belong to the calling agent.

The `local_observation_v1` profile accepts `goto`, `inspect`, `read`, `text`, `extract`, `assert`
and `wait`. Its first experiment must navigate to the exact entry URL. Later explicit
navigations must use the same HTTP(S) origin, including port; embedded URL credentials are
refused. Non-navigation commands check the current page's origin before dispatch. Successful
commands check it again afterwards, stopping the experiment if the page left that origin.

This profile limits explicit commands. It is **not a browser or code sandbox**: a site can send
requests on load, react to scrolling, or redirect before the next observation. Existing dialog
and session behavior still applies. Use it only in a caller-authorized browsing context. Public
recipe confinement is a separate roadmap gate.

Proposals are limited to 32 KiB, retained command responses to 64 KiB and state files to 16 MiB.
An oversized response stops the experiment with uncertainty; narrow the next read. Explicit
read truncation remains visible in the command response and must not be interpreted as complete
extraction.

Records and locks are regular files with mode 0600 on Unix; symbolic links are refused there.
They contain raw inputs and page observations, which may be private. Nothing is uploaded or
sanitized for public contribution. Protect and remove these records like other local session
data. The journal is user-owned evidence, not a signed independent attestation.

## Export a candidate for fresh reuse

Select successful experiment IDs in their recorded order. Failed exploratory branches may be
excluded. The selected path must start with `goto`, end with an explicit assertion and contain
no document-specific uids. Repeat any uid-based observation with a selector before selecting it.

```bash
chrome-agent --json discover export news.json --name news-headlines --steps open,headlines
chrome-agent --json macro check news-headlines --var url=https://news.example/archive --var "edition=Archive news"
chrome-agent --json --browser news-reuse macro run news-headlines --var url=https://news.example/archive --var "edition=Archive news"
```

Export creates a new file in `~/.chrome-agent/macros/` and refuses to replace an existing name.
It retains command templates and required parameters without copying input values as defaults.
It does not copy the observation journal into the macro. Literal values already present in
commands remain, so this local export is not a public-data filter.

The result is a **candidate**, not a validated recipe. Selecting steps can omit a dependency,
and a successful assertion can still be too weak for the task. Run the exported path in a
fresh browser with new inputs and check the outcome independently. Its `site` field is metadata;
legacy `macro run` does not enforce the discovery origin or budget. Source-qualified identity,
immutable revisions and independent acceptance are not part of this export.

## Evidence and next experiment

`tests/discovery_tests.rs` exercises real Chrome with a local HTTP site: continuation in fresh
CLI processes, new-input macro replay, an incorrect edition, stale and duplicate proposals,
process loss, concurrent access, expired limits, redirects, and oversized reads. Unit tests
check URL normalization, literal bindings, history consistency and interrupt ownership.

These are scripted protocol and executor regressions. The next M1 experiment must give a
calling agent only an objective, site access and inputs, prevent access to the site's source
and evaluator, and measure discovery plus fresh-context reuse. Record failed attempts and
costs as well as successful outcomes. Passing this scripted suite does not satisfy that gate.
