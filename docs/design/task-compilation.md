# Execution foundations for discovered recipes

Status: implemented foundations, 2026-09-13. Product direction is defined in the
[mission](../mission.md); future work is defined in the [roadmap](../roadmap.md) and
[discovery and recipe design](recipe-discovery.md).

The former document at this path investigated a dedicated task compiler. Its candidate CLI,
competitor comparison and deferred catalogue plan have been removed. The accepted direction is
autonomous discovery with a shared and private recipe lifecycle. The original probes remain in
[repository history](https://github.com/sderosiaux/chrome-agent/blob/3eb4ef9/docs/design/task-compilation.md).

## What has shipped

| Foundation | Evidence in this repository | Contribution to the new direction |
|---|---|---|
| Offline macro preparation | `src/macros_prepare.rs`, `tests/macro_tasks_tests.rs` | Validate command shape, parameters and guards before opening Chrome. |
| More complete recordings | `src/macros_record.rs`, `tests/macro_tests.rs` | Retain waits, assertions, reads and downloads; refuse promotion when essential behavior was lost. |
| Outcome evidence | `src/macros_run.rs`, `src/commands/assert.rs` | Preserve command outputs and failed checks instead of treating dispatch as task completion. |
| Discovery continuation | `src/discovery*.rs`, `tests/discovery_tests.rs` | Persist revision-bound experiments, retain uncertainty after interruption, and export selected paths as local candidates. See [the protocol](../discovery.md); autonomous discovery is not established by these scripted tests. |
| Bounded observations | `src/commands/assert_wait.rs`, `tests/assert_wait_tests.rs` | Wait for a declared condition without repeating the action. |
| Bounded browser discovery | `src/browser.rs`, `tests/browser_timeout_tests.rs` | Bound the complete HTTP response, including waiting for headers. This discovers a Chrome endpoint, not website capabilities. |
| Shared execution protocol | `src/pipe_command.rs`, `src/pipe_dispatch.rs`, `examples/pipe_client.py` | Reuse command semantics across CLI, pipe, batch and macros; ordinary code supplies control flow. |
| Verified report export | [Report example](../../examples/report_export/README.md) | Check account, period, CSV contents and file publication; retain uncertainty after a lost response. |
| Complete paginated collection | [Workflow examples](../../examples/verified_workflows/README.md) | Check dataset revision, scope, overlap and total; label incomplete results. |
| Draft reconciliation | [Workflow examples](../../examples/verified_workflows/README.md) | Journal a possible submission before dispatch; search for its result on a later connection or invocation. |

The example procedures and their selectors were authored explicitly against controlled reference
applications. They test execution and result handling. They do not demonstrate autonomous
procedure discovery, independent recipe acceptance or adaptation to an unfamiliar site.

## What these checks mean

`macro check` and `macro run` share offline preparation. It binds parameters, rejects malformed
commands and guards, and prepares the whole path before browser mutation. CSS selectors,
JavaScript, permissions and live page conditions remain runtime concerns. Preparation is
neither an execution sandbox nor proof that a task will succeed.

Recording preserves explicit assertions and waits. Negative or undelivered actions and
unsupported essential steps can prevent a recording from replacing a working macro. A
recording retains observations; it does not infer the intended account, item or business result.

Replay retains dispatcher responses. An explicit assertion that was evaluated and did not hold
exits 2. Unreadable targets, invalid commands and transport failures exit 1. Declared secret
parameters are redacted from reports, but this does not sanitize arbitrary page data for public
contribution.

`assert --within N` reuses the existing page readers and comparators, stopping when the condition
holds or its observation window expires. It adds no action retry. A final condition must be
bound to the requested inputs; a page changing after a click does not establish that result.

## Findings that constrain recipes

A procedure may start with the requested state already satisfied. Recognize that state before
mutating, unless the request explicitly requires a new event or object.

A selector can match several objects. A recipe promising one target needs a uniqueness check
and identity scoped to the requested record. A copied URL or label does not establish account
identity. Frame and tab context also belong to the procedure.

The page exposes partial evidence. The collection example relies on an application-supplied
revision and total; its independent server oracle exists only in tests. A site without those
signals needs other checks or an explicit statement of what cannot be established.

The draft tests observed Chrome retrying a POST after a connection closed before any response
bytes. One create command can therefore produce multiple server writes. The procedure detects
multiple matches and reports uncertainty. Its local journal prevents that caller from issuing
another create command; it cannot enforce server uniqueness or coordinate separate journals.

Read-only reconciliation is distinct from replaying a possibly completed mutation. Repair must
preserve the operation journal and reconcile outstanding effects before taking a new route.
Replacing the recipe does not reset what may already have happened.

`eval` is opaque to offline preparation and can mutate a page or send requests. Example Python
programs can access the host like any other program. Neither is a restricted public recipe
format merely because its inputs or outputs have a schema.

## Carry forward

Keep the existing dispatcher, macro compatibility and failure distinctions while introducing
discovery. A recipe may reuse an ordinary script or macro only under an execution policy that
actually supports its required capabilities. Public acceptance needs a separate validator,
source identity, immutable revisions and tests against outcomes the candidate cannot redefine.

The next evidence comes from an agent discovering procedures on an application whose source,
fixtures and reference solutions it cannot read. Existing examples remain useful regressions
for the executor and independent evaluator.
