"""Verify breakpoint checks cannot mutate the primary proxy."""

import contextlib
import importlib
import io
import os
import sys
import unittest
from unittest.mock import patch

from fixtures.isolated_proxy import IsolatedProxy


class BreakpointChecksTests(unittest.TestCase):
    def suite(self):
        with patch.object(sys, "argv", ["e2e_protocol_test.py"]):
            return importlib.import_module("e2e_protocol_test")

    def test_section_preserves_primary_rules(self):
        suite = self.suite()
        with IsolatedProxy() as primary:
            response = primary.client.post(primary.url + "/admin/breakpoints", json={
                "id": "", "enabled": True, "bp_type": "Request",
                "location": {"host": "preserved.example.com", "path": None,
                             "port": None, "protocol": None, "query": None,
                             "methods": [], "mode": "glob"},
            }, timeout=5)
            response.raise_for_status()
            initial = primary.client.get(primary.url + "/admin/breakpoints", timeout=5).json()
            results = []
            suite.start_echo_server()
            try:
                with patch.object(suite, "RESULTS", results), \
                     patch.object(suite, "admin", side_effect=AssertionError("primary admin used")), \
                     patch.object(suite, "proxied", side_effect=AssertionError("primary traffic used")), \
                     contextlib.redirect_stdout(io.StringIO()):
                    suite.section_breakpoints()
                self.assertEqual(len(results), 3, results)
                self.assertTrue(all(r["status"] == "PASS" for r in results), results)
                remaining = primary.client.get(primary.url + "/admin/breakpoints", timeout=5)
                remaining.raise_for_status()
                self.assertEqual(remaining.json(), initial)
            finally:
                suite.ECHO_SERVER.shutdown()
                suite.ECHO_SERVER.server_close()

    def test_missing_binary_fails_all_required_cases(self):
        suite = self.suite()
        results = []
        with patch.dict(os.environ, {"OPROXY_TEST_BINARY": __file__ + ".missing"}), \
             patch.object(suite, "RESULTS", results), \
             contextlib.redirect_stdout(io.StringIO()):
            suite.section_breakpoints()
        self.assertEqual(len(results), 3, results)
        self.assertTrue(all(r["status"] == "FAIL" for r in results), results)
        self.assertTrue(all("Missing tested binary" in r["detail"] for r in results))


if __name__ == "__main__":
    unittest.main()
