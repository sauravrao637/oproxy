"""Run with python -m unittest discover -s tests/fixtures -p test_https_origin.py."""

import socket
import unittest
from pathlib import Path

import requests

from https_origin import HttpsOrigin


class HttpsOriginTests(unittest.TestCase):
    def setUp(self):
        self.client = requests.Session()
        self.client.trust_env = False
        self.addCleanup(self.client.close)

    def test_response_trust_and_cleanup(self):
        with HttpsOrigin() as origin:
            ca = str(origin.ca_path)
            url = origin.url + "/echo?nonce=unique-request"
            response = self.client.get(url, verify=ca, timeout=3)
            self.assertEqual(response.status_code, 200)
            self.assertEqual(response.json(), {
                "method": "GET", "path": "/echo?nonce=unique-request",
                "nonce": "unique-request",
            })
            self.assertEqual(self.client.get(origin.url + "/unknown", verify=ca,
                                             timeout=3).status_code, 404)
            with self.assertRaises(requests.exceptions.SSLError):
                self.client.get(url, timeout=3)
        self.assertFalse(Path(ca).exists())
        self.assertFalse(origin.thread.is_alive())
        with self.assertRaises(requests.exceptions.ConnectionError):
            self.client.get(url, verify=False, timeout=3)

    def test_incomplete_tls_handshake_does_not_prevent_shutdown(self):
        with HttpsOrigin() as origin:
            connection = socket.create_connection(("127.0.0.1", origin.port), timeout=3)
            self.addCleanup(connection.close)
        self.assertFalse(origin.thread.is_alive())


if __name__ == "__main__":
    unittest.main()
