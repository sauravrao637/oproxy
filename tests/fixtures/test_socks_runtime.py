"""Live SOCKS checks using OPROXY_TEST_BINARY; absent prerequisites fail."""

import contextlib
import importlib
import io
import os
import sys
from unittest.mock import patch

import socket
import unittest

from fixtures.isolated_proxy import IsolatedProxy
from fixtures.socks_checks import assert_invalid_version_rejected, assert_udp_rejected


class SocksRuntimeTests(unittest.TestCase):
    def test_required_section_captures_terminal_tunnel(self):
        with patch.object(sys, "argv", ["e2e_protocol_test.py"]):
            suite = importlib.import_module("e2e_protocol_test")
        with IsolatedProxy() as proxy:
            with socket.socket() as reservation:
                reservation.bind(("127.0.0.1", 0))
                port = reservation.getsockname()[1]
            proxy.configure(socks5_port=port)
            proxy.restart()
            results = []
            suite.start_echo_server()
            try:
                with patch.object(suite, "BASE_URL", proxy.url), \
                     patch.object(suite, "ADMIN_TOKEN", proxy.admin_token), \
                     patch.object(suite, "SOCKS5_PORT", port), \
                     patch.object(suite, "RESULTS", results), \
                     contextlib.redirect_stdout(io.StringIO()):
                    suite.section_socks5()
                self.assertEqual(len(results), 4, results)
                self.assertTrue(all(r["status"] == "PASS" for r in results), results)
            finally:
                suite.ECHO_SERVER.shutdown()
                suite.ECHO_SERVER.server_close()

    def test_disabled_primary_uses_isolated_listener(self):
        with patch.object(sys, "argv", ["e2e_protocol_test.py"]):
            suite = importlib.import_module("e2e_protocol_test")
        with IsolatedProxy() as primary:
            results = []
            suite.start_echo_server()
            try:
                with patch.object(suite, "BASE_URL", primary.url), \
                     patch.object(suite, "ADMIN_TOKEN", primary.admin_token), \
                     patch.object(suite, "RESULTS", results), \
                     contextlib.redirect_stdout(io.StringIO()):
                    self.assertFalse(suite.socks5_enabled())
                    suite.section_socks5()
                    self.assertFalse(suite.socks5_enabled())
                self.assertEqual(len(results), 4, results)
                self.assertTrue(all(r["status"] == "PASS" for r in results), results)
                self.assertTrue(all("primary listener disabled" in r["detail"]
                                    for r in results), results)
            finally:
                suite.ECHO_SERVER.shutdown()
                suite.ECHO_SERVER.server_close()

    def test_disabled_primary_missing_binary_fails_all_cases(self):
        with patch.object(sys, "argv", ["e2e_protocol_test.py"]):
            suite = importlib.import_module("e2e_protocol_test")
        results = []
        with patch.object(suite, "socks5_enabled", return_value=False), \
             patch.dict(os.environ, {"OPROXY_TEST_BINARY": __file__ + ".missing"}), \
             patch.object(suite, "RESULTS", results), \
             contextlib.redirect_stdout(io.StringIO()):
            suite.section_socks5()
        self.assertEqual(len(results), 4, results)
        self.assertTrue(all(r["status"] == "FAIL" for r in results), results)
        self.assertTrue(all("Missing tested binary" in r["detail"] for r in results))

    def test_invalid_version_and_unsupported_command(self):
        with IsolatedProxy() as proxy:
            with socket.socket() as reservation:
                reservation.bind(("127.0.0.1", 0))
                port = reservation.getsockname()[1]
            proxy.configure(socks5_port=port)
            proxy.restart()
            # A valid negotiation verifies the listener before testing rejection.
            assert_udp_rejected("127.0.0.1", port, 3)
            assert_invalid_version_rejected("127.0.0.1", port, 3)


if __name__ == "__main__":
    unittest.main()
