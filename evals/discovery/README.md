# Discovery evaluation

This lab tests whether a calling agent can discover a procedure from browser observations,
produce executable code, and reuse it with different inputs. The evaluator owns the expected
records and grades exact IDs, fields, scope, duplicates and completeness. A candidate's
`complete: true` does not determine acceptance.

The [first experiment](../../docs/experiments/discovery-2026-09-13.md) found successful discovery
and replay, but also two false successes. The archived candidate is deliberately **rejected**.
It is evidence to test against, not a recipe to install.

## Run

Requirements: a built chrome-agent binary, Chrome and Python 3.10+. Discovery additionally
uses an installed, authenticated `claude` CLI; inference uses that client's configured model
and account. Replay uses a local Docker image. There is no model integration in chrome-agent.
These experiments run only when explicitly invoked; ordinary tests do not make model calls.

```bash
cargo build --locked
# Once, if the image is not already local. Replay itself never pulls an image.
docker pull python:3.12-slim

# No model: test the evaluation boundary before executing candidates.
python3 evals/discovery/check_boundary.py

# Model calls: discover from the URL and objective, then replace the caller after four decisions.
mkdir -p evals/discovery/runs
python3 evals/discovery/run.py --out evals/discovery/runs/learn --max-decisions 20 --restart-after 4

# No model: freeze the generated program and evaluate nine fixture variants.
python3 evals/discovery/suite.py --candidate evals/discovery/runs/learn/candidate.py --out evals/discovery/runs/heldout

# Reproduce the archived candidate's rejection. Exit 1 is the expected result.
python3 evals/discovery/suite.py --candidate evals/discovery/results/2026-09-13/repaired.py --out evals/discovery/runs/rejected
```

Each output directory must be new. `--binary` selects another build. `suite.py --image` selects
an already installed image; its resolved immutable image ID is retained for every replay.
`suite.py --include-callers` adds model calls for a paired trial: a fresh agent with the candidate,
then a fresh agent without it, against the same server, records and inputs. It does not force
the informed agent to use the program or assume that having it saves work.

For one replay, `run.py --replay FILE` accepts `--count`, `--page-size`, `--edition`, `--since`,
`--scenario` and `--seed`. A seed reproduces records and cursors, while the loopback port changes.
`run.py --knowledge FILE` supplies JSON from an earlier attempt, such as its program and runtime
error. Do not feed evaluator answers into discovery or repair. `run.py` exits 1 for incomplete
collection, operational failure or cleanup failure. `suite.py` also accepts an explicit incomplete
result on the three damaged-feed cases; a crash is not a successful refusal.

## Boundaries

The model receives a general command guide, objective, inputs and browser observations. It gets
no selectors, application source, reference implementation or expected records. The synthetic
site serves one cursor page at a time and exposes no evaluator endpoint. Reading JavaScript
delivered to the browser is a valid observation. The website includes delayed rendering and a
distracting duplicate "Next" label; its IDs and cursors vary between discoveries.

`caller.py` invokes Claude with safe mode, empty tools and MCP configuration, disabled hooks,
skills and browser integration, no settings sources, and no session persistence. Every response
must report empty tools, MCP servers, skills and plugins; unexpected native tool use fails the
trial. Each invocation receives the explicit conversation as input. Restart discards that
conversation and supplies `discover show` instead. This adapter tests an external caller; it
does not make Claude a product dependency.

Agent-authored Python runs only inside a disposable Docker container. It has no network,
runs as an unprivileged UID with dropped capabilities, and mounts only the frozen source and
the trusted bridge/runner as read-only files. It receives task inputs over stdin. The host
accepts only a bounded command message or a final result. Every command goes through the real
`discover step` revision, origin and observation-profile checks. The container cannot import
the evaluator or change its answers. Candidate code is parsed for syntax on the host, never
imported or executed there.

Each discovery permits 60 reserved browser commands and 1,800 seconds; the driver also limits
model decisions and gives each model invocation 120 seconds. Replay permits 60 proposals and
a 60-second execution limit, with bounded additional setup and cleanup. Candidate protocol
lines, input messages and stderr are capped at 64 KiB; model output is capped at 1 MiB per call.
Containers have 128 MiB memory, one CPU and 32 processes. Unique browser names and container
names scope cleanup to each run. A killed host process can require cleanup of its recorded
browser or `chrome-agent-eval-*` container; do not prune unrelated resources.

This is **not the public recipe sandbox**. Chrome runs outside the container and executes page
JavaScript. The fixture is controlled, has no credentials or external resources, and rejects
writes. Restricting explicit browser commands does not confine arbitrary sites, their redirects,
frames or network effects. Package identity, dependencies, permissions and catalogue acceptance
records still require the roadmap's trust work.

## Evidence and checks

Reports retain requested inputs, expected and submitted records, errors, server requests,
discovery journals, model-call counts and caller-reported usage/cost. Unknown usage stays null;
a failed model invocation still counts. Usage includes auxiliary models reported by the CLI.
Conversation replay and CLI overhead make these pilot costs unsuitable for general performance
claims. A direct program replay has zero model calls by construction.

Current runs also record the binary digest, Git revision, driver file digests, Python and CLI
versions, fixture seed, candidate digest and Docker image ID. Working runs are ignored by Git
and private by directory permissions. The committed archive was selected explicitly from
synthetic data; there is no automatic contribution path from private runs.

```bash
# Also run by the existing Rust integration test for the Python examples.
python3 -m unittest discover -s tests/python -q
# Opt-in real Chrome + Docker checks; no model or account needed.
python3 evals/discovery/check_boundary.py
```

The checks reject bad grader inputs, incomplete or unexpected caller tool configuration,
malformed model decisions, missing usage after interruption, hangs and excess output. The
container probe attempts host-file access, network access, writes to its source, and forbidden
browser commands. These hand-written probes test boundaries; they do not count as discovery.
