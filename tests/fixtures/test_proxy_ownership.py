"""Live listener collision and foreign readiness rejection."""

import socket
import unittest
from unittest.mock import Mock, patch
from urllib.parse import urlsplit

from fixtures.isolated_proxy import IsolatedProxy


class ProxyOwnershipTests(unittest.TestCase):
    def test_occupied_port_fails_before_spawning(self):
        with IsolatedProxy() as proxy:
            proxy.stop()
            with socket.socket() as other:
                other.bind(("127.0.0.1", urlsplit(proxy.url).port))
                other.listen()
                with patch("fixtures.isolated_proxy.subprocess.Popen") as spawn:
                    with self.assertRaises(OSError):
                        proxy.start()
                    spawn.assert_not_called()

    def test_foreign_ca_cannot_satisfy_readiness(self):
        with IsolatedProxy() as proxy:
            proxy.stop()
            foreign = Mock(ok=True, text="foreign CA")
            with patch.object(proxy.client, "get", return_value=foreign):
                with self.assertRaisesRegex(RuntimeError, "does not belong"):
                    proxy.start()
            process = proxy.process
            root = proxy.root
        self.assertIsNotNone(process.poll())
        self.assertFalse(root.exists())


if __name__ == "__main__":
    unittest.main()
