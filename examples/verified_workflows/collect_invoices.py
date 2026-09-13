#!/usr/bin/env python3
"""Collect a complete invoice dataset from the reference application's paginated table."""

import time

from workflow_common import (Pipe, PipeError, TaskError, command, emit, expect, finish,
                             identifier, inputs, integer, open_page, parser, period)


READ_PAGE = """(() => {
  const table = document.querySelector('#invoices');
  return {url:location.href, account:document.querySelector('#account').value,
    period:table.dataset.period, revision:table.dataset.revision, total:table.dataset.total,
    page:document.querySelector('#page').value, next:table.dataset.next,
    rows:Array.from(table.querySelectorAll('tbody tr'), row => ({
      id:row.dataset.id, account:row.dataset.account, period:row.dataset.period,
      amount_cents:row.dataset.amountCents, currency:row.dataset.currency
    }))};
})()"""


def read_rows(rows, account, wanted_period):
    if not isinstance(rows, list) or len(rows) > 1000:
        raise TaskError("Page rows must be a list of at most 1000 records")
    parsed = []
    for row in rows:
        if not isinstance(row, dict) or set(row) != {"id", "account", "period", "amount_cents", "currency"}:
            raise TaskError("Invoice fields do not match the expected schema")
        try:
            identifier(row["id"], "invoice id")
        except ValueError as error:
            raise TaskError(str(error)) from error
        if row["account"] != account or row["period"] != wanted_period or row["currency"] != "EUR":
            raise TaskError("Invoice belongs to another account, period or currency")
        parsed.append(dict(row, amount_cents=integer(row["amount_cents"], "amount_cents")))
    return parsed


def merge_page(previous, rows):
    """Accept identical overlap, refuse conflicting identities, and commit one page at a time."""
    merged = dict(previous)
    duplicates = 0
    for row in rows:
        if row["id"] in merged:
            if merged[row["id"]] != row:
                raise TaskError("The same invoice id has conflicting data")
            duplicates += 1
        merged[row["id"]] = row
    return merged, duplicates


def collect(args):
    started = time.monotonic()
    report = dict(status="error", complete=False, outputs={"items": []}, checks=[],
                  browser=args.browser, pages=0, duplicates=0, phase="inputs")
    pipes, records = [], {}
    try:
        inputs(args)
        period(args.period)
        if not 1 <= args.max_pages <= 1000 or not 1 <= args.max_rows <= 100000:
            raise ValueError("max-pages must be 1–1000 and max-rows 1–100000")
        url = args.url + "/invoices?period=" + args.period
        with Pipe(args.binary, args.browser, args.timeout) as pipe:
            pipes.append(pipe)
            open_page(pipe, report, url, args.account)
            revision, total, page = None, None, 1
            while True:
                command(pipe, report, "page_ready", dict(cmd="assert", what="value", selector="#page",
                                                         equals=str(page), within=args.timeout))
                command(pipe, report, "unique_table", dict(cmd="assert", what="exists", selector="#invoices", count=1))
                data = command(pipe, report, "read_page", dict(cmd="eval", expression=READ_PAGE)).get("result")
                if not isinstance(data, dict):
                    raise TaskError("Table snapshot is not an object")
                for name, expected in [("url", url), ("account", args.account), ("period", args.period), ("page", str(page))]:
                    expect(report, name, expected, data.get(name))
                page_total = integer(data.get("total"), "total", args.max_rows)
                if revision is None:
                    if not isinstance(data.get("revision"), str) or not 1 <= len(data["revision"]) <= 128:
                        raise TaskError("A dataset revision is required to check consistency across pages")
                    revision, total = data["revision"], page_total
                    report.update(revision=revision, expected_total=total)
                expect(report, "revision", revision, data.get("revision"))
                expect(report, "total", total, page_total)
                rows = read_rows(data.get("rows"), args.account, args.period)
                merged, duplicates = merge_page(records, rows)
                if len(merged) > total:
                    raise TaskError("Unique record count exceeds the advertised total")
                next_page = data.get("next")
                if next_page != "":
                    expect(report, "next_page", page + 1, integer(next_page, "next page", 1000))
                    if len(merged) == len(records):
                        raise TaskError("Pagination made no progress")
                records = merged
                report["outputs"]["items"] = list(records.values())
                report["pages"] += 1
                report["duplicates"] += duplicates
                if next_page == "":
                    expect(report, "complete_count", total, len(records))
                    break
                if report["pages"] >= args.max_pages:
                    raise TaskError("Page limit reached before the end of the dataset")
                command(pipe, report, "unique_next", dict(cmd="assert", what="exists", selector="#next", count=1))
                command(pipe, report, "next_enabled", dict(cmd="assert", what="state", selector="#next", enabled=True))
                command(pipe, report, "next_page", dict(cmd="click", selector="#next"))
                page += 1
            report["phase"] = "pipe_finalization"
        report.update(status="verified", complete=True, phase="complete")
    except (TaskError, PipeError, OSError, ValueError) as error:
        report.update(status=getattr(error, "status", "error"), error=str(error))
        if report["pages"] and report["status"] == "failed":
            report["status"] = "partial"
    return finish(report, started, pipes)


if __name__ == "__main__":
    cli = parser(__doc__)
    cli.add_argument("--period", required=True)
    cli.add_argument("--max-pages", type=int, default=20)
    cli.add_argument("--max-rows", type=int, default=10000)
    raise SystemExit(emit(collect(cli.parse_args())))
