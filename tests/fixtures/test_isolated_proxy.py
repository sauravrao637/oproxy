"""Live checks: set OPROXY_TEST_BINARY to the exact locally built executable.

Run with python -m unittest discover -s tests/fixtures -p test_isolated_proxy.py -v.
Missing runtime prerequisites are errors, never skipped coverage.
"""

import json
import os
from pathlib import Path
import shutil
import subprocess
import unittest

from https_origin import HttpsOrigin
from isolated_proxy import IsolatedProxy


class TrustBoundaryTests(unittest.TestCase):
    def request(self, origin, proxy=None, trusted=True):
        curl = shutil.which("curl")
        self.assertIsNotNone(curl, "Missing prerequisite: curl on PATH")
        args = [curl, "-sS", "--noproxy", "", "--http1.1", "--max-time", "8",
                "--write-out", "\n%{http_code}"]
        if proxy:
            args += ["--proxy", proxy.url]
        if trusted:
            args += ["--cacert", str(proxy.ca_path if proxy else origin.ca_path)]
        return subprocess.run(args + [origin.url + "/echo?nonce=trust-check"],
                              capture_output=True, text=True, timeout=12)

    def test_default_rejection_and_explicit_override(self):
        inherited = dict(os.environ)
        with HttpsOrigin() as origin:
            direct = self.request(origin)
            self.assertEqual(direct.returncode, 0, direct.stderr)
            self.assertTrue(direct.stdout.endswith("\n200"), direct.stdout)
            for value in (None, "0", "false", "invalid", "1"):
                with self.subTest(upstream_override=value):
                    with IsolatedProxy(value) as proxy:
                        root = Path(proxy.directory.name)
                        process = proxy.process
                        result = self.request(origin, proxy)
                        self.assertEqual(result.returncode, 0, result.stderr)
                        body, status = result.stdout.rsplit("\n", 1)
                        if value == "1":
                            self.assertEqual(status, "200", body)
                            self.assertEqual(json.loads(body), {
                                "method": "GET", "path": "/echo?nonce=trust-check",
                                "nonce": "trust-check"})
                        else:
                            self.assertEqual(status, "502", body)
                    self.assertIsNotNone(process.poll())
                    self.assertFalse(root.exists())
        self.assertEqual(dict(os.environ), inherited)

    def test_untrusted_client_rejected(self):
        with HttpsOrigin() as origin, IsolatedProxy("1") as proxy:
            result = self.request(origin, proxy, trusted=False)
            self.assertEqual(result.returncode, 60, result.stderr)


class AuthenticationLifecycleTests(unittest.TestCase):
    def test_token_changes_require_restart(self):
        import requests
        with IsolatedProxy(admin_token="token-a") as proxy:
            ca = proxy.ca_path.read_bytes()
            storage_marker = proxy.root / "storage" / "lifecycle-marker"
            storage_marker.parent.mkdir(exist_ok=True)
            storage_marker.write_text("persist", encoding="utf-8")
            with requests.Session() as client:
                client.trust_env = False
                def status(token):
                    headers = {"x-oproxy-admin-token": token} if token else {}
                    return client.get(proxy.url + "/admin/sessions",
                                      headers=headers, timeout=3).status_code
                self.assertEqual(status("token-a"), 200)
                self.assertEqual(status("token-b"), 401)
                self.assertEqual(status(None), 401)
                proxy.configure(admin_token="token-b")
                response = proxy.client.post(proxy.url + "/admin/config/reload", timeout=3)
                self.assertEqual(response.status_code, 200, response.text)
                self.assertTrue(response.json()["reloaded"])
                self.assertEqual(status("token-a"), 200)
                self.assertEqual(status("token-b"), 401)
                old_process = proxy.process
                proxy.restart()
                self.assertIsNotNone(old_process.poll())
                self.assertEqual(status("token-a"), 401)
                self.assertEqual(status("token-b"), 200)
                self.assertEqual(status(None), 401)
                self.assertEqual(proxy.ca_path.read_bytes(), ca)
                self.assertEqual(storage_marker.read_text(encoding="utf-8"), "persist")

    def test_open_instance_has_no_inherited_token(self):
        from unittest.mock import patch
        with patch.dict(os.environ, {"OPROXY_ADMIN_TOKEN": "inherited-token"}):
            with IsolatedProxy(open_admin=True) as proxy:
                self.assertNotIn("x-oproxy-admin-token", proxy.client.headers)
                response = proxy.client.get(proxy.url + "/admin/config", timeout=3)
                self.assertEqual(response.status_code, 200, response.text)
                self.assertFalse(response.json()["admin_auth_enabled"])
                self.assertEqual(proxy.client.get(
                    proxy.url + "/admin/sessions", timeout=3).status_code, 200)


if __name__ == "__main__":
    unittest.main()
