#!/usr/bin/env python3
"""Export and verify the reference app's CSV through chrome-agent pipe.

This is an ordinary program for one documented page and report format. Its browser operations
are all existing pipe commands; business checks and result assembly belong to the caller.
"""

import argparse
import csv
import datetime
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import tempfile
import time
from urllib.parse import urlsplit, urlunsplit
import uuid

from pipe_client import Pipe, PipeError


class TaskError(Exception):
    def __init__(self, message, status="failed"):
        super().__init__(message)
        self.status = status


def require(checks, name, expected, observed):
    held = type(expected) is type(observed) and expected == observed
    checks.append({"name": name, "held": held, "expected": expected, "observed": observed})
    if not held:
        raise TaskError(f"Check failed: {name}")


def money(value):
    """The reference report uses nonnegative amounts with exactly two decimal places."""
    if not isinstance(value, str) or not re.fullmatch(r"[0-9]+\.[0-9]{2}", value):
        raise TaskError("Report amount must be a nonnegative decimal with two places")
    whole, fraction = value.split(".")
    try:
        return int(whole) * 100 + int(fraction)
    except ValueError as error:
        raise TaskError("Report amount exceeds supported integer precision") from error


def verify_csv(path, account, period, summary, checks):
    raw = path.read_bytes()
    require(checks, "file_nonempty", True, bool(raw))
    try:
        reader = csv.DictReader(io.StringIO(raw.decode("utf-8-sig"), newline=""), strict=True)
        require(checks, "columns", ["account", "period", "invoice_id", "amount", "currency"], reader.fieldnames)
        rows = list(reader)
    except (UnicodeError, csv.Error) as error:
        raise TaskError("Downloaded file is not a valid UTF-8 CSV") from error
    require(checks, "rows_nonempty", True, bool(rows))
    require(checks, "row_shape", True, all(
        None not in row and all(value is not None for value in row.values()) for row in rows
    ))
    require(checks, "file_account", [account], sorted({row["account"] for row in rows}))
    require(checks, "file_period", [period], sorted({row["period"] for row in rows}))
    ids = [row["invoice_id"] for row in rows]
    require(checks, "invoice_ids_present_and_unique", True, all(bool(i.strip()) for i in ids) and len(set(ids)) == len(ids))
    require(checks, "file_currency", [summary["currency"]], sorted({row["currency"] for row in rows}))
    require(checks, "file_row_count", summary["row_count"], len(rows))
    total = sum(money(row["amount"]) for row in rows)
    require(checks, "file_total_cents", summary["total_cents"], total)
    return {"row_count": len(rows), "bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}


def inputs(args):
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,63}", args.account):
        raise ValueError("account must be a 1–64 character identifier (letters, digits, underscore, hyphen)")
    if not re.fullmatch(r"[0-9]{4}-[0-9]{2}", args.period):
        raise ValueError("period must have YYYY-MM form")
    datetime.date.fromisoformat(args.period + "-01")
    if not 1 <= args.timeout <= 120:
        raise ValueError("timeout must be between 1 and 120 seconds")
    url = urlsplit(args.url)
    if url.scheme not in ("http", "https") or not url.hostname or url.username or url.password or url.query or url.fragment:
        raise ValueError("url must be an HTTP(S) page URL without credentials, query or fragment")
    url.port  # Validate a supplied port before starting the browser.
    args.url = urlunsplit((url.scheme, url.netloc, url.path or "/", "", ""))
    args.out = Path(os.path.abspath(os.path.expanduser(args.out)))
    if os.path.lexists(args.out):
        raise ValueError("Destination already exists; choose a new file (nothing was exported)")
    if not args.out.parent.is_dir():
        raise ValueError("Destination directory must already exist")


def export_report(args):
    started = time.monotonic()
    checks = []
    report = {"status": "error", "outputs": {}, "checks": checks, "browser": args.browser,
              "export_attempted": False}
    stage = None
    pipe = None
    phase = "inputs"
    try:
        inputs(args)
        # Staging on the destination filesystem permits atomic, non-overwriting publication.
        stage = Path(tempfile.mkdtemp(prefix=".report-export-", dir=args.out.parent))
        candidate = stage / "download.csv"

        def command(name, value):
            nonlocal phase
            phase = name
            response = pipe.request(value)
            if response["ok"] is not True:
                if "assertion" in response:
                    observed = response["assertion"]
                    checks.append({**observed, "name": name})
                    raise TaskError(f"Page check failed: {name}")
                raise TaskError(f"Command failed: {name}: {response.get('error', 'unknown error')}", "error")
            if "assertion" in response:
                checks.append({**response["assertion"], "name": name})
            return response

        with Pipe(args.binary, args.browser, args.timeout) as pipe:
            command("open_report", {"cmd": "goto", "url": args.url})
            command("page_url", {"cmd": "assert", "what": "url", "equals": args.url})
            for selector in ("#account", "#period"):
                command("unique_" + selector[1:], {"cmd": "assert", "what": "exists", "selector": selector, "count": 1})
            command("page_account", {"cmd": "assert", "what": "value", "selector": "#account", "equals": args.account})
            selected = command("select_period", {"cmd": "select", "selector": "#period", "value": args.period})
            require(checks, "period_retained", True, selected.get("value", {}).get("verbatim"))
            ready = f'#report-summary[data-ready="true"][data-account="{args.account}"][data-period="{args.period}"]'
            command("wait_for_report", {"cmd": "wait", "selector": ready, "timeout": args.timeout})
            command("unique_report", {"cmd": "assert", "what": "exists", "selector": ready, "count": 1})
            metadata = command("read_report_summary", {"cmd": "eval", "expression":
                "(() => { const d = document.querySelector('#report-summary').dataset; "
                "return {account:d.account,period:d.period,row_count:Number(d.rowCount),"
                "total_cents:Number(d.totalCents),currency:d.currency}; })()"})
            summary = metadata.get("result")
            require(checks, "summary_shape", True, isinstance(summary, dict))
            require(checks, "summary_account", args.account, summary.get("account"))
            require(checks, "summary_period", args.period, summary.get("period"))
            require(checks, "summary_row_count", True, type(summary.get("row_count")) is int and 0 < summary["row_count"] <= 2**53 - 1)
            require(checks, "summary_total", True, type(summary.get("total_cents")) is int and 0 <= summary["total_cents"] <= 2**53 - 1)
            require(checks, "summary_currency", True, isinstance(summary.get("currency"), str) and bool(re.fullmatch(r"[A-Z]{3}", summary["currency"])))
            command("current_account", {"cmd": "assert", "what": "value", "selector": "#account", "equals": args.account})
            command("current_period", {"cmd": "assert", "what": "value", "selector": "#period", "equals": args.period})
            command("unique_export", {"cmd": "assert", "what": "exists", "selector": "#export", "count": 1})
            command("export_enabled", {"cmd": "assert", "what": "state", "selector": "#export", "enabled": True})
            phase = "download"
            report["export_attempted"] = True
            # One dispatch, never retried, even if its response is lost.
            receipt = pipe.request({"cmd": "download", "selector": "#export", "out": str(candidate),
                                    "timeout": args.timeout, "max_bytes": 5 * 1024 * 1024})
            report["download"] = receipt
            if receipt.get("dispatched") is False:
                raise TaskError("Export control was not dispatched")
            if receipt.get("ok") is not True or receipt.get("downloaded") is not True:
                raise TaskError("Export was attempted but no completed file was confirmed", "uncertain")
            require(checks, "download_path", str(candidate), receipt.get("path"))
            phase = "pipe_finalization"

        phase = "verify_file"
        verified = verify_csv(candidate, args.account, args.period, summary, checks)
        phase = "publish_file"
        os.link(candidate, args.out)  # EEXIST is an error, including a concurrent destination writer.
        report.update(status="verified", outputs={"file": str(args.out), "account": args.account,
                       "period": args.period, **verified})
        published_stage = stage
        stage = None
        try:
            shutil.rmtree(published_stage)
        except OSError as error:
            # The verified output is already published. A leftover staging link is a
            # cleanup problem, not grounds to export again or erase the successful result.
            report["cleanup_error"] = {"directory": str(published_stage), "error": str(error)}
    except TaskError as error:
        report.update(status=error.status, error=str(error), stopped_at=phase)
    except PipeError as error:
        report.update(status="uncertain" if report["export_attempted"] else "error", error=str(error), stopped_at=phase)
    except (OSError, ValueError) as error:
        report.update(status="error", error=str(error), stopped_at=phase)
    finally:
        if stage is not None:
            candidate = stage / "download.csv"
            if candidate.exists():
                report["artifact"] = {"file": str(candidate), "published": False}
            else:
                shutil.rmtree(stage, ignore_errors=True)
        if pipe is not None:
            report["commands_sent"] = len(pipe.history)
            if report["status"] != "verified" and pipe.history:
                report["last_command"] = pipe.history[-1]
        report["elapsed_ms"] = round((time.monotonic() - started) * 1000)
        if report["status"] == "uncertain":
            report["next"] = "Inspect the export state in the named browser before attempting another export."
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--account", required=True)
    parser.add_argument("--period", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--browser", default="report-" + uuid.uuid4().hex[:12])
    parser.add_argument("--binary", default="chrome-agent")
    parser.add_argument("--timeout", type=int, default=5, help="Per-browser-command timeout, seconds (1–120)")
    args = parser.parse_args()
    result = export_report(args)
    print(json.dumps(result, ensure_ascii=False))
    return {"verified": 0, "failed": 2, "error": 1, "uncertain": 1}[result["status"]]


if __name__ == "__main__":
    raise SystemExit(main())
