"""Data consistency, journal ownership and the durable boundary before a create command."""

import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "examples" / "verified_workflows"))
from collect_invoices import collect, merge_page, read_rows
from create_draft import create
from draft_journal import Journal
from workflow_common import PipeError, TaskError


class CollectionChecks(unittest.TestCase):
    def test_identical_overlap_is_accepted_and_conflicts_are_not_committed(self):
        row = dict(id="INV-1", account="acme", period="2026-08", amount_cents="1250", currency="EUR")
        parsed = read_rows([row], "acme", "2026-08")
        self.assertEqual(parsed[0]["amount_cents"], 1250)
        prior = {"INV-1": parsed[0]}
        self.assertEqual(merge_page(prior, parsed), (prior, 1))
        with self.assertRaises(TaskError):
            merge_page(prior, [dict(parsed[0], amount_cents=1251)])
        self.assertEqual(prior["INV-1"]["amount_cents"], 1250)

    def test_bad_row_data_is_not_returned_as_typed_records(self):
        good = dict(id="INV-1", account="acme", period="2026-08", amount_cents="1250", currency="EUR")
        for row in [None, {}, dict(good, account="other"), dict(good, period="2026-09"),
                    dict(good, currency="USD"), dict(good, id=""), dict(good, amount_cents="NaN"),
                    dict(good, amount_cents="1.5"), dict(good, amount_cents=True), dict(good, extra="ignored?")]:
            with self.subTest(row=row), self.assertRaises(TaskError):
                read_rows([row], "acme", "2026-08")

    def test_bad_parameters_do_not_open_chrome(self):
        base = dict(url="http://127.0.0.1:1234", account="acme", period="2026-08", timeout=1,
                    max_pages=20, max_rows=100, browser="unused", binary="unused")
        for override in [dict(period="2026-13"), dict(max_pages=0), dict(max_rows=0),
                         dict(url="file:///etc/passwd"), dict(url="http://u:p@example.com"), dict(account="bad/id")]:
            with self.subTest(override=override), patch("collect_invoices.Pipe") as pipe:
                result = collect(SimpleNamespace(**(base | override)))
                self.assertEqual(result["status"], "error")
                self.assertFalse(result["complete"])
                pipe.assert_not_called()


class DraftJournalChecks(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="draft-journal-tests-")
        self.addCleanup(directory.cleanup)
        self.path = Path(directory.name) / "operation.json"
        self.request = dict(url="http://127.0.0.1:1234", account="acme", reference="run-1",
                            title="Test", amount_cents=1250, currency="EUR")

    def test_state_survives_restart_and_the_same_journal_excludes_concurrent_runs(self):
        with Journal(self.path, self.request) as journal:
            self.assertEqual(journal.state, "prepared")
            journal.save("attempted")
            with self.assertRaises(BlockingIOError):
                with Journal(self.path, self.request):
                    self.fail("concurrent run acquired the same operation")
        with Journal(self.path, self.request) as journal:
            self.assertEqual(journal.state, "attempted")
        self.assertEqual(self.path.stat().st_mode & 0o777, 0o600)
        self.assertEqual(Path(str(self.path) + ".lock").stat().st_mode & 0o777, 0o600)

    def test_a_different_request_corrupt_file_and_symlink_are_refused(self):
        with Journal(self.path, self.request):
            pass
        original = self.path.read_bytes()
        with self.assertRaisesRegex(ValueError, "exact request"):
            with Journal(self.path, dict(self.request, title="Different")):
                pass
        self.assertEqual(self.path.read_bytes(), original)
        for raw in [b"broken", b"[]", json.dumps(dict(schema=True, request=self.request, state="prepared")).encode()]:
            self.path.write_bytes(raw)
            with self.assertRaises(ValueError):
                with Journal(self.path, self.request):
                    pass
            self.assertEqual(self.path.read_bytes(), raw)
        self.path.unlink()
        other = self.path.parent / "other.json"
        other.write_bytes(original)
        self.path.symlink_to(other)
        with self.assertRaises(OSError):
            with Journal(self.path, self.request):
                pass
        self.assertEqual(other.read_bytes(), original)

    def args(self):
        return SimpleNamespace(url=self.request["url"], account="acme", reference="run-1", title="Test",
                               amount="12.50", journal=str(self.path), timeout=1, browser="unused", binary="unused")

    def test_failed_journal_write_prevents_submission(self):
        commands = []
        class Peer:
            def __init__(self, *args):
                self.history = []
            def __enter__(self):
                return self
            def __exit__(self, *args):
                pass
            def request(self, command):
                commands.append(command)
                self.history.append(command)
                if command["cmd"] == "eval":
                    return dict(ok=True, result=dict(url="http://127.0.0.1:1234/drafts/search?reference=run-1",
                                                    account="acme", reference="run-1", total="0", items=[]))
                return dict(ok=True, value=dict(verbatim=True))
        save = Journal.save
        def unavailable(journal, state, record=None):
            if state == "attempted":
                raise OSError("disk write failed")
            save(journal, state, record)
        with patch("create_draft.Pipe", Peer), patch.object(Journal, "save", unavailable):
            report = create(self.args())
        self.assertEqual(report["status"], "error")
        self.assertFalse(report["creation_attempted"])
        self.assertFalse(any(c["cmd"] == "click" for c in commands))
        self.assertEqual(json.loads(self.path.read_text())["state"], "prepared")

    def test_invalid_draft_inputs_leave_no_journal_and_no_browser(self):
        for override in [dict(title=""), dict(title="two\nlines"), dict(reference="../bad"),
                         dict(amount="NaN"), dict(amount="-1.00"), dict(amount="1.001")]:
            args = self.args()
            for key, value in override.items():
                setattr(args, key, value)
            with self.subTest(override=override), patch("create_draft.Pipe") as pipe:
                self.assertEqual(create(args)["status"], "error")
                pipe.assert_not_called()
                self.assertFalse(self.path.exists())

    def test_lost_pipe_after_submit_uses_a_new_connection_to_read_back_the_record(self):
        owner, commands, connections = self, [], []
        committed = []
        class Peer:
            def __init__(self, *args):
                self.history = []
                connections.append(self)
            def __enter__(self):
                return self
            def __exit__(self, *args):
                pass
            def request(self, command):
                commands.append(command)
                self.history.append({"command": command})
                if command["cmd"] == "click":
                    owner.assertEqual(json.loads(owner.path.read_text())["state"], "attempted")
                    committed.append(dict(id="DRAFT-1", account="acme", reference="run-1", title="Test",
                                          amount_cents="1250", currency="EUR", status="draft"))
                    raise PipeError("connection lost after dispatch")
                if command["cmd"] == "eval":
                    return dict(ok=True, result=dict(url="http://127.0.0.1:1234/drafts/search?reference=run-1",
                                                    account="acme", reference="run-1", total=str(len(committed)), items=committed))
                return dict(ok=True, value=dict(verbatim=True))
        with patch("create_draft.Pipe", Peer):
            report = create(self.args())
        self.assertEqual(report["status"], "verified", report)
        self.assertEqual(report["outputs"]["draft"]["id"], "DRAFT-1")
        self.assertEqual(report["submission_commands"], 1)
        self.assertEqual(len(connections), 2)
        self.assertFalse(any(entry["command"]["cmd"] == "click" for entry in connections[1].history))
        self.assertEqual(json.loads(self.path.read_text())["state"], "verified")


if __name__ == "__main__":
    unittest.main()
