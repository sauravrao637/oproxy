"""Live access-rule transitions, regression detection and owned cleanup."""
import importlib
import sys
import unittest
from unittest.mock import patch

from fixtures.isolated_proxy import IsolatedProxy


class AccessChecksTests(unittest.TestCase):
    def test_transitions_faults_and_interruption(self):
        with patch.object(sys, "argv", ["e2e_protocol_test.py"]):
            suite = importlib.import_module("e2e_protocol_test")
        suite.start_echo_server()
        try:
            with IsolatedProxy() as proxy:
                def api(method, path, **kwargs):
                    return proxy.client.request(method, proxy.url + "/admin" + path,
                                                timeout=5, **kwargs)

                api("POST", "/access-rules", json={
                    "id": "", "name": "ua-access-preserved", "enabled": True,
                    "location": {"host": None, "path": "/preserved", "port": None,
                                 "protocol": None, "query": None, "methods": [], "mode": "glob"},
                    "action": "block",
                }).raise_for_status()
                initial = api("GET", "/access-rules").json()
                names = []
                for fault in (None, "not-blocked", "not-restored", "wrong-path", "interrupt"):
                    calls = []
                    def admin(method, path, **kwargs):
                        response = api(method, path, **kwargs)
                        if method == "POST":
                            names.append(kwargs["json"]["name"])
                            if fault == "interrupt":
                                raise KeyboardInterrupt("after creation")
                        return response

                    def traffic(method, url):
                        calls.append(url)
                        response = proxy.client.request(method, url, proxies={"http": proxy.url}, timeout=5)
                        if fault == "not-blocked" and len(calls) == 2:
                            response.status_code = 200
                        if fault == "not-restored" and len(calls) == 3:
                            response.status_code = 403
                        if fault == "wrong-path" and len(calls) == 3:
                            response._content = b'{"method":"GET","path":"/incorrect"}'
                        return response

                    with self.subTest(fault=fault):
                        if fault is None:
                            suite._check_access_block(admin, traffic)
                            self.assertEqual(len(calls), 3)
                        else:
                            with self.assertRaises(KeyboardInterrupt if fault == "interrupt" else AssertionError):
                                suite._check_access_block(admin, traffic)
                        self.assertEqual(api("GET", "/access-rules").json(), initial)
                self.assertEqual(len(set(names)), 5)
        finally:
            suite.ECHO_SERVER.shutdown()
            suite.ECHO_SERVER.server_close()


if __name__ == "__main__":
    unittest.main()
