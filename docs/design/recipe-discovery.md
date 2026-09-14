# Autonomous discovery and recipe trust

Status: target design, 2026-09-13. This is not a description of shipped commands. The
[mission](../mission.md) defines the intended experience; the [roadmap](../roadmap.md) contains
implementation gates and the settled product decisions.

## Components and authority

| Component | Responsibility | Authority it does not receive |
|---|---|---|
| Calling agent | Reason about observations, select experiments, propose capabilities, compose paths and propose repairs | Changing the user's goal, execution limits, access or publication policy |
| Discovery protocol | Return applicable knowledge and evidence, record experiments and maintain resumable discovery state | Claiming that a model proposal is an observed fact |
| Browser executor | Enforce the selected execution policy, perform actions, collect observations and preserve uncertain effects | Declaring arbitrary business success from a successful dispatch |
| Local knowledge store | Keep scoped observations, hypotheses, capabilities, revisions and run journals | Promoting a historical observation into a current guarantee |
| Recipe resolver | Select an applicable source-qualified revision and its pinned dependencies | Installing a same-name replacement or widening permissions silently |
| Independent validator | Test a candidate against protected criteria and produce revision-bound evidence | Letting the candidate rewrite the criteria used to accept it |
| Catalogue publisher | Check acceptance records, merge eligible revisions and publish trust metadata | Executing candidate code with publication credentials |

Decision: discovery is driven by the calling agent. It owns inference and model access;
chrome-agent defines the continuation protocol, knowledge lifecycle and validation behavior.
A prompt telling an agent to write a script is insufficient evidence that those components
exist. No embedded model or provider layer is planned.

Each continuation exposes the objective and input binding, policy and state revision,
applicable knowledge, observed results, unresolved effects, remaining unknowns and execution
budget. The agent proposes an experiment against that revision, with its expected observation.
The executor validates the proposal, records the attempt and returns actual evidence.

Persist experiment identity before dispatch. A repeated request retrieves the recorded outcome
or reports an unresolved attempt; it must not silently repeat the browser action. Refuse a
proposal based on stale state. A replacement agent can resume using the persisted record
without the previous conversation. This is a planned protocol contract, not a shipped API.

Model usage is enforced by the caller and may be reported with its provenance. Missing usage
is unknown, not zero. chrome-agent enforces its own browser and continuation limits; it cannot
bound inference or unrelated actions performed outside its interfaces.

The existing Rust executor and typed pipe commands are the starting point. Preserve shared
dispatch and compatibility. Choose the recipe execution representation through a confinement
experiment; neither a new general workflow language nor unrestricted Python is assumed.

## Discover useful transitions

Input to discovery is an objective, parameters, access binding, allowed experiments and budget.
Retrieve known capabilities whose conditions might apply. Observe the current account, page,
relevant objects and available controls before using them.

Select an experiment because it can resolve an uncertainty that matters to the objective.
Record the expected observation, actual observation and cost. Retain a failed attempt with its
conditions so later exploration can avoid repeating it, while recognizing when those conditions
have changed. Stop when the objective is established, the remaining experiments exceed policy,
or the budget is exhausted.

The operational model contains relevant states and transitions. A state is scoped by account,
application context and facts needed for action, rather than URL alone or the entire DOM. A
transition records its preconditions, action, observed effect and way to check the destination.
Recipes can compose transitions and reuse subprocedures. A graph database is not required for
the first representation.

Knowledge has an evidence state:

- **Hypothesis:** suggested by labels, structure, prior knowledge or model reasoning.
- **Observed:** exercised in a recorded context, with the observed outcome retained.
- **Validated:** passed declared checks and independent tests for specified inputs and contexts.
- **Invalidated:** a condition or result no longer holds; the reason and prior evidence remain.

Keep observation counts, tested input ranges, environment identity and timestamps. A numeric
confidence score cannot substitute for those facts. Never infer full site coverage from the
set of known capabilities.

## Generate a reusable capability

Parameterize a successful path by separating task inputs from observed page state. Generate
result checks tied to the requested objects, values and scope. Test new inputs and negative
cases before claiming reuse. Persistent browser node IDs, account IDs, cookies and transient
tokens must not become portable procedure constants.

A recipe package needs the following information, regardless of its eventual file syntax:

| Field group | Purpose |
|---|---|
| Identity | Source, recipe ID, immutable revision, package and dependency digests |
| Applicability | Supported origins, application contexts, required states and known exclusions |
| Inputs and outputs | Typed parameters, access-binding references, returned records or file metadata |
| Effect request | Browser operations, destinations and file access the procedure needs |
| Procedure | Executable path or composed capabilities, with bounded control flow |
| Result and recovery | Checks bound to inputs, uncertain-effect reconciliation and stop conditions |
| Provenance | Generator identity/version, executor compatibility and derivation evidence suitable for the package's visibility |
| Validation | Protected verifier identity, environment, cases, results and digest of the tested package |

The package references access supplied at execution. It does not carry authentication material.
Output schemas help consumers parse results; field types alone do not establish that the
requested invoice, article or product was returned.

## Separate execution policy from outcome evidence

The user or workspace configures allowable exploration, model access, budgets and sharing.
Recipes request capabilities within that policy. Web content and model responses are inputs to
the controller, not policy updates. Reject malformed proposed commands before dispatch.

Enforcement must cover every way a recipe can act: its process, dependencies, browser connection,
page JavaScript, navigations, frames, requests and file operations. A normal browser sandbox
does not enforce a task's account or effect policy. Calling `eval` read-only or allowing only
GET requests does not establish read-only business behavior.

The first implementation must demonstrate its actual confinement boundary with attempted
escapes. Until then, current macros and Python procedures remain trusted local code, not
automatically admissible public packages. Controlled discovery tests can run in disposable
applications with synthetic data and no access to the source or evaluator.

Effect confinement and functional validation answer different questions. The former limits
what can be exercised; the latter checks what the application actually did. Neither establishes
every possible future behavior of a changing site. Recipes outside an independently testable
scope remain experimental or private, with their limitations reported.

## Verify without certifying the candidate's own story

During execution, use observations such as a fresh record lookup, downloaded contents or the
actual cart to check the requested result. Keep the strength and limits of that evidence.
A success toast alone is not enough when the target object can be checked.

For acceptance, the evaluator owns the requested result and test environment. The discovery
process may generate candidate assertions and extra regression cases, but cannot read hidden
answers or modify the acceptance tests. Using a second model without an independent source
of truth does not create an independent check.

Run held-out inputs, misleading labels, wrong objects and account changes. Include tests where
the desired state already holds and tests where execution appears successful but produces the
wrong output. The evaluator observes external effects as well as the returned report. Record
uncertainty as such rather than counting every refusal as a successful task.

## Repair only what the evidence invalidates

At use time, check applicability and freshness. A layout change can invalidate a locator while
leaving the intended result intact. Authentication loss, an unavailable service and changed
business behavior need different responses; repeatedly regenerating a selector cannot fix all
three.

Create a candidate revision for a repair and preserve its parent and changed assumptions. Run
the original protected result checks and retained regression cases before selecting it. A
recipe cannot certify its repair by removing the failing condition.

Run journals bind the operation, resolved recipe revision and inputs. They survive repair and
process loss. When a write may have happened, reconcile it before sending another mutation.
A new recipe revision does not prove the previous effect was absent. Server-side uniqueness
still depends on the target application; a local journal cannot provide it.

## Shared catalogue and private sources

Start with a GitHub catalogue and immutable recipe references. Keep the acceptance policy in a
protected control path. Additional private Git repositories and local packages implement the
same source interface. Resolve fully qualified identities; an unqualified search result must
show its source before it becomes an executable selection.

Keep source trust, catalogue acceptance and current-run verification separate. A source may be
trusted to publish; a particular revision may have passed specified tests; that revision may
still fail against today's site. Each fact needs its own evidence.

Keep run histories private by default. A standing contribution policy can allow automatic
proposals for selected public or synthetic test contexts. Build a separate distributable
package with allowlisted fields and reproducible test data. Secret scanning is a backstop, not
proof that arbitrary private traces are safe to upload. A private run or failed recipe must
not cause a public issue, PR, trace upload or health report without a sharing policy allowing it.

Clients pin recipes and dependencies. Updates require acceptable provenance, current validation
and unchanged permissions, or an explicitly configured policy for the change. Pinning prevents
silent replacement; it does not guarantee future correctness. Withdrawal metadata must be
authenticated and versioned so a forged or older index cannot silently restore a rejected
revision. Clients report stale trust information and follow their configured freshness policy.

## Automated acceptance and publication

The target pipeline is candidate creation, isolated validation, acceptance of an exact digest,
merge, and distribution of that digest. The first public catalogue accepts read and extraction
recipes. Eligibility depends on tested behavior and enforced capabilities; a read label cannot
hide a mutation such as creating an export job. Local result files need scoped output access.

Public write recipes require a later policy revision, isolated test contexts and evidence for
effect verification and recovery. This catalogue restriction leaves existing trusted local
mutation commands available. A routine recipe repair cannot expand its accepted effect class.

The validator runs protected code and tests against an untrusted candidate. It has no publisher
credentials. Candidate-supplied tests may add evidence, but cannot replace protected checks.
Changes to the validator, acceptance policy or publisher use a separate governance path.

An acceptance record binds the candidate payload, dependencies, validator and policy versions,
environment and completed checks. Authenticate the issuing validator through the configured
trust root. The publisher checks that identity and the bound digests; a candidate's JSON
`passed: true` is not an acceptance record. Key or authority changes must be explicit trust
updates, with recovery for compromised publishers.

Validate the revision resulting from the proposed merge with the current base. If either
changes, the old record cannot authorize publication of the new content. A merge queue can
provide the combined-revision testing step; the package and acceptance binding are still our
responsibility. GitHub documents that queue behavior in
[managing a merge queue](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/managing-a-merge-queue).

Keep privileged publication separate from candidate execution. Do not check out and execute
untrusted recipes in a job that holds merge credentials. Treat artifacts from candidate jobs
as untrusted until their issuer and bound content are verified. These boundaries follow
[GitHub's secure use guidance](https://docs.github.com/en/actions/reference/security/secure-use).

The catalogue's routine flow can be autonomous under an established policy. Policy exceptions,
new authorities and untestable effects stay outside that flow. Acceptance reports name the
tested scope, limitations and withdrawal status rather than promising that everything on a
site is correct.
