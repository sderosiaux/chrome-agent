"""Independent grading, caller isolation and bounded evaluation processes. No model calls."""

import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.request import urlopen

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "evals" / "discovery"))
from caller import Caller, validate_events
from process import bounded_run
from run import Executor
from suite import accepted_case
from website import Site, evaluate


class OutcomeChecks(unittest.TestCase):
    def test_acceptance_refuses_false_success_and_infrastructure_failure(self):
        def report(result):
            return dict(grade=evaluate(result, [], 0), error=None, cleanup_error=None)
        partial = report(dict(complete=False, articles=[]))
        self.assertTrue(accepted_case(partial, "repeat"))
        self.assertFalse(accepted_case(partial, "normal"))
        complete = report(dict(complete=True, articles=[]))
        self.assertFalse(accepted_case(complete, "missing-cursor"))
        self.assertTrue(accepted_case(complete, "empty"))
        self.assertFalse(accepted_case(dict(complete, error="timeout"), "empty"))
        self.assertFalse(accepted_case(dict(complete, cleanup_error="container still running"), "empty"))

    def test_exact_fields_scope_duplicates_and_complete_are_independent(self):
        row = dict(id="one", title="Article", date="2026-09-13", section="North", url="http://site/article/one")
        self.assertTrue(evaluate(dict(complete=True, articles=[row]), [row], 0)["passed"])
        for rows in [[], [row, row], [dict(row, id="other")], [dict(row, section="South")],
                     [dict(row, date="2026-01-01")], [dict(row, title="Guess")],
                     [dict(row, url="http://other/article/one")], [dict(row, id=[])], {}, None]:
            with self.subTest(rows=rows):
                grade = evaluate(dict(complete=True, articles=rows), [row], 0)
                self.assertFalse(grade["passed"])
                self.assertTrue(grade["false_success"])
        self.assertFalse(evaluate(dict(complete=False, articles=[row]), [row], 0)["false_success"])
        self.assertTrue(evaluate(dict(complete=True, articles=[row]), [row], 1)["false_success"])

    def test_empty_is_valid_only_with_an_explicit_complete_result(self):
        self.assertTrue(evaluate(dict(complete=True, articles=[]), [], 0)["passed"])
        for result in [None, {}, dict(complete=True), dict(complete=1, articles=[]), dict(articles=[])]:
            self.assertFalse(evaluate(result, [], 0)["passed"])


class FixtureChecks(unittest.TestCase):
    def test_pages_never_serve_an_oracle_or_all_rows_at_once(self):
        site = Site("North", count=7, page_size=2, seed="test")
        self.addCleanup(site.close)
        with urlopen(site.base + "/", timeout=3) as response:
            page = response.read().decode()
        self.assertNotIn(site.rows[-1]["id"], page)
        for path in ["/state", "/oracle", "/website.py", "/../website.py", "/feed?cursor=guess"]:
            with self.subTest(path=path), self.assertRaises(HTTPError) as error:
                urlopen(site.base + path, timeout=3)
            self.assertEqual(error.exception.code, 404)
        with urlopen(site.base + "/feed?cursor=" + site.cursors[0], timeout=3) as response:
            data = json.load(response)
        self.assertEqual(data["rows"], site.rows[:2])
        self.assertEqual(data["cursor"], site.cursors[1])

    def test_reproducible_fixture_and_empty_archive(self):
        for count in [0, 7, 30]:
            first = Site("North", count=count, seed="reproducible")
            second = Site("North", count=count, seed="reproducible")
            try:
                self.assertEqual(first.rows, second.rows)
                self.assertEqual(first.cursors, second.cursors)
                self.assertIn(first.cursors[0], first.page())
            finally:
                first.close()
                second.close()
        for args in [dict(count=-1), dict(page_size=0), dict(scenario="typo")]:
            with self.assertRaises(ValueError):
                Site("North", **args)


def events():
    return [dict(type="system", subtype="init", tools=[], mcp_servers=[], skills=[], plugins=[], model="test-model"),
            dict(type="result", is_error=False, result='{"type":"finish"}',
                 usage={"output_tokens": 10}, modelUsage={}, total_cost_usd=0.01)]


class CallerChecks(unittest.TestCase):
    def test_isolation_gate_requires_all_surfaces_and_rejects_native_tools(self):
        validate_events(events())
        for key in ["tools", "mcp_servers", "skills", "plugins"]:
            for value in [None, ["unexpected"]]:
                changed = copy.deepcopy(events())
                changed[0][key] = value
                with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                    validate_events(changed)
        for changed in [events()[1:], events() + [events()[0]],
                        events() + [dict(type="assistant", message={"content": [{"type": "tool_use"}]})],
                        events() + [dict(type="assistant", message={"content": [None]})],
                        events() + [dict(type="assistant", message=None)],
                        [None]]:
            with self.assertRaises(ValueError):
                validate_events(changed)

    def test_bad_model_json_still_records_reported_cost_and_failed_attempt(self):
        for result in ["not json", "[]"]:
            stream = events()
            stream[1]["result"] = result
            completed = subprocess.CompletedProcess([], 0, "\n".join(map(json.dumps, stream)), "")
            with tempfile.TemporaryDirectory() as directory, patch("caller.bounded_run", return_value=completed):
                caller = Caller(Path(directory))
                with self.assertRaises(ValueError):
                    caller.ask({"task": "test"})
                self.assertEqual(len(caller.calls), 1)
                self.assertEqual(caller.calls[0]["reported_cost_usd"], 0.01)
                self.assertIn("error", caller.calls[0])

    def test_model_interruption_records_unknown_usage_and_does_not_retry(self):
        with tempfile.TemporaryDirectory() as directory, patch("caller.bounded_run", side_effect=OSError("interrupted")) as run:
            caller = Caller(Path(directory))
            with self.assertRaises(OSError):
                caller.ask({})
            self.assertEqual(run.call_count, 1)
            self.assertIsNone(caller.calls[0]["usage"])
            self.assertIn("error", caller.calls[0])


class ProcessChecks(unittest.TestCase):
    def test_interrupted_browser_dispatch_retains_the_attempt(self):
        with tempfile.TemporaryDirectory() as temporary:
            executor = Executor.__new__(Executor)
            executor.directory = Path(temporary)
            executor.file = executor.directory / "discovery.json"
            executor.calls = []
            with patch.object(executor, "cli", side_effect=subprocess.TimeoutExpired("browser", 1)):
                with self.assertRaises(subprocess.TimeoutExpired):
                    executor.step({"id": "one", "command": {"cmd": "inspect"}})
            stored = json.loads((executor.directory / "browser-calls.json").read_text())
            self.assertEqual(len(stored), 1)
            self.assertIsNone(stored[0]["result"])
            self.assertIn("error", stored[0])

    def test_drains_both_streams_and_delivers_input(self):
        result = bounded_run([sys.executable, "-c", "import sys;print(sys.stdin.read());print('error',file=sys.stderr)"], input="hello")
        self.assertEqual(result.stdout.strip(), "hello")
        self.assertEqual(result.stderr.strip(), "error")

    def test_excess_output_and_hangs_are_bounded(self):
        for stream in ["stdout", "stderr"]:
            with self.subTest(stream=stream), self.assertRaisesRegex(ValueError, "exceeded"):
                bounded_run([sys.executable, "-c", f"import sys;sys.{stream}.write('x'*100000)"], limit=1024)
        with self.assertRaises(subprocess.TimeoutExpired):
            bounded_run([sys.executable, "-c", "import time;time.sleep(5)"], timeout=0.1)


if __name__ == "__main__":
    unittest.main()
