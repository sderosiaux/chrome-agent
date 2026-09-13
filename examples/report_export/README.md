# A report export with checked results

This example calls `chrome-agent pipe` from an ordinary Python program. It selects a report
period, checks the account and page state, downloads one CSV, verifies its contents, and returns
the file with the evidence behind that result. Python 3.9+ and Chrome are required; there are no
Python packages to install.

The bundled application is a local reference site. Its selectors and CSV format are specified
below. Adapting this procedure to another application requires inspecting that application and
updating those details.

## Run it

From the repository root, build the CLI and start the reference application:

```bash
cargo build --locked
python3 examples/report_export/demo_server.py
```

The server binds to `127.0.0.1` on a free port and prints its URL as JSON. In a second terminal,
substitute that URL for `<printed-url>`:

```bash
python3 examples/report_export/export_report.py \
  --binary ./target/debug/chrome-agent \
  --browser report-demo \
  --url '<printed-url>' \
  --account acme --period 2026-08 --out ./august.csv
```

Run the same procedure with `--period 2026-09 --out ./september.csv`. August has two invoices;
September has three. Both invocations run without model calls. The application also accepts
`--account beta` when started, so the same procedure can be tested against another account.

The destination directory must exist. An existing destination is rejected before Chrome starts.
Commas, quotes and newlines in command data are encoded by `json.dumps`; commands are sent as
JSON objects, with no shell interpolation. Account identifiers are limited to letters, digits,
`_` and `-`, and periods use `YYYY-MM`.

The browser remains available for inspection, as it does after other CLI commands. When finished:

```bash
./target/debug/chrome-agent --browser report-demo close --purge
```

Stop the demo server with Ctrl+C. If `--browser` is omitted, the script generates a unique name
and includes it in its result. `--binary` defaults to `chrome-agent` on PATH.

## What a successful result means

The program prints one JSON object. A successful report includes:

```json
{
  "status": "verified",
  "outputs": {
    "file": "/chosen/directory/august.csv",
    "account": "acme",
    "period": "2026-08",
    "row_count": 2,
    "bytes": 112,
    "sha256": "<digest of the downloaded bytes>"
  }
}
```

This is an abbreviated shape: actual byte count and digest come from the file. The full result
also contains `checks`, the download receipt, browser name, command count and elapsed time.

The checks establish:

- The page URL equals the requested URL. Login redirects fail this check.
- The account and period controls are unique, and hold the requested values.
- The loaded report summary belongs to that account and period. Its row count, total and
  currency are available before exporting.
- The export control is unique and enabled.
- Chrome reports a completed download at the requested staging path.
- The file is nonempty UTF-8 CSV, with columns `account,period,invoice_id,amount,currency`.
- Every row has the requested account and period, with a nonempty, unique invoice ID.
- The row count, currency and amount total match the page summary. Amounts are nonnegative
  decimals with two places, summed as integer cents.

Only after these checks does the program publish the file. Downloading takes place in a private
directory beside the destination. Publication uses an atomic hard link, so a destination created
by another process is not overwritten. The file retains chrome-agent's `0600` permissions. The
destination filesystem must support hard links.

`verified` refers to these checks. The page summary is a comparison source, not an independent
audit of the application's database. A different report format needs its own checks, including
a different rule if an empty report is a valid result.

## Failure and uncertainty

| Status | Exit | Meaning |
|---|---|---|
| `verified` | 0 | The checked file was published. |
| `failed` | 2 | An explicit page or file check did not hold. |
| `error` | 1 | Inputs, a browser command, the pipe or local file handling failed. |
| `uncertain` | 1 | An export was attempted, but its completed result could not be established. |

Failure reports include the phase, completed checks and last command response when available.
`export_attempted` records whether the export command was attempted; its download receipt may
separately prove that nothing was dispatched. A downloaded but rejected file stays at the private
`artifact.file` path for inspection, while `outputs` remains empty. The requested destination is
not published.

After a dispatched export, a lost response or missing download never triggers another click.
An `uncertain` result names the browser and directs the caller to inspect the export state before
attempting another export. The script has no automatic retry or resume mechanism.

Each browser command has a bounded timeout. The JSONL caller also bounds its wait for a response,
checks terminal startup/finalization messages, and requires a clean pipe exit before verifying
and publishing the file. A pipe exit code of zero alone is insufficient: each response is checked.

## Exercise a broken page or export

Start a new server with a scenario, then use its printed URL:

```bash
python3 examples/report_export/demo_server.py --scenario wrong-file-period
```

| Scenario | Expected behavior |
|---|---|
| `delayed` | Wait for the report and the file; succeed with the default timeout. |
| `wrong-account`, `expired-session`, `duplicate-export` | Fail a page check before exporting. |
| `wrong-selection` | September selection is reverted; `select` returns a command error before export. |
| `wrong-file-account`, `wrong-file-period`, `empty-file`, `truncated-report` | Download completes; file verification rejects it. |
| `missing-download` | Server accepts the export but sends no file; return uncertainty without retry. |
| `disconnect` | Server accepts the export and drops its response; return uncertainty without retry. |

The test-only `/state` endpoint lists exports accepted by the reference server. The task script
never reads it. Integration tests use it as an independent oracle for input selection and the
number of exports, then compare the published CSV's invoice IDs with known fixture records.

```bash
CHROME_AGENT_REQUIRE_CHROME=1 cargo test --locked --test report_export_tests -q
python3 -m unittest discover -s tests/python -q
```

## What belongs where

The CLI supplies navigation, selection, assertions, bounded waits and download capture. This
program supplies the procedure, data dependencies, CSV checks and final result. No new task
format or browser command was needed for this workflow.

The pipe caller handles JSONL framing, timeouts, terminal failures and process cleanup. It is
kept beside this example so another workflow can show whether that code warrants a shared
library. This example does not measure discovery cost, model usage during discovery, or a speed
advantage over agent-driven execution.
