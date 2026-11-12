"""Verify breakpoint checks cannot mutate the primary proxy."""

import contextlib
import importlib
import io
import os
import json
import sys
import unittest
from unittest.mock import patch

from fixtures.isolated_proxy import IsolatedProxy


class BreakpointChecksTests(unittest.TestCase):
    def test_incorrect_released_responses_fail(self):
        suite = self.suite()
        suite.start_echo_server()
        try:
            for fault in ("status", "method", "path", "pending"):
                with self.subTest(fault=fault), IsolatedProxy() as proxy:
                    def admin(method, path, **kwargs):
                        response = proxy.client.request(method, proxy.url + "/admin" + path,
                                                        timeout=5, **kwargs)
                        if fault == "pending" and method == "POST" and path == "/breakpoints":
                            # Remove the hold rule to simulate a matcher that never pauses.
                            rule = response.json()
                            if rule["location"].get("path") == "/bp-hold-test":
                                proxy.client.delete(proxy.url + "/admin/breakpoints/" + rule["id"],
                                                    timeout=5).raise_for_status()
                        return response

                    def proxied(method, url, **kwargs):
                        response = proxy.client.request(method, url,
                                                        proxies={"http": proxy.url}, **kwargs)
                        if fault == "status":
                            response.status_code = 201
                        elif fault != "pending":
                            body = response.json()
                            body[fault] = "incorrect"
                            response._content = json.dumps(body).encode()
                        return response

                    results = []
                    with patch.object(suite, "RESULTS", results), \
                         contextlib.redirect_stdout(io.StringIO()):
                        suite._run_breakpoint_checks(admin, proxied)
                    hold = next(r for r in results if r["name"] == "Breakpoint hold request and release")
                    self.assertEqual(hold["status"], "FAIL", results)
                    expected = {"status": "status after release", "pending": "did not enter"}
                    self.assertIn(expected.get(fault, "wrong released"), hold["detail"])
        finally:
            suite.ECHO_SERVER.shutdown()
            suite.ECHO_SERVER.server_close()

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
