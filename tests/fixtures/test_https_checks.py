"""Run with PYTHONPATH=tests python -m unittest discover -s tests/fixtures
-p test_https_checks.py -v. Live tests require the tested binary and curl.
"""

import copy
import json
import subprocess
import sys
import unittest
from unittest.mock import patch

from fixtures.https_checks import (check_local_https, completed_capture,
                                  request_https, validate_response, wait_capture)
from fixtures.https_origin import HttpsOrigin
from fixtures.isolated_proxy import IsolatedProxy


class HttpsChecksTests(unittest.TestCase):
    def test_stopped_origin_fails_after_success(self):
        with HttpsOrigin() as origin, IsolatedProxy("1") as proxy:
            path = "/echo?nonce=stop"
            validate_response(request_https(proxy, origin.url + path), path, "stop")
            origin.server.shutdown()
            origin.server.server_close()
            with self.assertRaises((AssertionError, ValueError)):
                validate_response(request_https(proxy, origin.url + path), path, "stop")

    def test_suppressed_capture_fails_after_forwarding_success(self):
        with HttpsOrigin() as origin, IsolatedProxy("1") as proxy:
            before = "/echo?nonce=before-filter"
            validate_response(request_https(proxy, origin.url + before), before, "before-filter")
            wait_capture(proxy, origin.url + before)
            response = proxy.client.get(proxy.url + "/admin/capture-filter", timeout=2)
            response.raise_for_status()
            original = response.json()
            try:
                response = proxy.client.post(proxy.url + "/admin/capture-filter",
                    json={"mode": "denylist", "hosts": ["127.0.0.1"]}, timeout=2)
                response.raise_for_status()
                path = "/echo?nonce=suppressed"
                validate_response(request_https(proxy, origin.url + path), path, "suppressed")
                with self.assertRaisesRegex(AssertionError, "No completed GET"):
                    wait_capture(proxy, origin.url + path, timeout=.5)
            finally:
                proxy.client.post(proxy.url + "/admin/capture-filter",
                                  json=original, timeout=2).raise_for_status()
            after = "/echo?nonce=after-filter"
            validate_response(request_https(proxy, origin.url + after), after, "after-filter")
            wait_capture(proxy, origin.url + after)

    def test_section_entrypoint_preserves_required_cases(self):
        import importlib
        from urllib.parse import urlsplit
        from fixtures.isolated_proxy import IsolatedProxy
        with patch.object(sys, "argv", ["e2e_protocol_test.py"]):
            suite = importlib.import_module("e2e_protocol_test")
        with IsolatedProxy("1") as primary, patch.object(suite, "RESULTS", []), \
             patch.object(suite, "BASE_URL", primary.url), \
             patch.object(suite, "PROXY_PORT", urlsplit(primary.url).port):
            suite.section_https_mitm()
            self.assertEqual(len(suite.RESULTS), 4)
            self.assertTrue(all(r["status"] == "PASS" for r in suite.RESULTS), suite.RESULTS)

    def test_capture_filter_section_uses_reachable_origin(self):
        import importlib
        with patch.object(sys, "argv", ["e2e_protocol_test.py"]):
            suite = importlib.import_module("e2e_protocol_test")
        with IsolatedProxy("1") as primary, patch.object(suite, "RESULTS", []), \
             patch.object(suite, "BASE_URL", primary.url), \
             patch.object(suite, "PROXY_URL", primary.url), \
             patch.object(suite, "ADMIN_TOKEN", primary.admin_token):
            suite.start_echo_server()
            try:
                suite.section_capture_filter()
                self.assertEqual(len(suite.RESULTS), 2)
                self.assertTrue(all(r["status"] == "PASS" for r in suite.RESULTS), suite.RESULTS)
            finally:
                suite.ECHO_SERVER.shutdown()
                suite.ECHO_SERVER.server_close()

    def test_live_response_and_capture_twice(self):
        first = check_local_https()
        second = check_local_https()
        self.assertIn("terminal capture=", first)
        self.assertNotEqual(first, second)

    def test_missing_curl_fails(self):
        with patch("fixtures.https_checks.shutil.which", return_value=None):
            with self.assertRaisesRegex(RuntimeError, "curl"):
                check_local_https()

    def test_response_regressions_fail(self):
        expected = {"method": "GET", "path": "/echo?nonce=abc", "nonce": "abc"}
        good = json.dumps(expected)
        validate_response(subprocess.CompletedProcess([], 0, good + "\n200", ""),
                          expected["path"], "abc")
        for code, body, status in [(0, good, "500"), (0, "not JSON", "200"),
                                   (0, good.replace("abc", "stale"), "200"),
                                   (28, good, "200"), (60, "", "000")]:
            with self.subTest(code=code, body=body, status=status):
                with self.assertRaises((AssertionError, ValueError)):
                    validate_response(subprocess.CompletedProcess(
                        [], code, body + "\n" + status, "injected failure"),
                        expected["path"], "abc")

    def test_capture_regressions_fail(self):
        url = "https://127.0.0.1:44321/echo?nonce=abc"
        good = {"request": {"method": "GET", "host": "127.0.0.1:44321", "uri": url},
                "response": {"status": 200}, "metrics": {"status_code": 200}}
        self.assertTrue(completed_capture(good, url))
        for field, key, value in [("request", "method", "CONNECT"),
                                  ("request", "host", "wrong-host"),
                                  ("request", "uri", url + "stale"),
                                  ("response", "status", 502),
                                  ("metrics", "status_code", 0)]:
            bad = copy.deepcopy(good)
            bad[field][key] = value
            self.assertFalse(completed_capture(bad, url))
        for field in ("response", "metrics"):
            bad = copy.deepcopy(good)
            bad[field] = None
            self.assertFalse(completed_capture(bad, url))

    def test_absent_capture_deadline_fails(self):
        from unittest.mock import Mock
        proxy = Mock(url="http://127.0.0.1:1234")
        proxy.client.get.return_value.json.return_value = {"sessions": []}
        with self.assertRaisesRegex(AssertionError, "No completed GET"):
            wait_capture(proxy, "https://127.0.0.1:443/echo?nonce=absent", timeout=.01)


if __name__ == "__main__":
    unittest.main()
