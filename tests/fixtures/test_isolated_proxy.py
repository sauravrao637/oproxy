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


if __name__ == "__main__":
    unittest.main()
