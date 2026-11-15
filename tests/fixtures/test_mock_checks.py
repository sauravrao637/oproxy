"""Live mock assertions and missing-header regressions."""
import contextlib
import importlib
import io
import os
import sys
import unittest
from unittest.mock import patch

from fixtures.isolated_proxy import IsolatedProxy


class MockChecksTests(unittest.TestCase):
    def test_isolation_and_missing_binary(self):
        with patch.object(sys, "argv", ["e2e_protocol_test.py"]):
            suite = importlib.import_module("e2e_protocol_test")
        suite.start_echo_server()
        try:
            for missing in (False, True):
                results = []
                env = {"OPROXY_TEST_BINARY": __file__ + ".missing"} if missing else {}
                with patch.dict(os.environ, env), patch.object(suite, "RESULTS", results), \
                     patch.object(suite, "admin", side_effect=AssertionError("primary admin used")), \
                     patch.object(suite, "proxied", side_effect=AssertionError("primary traffic used")), \
                     contextlib.redirect_stdout(io.StringIO()):
                    suite.section_mock()
                self.assertEqual(len(results), 2, results)
                self.assertTrue(all(r["status"] == ("FAIL" if missing else "PASS")
                                    for r in results), results)
        finally:
            suite.ECHO_SERVER.shutdown()
            suite.ECHO_SERVER.server_close()

    def test_interruption_cleans_only_owned_rules(self):
        with patch.object(sys, "argv", ["e2e_protocol_test.py"]):
            suite = importlib.import_module("e2e_protocol_test")
        with IsolatedProxy() as proxy:
            endpoint = proxy.url + "/admin/mock/rules"
            response = proxy.client.post(endpoint, json={
                "id": "", "name": "ua-mock-preserved", "enabled": True,
                "location": {"host": None, "path": "/preserved",
                             "port": None, "protocol": None, "query": None,
                             "methods": [], "mode": "glob"},
                "responses": [{"status": 202, "headers": {}, "body": "kept", "delay_ms": 0}],
            }, timeout=5)
            response.raise_for_status()
            initial = proxy.client.get(endpoint, timeout=5).json()
            names = []
            for _ in range(2):
                def admin(method, path, **kwargs):
                    response = proxy.client.request(method, proxy.url + "/admin" + path,
                                                    timeout=5, **kwargs)
                    response.raise_for_status()
                    if method == "POST":
                        names.append(kwargs["json"]["name"])
                        raise KeyboardInterrupt("interrupted after creation")
                    return response

                with contextlib.redirect_stdout(io.StringIO()), \
                     self.assertRaises(KeyboardInterrupt):
                    suite._run_mock_checks(admin, None)
                remaining = proxy.client.get(endpoint, timeout=5)
                remaining.raise_for_status()
                self.assertEqual(remaining.json(), initial)
            self.assertEqual(len(set(names)), 2)

    def test_wrong_status_body_or_headers_fail(self):
        with patch.object(sys, "argv", ["e2e_protocol_test.py"]):
            suite = importlib.import_module("e2e_protocol_test")
        suite.start_echo_server()
        try:
            with IsolatedProxy() as proxy:
                def admin(method, path, **kwargs):
                    return proxy.client.request(method, proxy.url + "/admin" + path,
                                                timeout=5, **kwargs)

                initial = admin("GET", "/mock/rules").json()
                for fault in ("status", "body", "missing-header", "wrong-header"):
                    results = []
                    def traffic(method, url, **kwargs):
                        response = proxy.client.request(method, url,
                            proxies={"http": proxy.url}, timeout=5, **kwargs)
                        if fault == "status":
                            response.status_code = 200
                        elif fault == "body":
                            response._content = b'{"mocked":true,"unexpected":true}'
                        elif fault == "missing-header":
                            response.headers.pop("x-mocked", None)
                        else:
                            response.headers["x-mocked"] = "incorrect"
                        return response
                    with patch.object(suite, "RESULTS", results), \
                         contextlib.redirect_stdout(io.StringIO()):
                        suite._run_mock_checks(admin, traffic)
                    self.assertEqual([r["status"] for r in results],
                                     ["PASS", "FAIL"], results)
                    self.assertEqual(admin("GET", "/mock/rules").json(), initial)
        finally:
            suite.ECHO_SERVER.shutdown()
            suite.ECHO_SERVER.server_close()


if __name__ == "__main__":
    unittest.main()
