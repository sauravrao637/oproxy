import socket
import unittest
from unittest.mock import MagicMock, patch

from fixtures.socks_checks import assert_invalid_version_rejected, assert_udp_rejected


class InvalidVersionTests(unittest.TestCase):
    def execute(self, reply):
        connection = MagicMock()
        connection.__enter__.return_value = connection
        connection.recv.side_effect = [reply]
        with patch("fixtures.socks_checks.socket.create_connection", return_value=connection):
            assert_invalid_version_rejected("127.0.0.1", 1080, 1)
        connection.sendall.assert_called_once_with(b"\x04\x01\x00\x50\x7f\x00\x00\x01\x00")

    def test_close_and_reset_are_rejection(self):
        for reply in (b"", ConnectionResetError()):
            with self.subTest(reply=reply):
                self.execute(reply)

    def test_arbitrary_reply_negotiation_and_timeout_fail(self):
        for reply in (b"\x00\x5a", b"\x05\x00", b"\x05\xff", b"garbage", socket.timeout()):
            with self.subTest(reply=reply):
                with self.assertRaises((AssertionError, socket.timeout)):
                    self.execute(reply)

    def test_absent_listener_is_not_rejection(self):
        with patch("fixtures.socks_checks.socket.create_connection",
                   side_effect=ConnectionRefusedError()):
            with self.assertRaises(ConnectionRefusedError):
                assert_invalid_version_rejected("127.0.0.1", 1080, 1)


class SocksReplyTests(unittest.TestCase):
    def execute(self, chunks):
        connection = MagicMock()
        connection.__enter__.return_value = connection
        connection.recv.side_effect = chunks
        with patch("fixtures.socks_checks.socket.create_connection", return_value=connection):
            assert_udp_rejected("127.0.0.1", 1080, 1)
        self.assertEqual(connection.sendall.call_args.args[0][1], 3)

    def test_complete_rejection(self):
        self.execute([b"\x05", b"\x00", b"\x05\x07\x00\x01", b"\x00" * 6])

    def test_wrong_truncated_and_timed_out_replies_fail(self):
        for chunks in ([b"\x05\x00", b"\x05\x00\x00\x01" + b"\x00" * 6],
                       [b"\x05\x00", b"\x05\x07", b""],
                       [b"\x05\x00", socket.timeout("reply timeout")],
                       [b"\x04\x00"]):
            with self.subTest(chunks=chunks):
                with self.assertRaises((AssertionError, socket.timeout)):
                    self.execute(chunks)


if __name__ == "__main__":
    unittest.main()
