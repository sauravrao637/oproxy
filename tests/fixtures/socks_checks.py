"""Verify the supported SOCKS5 command boundary on the wire."""

import socket


def assert_invalid_version_rejected(host, port, timeout):
    """Invalid protocol versions must close/reset, not negotiate or stall."""
    with socket.create_connection((host, port), timeout=timeout) as connection:
        connection.settimeout(timeout)
        connection.sendall(b"\x04\x01\x00\x50\x7f\x00\x00\x01\x00")
        try:
            reply = connection.recv(8)
        except ConnectionResetError:
            return
        assert reply == b"", f"Invalid version received unexpected reply: {reply!r}"


def receive_exact(connection, size):
    data = b""
    while len(data) < size:
        chunk = connection.recv(size - len(data))
        if not chunk:
            raise AssertionError("Truncated SOCKS5 reply")
        data += chunk
    return data


def assert_udp_rejected(host, port, timeout):
    with socket.create_connection((host, port), timeout=timeout) as connection:
        connection.settimeout(timeout)
        connection.sendall(b"\x05\x01\x00")
        assert receive_exact(connection, 2) == b"\x05\x00", "Invalid SOCKS5 negotiation"
        connection.sendall(b"\x05\x03\x00\x01\x00\x00\x00\x00\x00\x00")
        reply = receive_exact(connection, 10)
        assert reply == b"\x05\x07\x00\x01\x00\x00\x00\x00\x00\x00", \
            f"Expected complete command-not-supported reply, got {reply!r}"
