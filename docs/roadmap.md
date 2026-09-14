# Discovery and recipes: implementation roadmap

Status: product reset, 2026-09-13. The [mission](mission.md) is the source of product direction.
The [design](design/recipe-discovery.md) describes the discovery and trust boundaries. Items
below are planned unless explicitly marked implemented. The first
[discovery protocol](discovery.md) is implemented; independent recipe acceptance, the registry
and automatic merge service remain planned.

## What changes now

The unit of progress is a capability that an agent discovered, another execution reused, and
the system can revalidate when the site changes. Human procedural teaching and manually authored
site integrations are not prerequisites for that lifecycle.

The GitHub recipe catalogue is now part of the target product. Private repositories and local
recipes use the same lifecycle. Publication is independent of execution; useful private work
does not require contacting a public service.

The competitor feature tables, speculative task-compiler CLI and former A1-A11 implementation
backlog have been removed. The previous experiments are summarized in
[execution foundations](design/task-compilation.md). Existing CLI commands remain supported;
new investments follow the decisions below.

Two product decisions are settled: the calling agent drives discovery, and the first public
catalogue accepts read and extraction recipes. External-write recipes enter the catalogue only
after the later acceptance gate below.

## Investment decisions

| Area | Decision | Next evidence or work |
|---|---|---|
| Observations, assertions, result identity and uncertainty | Invest | Make these the evidence used to accept or reject discovered capabilities. Preserve operational errors separately from failed conditions. |
| Autonomous exploration | Invest through the calling agent | Use the persistent experiment protocol to test discovery without supplied procedures, then fresh-context reuse against an independent evaluator. |
| Site knowledge and recipe memory | Extend | The local journal stores observations, parameters, checks and revisions. Add applicability checks and retrieval across discoveries based on M1/M2 evidence. |
| Recomposition and repair | Invest | Combine known capabilities for an unseen request; rediscover changed transitions while retaining result checks and unresolved effects. |
| Shared and private recipe sources | Add | Source-qualified identity, immutable references, deterministic resolution, private forks and explicit update policy. |
| Independent recipe acceptance | Add | A protected evaluator checks the actual outcome, allowed effects and artifact provenance. Candidate code cannot edit its own acceptance criteria. |
| Automatic publication and merge | Add after acceptance gates | Validate the revision that will be published, isolate candidates, and keep merge credentials outside candidate execution. |
| Pipe, batch and local macros | Keep as execution foundations | Reuse dispatch and compatibility. A recipe can use them only under a capability boundary that is actually enforced. |
| Existing Python workflows | Keep as test references | Use their outcomes and failure cases to evaluate discovery. Additional hand-written workflows need a specific missing test case. |
| Browser sessions, frames, tabs, input controls and files | Keep | Discovery and recovery need access to real browser state. Fix concrete failures encountered in the product loop. |
| New command count, competitor parity and benchmark rank as goals | Remove | Evaluate autonomous discovery, reuse and repair against requested outcomes. |
| Dedicated general workflow language or universal compiler | Remove from plan | Ordinary control flow remains available; select a recipe representation through the confinement experiment below. |
| Human teaching as the learning mechanism | Remove from mission | People provide goals, access and permissions. The system acquires procedural knowledge. |
| Marketplace billing, hosted browser fleet, visual workflow editor, vendor memory adapters | Outside scope | They do not establish discovery or recipe trust. |
| Foundation-model training and exhaustive site crawling | Outside scope | Start with model reasoning over a partial operational memory and goal-directed experiments. |
| Embedded explorer model and provider integrations | Outside scope | The calling agent supplies reasoning and model access. chrome-agent supplies persistent discovery state and execution. |

## Settled decisions and remaining experiments

The calling agent owns model selection, inference and its model-usage budget. chrome-agent owns
the continuation protocol, stored knowledge, browser execution limits and validation records.
Usage supplied by the caller must be identified as reported; unavailable model usage stays
unknown. The executor cannot enforce a model budget outside its process.

The first public acceptance policy covers reading and extracting information. A recipe's name
or declared intent cannot establish that it is read-only. A flow that creates an export job,
saves a draft or changes a cart has external effects and needs the later write-recipe gate.
Local output files are allowed only within the execution policy's declared output locations.

The following questions need experiments before an architectural commitment:

| Question | Current treatment | Evidence that decides it |
|---|---|---|
| Can restricted typed commands express the first discovered recipes, or is isolated general code necessary? | No untrusted public execution yet. Both existing Python and `eval` exceed a declared read-only boundary. | Execute the M1 recipes under each candidate boundary; test forbidden effects and escapes, then choose the smallest representation that passes. |
| Does a graph of capabilities improve composition beyond a set of parameterized procedures? | Persist conditions and transitions without choosing a graph database. | M2 must reuse part of a known path in a task absent from the learning set. Add graph-specific infrastructure only for a demonstrated query. |
| How much low-level verdict detail does the explorer need? | Keep existing evidence and protocol compatibility. | Inspect wrong decisions and repeated observations in M1; reduce irrelevant context while preserving the facts that change the next action. |
| Should WebMCP, HTTP access or another agent transport receive more investment? | Existing WebMCP stays supported; automatic browser-to-HTTP conversion is outside the initial work. | A discovery case must show which route or transport is needed and how its effects are checked under the same policy. |
| Do device emulation, stealth, PDF expansion or the optional daemon deserve further work? | Maintenance only; no deletion based solely on the new mission. | A reproducible discovery, execution or validation failure must justify expansion. Any removal needs a compatibility inventory and an explicit migration decision. |
| How should catalogue validation scale to authenticated sites? | Personal accounts and private traces are excluded from public validation. | Prove a resettable test tenant or publish a narrower capability whose acceptance conditions can be independently exercised. |

## M1: discover a capability without a supplied procedure

Implemented foundation: `discover start/show/step/export` persists caller proposals,
observations and uncertainty, enforces revisions and experiment budgets, and exports selected
paths as local candidate macros. Real-Chrome tests cover process loss and fresh-input reuse.
The profile limits explicit observation commands; it is not a public recipe sandbox.
**M1 remains open:** scripted protocol tests do not establish autonomous discovery. The next
experiment must isolate the calling agent from the application source and independent evaluator.

Build the smallest complete discovery loop through a continuation protocol for the calling
agent, using the existing executor. Persist each selected experiment, its reason, before/after
observations, effects, result checks and remaining unknowns. Separate candidate knowledge from
knowledge supported by a validation run. Enforce browser action and elapsed-time limits; expose
remaining work and caller-reported model usage without claiming control over external inference.

Use an unfamiliar application in an isolated test environment. The explorer receives a URL,
objective, input values, permissions and browser tools. Its environment must deny access to the
application source, test fixtures, reference procedures and evaluator answers. Agent-generated
code must not be able to escape those restrictions.

Acceptance:

- Discover a read/extraction path without selectors, page-specific instructions or human help.
- Generate a candidate recipe with parameter bindings and input-bound result checks.
- A fresh execution with no original conversation reuses it on held-out inputs.
- A replacement calling agent resumes the persisted discovery state after interruption; stale
  proposals are refused and resending an experiment does not silently repeat an action.
- The independent evaluator checks exact identities, fields, scope and completeness, not the
  candidate's success message. Record false successes and incomplete results separately.
- Test irrelevant navigation loops, duplicate labels, delayed rendering, malformed model output,
  model interruption, a changed account and a page that tries to alter the task or permissions.
- Report every attempt and its discovery cost. A scripted replay that was manually authored
  does not satisfy this milestone.

## M2: compose, invalidate and repair

Retain only task-relevant state: account and page context, object identities, usable controls,
preconditions and observed transitions. Do not encode every DOM snapshot as a separate state.
Reuse a known capability when its conditions hold, and select a new experiment when they do not.

Acceptance:

- Solve a new task by combining part of an existing capability with a newly discovered path.
- Change a locator or layout, then a semantic property such as required variant or account.
  The former can produce a repaired candidate; the latter must preserve the original result
  requirement and reject an incompatible context.
- Verify a repair in a fresh context and on retained regression cases before replacing the
  pinned working revision. A changed test or weaker assertion cannot certify the repair.
- Resume after process loss. Reconcile any possible prior write before taking a replacement
  path; changing recipe versions must not erase the operation journal.
- Measure repeated discovery, repaired transitions, successful reuse and procedural help.
  Include failed repairs and requests that required fresh exploration.

## M3: establish recipe identity and execution boundaries

Define a versioned package boundary around the demonstrated capabilities. Start with files and
an explicit source index; a registry service or new workflow language is not a prerequisite.
Use source identity, recipe identity and immutable revision together. Bind the complete payload,
dependencies, checks, supported executor versions and permission request into the artifact digest.

Implement the selected confinement boundary before executing imported recipes with user access.
Enforce permissions in the executor and runtime environment, including redirects, frames,
network destinations, files and all dependencies. A recipe's self-declared effect class is only
a request. Page JavaScript and arbitrary Python must not bypass enforcement.

Acceptance:

- Resolve and replay pinned public, private and local packages using the same contract.
  A same-name source or private override cannot silently replace a selected revision.
- Test wrong origin/account, hidden mutation, credential access, unauthorized upload, redirected
  exfiltration, dependency substitution and code execution outside the declared boundary.
- Reject permission expansion on update; preserve the old pin until the configured update
  policy permits a validated replacement. An ordinary recipe cannot change that policy.
- Run from cached packages under an explicit trust-freshness policy. Show when catalogue
  withdrawal information cannot be refreshed; fail when that policy requires fresh evidence.
- Keep private data, runs and access bindings separate from any distributable recipe package.

## M4: validate, merge and distribute recipes automatically

Use a GitHub catalogue as the first shared source. Candidate proposals contain a recipe diff
and distributable evidence. Validate candidates with a protected test runner and acceptance policy
whose trusted revision is independent of the proposal. Run them in disposable environments
without repository write credentials or personal browser sessions.

Acceptance:

- A qualifying recipe or repair passes static checks, independent execution tests, held-out
  cases and contribution-data checks, then receives an acceptance record for its exact digest.
- Test the combined revision to be merged. A changed base, candidate, dependency or validator
  invalidates the previous acceptance; revalidate before merge or publication.
- A separate publisher verifies the accepted digest and policy identity before merging. It
  never imports or executes candidate code. Eligibility and distribution can be automated
  under the owner's standing policy, without per-recipe human approval.
- A proposal changing the acceptance policy, permissions, validator or publishing workflow
  cannot use the routine recipe auto-merge path to approve itself.
- Test a forged success report, altered evaluator, stale validation, name collision, secret
  canaries, malicious dependencies and an attempted write to the catalogue from a test run.
- Withdraw a broken revision, stop selecting it, and verify client behavior with cached trust
  data. A fallback revision must still pass current context checks before use.

Initial public eligibility is limited to read and extraction recipes. Test attempted mutations
hidden behind a read label, including export-job creation. A site without an independently
testable outcome cannot receive the same acceptance claim as a fully exercised recipe. It may
remain a private or explicitly experimental candidate.

## M5: maintain the shared knowledge through use

Run bounded health checks on the supported catalogue environments. A failed invariant opens a
repair attempt and retains the old evidence; temporary outages do not prove that the procedure
itself has changed. Repairs follow the same independent acceptance and publication path.

Acceptance: detect drift, generate and validate a repair, publish a new revision, and update an
eligible client under its configured policy. Demonstrate that a private fork stays private and
that public updates do not overwrite it. Test login expiry, rate limiting, repeated repair
failure and an unavailable catalogue; bound browser work and continuation requests. Model work
remains subject to the calling agent's limits. A maintenance trigger must invoke a calling agent
to perform rediscovery; the CLI does not run an embedded model when the site changes.

## Later gate: public recipes with external effects

Extend catalogue eligibility only after demonstrating isolated, resettable test contexts for
the proposed effect classes. Validate the intended write, absence of unintended writes, an
already-satisfied result and recovery after interruption or response loss. Browser retries and
concurrent actors must not be mistaken for exactly-once execution.

The acceptance policy and client permissions must explicitly add each supported effect class.
A repaired read recipe cannot acquire write permission through routine auto-merge. Existing
local mutation commands remain available under their current trusted-caller model while this
catalogue gate is closed.

## Release evidence

Report outcomes by discovery, fresh reuse, composition, repair and catalogue publication. For
each, retain the requested inputs, expected result, observed result, validator/environment
revision and resource usage. Present incorrect success claims, uncertain runs and external
effects alongside completion counts.

Release milestones require the corresponding acceptance cases. Existing executor tests continue
to guard compatibility. A green executor suite does not mark discovery or catalogue work done.
