"""Exercise Lua baseline assertions against live and deliberately wrong responses."""

import contextlib
import importlib
import io
import sys
import unittest
from unittest.mock import patch

import requests

from fixtures.isolated_proxy import IsolatedProxy


class LuaChecksTests(unittest.TestCase):
    def test_live_and_incorrect_responses(self):
        with patch.object(sys, "argv", ["e2e_protocol_test.py"]):
            suite = importlib.import_module("e2e_protocol_test")
        scenarios = (
            (None, None, None),
            ("/lua-abort-test", 200, "lua blocked this"),
            ("/lua-abort-test", 418, "lua blocked this extra"),
            ("/lua-header-test", 200, "missing header"),
            ("/lua-header-test", 502, "upstream failed"),
        )
        with IsolatedProxy() as proxy:
            for name in ("ua-test-noop", "ua-test-user", "probe-test"):
                seeded = proxy.client.post(proxy.url + "/admin/scripts", json={
                    "id": "", "name": name, "enabled": False, "code": "-- preserved",
                }, timeout=5)
                seeded.raise_for_status()
            initial = proxy.client.get(proxy.url + "/admin/scripts", timeout=5)
            initial.raise_for_status()
            original_scripts = initial.json()
            suite.start_echo_server()
            original = suite.proxied
            try:
                for path, status, body in scenarios:
                    with self.subTest(path=path, status=status, body=body):
                        results = []

                        def request(method, url, **kwargs):
                            if path and url.endswith(path):
                                response = requests.Response()
                                response.status_code = status
                                response._content = body.encode()
                                if status == 502:
                                    response.headers["x-lua-injected"] = "yes"
                                return response
                            return original(method, url, **kwargs)

                        with patch.object(suite, "BASE_URL", proxy.url), \
                             patch.object(suite, "PROXY_URL", proxy.url), \
                             patch.object(suite, "ADMIN_TOKEN", proxy.admin_token), \
                             patch.object(suite, "RESULTS", results), \
                             patch.object(suite, "proxied", request), \
                             contextlib.redirect_stdout(io.StringIO()):
                            suite.section_lua()
                        self.assertEqual(len(results), 5, results)
                        failures = [r for r in results if r["status"] != "PASS"]
                        if path is None:
                            self.assertEqual(failures, [], results)
                        else:
                            expected = ("Lua response header injection" if "header" in path
                                        else "Lua abort() blocks request with custom status")
                            self.assertEqual([r["name"] for r in failures], [expected])
                            self.assertEqual(failures[0]["status"], "FAIL")
                        remaining = proxy.client.get(proxy.url + "/admin/scripts", timeout=5)
                        remaining.raise_for_status()
                        self.assertEqual(remaining.json(), original_scripts)

                # An interruption outside normal assertion handling must still clean up.
                original_run = suite.run_test

                def interrupted(name, fn):
                    if name == "Lua script list includes created script":
                        raise KeyboardInterrupt("deliberate interruption")
                    original_run(name, fn)

                with patch.object(suite, "BASE_URL", proxy.url), \
                     patch.object(suite, "ADMIN_TOKEN", proxy.admin_token), \
                     patch.object(suite, "RESULTS", []), \
                     patch.object(suite, "run_test", interrupted), \
                     contextlib.redirect_stdout(io.StringIO()):
                    with self.assertRaises(KeyboardInterrupt):
                        suite.section_lua()
                remaining = proxy.client.get(proxy.url + "/admin/scripts", timeout=5)
                remaining.raise_for_status()
                self.assertEqual(remaining.json(), original_scripts)
            finally:
                suite.ECHO_SERVER.shutdown()
                suite.ECHO_SERVER.server_close()


if __name__ == "__main__":
    unittest.main()
