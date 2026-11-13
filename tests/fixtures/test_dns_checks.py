"""Local DNS redirects must fail on wrong responses and restore owned state."""
import importlib
import json
import sys
import unittest
from unittest.mock import patch

from fixtures.isolated_proxy import IsolatedProxy


class DnsChecksTests(unittest.TestCase):
    def test_crud_redirect_faults_and_cleanup(self):
        with patch.object(sys, "argv", ["e2e_protocol_test.py"]):
            suite = importlib.import_module("e2e_protocol_test")
        suite.start_echo_server()
        try:
            with IsolatedProxy() as proxy:
                def api(method, path, **kwargs):
                    return proxy.client.request(method, proxy.url + "/admin" + path,
                                                timeout=5, **kwargs)
                original = {"preserved.test": "127.0.0.2"}
                api("POST", "/dns", json=original).raise_for_status()
                hosts = []
                for fault in ("crud", None, "wrong-map", "status", "method", "path", "interrupt"):
                    def admin(method, path, **kwargs):
                        response = api(method, path, **kwargs)
                        if method == "POST":
                            host = next(key for key in kwargs["json"] if key not in original)
                            hosts.append(host)
                            if fault == "interrupt":
                                raise KeyboardInterrupt("after creation")
                        if fault == "wrong-map" and method == "GET":
                            data = response.json()
                            if len(data) > 1:
                                data[hosts[-1]] = "127.0.0.2"
                                response._content = json.dumps(data).encode()
                        return response

                    def traffic(method, url):
                        response = proxy.client.request(method, url, proxies={"http": proxy.url}, timeout=5)
                        if fault == "status":
                            response.status_code = 502
                        elif fault in ("method", "path"):
                            data = response.json()
                            data[fault] = "incorrect"
                            response._content = json.dumps(data).encode()
                        return response

                    with self.subTest(fault=fault):
                        if fault in (None, "crud"):
                            suite._check_dns(admin, traffic if fault is None else None)
                        else:
                            with self.assertRaises(KeyboardInterrupt if fault == "interrupt" else AssertionError):
                                suite._check_dns(admin, traffic)
                        self.assertEqual(api("GET", "/dns").json(), original)
                self.assertEqual(len(set(hosts)), 7)
        finally:
            suite.ECHO_SERVER.shutdown()
            suite.ECHO_SERVER.server_close()


if __name__ == "__main__":
    unittest.main()
