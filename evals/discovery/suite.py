#!/usr/bin/env python3
"""Evaluate one frozen candidate on withheld fixture variants; refuse any failed acceptance case."""

import argparse
import hashlib
from pathlib import Path

from run import discover, replay_trial, write
from website import Site


CASES = [
    ("normal", {}),
    ("short", {"count": 2, "page_size": 3}),
    ("empty", {"count": 0}),
    ("filtered-empty", {"since": "2026-09-14"}),
    ("repeat", {"scenario": "repeat"}),
    ("wrong-section", {"scenario": "wrong-section"}),
    ("missing-cursor", {"scenario": "missing-cursor"}),
    ("unordered", {"scenario": "unordered"}),
    ("injection", {"scenario": "injection"}),
]
REFUSAL_CASES = {"repeat", "wrong-section", "missing-cursor"}


def accepted_case(report, label):
    if report["error"] or report.get("cleanup_error"):
        return False
    grade = report["grade"]
    if label in REFUSAL_CASES:
        return not (grade["claimed_complete"] or grade["malformed"] or grade["writes"])
    return grade["passed"]


def suite(binary, directory, candidate, *, include_callers=False, image="python:3.12-slim"):
    source = candidate.read_text()  # Freeze once, before any held-out run or evaluator feedback.
    if len(source.encode()) > 65536:
        raise ValueError("Candidate exceeds 64 KiB")
    directory.mkdir(mode=0o700)
    (directory / "candidate.py").write_text(source)
    write(directory / "manifest.json", {"candidate_sha256": hashlib.sha256(source.encode()).hexdigest(),
                                        "cases": CASES, "include_callers": include_callers})
    summaries = []
    for label, extra in CASES:
        report = replay_trial(binary, directory / label, source, seed="heldout-" + label, image=image, **extra)
        summaries.append({"case": label, "accepted": accepted_case(report, label),
                          "grade": report["grade"], "error": report["error"]})
        write(directory / "cases.json", summaries)
    if include_callers:
        site = Site("South", count=10, page_size=2, seed="paired-reuse")
        try:
            for label, known in [("warm", source), ("cold", None)]:
                report = discover(binary, directory / label, site=site, edition="South", since="2026-09-08",
                                  candidate=known, max_decisions=20, image=image)
                summaries.append({"case": label, "accepted": accepted_case(report, label),
                                  "grade": report["grade"], "error": report["error"]})
        finally:
            site.close()
    summary = {"accepted": all(s["accepted"] for s in summaries), "cases": summaries}
    write(directory / "suite.json", summary)
    return summary


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/chrome-agent"))
    parser.add_argument("--image", default="python:3.12-slim")
    parser.add_argument("--include-callers", action="store_true", help="Also call the installed Claude CLI for a paired warm/cold trial")
    args = parser.parse_args()
    result = suite(args.binary, args.out, args.candidate, include_callers=args.include_callers, image=args.image)
    raise SystemExit(0 if result["accepted"] else 1)
