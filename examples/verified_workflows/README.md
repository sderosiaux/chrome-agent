# Paginated collection and draft creation

Two runnable Python procedures drive `chrome-agent pipe` against a local reference application.
They reuse [the JSONL caller](../pipe_client.py) from the report export example. Iteration,
aggregation and recovery live in the scripts; browser actions use existing commands, including
`assert --within`. No model calls occur during execution.

Requires Chrome and Python 3.9+ on macOS or Linux. The draft journal uses POSIX file locking.
No Python packages, credentials or external service are required.

## Start the reference application

```bash
cargo build --locked
python3 examples/verified_workflows/demo_server.py
```

The server prints its URL as JSON. Substitute it for `<url>` in the commands below. It binds to
loopback on a free port. Stop it with Ctrl+C. Use `--account beta` or `--page-size 3` to vary the
reference data's scope and pagination.

## Collect invoices across pages

```bash
python3 examples/verified_workflows/collect_invoices.py \
  --binary ./target/debug/chrome-agent --browser invoices-demo \
  --url '<url>' --account acme --period 2026-08
```

August has five invoices across three pages at the default page size; September has three.
The result contains typed `outputs.items`, `complete`, `pages`, `duplicates`, `expected_total`,
`revision`, the checks performed, command count and elapsed time.

Each page must have the requested URL, account and period, the expected page number, and the
same dataset revision and total as the first page. Invoice IDs must be nonempty and amounts
must parse as nonnegative integer cents. Identical overlapping records are deduplicated;
conflicting values for the same ID fail the collection. A page contributes records only after
its scope and contents pass validation.

Success requires observing the end of pagination, matching the advertised total with unique
records, and a clean pipe exit. An empty dataset is valid when the total is zero. `--max-pages`
(default 20, maximum 1000) and `--max-rows` (default 10000, maximum 100000) bound the collection.
A repeated page, backward cursor, page with no progress, changed revision or count mismatch
stops the run. Each page is limited to 1000 rows; JSONL responses are capped at 1 MiB.

| Status | Exit | Meaning |
|---|---|---|
| `verified` | 0 | `complete:true`; the declared checks held for the collected records. |
| `partial` | 2 | Some pages passed, then a condition or limit stopped collection. |
| `failed` | 2 | A condition failed before a page was accepted. |
| `error` | 1 | Inputs, a command, transport or finalization failed. |

Every unsuccessful result has `complete:false`, including an `error` that retains records from
earlier pages. Consumers must check this flag before treating `outputs.items` as the full dataset.
The page's revision and total are application-supplied evidence, not an independent database audit.

## Create a draft and recover its result

```bash
python3 examples/verified_workflows/create_draft.py \
  --binary ./target/debug/chrome-agent --browser draft-demo \
  --url '<url>' --account acme --reference invoice-review-2026-08 \
  --title 'Review August invoices' --amount 12.50 --journal ./draft-operation.json
```

Use the same reference, journal and inputs when invoking the same operation again. The script
first searches for that reference. One existing draft with matching account, title, amount,
currency and status can satisfy the request immediately. Its identifier is returned in
`outputs.draft.id`. Multiple matches produce `uncertain`; conflicting fields produce `failed`.

When no draft is found and the journal shows no possible previous submission, the procedure
checks and fills the form. It persists `attempted` before sending one create command. Then it
opens the search page on a fresh pipe connection and verifies the stored record, even if the
create response was lost. Search reloads may repeat while waiting for a delayed commit; the
create command does not repeat.

The journal moves through three states:

| State | Meaning on the next invocation |
|---|---|
| `prepared` | No create command has been attempted through this journal; search before deciding to create. |
| `attempted` | A create command may have reached the page; only search and verification are allowed. |
| `verified` | A previous run verified a record; verify the current record again. |

The journal binds the origin, account, reference, title, normalized amount and currency. Different
inputs or a malformed journal are refused before Chrome starts. Writes use a private temporary
file, fsync and atomic replacement. Journal and lock files are `0600`; the adjacent `.lock` file
stays on disk and excludes concurrent users of the same journal. Its parent directory must exist.
Deleting a journal loses the evidence that prevents resubmission.

If no record appears after a possible submission, the result is `uncertain` (exit 1), with empty
outputs and an instruction to inspect or reconcile using the same journal. Repeated invocations
will continue searching without creating another draft. A disk failure before the `attempted`
write prevents the click. A crash after that write but before the click leaves uncertainty too;
the journal deliberately cannot claim that nothing was sent.

`verified` exits 0, explicit field mismatches exit 2 as `failed`, and operational failures exit 1
as `error`. Reports retain the submission receipt or submission error, `creation_attempted`,
`prior_attempt`, `submission_commands`, and the resolution (`already_present` or
`found_after_attempt`). Finding the requested record does not prove which actor created it.

One browser click does not establish one server write. In the `transport-retry` fixture, a
connection closed before any response bytes caused Chrome to send the POST again during testing.
The script detected multiple matching drafts and returned uncertainty. The journal governs this
script's submissions; it cannot enforce server uniqueness or coordinate separate journals.
Those guarantees require support from the target application.

## Exercise failures

Start a new server with a scenario, then use its printed URL:

```bash
python3 examples/verified_workflows/demo_server.py --scenario lost-response
```

| Scenario | Expected behavior |
|---|---|
| `delayed`, `overlap`, `empty` | Complete collection after delayed rendering, deduplication, or a valid zero count. |
| `conflict`, `repeat-page`, `repeated-cursor`, `no-progress` | Stop collection without looping or silently choosing conflicting data. |
| `changed-revision`, `truncated`, `wrong-row-account`, `invalid-amount` | Refuse an inconsistent or incomplete collection. |
| `expired-session`, `disconnect` | Preserve accepted pages with `complete:false`. |
| `wrong-account`, `duplicate-submit` | Refuse the draft before submitting; wrong account also blocks collection. |
| `lost-response`, `delayed-commit` | Find and verify the created draft through search after losing a partial HTTP response. |
| `late-commit` | With `--timeout 1`, return uncertainty; a later invocation can find the committed draft. |
| `missing-draft`, `lookup-unavailable` | Return uncertainty and retain the attempted journal, including on later invocations. |
| `duplicate-draft`, `wrong-fields` | Refuse to choose among duplicates or accept mismatched fields. |
| `transport-retry` | Close before headers; Chrome may retry independently. Report the records actually found. |

`--timeout` defaults to three seconds per command and observation. The reconciliation loop starts
further search reads only within its own window of that duration; an in-progress search still
has the individual command deadlines. These are not a deadline for the whole task. The JSONL
caller also bounds response waits, handles terminal errors and checks process exit.

Browsers remain available for inspection. Each script creates a unique name when `--browser`
is omitted and includes it in its report. Close named demo browsers when finished:

```bash
./target/debug/chrome-agent --browser invoices-demo close --purge
./target/debug/chrome-agent --browser draft-demo close --purge
```

## Tests and adaptation

```bash
CHROME_AGENT_REQUIRE_CHROME=1 cargo test --locked --test workflow_examples_tests -- --test-threads=2
python3 -m unittest discover -s tests/python -q
```

Tests compare invoice identities and draft records with expected data, and use the server's
`/state` endpoint as an independent oracle for page requests and received submissions. The task
scripts never read that endpoint. Tests also inject a pipe failure after submission and a journal
write failure before submission, and exercise journal locking and input binding.

These procedures target the reference app's documented DOM: `/invoices` exposes a revision,
total, next page and typed row attributes; `/drafts/search` exposes a complete search by reference;
`/drafts/new` exposes the reference, title and EUR amount fields. Adapting them requires inspecting
the real application's controls, pagination consistency and search semantics. A site without
a reliable revision or complete reference lookup needs different checks or a stated limitation.
These authored examples test execution and result handling. Autonomous discovery and maintenance
are the next product direction, described in the [mission](../../docs/mission.md) and
[roadmap](../../docs/roadmap.md). The planned GitHub catalogue will distribute independently
validated recipes and support private sources. It is not implemented yet.
