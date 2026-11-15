"""Live persistence check and deletion regression injections."""

import unittest
import contextlib
import importlib
import io
import sys
from unittest.mock import MagicMock, patch

import requests

from fixtures.session_checks import check_session_deletion, wait_for_ids


class SessionDeletionTests(unittest.TestCase):
    def test_live_deletion_and_restart(self):
        self.assertIn("remain absent after restart", check_session_deletion())

    def test_baseline_reports_missing_binary_as_failure(self):
        with patch.object(sys, "argv", ["e2e_protocol_test.py"]):
            suite = importlib.import_module("e2e_protocol_test")
        results = []
        run_test = suite.run_test

        def only_deletion(name, test):
            if name == "Per-session delete or bulk clear":
                run_test(name, test)

        with patch.object(suite, "run_test", only_deletion), \
             patch.object(suite, "RESULTS", results), \
             patch("fixtures.session_checks.IsolatedProxy.__enter__",
                   side_effect=RuntimeError("Missing tested binary")), \
             contextlib.redirect_stdout(io.StringIO()):
            suite.section_misc()
        self.assertEqual(len(results), 1)
        self.assertEqual(results[0]["status"], "FAIL")
        self.assertIn("Missing tested binary", results[0]["detail"])

    def test_stale_sessions_fail(self):
        proxy = MagicMock()
        proxy.client.get.return_value.json.return_value = [{"id": "stale"}]
        with self.assertRaisesRegex(AssertionError, "Expected session IDs"):
            wait_for_ids(proxy, set(), timeout=0)

    def test_failed_bulk_delete_and_restored_data_fail(self):
        for fault in ("delete", "restart"):
            with self.subTest(fault=fault), \
                 patch("fixtures.session_checks.IsolatedProxy") as factory, \
                 patch("fixtures.session_checks.wait_for_ids") as wait:
                proxy = factory.return_value.__enter__.return_value
                proxy.client.post.return_value.json.return_value = {"imported": 1}
                unsupported, bulk = MagicMock(), MagicMock()
                unsupported.status_code = 404
                proxy.client.delete.side_effect = [unsupported, bulk]
                if fault == "delete":
                    bulk.raise_for_status.side_effect = requests.HTTPError("bulk failed")
                    expected = requests.HTTPError
                else:
                    wait.side_effect = [None, None, AssertionError("data restored")]
                    expected = AssertionError
                with self.assertRaises(expected):
                    check_session_deletion()
                self.assertEqual(proxy.client.delete.call_count, 2)


if __name__ == "__main__":
    unittest.main()
