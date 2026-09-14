#!/usr/bin/env python3
"""Run an isolated caller against a website and grade its submitted result independently."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import secrets
import subprocess
import sys
import time

from caller import Caller
from process import bounded_run
from program import replay
from website import Site, evaluate


GUIDE = """Choose experiments yourself using the observed page, with no supplied selectors or path.
Respond with exactly one JSON object per decision:
{"type":"step","proposal":{"id":"unique-id","revision":0,"reason":"why this experiment",
 "command":{"cmd":"inspect"},"checks":[],"hypotheses":[],"unknowns":[]}}
or {"type":"finish","complete":true|false,"articles":[{"id":"...","title":"...",
 "date":"YYYY-MM-DD","section":"...","url":"absolute article URL"}],"reason":"...",
 "recipe_python":"Python source, or empty if you could not discover a reusable procedure"}.
Use the current revision from each executor reply. An identical proposal retrieves its old result;
a reused ID with changed content is refused. The first experiment must goto the exact entry_url.
Commands use the chrome-agent pipe JSON vocabulary. Available observation verbs: goto(url,
optional inspect:true), inspect(optional urls:true, filter, max_depth), text(optional selector),
read(optional html:true), extract(optional selector), assert, wait. Navigation stays within the
entry origin. No eval, shell, arbitrary file access or network client is available to you.
Assertions: {cmd:"assert",what:"exists",selector:CSS,count:N|min:N};
{cmd:"assert",what:"text",selector:CSS,contains:TEXT|matches:REGEX};
{cmd:"assert",what:"state",selector:CSS,enabled:true|disabled:true};
{cmd:"assert",what:"url",equals:URL|matches:REGEX}. Add within:3 to wait for a condition.
Text assertions do NOT support equals. Optional proposal.checks must all be assertion commands.
Result ok:true means the command ran, not that the dataset is complete. Verify identity, scope,
requested dates and termination. A partial dataset must not be submitted as complete.
When finished, also write a reusable Python program defining run(inputs), returning an object
with complete and articles. Inputs contain url, section, since. The program may import the
standard library and `from bridge import browser`. browser(command_dict) returns the raw
chrome-agent command response and raises on any failed command. It runs in a fresh browser,
with no conversation or old observations. The program has no network or host filesystem access.
It must handle different input values and page counts with bounded work, preserve result checks,
and report uncertainty rather than partial success. Do not hardcode observed rows or cursors.
This Python artifact is an evaluation candidate, not an installed or published product recipe.
"""


def write(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")
    path.chmod(0o600)


def provenance(binary, directory):
    root = Path(__file__).resolve().parents[2]
    head = bounded_run(["git", "rev-parse", "HEAD"], cwd=root, timeout=5)
    versions = {}
    for command in ["claude", "docker"]:
        try:
            result = bounded_run([command, "--version"], timeout=5, limit=4096)
            versions[command] = result.stdout.strip() if not result.returncode else None
        except (OSError, ValueError, subprocess.SubprocessError):
            versions[command] = None
    record = {"started_utc": datetime.now(timezone.utc).isoformat(), "git_head": head.stdout.strip(),
              "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "python": sys.version,
              "versions": versions, "harness_sha256": {
                  p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in Path(__file__).parent.glob("*.py")}}
    write(directory / "provenance.json", record)


class Executor:
    def __init__(self, binary, directory, inputs):
        self.binary = str(binary.resolve())
        self.directory = directory
        self.browser = "discovery-eval-" + secrets.token_hex(8)
        self.file = directory / "discovery.json"
        self.calls = []
        args = ["discover", "start", str(self.file), "--goal", "Collect all requested articles exactly once",
                "--url", inputs["url"], "--inputs", json.dumps(inputs), "--max-commands", "60", "--within", "1800"]
        self.initial = self.cli(args)
        if not self.initial.get("ok"):
            raise ValueError(self.initial)

    def cli(self, args, timeout=40):
        output = bounded_run([self.binary, "--json", "--browser", self.browser, *args],
                             timeout=timeout, limit=16 * 1024 * 1024)
        return json.loads(output.stdout)

    def step(self, proposal, timeout=40):
        if not isinstance(proposal, dict) or len(json.dumps(proposal).encode()) > 32768:
            raise ValueError("Invalid proposal envelope")
        began = time.monotonic()
        record = {"proposal": proposal, "result": None}
        self.calls.append(record)
        try:
            record["result"] = self.cli(["discover", "step", str(self.file), "--proposal", json.dumps(proposal)], timeout=timeout)
            return record["result"]
        except Exception as exc:
            record["error"] = str(exc)
            raise
        finally:
            record["duration_ms"] = round((time.monotonic() - began) * 1000)
            write(self.directory / "browser-calls.json", self.calls)

    def close(self):
        self.cli(["close", "--purge"])

    def metrics(self):
        state = json.loads(self.file.read_text())
        return {"proposals": len(self.calls), "reserved_commands": state["reserved_commands"],
                "observed_commands": sum(len(e["results"]) for e in state["experiments"])}


def cleanup(executor):
    try:
        if executor:
            executor.close()
        return None
    except (OSError, ValueError, subprocess.SubprocessError) as exc:
        return str(exc)


def replay_trial(binary, directory, source, *, count=10, page_size=2, edition="South",
                 since="2026-09-08", scenario="normal", seed=None, image="python:3.12-slim"):
    directory.mkdir(mode=0o700)
    provenance(binary, directory)
    site = Site(edition, count=count, page_size=page_size, scenario=scenario, seed=seed)
    inputs = {"url": site.base + "/", "section": edition, "since": since}
    executor, execution, error = None, {}, None
    began = time.monotonic()
    try:
        executor = Executor(binary, directory, inputs)
        execution = replay(source, inputs, executor, directory, image=image)
    except (OSError, ValueError, SyntaxError, subprocess.SubprocessError) as exc:
        error = str(exc)
        if (directory / "program-execution.json").exists():
            execution = json.loads((directory / "program-execution.json").read_text())
    finally:
        cleanup_error = cleanup(executor)
        site.close()
    cleanup_error = cleanup_error or execution.get("cleanup_error")
    grade = evaluate(execution.get("result"), site.expected(since), site.writes)
    report = {"trial": directory.name, "kind": "replay", "inputs": inputs,
              "fixture": site.config, "grade": grade, "error": error, "cleanup_error": cleanup_error,
              "execution": execution, "model_calls": [], "candidate_sha256": hashlib.sha256(source.encode()).hexdigest(),
              "browser": executor.metrics() if executor else {}, "expected": site.expected(since),
              "requests": site.requests, "duration_ms": round((time.monotonic() - began) * 1000)}
    write(directory / "report.json", report)
    print(json.dumps({"trial": directory.name, "grade": grade, "error": error}), flush=True)
    return report


def discover(binary, directory, *, count=7, max_decisions=16, knowledge=None, edition="North",
             since="2026-09-01", restart_after=None, site=None, candidate=None, image="python:3.12-slim"):
    directory.mkdir(mode=0o700)
    provenance(binary, directory)
    caller_dir = directory / "caller"
    caller_dir.mkdir(mode=0o700)
    owns_site = site is None
    site = site or Site(edition, count=count)
    request_start, writes_start = len(site.requests), site.writes
    inputs = {"url": site.base + "/", "section": edition, "since": since}
    executor = None
    caller = Caller(caller_dir)
    callers = [caller]
    decision = None
    error = None
    reuse_cleanup_error = None
    reused = False
    began = time.monotonic()
    try:
        executor = Executor(binary, directory, inputs)
        message = {"guide": GUIDE, "task": "Collect ALL articles in the requested section dated on or after since, without duplicates.",
                   "inputs": inputs, "state": executor.initial, "knowledge": knowledge}
        if candidate is not None:
            message["known_candidate"] = candidate
            message["guide"] += '\nYou may request {"type":"reuse"} once to run this exact candidate on the supplied inputs in this trial\'s browser, starting again from the entry URL. Assess the returned evidence before finishing.\n'
        for turn in range(max_decisions):
            if restart_after is not None and turn == restart_after:
                restarted = directory / "caller-restarted"
                restarted.mkdir(mode=0o700)
                caller = Caller(restarted)
                callers.append(caller)
                message = {"guide": GUIDE, "inputs": inputs,
                           "task": "Resume this discovery from its persisted record. Collect ALL requested articles and produce a reusable procedure.",
                           "state": executor.cli(["discover", "show", str(executor.file)])}
            print(json.dumps({"trial": directory.name, "decision": turn + 1}), flush=True)
            decision = caller.ask(message)
            write(caller.directory / "conversation.json", caller.history)
            write(directory / "model-calls.json", [call for c in callers for call in c.calls])
            if decision.get("type") == "finish":
                break
            if decision.get("type") == "reuse" and candidate is not None and not reused:
                reused = True
                try:
                    message = replay(candidate, inputs, executor, directory, image=image)
                    reuse_cleanup_error = message.get("cleanup_error")
                except (OSError, ValueError, SyntaxError, subprocess.SubprocessError) as exc:
                    message = {"error": str(exc)}
                    if (directory / "program-execution.json").exists():
                        message["execution"] = json.loads((directory / "program-execution.json").read_text())
                        reuse_cleanup_error = message["execution"].get("cleanup_error")
                write(directory / "reuse.json", message)
                continue
            if decision.get("type") != "step":
                message = {"error": "Expected step or finish; no browser command was executed"}
                continue
            message = executor.step(decision.get("proposal"))
        else:
            error = "Calling-agent decision budget exhausted"
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as exc:
        error = str(exc)
    finally:
        write(directory / "model-calls.json", [call for c in callers for call in c.calls])
        write(directory / "conversations.json", [c.history for c in callers])
        cleanup_error = cleanup(executor)
        cleanup_error = cleanup_error or reuse_cleanup_error
        if owns_site:
            site.close()
    expected = site.expected(since)
    grade = evaluate(decision, expected, site.writes - writes_start)
    if error:
        grade["passed"] = False
    report = {"trial": directory.name, "kind": "reuse-agent" if candidate else "discovery",
              "profile": "local_observation_v1", "inputs": inputs, "fixture": site.config,
              "grade": grade, "error": error, "cleanup_error": cleanup_error, "result": decision,
              "model_calls": [call for c in callers for call in c.calls], "restart_after": restart_after,
              "max_decisions": max_decisions, "candidate_reuse_requested": reused,
              "supplied_candidate_sha256": hashlib.sha256(candidate.encode()).hexdigest() if candidate else None,
              "browser": executor.metrics() if executor else {},
              "duration_ms": round((time.monotonic() - began) * 1000),
              "requests": site.requests[request_start:], "expected": expected}
    if isinstance(decision, dict) and isinstance(decision.get("recipe_python"), str) and decision["recipe_python"]:
        source = decision["recipe_python"]
        (directory / "candidate.py").write_text(source)
        (directory / "candidate.py").chmod(0o600)
        report["candidate_sha256"] = hashlib.sha256(source.encode()).hexdigest()
    write(directory / "report.json", report)
    print(json.dumps({"trial": directory.name, "grade": grade, "error": error}), flush=True)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/chrome-agent"))
    parser.add_argument("--count", type=int, default=7)
    parser.add_argument("--replay", type=Path, help="Replay this frozen Python candidate without a model")
    parser.add_argument("--page-size", type=int, default=3)
    parser.add_argument("--edition", default="North")
    parser.add_argument("--since", default="2026-09-01")
    parser.add_argument("--scenario", default="normal")
    parser.add_argument("--seed")
    parser.add_argument("--knowledge", type=Path, help="JSON observations from an earlier attempt; never evaluator answers")
    parser.add_argument("--max-decisions", type=int, default=16)
    parser.add_argument("--restart-after", type=int)
    args = parser.parse_args()
    if args.max_decisions < 1 or (args.restart_after is not None and not 0 < args.restart_after < args.max_decisions):
        parser.error("Decision budget must be positive; restart must fall strictly inside it")
    if args.replay:
        report = replay_trial(args.binary, args.out, args.replay.read_text(), count=args.count,
                              page_size=args.page_size, edition=args.edition, since=args.since,
                              scenario=args.scenario, seed=args.seed)
    else:
        site = Site(args.edition, count=args.count, page_size=args.page_size, scenario=args.scenario, seed=args.seed)
        try:
            report = discover(args.binary, args.out, max_decisions=args.max_decisions, site=site,
                              edition=args.edition, since=args.since, restart_after=args.restart_after,
                              knowledge=json.loads(args.knowledge.read_text()) if args.knowledge else None)
        finally:
            site.close()
    raise SystemExit(0 if report["grade"]["passed"] and not report["error"] and not report["cleanup_error"] else 1)


if __name__ == "__main__":
    main()
