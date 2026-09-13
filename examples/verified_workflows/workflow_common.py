"""Small helpers shared by these two procedures, not a task language or scheduler."""

import argparse
import datetime
import json
from pathlib import Path
import re
import sys
import time
from urllib.parse import urlsplit, urlunsplit
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from pipe_client import Pipe, PipeError


class TaskError(Exception):
    def __init__(self, message, status="failed"):
        super().__init__(message)
        self.status = status


def parser(description):
    result = argparse.ArgumentParser(description=description)
    result.add_argument("--url", required=True, help="Reference application's base URL")
    result.add_argument("--account", required=True)
    result.add_argument("--binary", default="chrome-agent")
    result.add_argument("--browser", default="workflow-" + uuid.uuid4().hex[:12])
    result.add_argument("--timeout", type=int, default=3, help="Seconds per command and observation")
    return result


def identifier(value, name):
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,63}", value):
        raise ValueError(f"{name} must be a 1–64 character identifier using letters, digits, _ or -")
    return value


def inputs(args):
    identifier(args.account, "account")
    if type(args.timeout) is not int or not 1 <= args.timeout <= 120:
        raise ValueError("timeout must be between 1 and 120 seconds")
    url = urlsplit(args.url)
    if (url.scheme not in ("http", "https") or not url.hostname or url.username or url.password
            or url.query or url.fragment or url.path not in ("", "/")):
        raise ValueError("url must be an HTTP(S) origin, without credentials, query or fragment")
    url.port
    args.url = urlunsplit((url.scheme, url.netloc, "", "", ""))


def period(value):
    if not re.fullmatch(r"[0-9]{4}-[0-9]{2}", value):
        raise ValueError("period must have YYYY-MM form")
    datetime.date.fromisoformat(value + "-01")


def integer(value, name, maximum=1000000000):
    if not isinstance(value, str) or not re.fullmatch(r"0|[1-9][0-9]*", value) or len(value) > 12:
        raise TaskError(f"{name} must be a nonnegative integer")
    result = int(value)
    if result > maximum:
        raise TaskError(f"{name} exceeds the supported bound")
    return result


def cents(value):
    if not isinstance(value, str) or not re.fullmatch(r"[0-9]{1,7}\.[0-9]{2}", value):
        raise ValueError("amount must be a nonnegative decimal with two places, at most 9999999.99")
    return int(value.replace(".", ""))


def expect(report, name, expected, observed):
    held = type(expected) is type(observed) and expected == observed
    report["checks"].append(dict(name=name, held=held, expected=expected, observed=observed))
    if not held:
        raise TaskError(f"Check failed: {name}")


def command(pipe, report, phase, payload):
    report["phase"] = phase
    response = pipe.request(payload)
    report["last_response"] = response
    if response["ok"] is not True:
        raise TaskError(f"{phase}: {response.get('error', 'condition did not hold')}",
                        "failed" if "assertion" in response else "error")
    if "assertion" in response:
        report["checks"].append({"phase": phase, **response["assertion"]})
    return response


def open_page(pipe, report, url, account):
    command(pipe, report, "navigate", dict(cmd="goto", url=url))
    command(pipe, report, "page_url", dict(cmd="assert", what="url", equals=url))
    command(pipe, report, "unique_account", dict(cmd="assert", what="exists", selector="#account", count=1))
    command(pipe, report, "page_account", dict(cmd="assert", what="value", selector="#account", equals=account))


def finish(report, started, pipes):
    report["elapsed_ms"] = round((time.monotonic() - started) * 1000)
    report["command_count"] = sum(len(p.history) for p in pipes)
    if report["status"] == "verified":
        report.pop("last_response", None)
    return report


def emit(report):
    print(json.dumps(report, ensure_ascii=False))
    return {"verified": 0, "partial": 2, "failed": 2, "error": 1, "uncertain": 1}[report["status"]]
