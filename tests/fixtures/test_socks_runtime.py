"""Live SOCKS checks using OPROXY_TEST_BINARY; absent prerequisites fail."""

import socket
import unittest

from fixtures.isolated_proxy import IsolatedProxy
from fixtures.socks_checks import assert_invalid_version_rejected, assert_udp_rejected


class SocksRuntimeTests(unittest.TestCase):
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
