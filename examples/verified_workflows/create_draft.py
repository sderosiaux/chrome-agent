#!/usr/bin/env python3
"""Create or recover a draft by an explicit reference, without repeating an uncertain submit."""

import time
from urllib.parse import quote

from draft_journal import Journal
from workflow_common import (Pipe, PipeError, TaskError, cents, command, emit, expect, finish,
                             identifier, inputs, integer, open_page, parser)


READ_RESULTS = """(() => {
  const root = document.querySelector('#results');
  return {url:location.href,account:document.querySelector('#account').value,
    reference:root.dataset.reference,total:root.dataset.total,
    items:Array.from(root.querySelectorAll('.draft'), el => ({
      id:el.dataset.id,account:el.dataset.account,reference:el.dataset.reference,
      title:el.dataset.title,amount_cents:el.dataset.amountCents,currency:el.dataset.currency,status:el.dataset.status
    }))};
})()"""


def verify_record(report, record, args):
    if not isinstance(record, dict):
        raise TaskError("Draft is not an object")
    try:
        identifier(record.get("id"), "draft id")
    except ValueError as error:
        raise TaskError(str(error)) from error
    observed = dict(record, amount_cents=integer(record.get("amount_cents"), "draft amount"))
    for name, expected in [("account", args.account), ("reference", args.reference), ("title", args.title),
                           ("amount_cents", cents(args.amount)), ("currency", "EUR"), ("status", "draft")]:
        expect(report, "draft_" + name, expected, observed.get(name))
    return observed


def find_draft(pipe, report, args, wait=False):
    """Reload only the read-only search page while waiting for an eventual commit to appear."""
    url = args.url + "/drafts/search?reference=" + quote(args.reference, safe="")
    deadline = time.monotonic() + args.timeout
    while True:
        open_page(pipe, report, url, args.account)
        command(pipe, report, "query_reference", dict(cmd="assert", what="value", selector="#query", equals=args.reference))
        command(pipe, report, "search_ready", dict(cmd="assert", what="value", selector="#search-state",
                                                   equals="ready", within=args.timeout))
        command(pipe, report, "unique_results", dict(cmd="assert", what="exists", selector="#results", count=1))
        result = command(pipe, report, "read_search", dict(cmd="eval", expression=READ_RESULTS)).get("result")
        if not isinstance(result, dict) or not isinstance(result.get("items"), list):
            raise TaskError("Search results do not have the expected shape", "uncertain")
        for name, expected in [("url", url), ("account", args.account), ("reference", args.reference)]:
            expect(report, "search_" + name, expected, result.get(name))
        expect(report, "search_count", integer(result.get("total"), "search total", 100), len(result["items"]))
        if len(result["items"]) > 1:
            raise TaskError("Several drafts have this reference; no record was chosen", "uncertain")
        if result["items"]:
            return verify_record(report, result["items"][0], args)
        if not wait or time.monotonic() >= deadline:
            return None
        time.sleep(min(0.2, max(0, deadline - time.monotonic())))


def create(args):
    started = time.monotonic()
    report = dict(status="error", outputs={}, checks=[], browser=args.browser, phase="inputs",
                  creation_attempted=False, journal=str(args.journal))
    pipes, record, prior_attempt = [], None, False
    try:
        inputs(args)
        identifier(args.reference, "reference")
        if (not isinstance(args.title, str) or not args.title.strip() or len(args.title) > 200
                or "\n" in args.title or "\r" in args.title):
            raise ValueError("title must contain 1–200 characters on one line and cannot be blank")
        request = dict(url=args.url, account=args.account, reference=args.reference,
                       title=args.title, amount_cents=cents(args.amount), currency="EUR")
        report["phase"] = "journal"
        with Journal(args.journal, request) as journal:
            prior_attempt = journal.state in ("attempted", "verified")
            report["prior_attempt"] = prior_attempt
            try:
                with Pipe(args.binary, args.browser, args.timeout) as pipe:
                    pipes.append(pipe)
                    record = find_draft(pipe, report, args)
                    if record is None and not prior_attempt:
                        open_page(pipe, report, args.url + "/drafts/new", args.account)
                        for name, value in [("reference", args.reference), ("title", args.title), ("amount", args.amount)]:
                            selector = "#" + name
                            command(pipe, report, "unique_" + name, dict(cmd="assert", what="exists", selector=selector, count=1))
                            response = command(pipe, report, "fill_" + name, dict(cmd="fill", selector=selector, value=value))
                            expect(report, "retained_" + name, True, response.get("value", {}).get("verbatim"))
                            command(pipe, report, "value_" + name, dict(cmd="assert", what="value", selector=selector, equals=value))
                        command(pipe, report, "current_account", dict(cmd="assert", what="value", selector="#account", equals=args.account))
                        command(pipe, report, "unique_create", dict(cmd="assert", what="exists", selector="#create", count=1))
                        command(pipe, report, "create_enabled", dict(cmd="assert", what="state", selector="#create", enabled=True))
                        report["phase"] = "journal_before_submit"
                        # Durable BEFORE dispatch. A crash between this write and the click
                        # leaves uncertainty; another invocation must not assume it can submit.
                        journal.save("attempted")
                        report["creation_attempted"] = True
                        report["phase"] = "submit"
                        report["submission"] = pipe.request(dict(cmd="click", selector="#create"))
                    report["phase"] = "pipe_finalization"
            except (TaskError, PipeError, OSError) as error:
                if not report["creation_attempted"]:
                    if prior_attempt and not any(c.get("name", "").startswith("draft_") and c.get("held") is False
                                                 for c in report["checks"]):
                        raise TaskError(str(error), "uncertain") from error
                    raise
                report["submission_error"] = str(error)
            if record is None:
                report["phase"] = "reconcile"
                # A new connection can recover after a lost pipe response. Every command in
                # this phase reads or opens the search page; the create button is never used.
                try:
                    with Pipe(args.binary, args.browser, args.timeout) as pipe:
                        pipes.append(pipe)
                        record = find_draft(pipe, report, args, wait=True)
                        report["phase"] = "pipe_finalization"
                except TaskError as error:
                    if any(c.get("name", "").startswith("draft_") and c.get("held") is False
                           for c in report["checks"]):
                        raise
                    raise TaskError(str(error), "uncertain") from error
            if record is None:
                raise TaskError("No draft was found after a possible submission; keep this journal and reconcile again", "uncertain")
            report["phase"] = "journal_verified"
            journal.save("verified", record)
            report.update(status="verified", outputs={"draft": record}, phase="complete",
                          resolution="found_after_attempt" if report["creation_attempted"] or prior_attempt else "already_present")
    except (TaskError, PipeError, OSError, ValueError) as error:
        status = getattr(error, "status", "error")
        if not isinstance(error, TaskError) and record is None and (prior_attempt or report["creation_attempted"]):
            status = "uncertain"
        report.update(status=status, error=str(error))
        if status == "uncertain":
            report["next"] = "Inspect drafts for this reference, then rerun with the same journal and inputs to reconcile. No automatic resubmission."
    report["submission_commands"] = sum(
        entry.get("command", {}).get("cmd") == "click" and entry["command"].get("selector") == "#create"
        for pipe in pipes for entry in pipe.history
    )
    return finish(report, started, pipes)


if __name__ == "__main__":
    cli = parser(__doc__)
    cli.add_argument("--reference", required=True, help="Stable reference for this operation across runs")
    cli.add_argument("--title", required=True)
    cli.add_argument("--amount", required=True, help="Nonnegative EUR amount, e.g. 12.50")
    cli.add_argument("--journal", required=True, help="Private local journal; reuse it to reconcile this operation")
    raise SystemExit(emit(create(cli.parse_args())))
