import copy
import socket
import unittest
from unittest.mock import MagicMock, patch

from fixtures.socks_checks import assert_invalid_version_rejected, assert_udp_rejected, completed_tunnel, wait_tunnel_capture


class TunnelCaptureTests(unittest.TestCase):
    def test_terminal_destination_protocol_and_counters_required(self):
        exchange = {
            "request": {"method": "CONNECT", "uri": "socks5://127.0.0.1:1234"},
            "protocol_context": {"downstream": "socks5", "body_mode": "tunnel"},
            "events": [{"type": "tunnel_opened"},
                       {"type": "tunnel_closed", "bytes_up": 1, "bytes_down": 1}],
        }
        self.assertTrue(completed_tunnel(exchange, "127.0.0.1", 1234))
        for key, field, value in (("request", "method", "GET"),
                                  ("request", "uri", "socks5://127.0.0.1:5678"),
                                  ("protocol_context", "downstream", "http1"),
                                  ("protocol_context", "body_mode", "buffered")):
            changed = copy.deepcopy(exchange)
            changed[key][field] = value
            self.assertFalse(completed_tunnel(changed, "127.0.0.1", 1234))
        for events in ([], exchange["events"][:1], exchange["events"][1:]):
            changed = dict(exchange, events=events)
            self.assertFalse(completed_tunnel(changed, "127.0.0.1", 1234))
        for counter in ("bytes_up", "bytes_down"):
            changed = copy.deepcopy(exchange)
            changed["events"][1][counter] = 0
            self.assertFalse(completed_tunnel(changed, "127.0.0.1", 1234))

    def test_missing_capture_fails(self):
        request = MagicMock()
        request.return_value.json.return_value = {"sessions": []}
        with self.assertRaisesRegex(AssertionError, "No completed SOCKS5"):
            wait_tunnel_capture(request, "127.0.0.1", 1234, timeout=.01)


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

    def test_fragmented_requests_require_complete_reply(self):
        for split_at in (4, 9):
            for response in (b"\x05\x07\x00\x01" + b"\x00" * 6,
                             ConnectionResetError(), socket.timeout(), b""):
                with self.subTest(split_at=split_at, response=response):
                    connection = MagicMock()
                    connection.__enter__.return_value = connection
                    connection.recv.side_effect = [b"\x05\x00", response]
                    with patch("fixtures.socks_checks.socket.create_connection", return_value=connection), \
                         patch("fixtures.socks_checks.time.sleep"):
                        if isinstance(response, bytes) and response:
                            assert_udp_rejected("127.0.0.1", 1080, 1, split_at=split_at)
                        else:
                            with self.assertRaises((AssertionError, ConnectionResetError, socket.timeout)):
                                assert_udp_rejected("127.0.0.1", 1080, 1, split_at=split_at)
                    fragments = [call.args[0] for call in connection.sendall.call_args_list][1:]
                    self.assertEqual(len(fragments[0]), split_at)
                    self.assertEqual(b"".join(fragments), b"\x05\x03\x00\x01" + b"\x00" * 6)

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
