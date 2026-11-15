"""Exercise authentication baseline cases on the actual tested binary."""

import contextlib
import importlib
import io
import sys
import unittest
from unittest.mock import patch

from fixtures.isolated_proxy import IsolatedProxy


class AdminSectionTests(unittest.TestCase):
    def test_missing_binary_fails_all_required_auth_cases(self):
        with patch.object(sys, "argv", ["e2e_protocol_test.py"]):
            suite = importlib.import_module("e2e_protocol_test")
        results = []
        with patch("fixtures.isolated_proxy.IsolatedProxy.__enter__",
                   side_effect=RuntimeError("Missing tested binary")), \
             patch.object(suite, "RESULTS", results), \
             contextlib.redirect_stdout(io.StringIO()):
            suite.section_admin_token()
        self.assertEqual(len(results), 6)
        self.assertTrue(all(r["status"] == "FAIL" for r in results), results)

    def test_authenticated_and_open_primary_profiles(self):
        with patch.object(sys, "argv", ["e2e_protocol_test.py"]):
            suite = importlib.import_module("e2e_protocol_test")
        for open_admin in (False, True):
            with self.subTest(open_admin=open_admin), IsolatedProxy(open_admin=open_admin) as primary:
                results = []
                with patch.object(suite, "BASE_URL", primary.url), \
                     patch.object(suite, "ADMIN_TOKEN", primary.admin_token), \
                     patch.object(suite, "RESULTS", results), \
                     contextlib.redirect_stdout(io.StringIO()):
                    suite.section_admin_token()
                self.assertEqual(len(results), 6, results)
                self.assertEqual({r["name"] for r in results}, set(suite.REQUIRED_CASES[23:29]))
                self.assertTrue(all(r["status"] == "PASS" for r in results), results)


if __name__ == "__main__":
    unittest.main()
