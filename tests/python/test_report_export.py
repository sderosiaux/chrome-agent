"""Business checks and JSONL failure handling, without Chrome or third-party packages."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "examples" / "report_export"))
from export_report import TaskError, export_report, money, verify_csv
from pipe_client import Pipe, PipeError


HEADER = "account,period,invoice_id,amount,currency\n"
ROWS = "acme,2026-08,INV-801,12.50,EUR\nacme,2026-08,INV-802,7.25,EUR\n"


class ReportChecks(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="report-checks-")
        self.addCleanup(self.directory.cleanup)
        self.path = Path(self.directory.name) / "report.csv"
        self.summary = {"row_count": 2, "currency": "EUR", "total_cents": 1975}

    def verify(self, text):
        self.path.write_bytes(text.encode("utf-8"))
        return verify_csv(self.path, "acme", "2026-08", self.summary, [])

    def test_valid_csv_returns_metadata_tied_to_actual_bytes(self):
        result = self.verify("\ufeff" + HEADER + ROWS)
        self.assertEqual(result["row_count"], 2)
        self.assertEqual(result["bytes"], self.path.stat().st_size)
        self.assertEqual(len(result["sha256"]), 64)

    def test_wrong_scope_incomplete_data_and_invalid_content_are_not_verified(self):
        for text in ["", HEADER, "<html>Sign in</html>",
                     HEADER + ROWS.replace("acme", "other"),
                     HEADER + ROWS.replace("2026-08", "2026-07"),
                     HEADER + ROWS.replace("INV-802", "INV-801"),
                     HEADER + ROWS.splitlines()[0] + "\n",
                     HEADER + ROWS.replace("7.25", "7.26"),
                     HEADER + ROWS.replace("EUR", "USD"),
                     HEADER + ROWS.replace("7.25,EUR", "7.25"),
                     HEADER + ROWS.replace("7.25,EUR", "7.25,EUR,extra")]:
            with self.subTest(text=text), self.assertRaises(TaskError):
                self.verify(text)

    def test_amounts_cannot_use_nonfinite_or_ambiguous_numeric_forms(self):
        for value in ["NaN", "Infinity", "1e3", "1.001", "-5.00", "1,25", "1", None]:
            with self.subTest(value=value), self.assertRaises(TaskError):
                money(value)

    def test_invalid_inputs_do_not_even_start_the_pipe(self):
        base = dict(account="acme", period="2026-08", url="http://127.0.0.1:1234/",
                    out=str(self.path), timeout=1, browser="unused", binary="unused")
        for override in [{"period": "2026-13"}, {"period": "26-08"}, {"account": 'bad"id'},
                         {"url": "file:///etc/passwd"}, {"url": "http://user:password@example.com/"},
                         {"url": "http://127.0.0.1:invalid/"}, {"timeout": 0}]:
            with self.subTest(override=override), patch("export_report.Pipe") as pipe:
                result = export_report(SimpleNamespace(**(base | override)))
                self.assertEqual(result["status"], "error")
                self.assertEqual(result["stopped_at"], "inputs")
                pipe.assert_not_called()
        self.path.write_text("existing report")
        with patch("export_report.Pipe") as pipe:
            result = export_report(SimpleNamespace(**base))
            self.assertEqual(result["status"], "error")
            self.assertEqual(self.path.read_text(), "existing report")
            pipe.assert_not_called()


class PipeProtocol(unittest.TestCase):
    def peer(self, body):
        directory = tempfile.TemporaryDirectory(prefix="report-pipe-")
        self.addCleanup(directory.cleanup)
        script = Path(directory.name) / "peer.py"
        script.write_text("import sys, json, time\n" + body)
        original = subprocess.Popen
        replacement = patch("pipe_client.subprocess.Popen", side_effect=lambda args, **kwargs:
                            original([sys.executable, str(script)], **kwargs))
        replacement.start()
        self.addCleanup(replacement.stop)
        return Pipe("unused", "unused", 1)

    def test_normal_response_and_clean_eof(self):
        with self.peer('for line in sys.stdin:\n print(json.dumps({"ok":True}), flush=True)\n') as pipe:
            self.assertTrue(pipe.request({"cmd": "text"})["ok"])

    def test_terminal_failure_is_not_a_response_or_successful_finalization(self):
        terminal = 'print(json.dumps({"ok":False,"terminal":True,"error":"storage failed"}), flush=True)\n'
        with self.assertRaises(PipeError):
            with self.peer(terminal) as pipe:
                pipe.request({"cmd": "text"})
        body = 'for line in sys.stdin:\n print(json.dumps({"ok":True}), flush=True)\n' + terminal
        with self.assertRaisesRegex(PipeError, "storage failed"):
            with self.peer(body) as pipe:
                self.assertTrue(pipe.request({"cmd": "text"})["ok"])

    def test_lost_or_malformed_response_never_reissues_a_command(self):
        for body in ['sys.stdin.readline()\n', 'sys.stdin.readline()\nprint("invalid", flush=True)\n',
                     'sys.stdin.readline()\nprint(json.dumps({"ok":"true"}), flush=True)\n']:
            with self.subTest(body=body), self.assertRaises(PipeError):
                with self.peer(body) as pipe:
                    try:
                        pipe.request({"cmd": "download", "selector": "#export"})
                    finally:
                        self.assertEqual(len(pipe.history), 1)

    def test_silent_peer_is_bounded(self):
        with self.assertRaisesRegex(PipeError, "Timed out"):
            with self.peer('sys.stdin.readline()\ntime.sleep(10)\n') as pipe:
                pipe.deadline = 0.05
                pipe.request({"cmd": "text"})


if __name__ == "__main__":
    unittest.main()
