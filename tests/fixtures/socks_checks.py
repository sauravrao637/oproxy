"""Verify the supported SOCKS5 command boundary on the wire."""

import socket
import time


def completed_tunnel(exchange, host, port):
    request = exchange.get("request") or {}
    protocol = exchange.get("protocol_context") or {}
    events = exchange.get("events") or []
    return (request.get("method") == "CONNECT"
            and request.get("uri") == f"socks5://{host}:{port}"
            and protocol.get("downstream") == "socks5"
            and protocol.get("body_mode") == "tunnel"
            and any(e.get("type") == "tunnel_opened" for e in events)
            and any(e.get("type") == "tunnel_closed"
                    and e.get("bytes_up", 0) > 0 and e.get("bytes_down", 0) > 0
                    for e in events))


def wait_tunnel_capture(api_request, host, port, timeout=5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        response = api_request("GET", "/api/sessions")
        response.raise_for_status()
        payload = response.json()
        sessions = payload.get("sessions") if isinstance(payload, dict) else payload
        assert isinstance(sessions, list), "Malformed sessions response"
        for session in sessions:
            if (session.get("request") or {}).get("uri") != f"socks5://{host}:{port}":
                continue
            response = api_request("GET", f"/api/sessions/{session['id']}")
            response.raise_for_status()
            detail = response.json()
            exchange = detail.get("exchange", detail)
            if completed_tunnel(exchange, host, port):
                return exchange
        time.sleep(.05)
    raise AssertionError(f"No completed SOCKS5 tunnel capture for {host}:{port}")


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


def assert_udp_rejected(host, port, timeout, split_at=None):
    with socket.create_connection((host, port), timeout=timeout) as connection:
        connection.settimeout(timeout)
        connection.sendall(b"\x05\x01\x00")
        assert receive_exact(connection, 2) == b"\x05\x00", "Invalid SOCKS5 negotiation"
        request = b"\x05\x03\x00\x01\x00\x00\x00\x00\x00\x00"
        if split_at is None:
            connection.sendall(request)
        else:
            assert 0 < split_at < len(request), "Invalid request fragment boundary"
            connection.sendall(request[:split_at])
            time.sleep(.025)
            connection.sendall(request[split_at:])
        reply = receive_exact(connection, 10)
        assert reply == b"\x05\x07\x00\x01\x00\x00\x00\x00\x00\x00", \
            f"Expected complete command-not-supported reply, got {reply!r}"
