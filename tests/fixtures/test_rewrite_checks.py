"""Live rewrite assertions and missing-header regressions."""
import contextlib
import importlib
import io
import os
import sys
import unittest
from unittest.mock import patch

from fixtures.isolated_proxy import IsolatedProxy


class RewriteChecksTests(unittest.TestCase):
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
                    suite.section_rewrites()
                self.assertEqual(len(results), 3, results)
                self.assertTrue(all(r["status"] == ("FAIL" if missing else "PASS")
                                    for r in results), results)
        finally:
            suite.ECHO_SERVER.shutdown()
            suite.ECHO_SERVER.server_close()

    def test_missing_or_wrong_headers_fail(self):
        with patch.object(sys, "argv", ["e2e_protocol_test.py"]):
            suite = importlib.import_module("e2e_protocol_test")
        suite.start_echo_server()
        try:
            with IsolatedProxy() as proxy:
                def admin(method, path, **kwargs):
                    return proxy.client.request(method, proxy.url + "/admin" + path,
                                                timeout=5, **kwargs)

                for value in (None, "incorrect"):
                    results = []
                    def traffic(method, url, **kwargs):
                        response = proxy.client.request(method, url,
                            proxies={"http": proxy.url}, timeout=5, **kwargs)
                        if url.endswith("/rw-req-hdr-test"):
                            data = response.json()
                            data["headers"].pop("x-rewritten", None)
                            if value is not None:
                                data["headers"]["x-rewritten"] = value
                            import json
                            response._content = json.dumps(data).encode()
                        if url.endswith("/rw-resp-hdr-test"):
                            response.headers.pop("x-resp-rewritten", None)
                            if value is not None:
                                response.headers["x-resp-rewritten"] = value
                        return response
                    with patch.object(suite, "RESULTS", results), \
                         contextlib.redirect_stdout(io.StringIO()):
                        suite._run_rewrite_checks(admin, traffic)
                    self.assertEqual([r["status"] for r in results],
                                     ["FAIL", "FAIL", "PASS"], results)
        finally:
            suite.ECHO_SERVER.shutdown()
            suite.ECHO_SERVER.server_close()


if __name__ == "__main__":
    unittest.main()
