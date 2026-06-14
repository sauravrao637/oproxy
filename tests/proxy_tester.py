#!/usr/bin/env python3
"""
Comprehensive Proxy Tester – auto‑venv, installs deps, runs all tests, cleans up.
Tests HTTP/1.1, HTTP/2, HTTP/3 (QUIC), WebSocket, gRPC, SOCKS5 TCP/UDP.

Note: HTTP/2 and HTTP/3 tests connect to a local test server via the proxy.
The test server uses a self‑signed certificate, so SSL verification is disabled
for the test server only – your proxy's certificate is not involved in these tests.

HTTP/3 tests require the proxy to be built with the `http3` Cargo feature and
OPROXY_HTTP3_ENABLED=true / OPROXY_HTTP3_PORT=<port> set. They are skipped
gracefully when aioquic is unavailable or the proxy's H3 listener is unreachable.

Usage:
  python proxy_tester.py --http-proxy http://localhost:8080 --socks-proxy socks5://localhost:1080 [--verbose]
  python proxy_tester.py --http-proxy http://localhost:8080 --h3-proxy h3://localhost:8443 [--verbose]
"""

import os
import sys
import subprocess
import tempfile
import shutil
import signal
import atexit
import time
import argparse

# ----------------------------------------------------------------------
# Check if we are running inside the temporary venv (flag --run-tests)
# ----------------------------------------------------------------------
RUN_TESTS_FLAG = '--run-tests'

if RUN_TESTS_FLAG not in sys.argv:
    # --------------- BOOTSTRAP: create venv, install deps, re-launch ---------------
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument('--http-proxy')
    parser.add_argument('--socks-proxy')
    parser.add_argument('--h3-proxy', default=None,
        help='HTTP/3 (QUIC) proxy URL, e.g. h3://localhost:8443. '
             'Derived from --http-proxy host + port 8443 when omitted.')
    parser.add_argument('--target-host', default='127.0.0.1')
    parser.add_argument('--timeout', type=int, default=10)
    parser.add_argument('--verbose', action='store_true')
    args, unknown = parser.parse_known_args()

    temp_dir = tempfile.mkdtemp(prefix='proxy_tester_venv_')
    venv_dir = os.path.join(temp_dir, 'venv')

    print(f"Creating temporary virtual environment at {venv_dir} ...")
    subprocess.run([sys.executable, '-m', 'venv', venv_dir], check=True)

    if os.name == 'nt':
        pip_path = os.path.join(venv_dir, 'Scripts', 'pip.exe')
        python_path = os.path.join(venv_dir, 'Scripts', 'python.exe')
    else:
        pip_path = os.path.join(venv_dir, 'bin', 'pip')
        python_path = os.path.join(venv_dir, 'bin', 'python')

    packages = [
        'requests',
        'httpx[http2]',
        'websockets',
        'grpcio',
        'grpcio-tools',
        'PySocks',
        'hypercorn',
        'brotli',
        'zstandard',
        'certifi',
        'trustme',
        'aioquic',  # HTTP/3 (QUIC) support; tests skip gracefully if absent
    ]
    print("Installing required packages (this may take a moment) ...")
    subprocess.run([pip_path, 'install', *packages], check=True)

    cmd = [
        python_path,
        os.path.abspath(__file__),
        RUN_TESTS_FLAG,
    ]
    for arg in sys.argv[1:]:
        if arg != RUN_TESTS_FLAG:
            cmd.append(arg)

    def cleanup():
        print("\nCleaning up temporary venv...")
        try:
            shutil.rmtree(temp_dir, ignore_errors=True)
        except Exception as e:
            print(f"Cleanup error: {e}")

    atexit.register(cleanup)

    inner_process = None
    def signal_handler(sig, frame):
        if inner_process and inner_process.poll() is None:
            inner_process.terminate()
            try:
                inner_process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                inner_process.kill()
        cleanup()
        sys.exit(0)

    signal.signal(signal.SIGINT, signal_handler)
    signal.signal(signal.SIGTERM, signal_handler)

    inner_process = subprocess.Popen(cmd)
    try:
        inner_process.wait()
    except:
        inner_process.terminate()
        inner_process.wait()
        raise
    finally:
        cleanup()
        atexit.unregister(cleanup)

    sys.exit(inner_process.returncode)

# ----------------------------------------------------------------------
# Inside venv: actual tests
# ----------------------------------------------------------------------
import argparse
import asyncio
import atexit
import os
import signal
import socket
import ssl
import sys
import threading
import time
import traceback
import base64
import gzip
import hashlib
import zlib
import json
import struct
from urllib.parse import urlparse
from concurrent import futures
from io import BytesIO

import requests
import httpx
import websockets
import grpc
import grpc_tools
from grpc_tools import protoc
import socks
import brotli
import zstandard
from hypercorn.config import Config as HypercornConfig
from hypercorn.asyncio import serve as hypercorn_serve

import urllib3
urllib3.disable_warnings(urllib3.exceptions.InsecureRequestWarning)

# ----------------------------------------------------------------------
# Global state
# ----------------------------------------------------------------------
server_manager = None
_shutdown_event = threading.Event()

def _handle_signal(signum, frame):
    print("\nShutting down...")
    _shutdown_event.set()
    if server_manager:
        server_manager.stop_all()
    sys.exit(0)

signal.signal(signal.SIGINT, _handle_signal)
signal.signal(signal.SIGTERM, _handle_signal)

# ----------------------------------------------------------------------
# Helper
# ----------------------------------------------------------------------
def free_port():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]

def _recv_until(sock, marker, timeout):
    sock.settimeout(timeout)
    data = b''
    while marker not in data:
        chunk = sock.recv(4096)
        if not chunk:
            break
        data += chunk
    return data

def _send_ws_frame(sock, payload, opcode):
    if isinstance(payload, str):
        payload = payload.encode('utf-8')
    mask_key = os.urandom(4)
    length = len(payload)
    header = bytearray([0x80 | opcode])
    if length < 126:
        header.append(0x80 | length)
    elif length <= 0xFFFF:
        header.extend([0x80 | 126])
        header.extend(struct.pack('!H', length))
    else:
        header.extend([0x80 | 127])
        header.extend(struct.pack('!Q', length))
    masked = bytes(b ^ mask_key[i % 4] for i, b in enumerate(payload))
    sock.sendall(bytes(header) + mask_key + masked)

def _recv_ws_frame(sock, timeout):
    sock.settimeout(timeout)
    head = sock.recv(2)
    if len(head) != 2:
        raise RuntimeError("truncated WebSocket frame header")
    opcode = head[0] & 0x0F
    length = head[1] & 0x7F
    masked = bool(head[1] & 0x80)
    if length == 126:
        length = struct.unpack('!H', sock.recv(2))[0]
    elif length == 127:
        length = struct.unpack('!Q', sock.recv(8))[0]
    mask_key = sock.recv(4) if masked else b''
    payload = b''
    while len(payload) < length:
        chunk = sock.recv(length - len(payload))
        if not chunk:
            raise RuntimeError("truncated WebSocket frame payload")
        payload += chunk
    if masked:
        payload = bytes(b ^ mask_key[i % 4] for i, b in enumerate(payload))
    return opcode, payload

def _open_websocket_via_http_proxy(proxy_url, target_url, timeout):
    proxy = urlparse(proxy_url)
    target = urlparse(target_url)
    if proxy.scheme != 'http' or not proxy.hostname or not proxy.port:
        raise ValueError("HTTP proxy must be http://host:port")
    if target.scheme != 'ws' or not target.hostname:
        raise ValueError("target must be a ws:// URL")

    target_port = target.port or 80
    path = target.path or '/'
    if target.query:
        path += '?' + target.query
    authority = f"{target.hostname}:{target_port}"
    absolute_uri = f"ws://{authority}{path}"
    key = base64.b64encode(os.urandom(16)).decode('ascii')
    expected_accept = base64.b64encode(
        hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode('ascii')).digest()
    ).decode('ascii')

    sock = socket.create_connection((proxy.hostname, proxy.port), timeout=timeout)
    try:
        request = (
            f"GET {absolute_uri} HTTP/1.1\r\n"
            f"Host: {authority}\r\n"
            "Upgrade: websocket\r\n"
            "Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\n"
            "Sec-WebSocket-Version: 13\r\n"
            "\r\n"
        )
        sock.sendall(request.encode('ascii'))
        response = _recv_until(sock, b'\r\n\r\n', timeout)
        response_text = response.decode('iso-8859-1', errors='replace')
        if not response_text.startswith('HTTP/1.1 101'):
            raise RuntimeError(f"proxy did not return WebSocket 101: {response_text[:120]!r}")
        if expected_accept.lower() not in response_text.lower():
            raise RuntimeError("proxy returned invalid Sec-WebSocket-Accept")
        return sock
    except Exception:
        sock.close()
        raise

def _websocket_roundtrip_via_http_proxy(proxy_url, target_url, payload, opcode, timeout):
    with _open_websocket_via_http_proxy(proxy_url, target_url, timeout) as sock:

        _send_ws_frame(sock, payload, opcode)
        while True:
            recv_opcode, recv_payload = _recv_ws_frame(sock, timeout)
            if recv_opcode in (0x1, 0x2):
                return recv_opcode, recv_payload
            if recv_opcode == 0x8:
                raise RuntimeError("WebSocket closed before echo")

def _admin_base_from_http_proxy(proxy_url):
    parsed = urlparse(proxy_url)
    if parsed.scheme != 'http' or not parsed.hostname:
        return None
    port = parsed.port or 80
    return f"http://{parsed.hostname}:{port}"

def _admin_request(proxy_url, method, path, timeout, **kwargs):
    admin_base = _admin_base_from_http_proxy(proxy_url)
    if not admin_base:
        raise ValueError("session assertions require an HTTP proxy/admin URL")
    return requests.request(method, f"{admin_base}{path}", timeout=timeout, **kwargs)

def _session_exchange(session_or_detail):
    if isinstance(session_or_detail, dict) and 'exchange' in session_or_detail:
        return session_or_detail.get('exchange') or {}
    return session_or_detail or {}

def _list_recorded_sessions(proxy_url, timeout, include_bodies=False):
    path = '/api/sessions?limit=200'
    if include_bodies:
        path += '&include_bodies=true'
    r = _admin_request(proxy_url, 'GET', path, timeout)
    r.raise_for_status()
    return r.json().get('sessions', [])

def _get_session_detail(proxy_url, session_id, timeout):
    r = _admin_request(proxy_url, 'GET', f"/api/sessions/{session_id}", timeout)
    r.raise_for_status()
    return _session_exchange(r.json())

def _event_types(exchange):
    return [event.get('type') for event in exchange.get('events', [])]

def _protocol_context(exchange):
    return exchange.get('protocol_context') or {}

def _wait_for_recorded_session(proxy_url, predicate, timeout, include_bodies=False):
    deadline = time.time() + timeout
    last_sessions = []
    while time.time() < deadline:
        last_sessions = _list_recorded_sessions(proxy_url, timeout, include_bodies=include_bodies)
        for session in last_sessions:
            session_id = session.get('id')
            exchange = session
            if session_id:
                try:
                    exchange = _get_session_detail(proxy_url, session_id, timeout)
                except Exception:
                    exchange = session
            if predicate(exchange):
                return exchange
        time.sleep(0.2)
    raise AssertionError(f"matching recorded session not found; saw {len(last_sessions)} sessions")

def _recorded_session_details(proxy_url, timeout, include_bodies=False):
    sessions = _list_recorded_sessions(proxy_url, timeout, include_bodies=include_bodies)
    details = []
    for session in sessions:
        session_id = session.get('id')
        if session_id:
            try:
                details.append(_get_session_detail(proxy_url, session_id, timeout))
                continue
            except Exception:
                pass
        details.append(session)
    return details

# ----------------------------------------------------------------------
# Simple protobuf message
# ----------------------------------------------------------------------
class EchoMessage:
    @staticmethod
    def encode(msg: str) -> bytes:
        payload = msg.encode('utf-8')
        field = (1 << 3) | 2
        return struct.pack('B', field) + struct.pack('B', len(payload)) + payload

    @staticmethod
    def decode(data: bytes) -> str:
        if not data:
            raise ValueError("empty data")
        pos = 0
        while pos < len(data):
            tag, pos = _read_varint(data, pos)
            field_number = tag >> 3
            wire_type = tag & 0x07
            if wire_type == 2:
                length, pos = _read_varint(data, pos)
                value = data[pos:pos+length]
                pos += length
                if field_number == 1:
                    return value.decode('utf-8')
        raise ValueError("field 1 not found")

def _read_varint(data, pos):
    result = 0
    shift = 0
    while True:
        if pos >= len(data):
            raise ValueError("truncated varint")
        byte = data[pos]
        result |= (byte & 0x7f) << shift
        pos += 1
        if not (byte & 0x80):
            break
        shift += 7
    return result, pos

# ----------------------------------------------------------------------
# HTTP/3 (QUIC) helpers — compiled lazily so aioquic absence doesn't break
# any non-H3 tests.
# ----------------------------------------------------------------------
_AIOQUIC_AVAILABLE = False
try:
    from aioquic.asyncio import connect as _quic_connect
    from aioquic.asyncio.protocol import QuicConnectionProtocol as _QuicProtocol
    from aioquic.h3.connection import H3Connection as _H3Connection
    from aioquic.h3.events import DataReceived as _H3Data
    from aioquic.h3.events import HeadersReceived as _H3Headers
    # StreamReset moved from h3.events → quic.events in aioquic ≥ 1.0; import
    # defensively so both old and new versions work.
    try:
        from aioquic.h3.events import StreamReset as _H3Reset
    except ImportError:
        _H3Reset = None  # handled via QUIC-level StreamReset in quic_event_received
    from aioquic.quic.configuration import QuicConfiguration as _QuicConfig
    from aioquic.quic.events import HandshakeCompleted as _HandshakeCompleted
    from aioquic.quic.events import StreamReset as _QuicStreamReset
    from aioquic.quic.events import StreamDataReceived as _QuicStreamDataReceived
    _AIOQUIC_AVAILABLE = True

    class _H3Client(_QuicProtocol):
        """Minimal HTTP/3 forward-proxy client.

        Initialises the H3Connection as soon as the QUIC handshake completes,
        so SETTINGS frames (sent immediately after the handshake) are processed
        correctly before we issue any requests.
        """

        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            self._h3 = None
            self._responses = {}  # stream_id → response entry dict

        def http_event_received(self, event):
            sid = getattr(event, 'stream_id', None)
            if sid not in self._responses:
                return
            entry = self._responses[sid]
            if isinstance(event, _H3Headers):
                for name, value in event.headers:
                    if name == b':status':
                        entry['status'] = int(value)
                    else:
                        try:
                            entry['headers'][name.decode()] = value.decode()
                        except Exception:
                            pass
                if event.stream_ended:
                    entry['done'].set()
            elif isinstance(event, _H3Data):
                entry['body'] += event.data
                if event.stream_ended:
                    entry['done'].set()
            elif _H3Reset is not None and isinstance(event, _H3Reset):
                # Old aioquic (< 1.0): stream resets surfaced as H3 events.
                entry['error'] = f'h3 stream reset (code={event.error_code})'
                entry['done'].set()

        def quic_event_received(self, event):
            # Initialise H3Connection on the first HandshakeCompleted so that
            # SETTINGS frames (sent right after the handshake) are captured.
            if isinstance(event, _HandshakeCompleted):
                try:
                    self._h3 = _H3Connection(self._quic, enable_webtransport=False)
                except TypeError:
                    # Older aioquic builds do not have enable_webtransport.
                    self._h3 = _H3Connection(self._quic)
            # New aioquic (≥ 1.0): stream resets surface as QUIC-level events.
            if _H3Reset is None and isinstance(event, _QuicStreamReset):
                entry = self._responses.get(event.stream_id)
                if entry and not entry['done'].is_set():
                    entry['error'] = f'stream reset (code={event.error_code})'
                    entry['done'].set()
            if self._h3 is not None:
                for http_event in self._h3.handle_event(event):
                    self.http_event_received(http_event)
            # Fallback: the h3 crate appends a GREASE frame after every response,
            # which causes buf.eof() to be False when the DATA frame is parsed,
            # so H3Connection emits DataReceived(stream_ended=False) even though
            # the QUIC stream has ended.  Once all H3 events are processed and
            # we still have a status but no done signal, complete the response.
            if isinstance(event, _QuicStreamDataReceived) and event.end_stream:
                entry = self._responses.get(event.stream_id)
                if entry is not None and not entry['done'].is_set() and entry['status'] is not None:
                    entry['done'].set()

        async def h3_fetch(self, method, scheme, authority, path,
                           body=None, extra_headers=None):
            """Send one H3 request; return the response-entry dict (awaitable
            via entry['done'])."""
            if self._h3 is None:
                raise RuntimeError('H3 connection not ready (handshake incomplete?)')
            stream_id = self._quic.get_next_available_stream_id()
            entry = {
                'status': None,
                'headers': {},
                'body': b'',
                'done': asyncio.Event(),
                'error': None,
            }
            self._responses[stream_id] = entry
            headers = [
                (b':method',    method.encode()),
                (b':scheme',    scheme.encode()),
                (b':authority', authority.encode()),
                (b':path',      (path or '/').encode()),
            ]
            if extra_headers:
                for k, v in extra_headers.items():
                    headers.append((
                        k.encode() if isinstance(k, str) else k,
                        v.encode() if isinstance(v, str) else v,
                    ))
            end_stream = body is None
            self._h3.send_headers(
                stream_id=stream_id, headers=headers, end_stream=end_stream
            )
            if body is not None:
                self._h3.send_data(
                    stream_id=stream_id, data=body, end_stream=True
                )
            self.transmit()
            return entry

    async def _h3_forward_request_async(
            h3_proxy_host, h3_proxy_port,
            method, target_scheme, target_authority, path,
            body=None, extra_headers=None, timeout=10):
        """Connect to oproxy's H3 (QUIC) listener and issue a forward-proxy
        request.  The proxy extracts the absolute URI from the H3 pseudo-headers
        and routes it through the same middleware pipeline as TCP listeners."""
        config = _QuicConfig(is_client=True, alpn_protocols=['h3'])
        config.verify_mode = ssl.CERT_NONE  # proxy's QUIC cert is MITM-issued

        async with _quic_connect(
            h3_proxy_host,
            h3_proxy_port,
            configuration=config,
            create_protocol=_H3Client,
        ) as client:
            entry = await client.h3_fetch(
                method, target_scheme, target_authority, path,
                body=body, extra_headers=extra_headers,
            )
            try:
                await asyncio.wait_for(entry['done'].wait(), timeout=timeout)
            except asyncio.TimeoutError:
                raise TimeoutError(
                    f'H3 request timed out after {timeout}s '
                    f'(is OPROXY_HTTP3_ENABLED=true and the http3 feature compiled in?)'
                )
            if entry['error']:
                raise RuntimeError(entry['error'])
            return entry['status'], entry['headers'], entry['body']

    def _h3_forward_request(h3_proxy_host, h3_proxy_port,
                            method, target_scheme, target_authority, path,
                            body=None, extra_headers=None, timeout=10):
        """Synchronous wrapper around :func:`_h3_forward_request_async`."""
        loop = asyncio.new_event_loop()
        try:
            return loop.run_until_complete(
                _h3_forward_request_async(
                    h3_proxy_host, h3_proxy_port,
                    method, target_scheme, target_authority, path,
                    body=body, extra_headers=extra_headers, timeout=timeout,
                )
            )
        finally:
            loop.close()

except Exception:
    pass  # aioquic unavailable or incompatible; H3 tests will be skipped at runtime


# ----------------------------------------------------------------------
# Test server implementations
# ----------------------------------------------------------------------
class ServerManager:
    def __init__(self, target_host='127.0.0.1'):
        self.target_host = target_host
        self.servers = []
        self.grpc_ca_pem = None  # set by start_grpc()

    def _add_server(self, name, port, shutdown):
        self.servers.append((name, port, shutdown))

    def stop_all(self):
        for name, port, shutdown in self.servers:
            try:
                shutdown()
                print(f"Stopped {name} server on port {port}")
            except Exception as e:
                print(f"Error stopping {name} server: {e}")

    # --- HTTP/1.1 server ---
    def start_http1(self):
        from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
        port = free_port()

        class Handler(BaseHTTPRequestHandler):
            # HTTP/1.1 is required for the /chunked response (Transfer-Encoding:
            # chunked is invalid over the BaseHTTPRequestHandler HTTP/1.0 default).
            protocol_version = 'HTTP/1.1'

            def log_message(self, format, *args):
                pass

            def _send_response(self, status, content_type, body, extra_headers=None):
                self.send_response(status)
                self.send_header('Content-Type', content_type)
                extra_headers = extra_headers or {}
                # Under HTTP/1.1 every non-chunked response needs a Content-Length
                # or the client blocks waiting for more. Add one unless the caller
                # already framed the body (Content-Length or Transfer-Encoding).
                lower = {k.lower() for k in extra_headers}
                if 'content-length' not in lower and 'transfer-encoding' not in lower:
                    self.send_header('Content-Length', str(len(body)))
                for k, v in extra_headers.items():
                    self.send_header(k, v)
                self.end_headers()
                self.wfile.write(body)

            def do_GET(self):
                if self.path == '/gzip':
                    body = gzip.compress(b'Hello gzip')
                    self._send_response(200, 'text/plain', body, {'Content-Encoding': 'gzip'})
                elif self.path == '/deflate':
                    body = zlib.compress(b'Hello deflate')
                    self._send_response(200, 'text/plain', body, {'Content-Encoding': 'deflate'})
                elif self.path == '/brotli':
                    body = brotli.compress(b'Hello brotli')
                    self._send_response(200, 'text/plain', body, {'Content-Encoding': 'br'})
                elif self.path == '/zstd':
                    body = zstandard.compress(b'Hello zstd')
                    self._send_response(200, 'text/plain', body, {'Content-Encoding': 'zstd'})
                elif self.path == '/json':
                    data = json.dumps({'message': 'Hello JSON'}).encode()
                    self._send_response(200, 'application/json', data)
                elif self.path == '/proto':
                    data = EchoMessage.encode('Hello Proto')
                    self._send_response(200, 'application/x-protobuf', data)
                elif self.path == '/chunked':
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/plain')
                    self.send_header('Transfer-Encoding', 'chunked')
                    self.end_headers()
                    chunks = [b'Hello ', b'chunked ', b'world!']
                    for chunk in chunks:
                        self.wfile.write(f"{len(chunk):X}\r\n".encode())
                        self.wfile.write(chunk)
                        self.wfile.write(b"\r\n")
                    self.wfile.write(b"0\r\n\r\n")
                elif self.path == '/keep-alive':
                    body = b'Hello keepalive'
                    self._send_response(200, 'text/plain', body,
                                        {'Connection': 'keep-alive', 'Content-Length': str(len(body))})
                elif self.path == '/large':
                    size = 1024 * 1024
                    self.send_response(200)
                    self.send_header('Content-Type', 'application/octet-stream')
                    self.send_header('Content-Length', str(size))
                    self.end_headers()
                    self.wfile.write(b'A' * size)
                elif self.path == '/headers':
                    # Reflect the request headers the upstream actually received,
                    # so the client can assert which headers the proxy forwarded.
                    received = {k.lower(): v for k, v in self.headers.items()}
                    data = json.dumps(received).encode()
                    self._send_response(200, 'application/json', data)
                elif self.path == '/set-cookies':
                    # Two Set-Cookie headers — must survive the proxy intact and
                    # not collapse to one (multi-value header preservation).
                    body = b'multi-cookie'
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/plain')
                    self.send_header('Set-Cookie', 'first=1; Path=/')
                    self.send_header('Set-Cookie', 'second=2; Path=/')
                    self.send_header('Content-Length', str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                elif self.path.startswith('/status/'):
                    try:
                        code = int(self.path.rsplit('/', 1)[1])
                    except ValueError:
                        code = 400
                    body = f'status {code}'.encode()
                    self._send_response(code, 'text/plain', body)
                elif self.path == '/sse':
                    # Server-Sent Events: text/event-stream triggers the proxy's
                    # streaming response path. Sent chunked, then closed.
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/event-stream')
                    self.send_header('Transfer-Encoding', 'chunked')
                    self.end_headers()
                    for i in range(3):
                        chunk = f"data: event-{i}\n\n".encode()
                        self.wfile.write(f"{len(chunk):X}\r\n".encode())
                        self.wfile.write(chunk)
                        self.wfile.write(b"\r\n")
                    self.wfile.write(b"0\r\n\r\n")
                else:
                    self._send_response(200, 'text/plain', b'Hello HTTP/1.1')

            def do_PUT(self):
                self._echo_method()

            def do_DELETE(self):
                self._echo_method()

            def do_PATCH(self):
                self._echo_method()

            def do_OPTIONS(self):
                body = b''
                self.send_response(204)
                self.send_header('Allow', 'GET,POST,PUT,DELETE,PATCH,HEAD,OPTIONS')
                self.send_header('Content-Length', '0')
                self.end_headers()

            def do_HEAD(self):
                # HEAD must return headers (incl. Content-Length) but no body.
                self.send_response(200)
                self.send_header('Content-Type', 'text/plain')
                self.send_header('Content-Length', str(len(b'Hello HTTP/1.1')))
                self.end_headers()

            def _echo_method(self):
                content_length = int(self.headers.get('Content-Length', 0))
                body = self.rfile.read(content_length) if content_length > 0 else b''
                # Echo the method name and body so the client can verify the verb
                # round-tripped through the proxy unchanged.
                payload = self.command.encode() + b':' + body
                self._send_response(200, 'text/plain', payload)

            def do_POST(self):
                content_length = int(self.headers.get('Content-Length', 0))
                body = self.rfile.read(content_length) if content_length > 0 else b''
                content_type = self.headers.get('Content-Type', '')

                if self.path == '/echo':
                    self._send_response(200, content_type, body)
                elif self.path == '/json-echo':
                    try:
                        data = json.loads(body)
                        data['received'] = True
                        resp_body = json.dumps(data).encode()
                        self._send_response(200, 'application/json', resp_body)
                    except Exception:
                        self.send_error(400, 'Bad JSON')
                elif self.path == '/proto-echo':
                    try:
                        msg = EchoMessage.decode(body)
                        resp_body = EchoMessage.encode(f"Echo: {msg}")
                        self._send_response(200, 'application/x-protobuf', resp_body)
                    except Exception:
                        self.send_error(400, 'Bad Proto')
                else:
                    self._send_response(200, 'text/plain', body)

        httpd = ThreadingHTTPServer((self.target_host, port), Handler)
        t = threading.Thread(target=httpd.serve_forever, daemon=True)
        t.start()
        self._add_server("HTTP/1.1", port, lambda: httpd.shutdown())
        return port

    # --- HTTPS/1.1 server (TLS, no h2 ALPN) — for CONNECT-tunnel tests ---
    def start_https1(self):
        """A plain HTTPS/1.1 server (no ALPN h2 negotiation).

        Used by :func:`test_https_connect` to verify that the proxy correctly
        handles the CONNECT tunnel + MITM TLS path without any HTTP/2 involvement.
        Clients receive HTTP/1.1 responses even when the proxy is in MITM mode.
        """
        import trustme as _trustme
        port = free_port()

        _ca    = _trustme.CA()
        _cert  = _ca.issue_cert("127.0.0.1", "localhost", self.target_host)
        import tempfile as _tempfile
        _cfile = _tempfile.NamedTemporaryFile(suffix='.pem', delete=False)
        _kfile = _tempfile.NamedTemporaryFile(suffix='.pem', delete=False)
        _cert.cert_chain_pems[0].write_to_path(_cfile.name)
        _cert.private_key_pem.write_to_path(_kfile.name)
        _cfile.close()
        _kfile.close()

        from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler

        class _Handler(BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'
            def log_message(self, fmt, *args): pass  # silence access log
            def do_GET(self):
                body = b'Hello HTTPS/1.1'
                self.send_response(200)
                self.send_header('Content-Type', 'text/plain')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        ctx.load_cert_chain(_cfile.name, _kfile.name)
        # Deliberately omit h2 from ALPN so we only speak HTTP/1.1 over TLS.

        httpsd = ThreadingHTTPServer((self.target_host, port), _Handler)
        httpsd.socket = ctx.wrap_socket(httpsd.socket, server_side=True)
        t = threading.Thread(target=httpsd.serve_forever, daemon=True)
        t.start()
        self._add_server("HTTPS/1.1", port, lambda: httpsd.shutdown())
        return port

    # --- HTTP/2 server (Hypercorn) ---
    def start_http2(self):
        port = free_port()

        async def app(scope, receive, send):
            assert scope['type'] == 'http'
            path = scope['path']
            method = scope['method']

            status = 200
            headers = [(b'content-type', b'text/plain')]
            body = b'Hello HTTP/2'

            if method == 'GET':
                if path == '/gzip':
                    body = gzip.compress(b'Hello gzip')
                    headers.append((b'content-encoding', b'gzip'))
                elif path == '/deflate':
                    body = zlib.compress(b'Hello deflate')
                    headers.append((b'content-encoding', b'deflate'))
                elif path == '/brotli':
                    body = brotli.compress(b'Hello brotli')
                    headers.append((b'content-encoding', b'br'))
                elif path == '/zstd':
                    body = zstandard.compress(b'Hello zstd')
                    headers.append((b'content-encoding', b'zstd'))
                elif path == '/json':
                    body = json.dumps({'message': 'Hello JSON'}).encode()
                    headers = [(b'content-type', b'application/json')]
                elif path == '/proto':
                    body = EchoMessage.encode('Hello Proto')
                    headers = [(b'content-type', b'application/x-protobuf')]
                elif path == '/chunked':
                    headers = [(b'content-type', b'text/plain')]
                    await send({
                        'type': 'http.response.start',
                        'status': 200,
                        'headers': headers,
                    })
                    for chunk in [b'Hello ', b'chunked ', b'world!']:
                        await send({
                            'type': 'http.response.body',
                            'body': chunk,
                            'more_body': True,
                        })
                    await send({
                        'type': 'http.response.body',
                        'body': b'',
                        'more_body': False,
                    })
                    return
                elif path == '/keep-alive':
                    body = b'Hello keepalive'
                elif path == '/large':
                    body = b'A' * 1024 * 1024
                elif path == '/headers':
                    received = {k.decode().lower(): v.decode() for k, v in scope['headers']}
                    body = json.dumps(received).encode()
                    headers = [(b'content-type', b'application/json')]
                elif path == '/set-cookies':
                    await send({'type': 'http.response.start', 'status': 200, 'headers': [
                        (b'content-type', b'text/plain'),
                        (b'set-cookie', b'first=1; Path=/'),
                        (b'set-cookie', b'second=2; Path=/'),
                    ]})
                    await send({'type': 'http.response.body', 'body': b'multi-cookie', 'more_body': False})
                    return
                elif path.startswith('/status/'):
                    try:
                        status = int(path.rsplit('/', 1)[1])
                    except ValueError:
                        status = 400
                    body = f'status {status}'.encode()
                elif path == '/sse':
                    await send({'type': 'http.response.start', 'status': 200,
                                'headers': [(b'content-type', b'text/event-stream')]})
                    for i in range(3):
                        await send({'type': 'http.response.body',
                                    'body': f'data: event-{i}\n\n'.encode(), 'more_body': True})
                    await send({'type': 'http.response.body', 'body': b'', 'more_body': False})
                    return
            elif method in ('PUT', 'DELETE', 'PATCH'):
                body_chunks = []
                more_body = True
                while more_body:
                    message = await receive()
                    body_chunks.append(message.get('body', b''))
                    more_body = message.get('more_body', False)
                body = method.encode() + b':' + b''.join(body_chunks)
            elif method == 'OPTIONS':
                status = 204
                headers = [(b'allow', b'GET,POST,PUT,DELETE,PATCH,HEAD,OPTIONS')]
                body = b''
            elif method == 'HEAD':
                body = b''
            elif method == 'POST':
                body_chunks = []
                more_body = True
                while more_body:
                    message = await receive()
                    body_chunks.append(message.get('body', b''))
                    more_body = message.get('more_body', False)
                request_body = b''.join(body_chunks)
                if path == '/echo':
                    body = request_body
                elif path == '/json-echo':
                    try:
                        data = json.loads(request_body)
                        data['received'] = True
                        body = json.dumps(data).encode()
                        headers = [(b'content-type', b'application/json')]
                    except:
                        status = 400
                        body = b'Bad JSON'
                elif path == '/proto-echo':
                    try:
                        msg = EchoMessage.decode(request_body)
                        body = EchoMessage.encode(f"Echo: {msg}")
                        headers = [(b'content-type', b'application/x-protobuf')]
                    except:
                        status = 400
                        body = b'Bad Proto'
                else:
                    body = request_body
            else:
                status = 405
                body = b'Method Not Allowed'

            await send({
                'type': 'http.response.start',
                'status': status,
                'headers': headers,
            })
            await send({
                'type': 'http.response.body',
                'body': body,
                'more_body': False,
            })

        config = HypercornConfig()
        config.bind = [f"{self.target_host}:{port}"]
        config.alpn_protocols = ["h2"]
        # Serve real TLS so the https:// h2 target is reachable through the proxy's
        # MITM (the client does CONNECT + TLS, so the origin must speak TLS too).
        # A self-signed cert via trustme; the proxy accepts it when run with
        # OPROXY_INSECURE_UPSTREAM=1.
        import trustme
        _ca = trustme.CA()
        _server_cert = _ca.issue_cert("127.0.0.1", "localhost", self.target_host)
        self._http2_cert = tempfile.NamedTemporaryFile(suffix=".pem", delete=False)
        self._http2_key = tempfile.NamedTemporaryFile(suffix=".pem", delete=False)
        _server_cert.cert_chain_pems[0].write_to_path(self._http2_cert.name)
        _server_cert.private_key_pem.write_to_path(self._http2_key.name)
        config.certfile = self._http2_cert.name
        config.keyfile = self._http2_key.name

        loop = asyncio.new_event_loop()
        t = threading.Thread(target=loop.run_forever, daemon=True)
        t.start()

        async def _serve():
            # shutdown_trigger must be an async callable (returns awaitable).
            # _shutdown_event is a threading.Event; poll it from the async loop.
            async def _shutdown_trigger():
                while not _shutdown_event.is_set():
                    await asyncio.sleep(0.05)

            await hypercorn_serve(
                app,
                config,
                shutdown_trigger=_shutdown_trigger
            )
        asyncio.run_coroutine_threadsafe(_serve(), loop)
        time.sleep(1)

        def shutdown():
            # Setting the global event causes _shutdown_trigger to return,
            # which signals Hypercorn to stop gracefully.
            _shutdown_event.set()
        self._add_server("HTTP/2", port, shutdown)
        return port

    # --- WebSocket echo server ---
    def start_websocket(self):
        port = free_port()
        server_holder = {}
        async def echo(websocket):
            async for message in websocket:
                await websocket.send(message)
        async def _serve():
            server = await websockets.serve(echo, self.target_host, port)
            server_holder['server'] = server
            await server.wait_closed()
        loop = asyncio.new_event_loop()
        t = threading.Thread(target=loop.run_forever, daemon=True)
        t.start()
        asyncio.run_coroutine_threadsafe(_serve(), loop)
        time.sleep(0.5)
        def shutdown():
            async def _stop():
                server = server_holder.get('server')
                if server:
                    server.close()
                    await server.wait_closed()
                loop.call_soon_threadsafe(loop.stop)
            asyncio.run_coroutine_threadsafe(_stop(), loop)
        self._add_server("WebSocket", port, shutdown)
        return port

    # --- gRPC echo server ---
    def start_grpc(self):
        port = free_port()
        gen_dir = os.path.join(os.path.dirname(__file__), "__generated_grpc__")
        os.makedirs(gen_dir, exist_ok=True)
        proto_path = os.path.join(gen_dir, "echo.proto")
        with open(proto_path, "w") as f:
            f.write("""
syntax = "proto3";
package echo;

service EchoService {
  rpc UnaryEcho (EchoRequest) returns (EchoResponse);
  rpc ServerStreamingEcho (EchoRequest) returns (stream EchoResponse);
  rpc ClientStreamingEcho (stream EchoRequest) returns (EchoResponse);
  rpc BidirectionalStreamingEcho (stream EchoRequest) returns (stream EchoResponse);
}

message EchoRequest {
  string message = 1;
}

message EchoResponse {
  string message = 1;
}
""")
        pb2_file = os.path.join(gen_dir, "echo_pb2.py")
        if not os.path.exists(pb2_file):
            ret = protoc.main([
                'grpc_tools.protoc',
                '-I', gen_dir,
                '--python_out=' + gen_dir,
                '--grpc_python_out=' + gen_dir,
                proto_path,
            ])
            if ret != 0:
                raise RuntimeError("protoc compilation failed")
        sys.path.insert(0, gen_dir)
        import echo_pb2
        import echo_pb2_grpc

        class EchoServicer(echo_pb2_grpc.EchoServiceServicer):
            def UnaryEcho(self, request, context):
                return echo_pb2.EchoResponse(message=request.message)
            def ServerStreamingEcho(self, request, context):
                for _ in range(3):
                    yield echo_pb2.EchoResponse(message=request.message)
            def ClientStreamingEcho(self, request_iterator, context):
                msgs = [req.message for req in request_iterator]
                return echo_pb2.EchoResponse(message=",".join(msgs))
            def BidirectionalStreamingEcho(self, request_iterator, context):
                for req in request_iterator:
                    yield echo_pb2.EchoResponse(message=req.message)

        server = grpc.server(futures.ThreadPoolExecutor(max_workers=10))
        echo_pb2_grpc.add_EchoServiceServicer_to_server(EchoServicer(), server)
        # Use TLS so the gRPC client goes through CONNECT (insecure_channel bypasses
        # the proxy for loopback addresses in many grpcio builds).
        import trustme as _trustme
        _grpc_ca = _trustme.CA()
        _grpc_srv_cert = _grpc_ca.issue_cert("127.0.0.1", "localhost", self.target_host)
        _key_pem = _grpc_srv_cert.private_key_pem.bytes()
        _cert_pem = b"".join(b.bytes() for b in _grpc_srv_cert.cert_chain_pems)
        server_cred = grpc.ssl_server_credentials([(_key_pem, _cert_pem)])
        server.add_secure_port(f"{self.target_host}:{port}", server_cred)
        server.start()
        # Expose the CA PEM so test_grpc can include it in root_certificates.
        self.grpc_ca_pem = _grpc_ca.cert_pem.bytes()
        self._add_server("gRPC", port, lambda: server.stop(0))
        return port

# ----------------------------------------------------------------------
# Test functions
# ----------------------------------------------------------------------
def make_test_url(base, path):
    return f"{base.rstrip('/')}{path}"

# HTTP/1.1 tests
def test_http1_basic(proxy_url, target_url, timeout, verbose=False):
    try:
        r = requests.get(target_url, proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout)
        if r.status_code == 200 and b'Hello' in r.content:
            print("[PASS] HTTP/1.1 GET")
            return True
        else:
            print(f"[FAIL] HTTP/1.1 GET: status={r.status_code}")
            return False
    except Exception as e:
        print(f"[FAIL] HTTP/1.1 GET: {e}")
        if verbose:
            traceback.print_exc()
        return False

def test_http1_encoding(proxy_url, base_url, path, encoding, expected_body, timeout, verbose):
    url = make_test_url(base_url, path)
    try:
        r = requests.get(url, proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout)
        if r.status_code == 200 and r.content == expected_body:
            print(f"[PASS] HTTP/1.1 {encoding}")
            return True
        else:
            print(f"[FAIL] HTTP/1.1 {encoding}: status={r.status_code}, body mismatch")
            if verbose:
                print(f"  Expected: {expected_body!r}")
                print(f"  Received: {r.content[:200]!r}")
            return False
    except Exception as e:
        print(f"[FAIL] HTTP/1.1 {encoding}: {e}")
        if verbose:
            traceback.print_exc()
        return False

def test_http1_json(proxy_url, base_url, timeout, verbose):
    url = make_test_url(base_url, '/json')
    try:
        r = requests.get(url, proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout)
        if r.status_code == 200 and r.json() == {'message': 'Hello JSON'}:
            print("[PASS] HTTP/1.1 JSON")
            return True
        else:
            print(f"[FAIL] HTTP/1.1 JSON")
            return False
    except Exception as e:
        print(f"[FAIL] HTTP/1.1 JSON: {e}")
        if verbose:
            traceback.print_exc()
        return False

def test_http1_proto(proxy_url, base_url, timeout, verbose):
    url = make_test_url(base_url, '/proto')
    try:
        r = requests.get(url, proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout)
        msg = EchoMessage.decode(r.content)
        if r.status_code == 200 and msg == 'Hello Proto':
            print("[PASS] HTTP/1.1 Proto")
            return True
        else:
            print(f"[FAIL] HTTP/1.1 Proto: status={r.status_code}, msg={msg}")
            return False
    except Exception as e:
        print(f"[FAIL] HTTP/1.1 Proto: {e}")
        if verbose:
            traceback.print_exc()
        return False

def test_http1_chunked(proxy_url, base_url, timeout, verbose):
    url = make_test_url(base_url, '/chunked')
    try:
        r = requests.get(url, proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout, stream=True)
        data = r.raw.read()
        if r.status_code == 200 and data == b'Hello chunked world!':
            print("[PASS] HTTP/1.1 Chunked")
            return True
        else:
            print(f"[FAIL] HTTP/1.1 Chunked: status={r.status_code}, body={data}")
            return False
    except Exception as e:
        print(f"[FAIL] HTTP/1.1 Chunked: {e}")
        if verbose:
            traceback.print_exc()
        return False

def test_http1_keepalive(proxy_url, base_url, timeout, verbose):
    """Test that the proxy supports HTTP/1.1 Keep-Alive by sending two
    requests on the same connection and ensuring both succeed."""
    target = make_test_url(base_url, '/keep-alive')
    parsed_proxy = urlparse(proxy_url)
    proxy_host = parsed_proxy.hostname
    proxy_port = parsed_proxy.port or 80

    try:
        import http.client

        # Open a single connection to the proxy
        conn = http.client.HTTPConnection(proxy_host, proxy_port, timeout=timeout)
        # First request
        conn.request('GET', target, headers={'Connection': 'keep-alive'})
        resp1 = conn.getresponse()
        body1 = resp1.read()

        if resp1.status != 200 or body1 != b'Hello keepalive':
            print(f"[FAIL] HTTP/1.1 Keep-Alive: first request failed status={resp1.status} body={body1!r}")
            conn.close()
            return False

        # Check if proxy indicates it will close the connection
        if resp1.will_close or resp1.getheader('Connection', '').lower() == 'close':
            print("[FAIL] HTTP/1.1 Keep-Alive: proxy closed connection after first request")
            conn.close()
            return False

        # Second request on the same connection
        conn.request('GET', target, headers={'Connection': 'keep-alive'})
        resp2 = conn.getresponse()
        body2 = resp2.read()
        conn.close()

        if resp2.status == 200 and body2 == b'Hello keepalive':
            print("[PASS] HTTP/1.1 Keep-Alive")
            return True
        else:
            print(f"[FAIL] HTTP/1.1 Keep-Alive: second request failed status={resp2.status} body={body2!r}")
            return False

    except Exception as e:
        print(f"[FAIL] HTTP/1.1 Keep-Alive: {e}")
        if verbose:
            traceback.print_exc()
        return False

def test_http1_large(proxy_url, base_url, timeout, verbose):
    url = make_test_url(base_url, '/large')
    large_timeout = max(timeout * 6, 60)
    try:
        r = requests.get(url, proxies={'http': proxy_url, 'https': proxy_url}, timeout=large_timeout, stream=True)
        data = r.raw.read()
        expected_len = 1024 * 1024
        if r.status_code == 200 and len(data) == expected_len and all(b == 0x41 for b in data):
            print("[PASS] HTTP/1.1 Large (1MB)")
            return True
        else:
            print(f"[FAIL] HTTP/1.1 Large: status={r.status_code}, len={len(data)}")
            return False
    except Exception as e:
        print(f"[FAIL] HTTP/1.1 Large: {e}")
        if verbose:
            traceback.print_exc()
        return False

def test_http1_post_json(proxy_url, base_url, timeout, verbose):
    url = make_test_url(base_url, '/json-echo')
    payload = {'hello': 'world'}
    try:
        r = requests.post(url, json=payload, proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout)
        if r.status_code == 200:
            data = r.json()
            if data.get('hello') == 'world' and data.get('received'):
                print("[PASS] HTTP/1.1 POST JSON")
                return True
        print(f"[FAIL] HTTP/1.1 POST JSON: status={r.status_code}, body={r.text[:200]}")
        return False
    except Exception as e:
        print(f"[FAIL] HTTP/1.1 POST JSON: {e}")
        if verbose:
            traceback.print_exc()
        return False

def test_http1_post_proto(proxy_url, base_url, timeout, verbose):
    url = make_test_url(base_url, '/proto-echo')
    payload = EchoMessage.encode('Test Proto')
    try:
        r = requests.post(url, data=payload,
                          headers={'Content-Type': 'application/x-protobuf'},
                          proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout)
        if r.status_code == 200:
            resp_msg = EchoMessage.decode(r.content)
            if resp_msg == 'Echo: Test Proto':
                print("[PASS] HTTP/1.1 POST Proto")
                return True
        print(f"[FAIL] HTTP/1.1 POST Proto: status={r.status_code}")
        return False
    except Exception as e:
        print(f"[FAIL] HTTP/1.1 POST Proto: {e}")
        if verbose:
            traceback.print_exc()
        return False

# HTTP/2 tests – we disable SSL verification because the test server uses a self-signed cert.
# The proxy's own certificate (for client <-> proxy) is not involved in these tests.
def test_http2_encoding(proxy_url, base_url, path, encoding, expected_body, timeout, verbose):
    url = make_test_url(base_url, path)
    try:
        client = httpx.Client(http2=True, proxy=proxy_url, timeout=timeout, verify=False)
        r = client.get(url)
        if r.http_version == 'HTTP/2' and r.status_code == 200 and r.content == expected_body:
            print(f"[PASS] HTTP/2 {encoding}")
            return True
        else:
            print(f"[FAIL] HTTP/2 {encoding}: version={r.http_version} status={r.status_code}")
            if verbose:
                print(f"  Expected: {expected_body!r}")
                print(f"  Received: {r.content[:200]!r}")
            return False
    except Exception as e:
        print(f"[FAIL] HTTP/2 {encoding}: {e}")
        if verbose:
            traceback.print_exc()
        return False

def test_http2_json(proxy_url, base_url, timeout, verbose):
    url = make_test_url(base_url, '/json')
    try:
        client = httpx.Client(http2=True, proxy=proxy_url, timeout=timeout, verify=False)
        r = client.get(url)
        if r.http_version == 'HTTP/2' and r.status_code == 200 and r.json() == {'message': 'Hello JSON'}:
            print("[PASS] HTTP/2 JSON")
            return True
        else:
            print("[FAIL] HTTP/2 JSON")
            if verbose:
                print(f"  Status: {r.status_code}, body: {r.text[:200]}")
            return False
    except Exception as e:
        print(f"[FAIL] HTTP/2 JSON: {e}")
        if verbose:
            traceback.print_exc()
        return False

def test_http2_proto(proxy_url, base_url, timeout, verbose):
    url = make_test_url(base_url, '/proto')
    try:
        client = httpx.Client(http2=True, proxy=proxy_url, timeout=timeout, verify=False)
        r = client.get(url)
        try:
            msg = EchoMessage.decode(r.content)
        except ValueError as decode_err:
            if verbose:
                print(f"  Raw content (hex): {r.content.hex()}")
                print(f"  Raw content (repr): {r.content!r}")
            raise decode_err
        if r.http_version == 'HTTP/2' and r.status_code == 200 and msg == 'Hello Proto':
            print("[PASS] HTTP/2 Proto")
            return True
        else:
            print(f"[FAIL] HTTP/2 Proto: version={r.http_version} status={r.status_code} msg={msg!r}")
            if verbose:
                print(f"  Raw content: {r.content!r}")
            return False
    except Exception as e:
        print(f"[FAIL] HTTP/2 Proto: {e}")
        if verbose:
            traceback.print_exc()
        return False

def test_http2_basic(proxy_url, target_url, timeout, verbose):
    try:
        client = httpx.Client(http2=True, proxy=proxy_url, timeout=timeout, verify=False)
        r = client.get(target_url)
        if r.http_version == 'HTTP/2' and r.status_code == 200:
            print("[PASS] HTTP/2 GET")
            return True
        else:
            print(f"[FAIL] HTTP/2 GET: version={r.http_version} status={r.status_code}")
            return False
    except Exception as e:
        print(f"[FAIL] HTTP/2 GET: {e}")
        if verbose:
            traceback.print_exc()
        return False

# WebSocket tests
def test_websocket_text(proxy_url, target_url, timeout, verbose):
    if not proxy_url.startswith('http://'):
        print("[FAIL] WebSocket Text: test requires HTTP proxy (http://host:port)")
        return False
    try:
        test_msg = "Hello WebSocket"
        opcode, resp = _websocket_roundtrip_via_http_proxy(
            proxy_url,
            target_url,
            test_msg,
            0x1,
            timeout,
        )
        if opcode == 0x1 and resp.decode('utf-8') == test_msg:
            print("[PASS] WebSocket Text")
            return True
        else:
            print("[FAIL] WebSocket Text: echo mismatch")
            return False
    except Exception as e:
        print(f"[FAIL] WebSocket Text: {e}")
        if verbose:
            traceback.print_exc()
        return False

def test_websocket_binary(proxy_url, target_url, timeout, verbose):
    if not proxy_url.startswith('http://'):
        print("[FAIL] WebSocket Binary: test requires HTTP proxy (http://host:port)")
        return False
    try:
        payload = b'\x00\x01\x02\xfe\xff'
        opcode, resp = _websocket_roundtrip_via_http_proxy(
            proxy_url,
            target_url,
            payload,
            0x2,
            timeout,
        )
        if opcode == 0x2 and resp == payload:
            print("[PASS] WebSocket Binary")
            return True
        else:
            print("[FAIL] WebSocket Binary: echo mismatch")
            return False
    except Exception as e:
        print(f"[FAIL] WebSocket Binary: {e}")
        if verbose:
            traceback.print_exc()
        return False

def test_websocket_close(proxy_url, target_url, timeout, verbose):
    if not proxy_url.startswith('http://'):
        print("[FAIL] WebSocket Close: test requires HTTP proxy (http://host:port)")
        return False
    try:
        with _open_websocket_via_http_proxy(proxy_url, target_url, timeout) as sock:
            _send_ws_frame(sock, struct.pack('!H', 1000), 0x8)
            try:
                opcode, payload = _recv_ws_frame(sock, timeout)
            except RuntimeError as e:
                if "truncated WebSocket frame header" in str(e):
                    print("[PASS] WebSocket Close")
                    return True
                raise
            if opcode == 0x8:
                print("[PASS] WebSocket Close")
                return True
            print(f"[FAIL] WebSocket Close: expected close frame, got opcode={opcode} payload={payload!r}")
            return False
    except Exception as e:
        print(f"[FAIL] WebSocket Close: {e}")
        if verbose:
            traceback.print_exc()
        return False

def test_websocket_session_events(proxy_url, target_url, timeout, verbose):
    if not proxy_url.startswith('http://'):
        print("[FAIL] WebSocket Session Events: test requires HTTP proxy (http://host:port)")
        return False
    try:
        parsed_target = urlparse(target_url)
        with _open_websocket_via_http_proxy(proxy_url, target_url, timeout) as sock:
            _send_ws_frame(sock, "session-event-text", 0x1)
            text_opcode, text_payload = _recv_ws_frame(sock, timeout)
            _send_ws_frame(sock, b'\x10\x11\x12', 0x2)
            bin_opcode, bin_payload = _recv_ws_frame(sock, timeout)
            _send_ws_frame(sock, struct.pack('!H', 1000), 0x8)
            try:
                _recv_ws_frame(sock, timeout)
            except Exception:
                pass
        if text_opcode != 0x1 or text_payload != b"session-event-text":
            print("[FAIL] WebSocket Session Events: text echo mismatch")
            return False
        if bin_opcode != 0x2 or bin_payload != b'\x10\x11\x12':
            print("[FAIL] WebSocket Session Events: binary echo mismatch")
            return False

        def matches(exchange):
            req = exchange.get('request') or {}
            pc = _protocol_context(exchange)
            events = _event_types(exchange)
            uri = req.get('uri') or ''
            return (
                str(parsed_target.port) in uri
                and 'session-event-text' in json.dumps(exchange.get('events') or [])
                and pc.get('body_mode') == 'frames'
                and events.count('ws_frame') >= 2
            )

        exchange = _wait_for_recorded_session(proxy_url, matches, timeout, include_bodies=True)
        frames = exchange.get('ws_frames') or []
        response = exchange.get('response') or {}
        metrics = exchange.get('metrics') or {}
        if (
            len(frames) >= 2
            and 'ws_frame' in _event_types(exchange)
            and response.get('status') == 101
            and metrics.get('status_code') == 101
        ):
            print("[PASS] WebSocket Session Events")
            return True
        print("[FAIL] WebSocket Session Events: upgrade response or frame read model missing")
        if verbose:
            print(json.dumps(exchange, indent=2, default=str)[:2000])
        return False
    except Exception as e:
        print(f"[FAIL] WebSocket Session Events: {e}")
        if verbose:
            traceback.print_exc()
        return False

# gRPC test
def test_grpc(proxy_url, target_host, target_port, timeout, verbose=False,
              grpc_server_ca_pem=None, session_admin_proxy=None):
    gen_dir = os.path.join(os.path.dirname(__file__), "__generated_grpc__")
    if gen_dir not in sys.path:
        sys.path.insert(0, gen_dir)
    import echo_pb2
    import echo_pb2_grpc

    # Fetch the proxy's root CA cert so MITM-generated certs are trusted.
    # grpc.secure_channel always uses HTTP CONNECT through the proxy (unlike
    # insecure_channel which silently bypasses the proxy for loopback addresses).
    proxy_ca_pem = None
    try:
        from urllib.parse import urlparse as _urlparse
        _pa = _urlparse(proxy_url)
        _ca_url = f"http://{_pa.netloc}/admin/ca"
        import urllib.request as _ur
        proxy_ca_pem = _ur.urlopen(_ca_url, timeout=5).read()
    except Exception as _e:
        if verbose:
            print(f"[WARN] gRPC: could not fetch proxy CA ({_e}); TLS will fail")

    # Combine proxy CA (trusted in MITM mode) and server CA (trusted in tunnel
    # mode).  Including both is harmless: in MITM mode the client sees the
    # proxy's cert; in tunnel mode it sees the server's cert directly.
    combined_ca = b""
    if proxy_ca_pem:
        combined_ca += proxy_ca_pem
    if grpc_server_ca_pem:
        combined_ca += grpc_server_ca_pem
    channel_creds = grpc.ssl_channel_credentials(root_certificates=combined_ca or None)

    target = f"{target_host}:{target_port}"
    # Use the channel option rather than env-vars: grpcio's env-var proxy
    # support is unreliable for loopback addresses on many builds.
    channel_options = [('grpc.http_proxy', proxy_url)]
    try:
        with grpc.secure_channel(target, channel_creds, options=channel_options) as channel:
            stub = echo_pb2_grpc.EchoServiceStub(channel)
            req = echo_pb2.EchoRequest(message="hello")

            # unary
            resp = stub.UnaryEcho(req, timeout=timeout)
            assert resp.message == "hello", f"unary mismatch: {resp.message}"

            # server streaming
            responses = stub.ServerStreamingEcho(req, timeout=timeout)
            count = 0
            for r in responses:
                assert r.message == "hello", f"server stream mismatch: {r.message}"
                count += 1
            assert count == 3, f"server stream count {count} != 3"

            # client streaming
            def req_iter():
                for _ in range(3):
                    yield req
            resp = stub.ClientStreamingEcho(req_iter(), timeout=timeout)
            assert resp.message == "hello,hello,hello", f"client stream mismatch: {resp.message}"

            # bidirectional streaming
            def bidi_req():
                yield req
                yield echo_pb2.EchoRequest(message="world")
            bidi_responses = stub.BidirectionalStreamingEcho(bidi_req(), timeout=timeout)
            msgs = [r.message for r in bidi_responses]
            assert msgs == ["hello", "world"], f"bidi mismatch: {msgs}"

        if session_admin_proxy:
            def matches(exchange):
                req = exchange.get('request') or {}
                pc = _protocol_context(exchange)
                events = _event_types(exchange)
                inspector = exchange.get('inspector_data') or {}
                uri = req.get('uri') or ''
                return (
                    str(target_port) in uri
                    and ('EchoService' in uri or (req.get('headers') or {}).get('content-type', '').startswith('application/grpc'))
                    and (
                        pc.get('application') == 'grpc'
                        or 'grpc_message' in events
                        or bool(inspector.get('grpc'))
                    )
                )

            try:
                _wait_for_recorded_session(session_admin_proxy, matches, timeout, include_bodies=True)
            except AssertionError:
                sessions = _recorded_session_details(session_admin_proxy, timeout, include_bodies=True)
                tunnel_only = any(
                    (s.get('request') or {}).get('method') == 'CONNECT'
                    and f"{target_host}:{target_port}" in ((s.get('request') or {}).get('host') or (s.get('request') or {}).get('uri') or '')
                    for s in sessions
                )
                if tunnel_only:
                    print("[SKIP] gRPC Session Events: proxy recorded CONNECT tunnel only; enable MITM to assert decoded gRPC messages")
                    print("[PASS] gRPC (unary, server streaming, client streaming, bidirectional)")
                    return True
                raise

        print("[PASS] gRPC (unary, server streaming, client streaming, bidirectional)")
        return True
    except Exception as e:
        print(f"[FAIL] gRPC: {e}")
        if verbose:
            traceback.print_exc()
        return False

# SOCKS5 TCP test
def _socks5_http_get(proxy_url, target_host, target_port, timeout):
    parsed = urlparse(proxy_url)
    proxy_host = parsed.hostname
    proxy_port = parsed.port or 1080
    s = socks.socksocket()
    s.set_proxy(socks.SOCKS5, proxy_host, proxy_port)
    s.settimeout(timeout)
    try:
        s.connect((target_host, target_port))
        request = f"GET / HTTP/1.1\r\nHost: {target_host}:{target_port}\r\nConnection: close\r\n\r\n"
        s.sendall(request.encode())
        response = b""
        while True:
            chunk = s.recv(4096)
            if not chunk:
                break
            response += chunk
        return response
    finally:
        s.close()

def test_socks5_tcp(proxy_url, target_host, target_port, timeout, verbose=False):
    try:
        response = _socks5_http_get(proxy_url, target_host, target_port, timeout)
        if b"200 OK" in response and b"Hello HTTP/1.1" in response:
            print("[PASS] SOCKS5 TCP (HTTP over SOCKS5)")
            return True
        else:
            print("[FAIL] SOCKS5 TCP: unexpected response")
            if verbose:
                print(f"  Raw response (first 500 bytes): {response[:500]}")
            return False
    except Exception as e:
        print(f"[FAIL] SOCKS5 TCP: {e}")
        if verbose:
            traceback.print_exc()
        return False

def test_socks5_session_events(socks_proxy_url, http_proxy_url, target_host, target_port, timeout, verbose=False):
    if not http_proxy_url:
        print("[SKIP] SOCKS5 Session Events: requires --http-proxy for admin API")
        return None
    try:
        response = _socks5_http_get(socks_proxy_url, target_host, target_port, timeout)
        if b"200 OK" not in response or b"Hello HTTP/1.1" not in response:
            print("[FAIL] SOCKS5 Session Events: SOCKS HTTP request failed")
            return False

        def matches(exchange):
            req = exchange.get('request') or {}
            pc = _protocol_context(exchange)
            events = _event_types(exchange)
            uri = req.get('uri') or ''
            return (
                uri == f"socks5://{target_host}:{target_port}"
                and pc.get('downstream') == 'socks5'
                and pc.get('body_mode') == 'tunnel'
                and 'tunnel_opened' in events
                and 'tunnel_closed' in events
            )

        exchange = _wait_for_recorded_session(http_proxy_url, matches, timeout, include_bodies=True)
        closed = next((e for e in exchange.get('events', []) if e.get('type') == 'tunnel_closed'), {})
        if closed.get('bytes_up', 0) > 0 and closed.get('bytes_down', 0) > 0:
            print("[PASS] SOCKS5 Session Events")
            return True
        print("[FAIL] SOCKS5 Session Events: tunnel byte counters missing")
        if verbose:
            print(json.dumps(exchange, indent=2, default=str)[:2000])
        return False
    except Exception as e:
        print(f"[FAIL] SOCKS5 Session Events: {e}")
        if verbose:
            traceback.print_exc()
        return False

# SOCKS5 UDP test
def test_socks5_udp(proxy_url, target_host, target_port, timeout, verbose):
    parsed = urlparse(proxy_url)
    proxy_host = parsed.hostname
    proxy_port = parsed.port or 1080
    try:
        s = socks.socksocket(socket.AF_INET, socket.SOCK_DGRAM)
        s.set_proxy(socks.SOCKS5, proxy_host, proxy_port)
        s.settimeout(timeout)
        s.bind(('0.0.0.0', 0))
        message = b'Hello UDP'
        s.sendto(message, (target_host, target_port))
        data, addr = s.recvfrom(1024)
        s.close()
        if data == message:
            print("[PASS] SOCKS5 UDP")
            return True
        else:
            print(f"[FAIL] SOCKS5 UDP: received {data}")
            return False
    except socks.SOCKS5Error as e:
        if '0x07' in str(e):
            # UDP ASSOCIATE (cmd 0x07) is not implemented; skip rather than fail.
            print(f"[SKIP] SOCKS5 UDP: UDP ASSOCIATE not supported by this proxy")
            return None
        print(f"[FAIL] SOCKS5 UDP: {e}")
        if verbose:
            traceback.print_exc()
        return False
    except Exception as e:
        print(f"[FAIL] SOCKS5 UDP: {e}")
        if verbose:
            traceback.print_exc()
        return False

# HTTP/2 body / streaming / large — server-side handlers already exist
def test_http2_post_json(proxy_url, base_url, timeout, verbose):
    url = make_test_url(base_url, '/json-echo')
    payload = {'hello': 'h2'}
    try:
        client = httpx.Client(http2=True, proxy=proxy_url, timeout=timeout, verify=False)
        r = client.post(url, json=payload)
        if r.http_version == 'HTTP/2' and r.status_code == 200:
            data = r.json()
            if data.get('hello') == 'h2' and data.get('received'):
                print('[PASS] HTTP/2 POST JSON')
                return True
        print(f'[FAIL] HTTP/2 POST JSON: version={r.http_version} status={r.status_code} body={r.text[:200]}')
        return False
    except Exception as e:
        print(f'[FAIL] HTTP/2 POST JSON: {e}')
        if verbose:
            traceback.print_exc()
        return False


def test_http2_chunked(proxy_url, base_url, timeout, verbose):
    """HTTP/2 multi-frame streaming — server sends separate DATA frames; body
    must be reassembled correctly by the proxy and delivered intact."""
    url = make_test_url(base_url, '/chunked')
    try:
        client = httpx.Client(http2=True, proxy=proxy_url, timeout=timeout, verify=False)
        r = client.get(url)
        # HTTP/2 has no chunked transfer-encoding; frames are reassembled into
        # a single body by the time the client sees the response.
        expected = b'Hello chunked world!'
        if r.http_version == 'HTTP/2' and r.status_code == 200 and r.content == expected:
            print('[PASS] HTTP/2 Chunked (streaming DATA frames)')
            return True
        print(
            f'[FAIL] HTTP/2 Chunked: version={r.http_version} '
            f'status={r.status_code} body={r.content[:60]!r}'
        )
        return False
    except Exception as e:
        print(f'[FAIL] HTTP/2 Chunked: {e}')
        if verbose:
            traceback.print_exc()
        return False


def test_http2_large(proxy_url, base_url, timeout, verbose):
    """1 MB response over HTTP/2 — exercises FLOW_CONTROL window handling."""
    url = make_test_url(base_url, '/large')
    large_timeout = max(timeout * 6, 60)
    try:
        client = httpx.Client(http2=True, proxy=proxy_url, timeout=large_timeout, verify=False)
        r = client.get(url)
        expected = 1024 * 1024
        if (r.http_version == 'HTTP/2' and r.status_code == 200
                and len(r.content) == expected
                and all(b == 0x41 for b in r.content)):
            print('[PASS] HTTP/2 Large (1 MB)')
            return True
        print(f'[FAIL] HTTP/2 Large: version={r.http_version} status={r.status_code} len={len(r.content)}')
        return False
    except Exception as e:
        print(f'[FAIL] HTTP/2 Large: {e}')
        if verbose:
            traceback.print_exc()
        return False


# HTTPS CONNECT tunnel test
def test_https_connect(proxy_url, target_host, https_port, timeout, verbose):
    """Verify the proxy correctly handles HTTPS CONNECT + MITM.

    *requests* (HTTP/1.1 only) always uses CONNECT for https:// targets when a
    proxy is configured.  The proxy must complete the CONNECT handshake, perform
    MITM TLS, and forward the underlying HTTP/1.1 request upstream.
    """
    https_url = f'https://{target_host}:{https_port}/'
    try:
        r = requests.get(
            https_url,
            proxies={'http': proxy_url, 'https': proxy_url},
            timeout=timeout,
            verify=False,  # accept the proxy's MITM cert
        )
        if r.status_code == 200:
            print('[PASS] HTTPS CONNECT tunnel')
            return True
        print(f'[FAIL] HTTPS CONNECT tunnel: status={r.status_code} body={r.text[:100]!r}')
        return False
    except Exception as e:
        print(f'[FAIL] HTTPS CONNECT tunnel: {e}')
        if verbose:
            traceback.print_exc()
        return False


# HTTP/1.1 session recording assertion
def test_http1_session_events(proxy_url, base_url, timeout, verbose):
    """Verify that plain HTTP/1.1 requests are recorded in the session log
    with correct URI, status, and timing metrics."""
    sentinel = f'h1-sess-{int(time.time())}'
    url = make_test_url(base_url, f'/?sentinel={sentinel}')
    try:
        r = requests.get(
            url,
            proxies={'http': proxy_url, 'https': proxy_url},
            timeout=timeout,
        )
        if r.status_code != 200:
            print(f'[FAIL] HTTP/1.1 Session Events: request returned {r.status_code}')
            return False

        def matches(exchange):
            req  = exchange.get('request') or {}
            resp = exchange.get('response') or {}
            return sentinel in (req.get('uri') or '') and resp.get('status') == 200

        exchange = _wait_for_recorded_session(proxy_url, matches, timeout, include_bodies=True)
        req     = exchange.get('request')  or {}
        resp    = exchange.get('response') or {}
        metrics = exchange.get('metrics')  or {}
        if (
            resp.get('status') == 200
            and sentinel in (req.get('uri') or '')
            and metrics.get('latency_ms', -1) >= 0
        ):
            print('[PASS] HTTP/1.1 Session Events')
            return True
        print('[FAIL] HTTP/1.1 Session Events: session not properly recorded')
        if verbose:
            print(json.dumps(exchange, indent=2, default=str)[:2000])
        return False
    except Exception as e:
        print(f'[FAIL] HTTP/1.1 Session Events: {e}')
        if verbose:
            traceback.print_exc()
        return False


# ----------------------------------------------------------------------
# Robustness / regression / security cases (release-gate hardening)
# ----------------------------------------------------------------------
def test_status_passthrough(proxy_url, base_url, timeout, verbose, label='HTTP/1.1', http2=False):
    """Non-200 upstream statuses (404, 500, 503) must be forwarded verbatim,
    body intact — not rewritten to a generic proxy error."""
    try:
        for code in (404, 500, 503):
            url = make_test_url(base_url, f'/status/{code}')
            if http2:
                client = httpx.Client(http2=True, proxy=proxy_url, timeout=timeout, verify=False)
                r = client.get(url)
                if r.http_version != 'HTTP/2':
                    print(f"[FAIL] {label} Status Passthrough: not HTTP/2 ({r.http_version})")
                    return False
            else:
                r = requests.get(url, proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout)
            if r.status_code != code or r.text != f'status {code}':
                print(f"[FAIL] {label} Status Passthrough: {code} -> {r.status_code} body={r.text[:60]!r}")
                return False
        print(f"[PASS] {label} Status Passthrough (404/500/503)")
        return True
    except Exception as e:
        print(f"[FAIL] {label} Status Passthrough: {e}")
        if verbose:
            traceback.print_exc()
        return False


def test_multi_set_cookie(proxy_url, base_url, timeout, verbose, label='HTTP/1.1', http2=False):
    """Two Set-Cookie headers from upstream must both survive the proxy and not
    collapse into one — the multi-value header path (HeaderMap append/get_all)."""
    url = make_test_url(base_url, '/set-cookies')
    try:
        if http2:
            client = httpx.Client(http2=True, proxy=proxy_url, timeout=timeout, verify=False)
            r = client.get(url)
            cookies = [v for k, v in r.headers.multi_items() if k.lower() == 'set-cookie']
        else:
            r = requests.get(url, proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout)
            # requests joins duplicate headers with ", " — count via raw urllib3 msg.
            cookies = r.raw.headers.get_all('Set-Cookie') or []
        has_first = any('first=1' in c for c in cookies)
        has_second = any('second=2' in c for c in cookies)
        if r.status_code == 200 and len(cookies) >= 2 and has_first and has_second:
            print(f"[PASS] {label} Multi Set-Cookie")
            return True
        print(f"[FAIL] {label} Multi Set-Cookie: got {cookies!r}")
        return False
    except Exception as e:
        print(f"[FAIL] {label} Multi Set-Cookie: {e}")
        if verbose:
            traceback.print_exc()
        return False


def test_http1_methods(proxy_url, base_url, timeout, verbose):
    """PUT/DELETE/PATCH must round-trip with body; HEAD returns headers but no
    body; OPTIONS returns 204. Exercises the engine's body-mode inference."""
    proxies = {'http': proxy_url, 'https': proxy_url}
    try:
        for method in ('PUT', 'DELETE', 'PATCH'):
            r = requests.request(method, make_test_url(base_url, '/anything'),
                                 data=b'payload', proxies=proxies, timeout=timeout)
            if r.status_code != 200 or r.content != f'{method}:'.encode() + b'payload':
                print(f"[FAIL] HTTP/1.1 Methods: {method} -> {r.status_code} {r.content[:60]!r}")
                return False

        h = requests.head(make_test_url(base_url, '/'), proxies=proxies, timeout=timeout)
        if h.status_code != 200 or h.content != b'':
            print(f"[FAIL] HTTP/1.1 Methods: HEAD -> {h.status_code} body={h.content[:40]!r}")
            return False

        o = requests.options(make_test_url(base_url, '/'), proxies=proxies, timeout=timeout)
        if o.status_code != 204:
            print(f"[FAIL] HTTP/1.1 Methods: OPTIONS -> {o.status_code}")
            return False

        print("[PASS] HTTP/1.1 Methods (PUT/DELETE/PATCH/HEAD/OPTIONS)")
        return True
    except Exception as e:
        print(f"[FAIL] HTTP/1.1 Methods: {e}")
        if verbose:
            traceback.print_exc()
        return False


def test_large_request_upload(proxy_url, base_url, timeout, verbose):
    """A 1 MB request body must be buffered and forwarded intact (request-side
    counterpart to the large-response test)."""
    url = make_test_url(base_url, '/echo')
    payload = b'B' * (1024 * 1024)
    large_timeout = max(timeout * 6, 60)
    try:
        r = requests.post(url, data=payload,
                          headers={'Content-Type': 'application/octet-stream'},
                          proxies={'http': proxy_url, 'https': proxy_url}, timeout=large_timeout)
        if r.status_code == 200 and r.content == payload:
            print("[PASS] HTTP/1.1 Large Upload (1MB request body)")
            return True
        print(f"[FAIL] HTTP/1.1 Large Upload: status={r.status_code} len={len(r.content)}")
        return False
    except Exception as e:
        print(f"[FAIL] HTTP/1.1 Large Upload: {e}")
        if verbose:
            traceback.print_exc()
        return False


def test_sse_streaming(proxy_url, base_url, timeout, verbose):
    """text/event-stream responses go through the proxy's streaming path; all
    events must arrive in order."""
    url = make_test_url(base_url, '/sse')
    try:
        r = requests.get(url, proxies={'http': proxy_url, 'https': proxy_url},
                         timeout=timeout, stream=True)
        data = r.raw.read()
        ok = (r.status_code == 200
              and r.headers.get('Content-Type', '').startswith('text/event-stream')
              and b'data: event-0' in data
              and b'data: event-1' in data
              and b'data: event-2' in data)
        if ok:
            print("[PASS] HTTP/1.1 SSE Streaming")
            return True
        print(f"[FAIL] HTTP/1.1 SSE Streaming: status={r.status_code} ct={r.headers.get('Content-Type')!r} body={data[:80]!r}")
        return False
    except Exception as e:
        print(f"[FAIL] HTTP/1.1 SSE Streaming: {e}")
        if verbose:
            traceback.print_exc()
        return False


def test_internal_headers_stripped(proxy_url, base_url, timeout, verbose):
    """SECURITY: a client must not be able to spoof the proxy's internal
    `x-oproxy-*` side-channel headers. They must be stripped before forwarding,
    while ordinary custom headers pass through unchanged."""
    url = make_test_url(base_url, '/headers')
    spoofed = {
        'x-oproxy-destination': 'http://evil.example.com',
        'x-oproxy-mock-response': '{"status":200}',
        'X-Custom-Header': 'keep-me',
    }
    try:
        r = requests.get(url, headers=spoofed,
                         proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout)
        if r.status_code != 200:
            print(f"[FAIL] Internal Header Stripping: status={r.status_code}")
            return False
        received = r.json()
        leaked = [k for k in received if k.lower().startswith('x-oproxy-')]
        if leaked:
            print(f"[FAIL] Internal Header Stripping: client spoofed headers reached upstream: {leaked}")
            return False
        if received.get('x-custom-header') != 'keep-me':
            print(f"[FAIL] Internal Header Stripping: custom header not forwarded: {received.get('x-custom-header')!r}")
            return False
        print("[PASS] Internal Header Stripping (x-oproxy-* spoofing blocked)")
        return True
    except Exception as e:
        print(f"[FAIL] Internal Header Stripping: {e}")
        if verbose:
            traceback.print_exc()
        return False


def test_concurrent_requests(proxy_url, base_url, timeout, verbose, count=20):
    """Many simultaneous requests must all succeed — smoke test for the
    connection pool and per-request isolation under load."""
    url = make_test_url(base_url, '/')

    def one(_):
        try:
            r = requests.get(url, proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout)
            return r.status_code == 200 and b'Hello' in r.content
        except Exception:
            return False

    try:
        with futures.ThreadPoolExecutor(max_workers=count) as pool:
            results = list(pool.map(one, range(count)))
        if all(results):
            print(f"[PASS] Concurrent Requests ({count} parallel)")
            return True
        print(f"[FAIL] Concurrent Requests: {results.count(False)}/{count} failed")
        return False
    except Exception as e:
        print(f"[FAIL] Concurrent Requests: {e}")
        if verbose:
            traceback.print_exc()
        return False


# ----------------------------------------------------------------------
# Manipulation-feature cases (drive the admin API, then assert behavior)
# ----------------------------------------------------------------------
_sentinel_seq = [0]

def _uniq(prefix):
    """A process-unique sentinel path. Guarantees H1 and H2 variants of the
    same test never share a path (which would cross-match each other's rule)."""
    _sentinel_seq[0] += 1
    return f'/{prefix}-{int(time.time())}-{_sentinel_seq[0]}'


def _gate_get(proxy_url, url, timeout, http2=False, **kwargs):
    """GET `url` through the proxy over HTTP/1.1 (requests) or HTTP/2 (httpx).
    Both return objects exposing .status_code / .headers / .text / .content."""
    if http2:
        client = httpx.Client(http2=True, proxy=proxy_url, timeout=timeout, verify=False)
        return client.get(url, **kwargs)
    return requests.get(url, proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout, **kwargs)


def test_mock_rule(proxy_url, base_url, timeout, verbose, http2=False, label='HTTP/1.1'):
    """Install a Mock rule via the admin API, verify the proxy short-circuits a
    matching request with the canned response (never reaching upstream), then
    clean the rule up. Runs over HTTP/1.1 or HTTP/2 (same middleware pipeline)."""
    if not proxy_url.startswith('http://'):
        print(f"[SKIP] {label} Mock Rule: requires HTTP proxy for admin API")
        return None
    sentinel = _uniq('mock')
    rule = {
        'name': 'release-gate-mock',
        'enabled': True,
        # Match by unique path only; host carries a port in the Host header, so
        # an exact host match is unreliable. The sentinel path keeps this scoped.
        'location': {'path': sentinel, 'mode': 'glob'},
        'responses': [{
            'status': 299,
            'headers': {'X-Mocked-By': 'oproxy-test'},
            'body': 'mocked-body',
            'delay_ms': 0,
        }],
    }
    rule_id = None
    try:
        cr = _admin_request(proxy_url, 'POST', '/admin/mock/rules', timeout, json=rule)
        if cr.status_code not in (200, 201):
            print(f"[FAIL] {label} Mock Rule: create returned {cr.status_code} {cr.text[:120]}")
            return False
        rule_id = (cr.json() or {}).get('id')

        r = _gate_get(proxy_url, make_test_url(base_url, sentinel), timeout, http2)
        if r.status_code == 299 and r.text == 'mocked-body' and r.headers.get('X-Mocked-By') == 'oproxy-test':
            print(f"[PASS] {label} Mock Rule (canned response served)")
            return True
        print(f"[FAIL] {label} Mock Rule: status={r.status_code} body={r.text[:80]!r} hdr={r.headers.get('X-Mocked-By')!r}")
        return False
    except Exception as e:
        print(f"[FAIL] {label} Mock Rule: {e}")
        if verbose:
            traceback.print_exc()
        return False
    finally:
        if rule_id:
            try:
                _admin_request(proxy_url, 'DELETE', f'/admin/mock/rules/{rule_id}', timeout)
            except Exception:
                pass


def test_access_block(proxy_url, base_url, timeout, verbose, http2=False, label='HTTP/1.1'):
    """Install an access-control Block rule scoped to a unique path, verify the
    proxy returns 403 for it (and still serves other paths), then clean up.
    Runs over HTTP/1.1 or HTTP/2."""
    if not proxy_url.startswith('http://'):
        print(f"[SKIP] {label} Access Block: requires HTTP proxy for admin API")
        return None
    sentinel = _uniq('blocked')
    rule = {
        'name': 'release-gate-block',
        'enabled': True,
        # Match by unique path only (see test_mock_rule for the host/port note).
        'location': {'path': sentinel, 'mode': 'glob'},
        'action': 'block',
    }
    rule_id = None
    try:
        cr = _admin_request(proxy_url, 'POST', '/admin/access-rules', timeout, json=rule)
        if cr.status_code not in (200, 201):
            print(f"[FAIL] {label} Access Block: create returned {cr.status_code} {cr.text[:120]}")
            return False
        rule_id = (cr.json() or {}).get('id')

        blocked = _gate_get(proxy_url, make_test_url(base_url, sentinel), timeout, http2)
        if blocked.status_code != 403:
            print(f"[FAIL] {label} Access Block: blocked path returned {blocked.status_code}, expected 403")
            return False

        allowed = _gate_get(proxy_url, make_test_url(base_url, '/'), timeout, http2)
        if allowed.status_code != 200:
            print(f"[FAIL] {label} Access Block: non-blocked path returned {allowed.status_code}")
            return False

        print(f"[PASS] {label} Access Block (403 on blocked path, 200 otherwise)")
        return True
    except Exception as e:
        print(f"[FAIL] {label} Access Block: {e}")
        if verbose:
            traceback.print_exc()
        return False
    finally:
        if rule_id:
            try:
                _admin_request(proxy_url, 'DELETE', f'/admin/access-rules/{rule_id}', timeout)
            except Exception:
                pass


def test_rewrite_rule(proxy_url, base_url, timeout, verbose, http2=False, label='HTTP/1.1'):
    """Install a rewrite rule-set that injects a response header on a matching
    path; verify the client sees the injected header. Then clean up. Runs over
    HTTP/1.1 or HTTP/2."""
    if not proxy_url.startswith('http://'):
        print(f"[SKIP] {label} Rewrite Rule: requires HTTP proxy for admin API")
        return None
    sentinel = _uniq('rewrite')
    rule = {
        'name': 'release-gate-rewrite',
        'enabled': True,
        'location': {'path': sentinel, 'mode': 'glob'},
        'applies_to': 'response',
        'actions': [{'type': 'set_header', 'name': 'X-Rewritten', 'value': 'yes'}],
    }
    rule_id = None
    try:
        cr = _admin_request(proxy_url, 'POST', '/admin/rule-sets', timeout, json=rule)
        if cr.status_code not in (200, 201):
            print(f"[FAIL] {label} Rewrite Rule: create returned {cr.status_code} {cr.text[:120]}")
            return False
        rule_id = (cr.json() or {}).get('id')
        r = _gate_get(proxy_url, make_test_url(base_url, sentinel), timeout, http2)
        if r.status_code == 200 and r.headers.get('X-Rewritten') == 'yes':
            print(f"[PASS] {label} Rewrite Rule (response header injected)")
            return True
        print(f"[FAIL] {label} Rewrite Rule: status={r.status_code} X-Rewritten={r.headers.get('X-Rewritten')!r}")
        return False
    except Exception as e:
        print(f"[FAIL] {label} Rewrite Rule: {e}")
        if verbose:
            traceback.print_exc()
        return False
    finally:
        if rule_id:
            try:
                _admin_request(proxy_url, 'DELETE', f'/admin/rule-sets/{rule_id}', timeout)
            except Exception:
                pass


def test_websocket_mock(proxy_url, ws_base, timeout, verbose):
    """Install a Mock rule with a WebSocketScript behavior, open a WebSocket via
    the proxy to a matching path, and confirm the proxy serves the scripted
    frame instead of proxying to a real server. Then clean up."""
    if not proxy_url.startswith('http://'):
        print("[SKIP] WebSocket Mock: requires HTTP proxy for admin API")
        return None
    sentinel = _uniq('wsmock')
    rule = {
        'name': 'release-gate-ws-mock',
        'enabled': True,
        'location': {'path': sentinel, 'mode': 'glob'},
        'behavior': {
            'type': 'web_socket_script',
            # Small delay so the scripted frame arrives AFTER the 101 handshake
            # response (otherwise it can coalesce into the same TCP read and be
            # swallowed by the handshake parser).
            'frames': [{'opcode': 1, 'payload': 'mocked-ws-frame', 'delay_ms': 300}],
        },
    }
    rule_id = None
    try:
        cr = _admin_request(proxy_url, 'POST', '/admin/mock/rules', timeout, json=rule)
        if cr.status_code not in (200, 201):
            print(f"[FAIL] WebSocket Mock: create returned {cr.status_code} {cr.text[:160]}")
            return False
        rule_id = (cr.json() or {}).get('id')

        target = f"{ws_base}{sentinel}"
        with _open_websocket_via_http_proxy(proxy_url, target, timeout) as sock:
            opcode, payload = _recv_ws_frame(sock, timeout)
        if opcode == 0x1 and payload == b'mocked-ws-frame':
            print("[PASS] WebSocket Mock (scripted frame served)")
            return True
        print(f"[FAIL] WebSocket Mock: opcode={opcode} payload={payload!r}")
        return False
    except Exception as e:
        print(f"[FAIL] WebSocket Mock: {e}")
        if verbose:
            traceback.print_exc()
        return False
    finally:
        if rule_id:
            try:
                _admin_request(proxy_url, 'DELETE', f'/admin/mock/rules/{rule_id}', timeout)
            except Exception:
                pass


def test_throttling(proxy_url, base_url, timeout, verbose):
    """Enable a latency throttle and confirm a request is measurably delayed,
    then disable it. Throttling is global, so this always restores the config."""
    if not proxy_url.startswith('http://'):
        print("[SKIP] Throttling: requires HTTP proxy for admin API")
        return None
    added_latency_ms = 700
    try:
        sr = _admin_request(proxy_url, 'PUT', '/admin/throttling', timeout,
                            json={'latency_ms': added_latency_ms, 'bandwidth_limit_kbps': 0, 'enabled': True})
        if sr.status_code not in (200, 201, 204):
            print(f"[FAIL] Throttling: enable returned {sr.status_code}")
            return False
        start = time.time()
        r = requests.get(make_test_url(base_url, '/'),
                         proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout + 5)
        elapsed = time.time() - start
        if r.status_code == 200 and elapsed >= (added_latency_ms / 1000.0) * 0.7:
            print(f"[PASS] Throttling (latency ~{elapsed*1000:.0f}ms >= {added_latency_ms}ms target)")
            return True
        print(f"[FAIL] Throttling: status={r.status_code} elapsed={elapsed*1000:.0f}ms (expected >= {added_latency_ms}ms)")
        return False
    except Exception as e:
        print(f"[FAIL] Throttling: {e}")
        if verbose:
            traceback.print_exc()
        return False
    finally:
        try:
            _admin_request(proxy_url, 'PUT', '/admin/throttling', timeout,
                           json={'latency_ms': 0, 'bandwidth_limit_kbps': 0, 'enabled': False})
        except Exception:
            pass


def test_map_local(proxy_url, target_host, target_port, timeout, verbose):
    """Upload a managed fixture via the admin API, install a Map Local rule that
    serves it for a matching path, and verify its contents are returned. Using
    the fixtures API (not a local temp file) avoids any filesystem-visibility gap
    between this test process and the proxy process."""
    if not proxy_url.startswith('http://'):
        print("[SKIP] Map Local: requires HTTP proxy for admin API")
        return None
    sentinel = _uniq('maplocal')
    fixture_name = f'gate-{int(time.time())}.txt'
    content = b'map-local-served-content'
    rule_id = None
    fixture_uploaded = False
    try:
        up = _admin_request(proxy_url, 'POST', f'/admin/map-local-rules/fixtures/{fixture_name}',
                            timeout, data=content,
                            headers={'Content-Type': 'application/octet-stream'})
        if up.status_code not in (200, 201):
            print(f"[FAIL] Map Local: fixture upload returned {up.status_code} {up.text[:160]}")
            return False
        fixture_uploaded = True

        rule = {
            'name': 'release-gate-maplocal',
            'enabled': True,
            'location': {'path': sentinel, 'mode': 'glob'},
            'file_path': fixture_name,  # relative → resolved from storage/map-local/
        }
        cr = _admin_request(proxy_url, 'POST', '/admin/map-local-rules', timeout, json=rule)
        if cr.status_code not in (200, 201):
            print(f"[FAIL] Map Local: rule create returned {cr.status_code} {cr.text[:160]}")
            return False
        rule_id = (cr.json() or {}).get('id')

        r = requests.get(f"http://{target_host}:{target_port}{sentinel}",
                         proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout)
        if r.status_code == 200 and r.content == content:
            print("[PASS] Map Local (managed fixture served)")
            return True
        print(f"[FAIL] Map Local: status={r.status_code} body={r.content[:60]!r}")
        return False
    except Exception as e:
        print(f"[FAIL] Map Local: {e}")
        if verbose:
            traceback.print_exc()
        return False
    finally:
        if rule_id:
            try:
                _admin_request(proxy_url, 'DELETE', f'/admin/map-local-rules/{rule_id}', timeout)
            except Exception:
                pass
        if fixture_uploaded:
            try:
                _admin_request(proxy_url, 'DELETE', f'/admin/map-local-rules/fixtures/{fixture_name}', timeout)
            except Exception:
                pass


def test_dns_override(proxy_url, target_host, target_port, timeout, verbose):
    """Override DNS for a sentinel hostname to the fixture's IP, then request
    that hostname through the proxy and confirm it reaches the fixture."""
    if not proxy_url.startswith('http://'):
        print("[SKIP] DNS Override: requires HTTP proxy for admin API")
        return None
    host = f'dnsgate-{int(time.time())}.test'
    try:
        sr = _admin_request(proxy_url, 'PUT', f'/admin/dns/{host}', timeout,
                            json={'ip': target_host, 'enabled': True})
        if sr.status_code not in (200, 201, 204):
            print(f"[FAIL] DNS Override: set returned {sr.status_code} {sr.text[:120]}")
            return False
        r = requests.get(f"http://{host}:{target_port}/",
                         proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout)
        if r.status_code == 200 and b'Hello' in r.content:
            print("[PASS] DNS Override (sentinel host routed to fixture)")
            return True
        print(f"[FAIL] DNS Override: status={r.status_code} body={r.content[:60]!r}")
        return False
    except Exception as e:
        print(f"[FAIL] DNS Override: {e}")
        if verbose:
            traceback.print_exc()
        return False
    finally:
        try:
            _admin_request(proxy_url, 'DELETE', f'/admin/dns/{host}', timeout)
        except Exception:
            pass


def test_capture_filter(proxy_url, base_url, timeout, verbose):
    """Enable a denylist capture filter for the fixture host and confirm a
    subsequent request is NOT recorded as a session; then restore recording."""
    if not proxy_url.startswith('http://'):
        print("[SKIP] Capture Filter: requires HTTP proxy for admin API")
        return None
    host = urlparse(base_url).hostname
    sentinel = f'/capfilter-{int(time.time())}'
    try:
        sr = _admin_request(proxy_url, 'POST', '/admin/capture-filter', timeout,
                            json={'mode': 'denylist', 'hosts': [host]})
        if sr.status_code not in (200, 201, 204):
            print(f"[FAIL] Capture Filter: set returned {sr.status_code}")
            return False
        r = requests.get(make_test_url(base_url, sentinel),
                         proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout)
        if r.status_code != 200:
            print(f"[FAIL] Capture Filter: request returned {r.status_code}")
            return False
        # Give the recorder a moment; the filtered request must NOT appear.
        time.sleep(1.0)
        details = _recorded_session_details(proxy_url, timeout)
        if any(sentinel in ((d.get('request') or {}).get('uri') or '') for d in details):
            print("[FAIL] Capture Filter: filtered request was recorded anyway")
            return False
        print("[PASS] Capture Filter (filtered host not recorded)")
        return True
    except Exception as e:
        print(f"[FAIL] Capture Filter: {e}")
        if verbose:
            traceback.print_exc()
        return False
    finally:
        try:
            _admin_request(proxy_url, 'POST', '/admin/capture-filter', timeout,
                           json={'mode': 'disabled', 'hosts': []})
        except Exception:
            pass


def test_lua_script(proxy_url, target_host, target_port, timeout, verbose):
    """Install a Lua script that aborts requests to a sentinel path with a
    custom status/body; verify the short-circuit, then clean up."""
    if not proxy_url.startswith('http://'):
        print("[SKIP] Lua Script: requires HTTP proxy for admin API")
        return None
    sentinel = f'/lua-{int(time.time())}'
    code = (
        f'if string.find(request.uri, "{sentinel}", 1, true) then\n'
        f'  abort(231, "lua-aborted")\n'
        f'end\n'
    )
    script = {'name': 'release-gate-lua', 'code': code, 'enabled': True}
    script_id = None
    try:
        cr = _admin_request(proxy_url, 'POST', '/admin/scripts', timeout, json=script)
        if cr.status_code not in (200, 201):
            print(f"[FAIL] Lua Script: create returned {cr.status_code} {cr.text[:120]}")
            return False
        script_id = (cr.json() or {}).get('id')
        r = requests.get(f"http://{target_host}:{target_port}{sentinel}",
                         proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout)
        if r.status_code == 231 and r.text == 'lua-aborted':
            print("[PASS] Lua Script (abort short-circuit)")
            return True
        print(f"[FAIL] Lua Script: status={r.status_code} body={r.text[:60]!r}")
        return False
    except Exception as e:
        print(f"[FAIL] Lua Script: {e}")
        if verbose:
            traceback.print_exc()
        return False
    finally:
        if script_id:
            try:
                _admin_request(proxy_url, 'DELETE', f'/admin/scripts/{script_id}', timeout)
            except Exception:
                pass


def _b64url(obj):
    raw = json.dumps(obj, separators=(',', ':')).encode()
    return base64.urlsafe_b64encode(raw).rstrip(b'=').decode()


def test_jwt_inspector(proxy_url, base_url, timeout, verbose):
    """Send a request with a Bearer JWT and confirm the proxy decodes it into
    the session's inspector_data (JWT inspector). No verification of signature."""
    if not proxy_url.startswith('http://'):
        print("[SKIP] JWT Inspector: requires HTTP proxy for admin API")
        return None
    token = (
        _b64url({'alg': 'HS256', 'typ': 'JWT'}) + '.'
        + _b64url({'sub': 'gate', 'name': 'release'}) + '.'
        + 'c2ln'
    )
    sentinel = f'jwt-{int(time.time())}'
    url = make_test_url(base_url, f'/?sentinel={sentinel}')
    try:
        r = requests.get(url, headers={'Authorization': f'Bearer {token}'},
                         proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout)
        if r.status_code != 200:
            print(f"[FAIL] JWT Inspector: request returned {r.status_code}")
            return False

        def matches(exchange):
            uri = (exchange.get('request') or {}).get('uri') or ''
            jwt = (exchange.get('inspector_data') or {}).get('jwt') or {}
            claims = jwt.get('claims') or {}
            return sentinel in uri and claims.get('sub') == 'gate'

        _wait_for_recorded_session(proxy_url, matches, timeout, include_bodies=True)
        print("[PASS] JWT Inspector (claims decoded into inspector_data)")
        return True
    except Exception as e:
        print(f"[FAIL] JWT Inspector: {e}")
        if verbose:
            traceback.print_exc()
        return False


def test_graphql_inspector(proxy_url, base_url, timeout, verbose):
    """POST a GraphQL document and confirm the proxy records the parsed
    operation in inspector_data (GraphQL inspector)."""
    if not proxy_url.startswith('http://'):
        print("[SKIP] GraphQL Inspector: requires HTTP proxy for admin API")
        return None
    sentinel = f'gql-{int(time.time())}'
    url = make_test_url(base_url, f'/echo?sentinel={sentinel}')
    # The GraphQL inspector keys on a JSON body containing a "query" field
    # (GraphQL-over-HTTP), not a raw application/graphql document.
    body = json.dumps({'query': 'query GateOp { hello }'})
    try:
        r = requests.post(url, data=body.encode(),
                          headers={'Content-Type': 'application/json'},
                          proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout)
        if r.status_code != 200:
            print(f"[FAIL] GraphQL Inspector: request returned {r.status_code}")
            return False

        def matches(exchange):
            uri = (exchange.get('request') or {}).get('uri') or ''
            gql = (exchange.get('inspector_data') or {}).get('graphql') or {}
            return sentinel in uri and gql.get('operation_type') == 'query'

        _wait_for_recorded_session(proxy_url, matches, timeout, include_bodies=True)
        print("[PASS] GraphQL Inspector (operation parsed into inspector_data)")
        return True
    except Exception as e:
        print(f"[FAIL] GraphQL Inspector: {e}")
        if verbose:
            traceback.print_exc()
        return False


def test_breakpoint(proxy_url, target_host, target_port, timeout, verbose):
    """Install a request breakpoint, fire a matching request (which pauses),
    resolve it via the admin API with 'continue', and confirm the request then
    completes against upstream. Cleans up the rule."""
    if not proxy_url.startswith('http://'):
        print("[SKIP] Breakpoint: requires HTTP proxy for admin API")
        return None
    sentinel = f'/bp-{int(time.time())}'
    rule = {
        'location': {'path': sentinel, 'mode': 'glob'},
        'bp_type': 'Request',
        'enabled': True,
    }
    rule_id = None
    result = {}

    def fire():
        try:
            r = requests.get(f"http://{target_host}:{target_port}{sentinel}",
                             proxies={'http': proxy_url, 'https': proxy_url}, timeout=timeout + 10)
            result['status'] = r.status_code
            result['body'] = r.content
        except Exception as ex:
            result['error'] = str(ex)

    try:
        cr = _admin_request(proxy_url, 'POST', '/admin/breakpoints', timeout, json=rule)
        if cr.status_code not in (200, 201):
            print(f"[FAIL] Breakpoint: create returned {cr.status_code} {cr.text[:120]}")
            return False
        rule_id = (cr.json() or {}).get('id')

        worker = threading.Thread(target=fire, daemon=True)
        worker.start()

        # Wait for the request to show up as paused.
        pending_id = None
        deadline = time.time() + timeout
        while time.time() < deadline:
            pr = _admin_request(proxy_url, 'GET', '/admin/breakpoints/pending', timeout)
            if pr.status_code == 200:
                for p in (pr.json() or []):
                    uri = p.get('uri') or (p.get('request') or {}).get('uri') or ''
                    if sentinel in uri or len(pr.json()) == 1:
                        pending_id = p.get('id')
                        break
            if pending_id:
                break
            time.sleep(0.2)

        if not pending_id:
            print("[SKIP] Breakpoint: request did not pause (breakpoints may be disabled in this build)")
            return None

        rr = _admin_request(proxy_url, 'POST',
                            f'/admin/breakpoints/pending/{pending_id}/resolve', timeout,
                            json={'action': 'continue'})
        if rr.status_code not in (200, 204):
            print(f"[FAIL] Breakpoint: resolve returned {rr.status_code}")
            return False

        worker.join(timeout + 5)
        if result.get('status') == 200 and b'Hello' in result.get('body', b''):
            print("[PASS] Breakpoint (pause + resume continued upstream)")
            return True
        print(f"[FAIL] Breakpoint: resumed request result={result}")
        return False
    except Exception as e:
        print(f"[FAIL] Breakpoint: {e}")
        if verbose:
            traceback.print_exc()
        return False
    finally:
        if rule_id:
            try:
                _admin_request(proxy_url, 'DELETE', f'/admin/breakpoints/{rule_id}', timeout)
            except Exception:
                pass


# HTTP/3 tests — all skip gracefully when aioquic is absent or H3 is disabled
def _h3_proxy_parts(h3_proxy_url):
    """Return (host, port) from an h3:// or https:// URL, defaulting port 8443."""
    parsed = urlparse(h3_proxy_url)
    return parsed.hostname, (parsed.port or 8443)


def test_http3_basic(h3_proxy_url, target_host, target_port, timeout, verbose):
    """Basic HTTP/3 GET through oproxy's QUIC listener."""
    if not _AIOQUIC_AVAILABLE:
        print('[SKIP] HTTP/3 GET: aioquic not installed')
        return None
    proxy_host, proxy_port = _h3_proxy_parts(h3_proxy_url)
    try:
        status, _, body = _h3_forward_request(
            proxy_host, proxy_port,
            'GET', 'http', f'{target_host}:{target_port}', '/',
            timeout=timeout,
        )
        if status == 200 and b'Hello' in body:
            print('[PASS] HTTP/3 GET')
            return True
        print(f'[FAIL] HTTP/3 GET: status={status} body={body[:100]!r}')
        return False
    except Exception as e:
        print(f'[FAIL] HTTP/3 GET: {e}')
        if verbose:
            traceback.print_exc()
        return False


def test_http3_json(h3_proxy_url, target_host, target_port, timeout, verbose):
    """HTTP/3 GET that returns a JSON body."""
    if not _AIOQUIC_AVAILABLE:
        print('[SKIP] HTTP/3 JSON: aioquic not installed')
        return None
    proxy_host, proxy_port = _h3_proxy_parts(h3_proxy_url)
    try:
        status, _, body = _h3_forward_request(
            proxy_host, proxy_port,
            'GET', 'http', f'{target_host}:{target_port}', '/json',
            timeout=timeout,
        )
        if status == 200:
            data = json.loads(body)
            if data == {'message': 'Hello JSON'}:
                print('[PASS] HTTP/3 JSON')
                return True
        print(f'[FAIL] HTTP/3 JSON: status={status} body={body[:100]!r}')
        return False
    except Exception as e:
        print(f'[FAIL] HTTP/3 JSON: {e}')
        if verbose:
            traceback.print_exc()
        return False


def test_http3_post_json(h3_proxy_url, target_host, target_port, timeout, verbose):
    """HTTP/3 POST with a JSON request body forwarded by the proxy."""
    if not _AIOQUIC_AVAILABLE:
        print('[SKIP] HTTP/3 POST JSON: aioquic not installed')
        return None
    proxy_host, proxy_port = _h3_proxy_parts(h3_proxy_url)
    payload = json.dumps({'hello': 'h3'}).encode()
    try:
        status, _, body = _h3_forward_request(
            proxy_host, proxy_port,
            'POST', 'http', f'{target_host}:{target_port}', '/json-echo',
            body=payload,
            extra_headers={'content-type': 'application/json'},
            timeout=timeout,
        )
        if status == 200:
            data = json.loads(body)
            if data.get('hello') == 'h3' and data.get('received'):
                print('[PASS] HTTP/3 POST JSON')
                return True
        print(f'[FAIL] HTTP/3 POST JSON: status={status} body={body[:100]!r}')
        return False
    except Exception as e:
        print(f'[FAIL] HTTP/3 POST JSON: {e}')
        if verbose:
            traceback.print_exc()
        return False


def test_http3_large(h3_proxy_url, target_host, target_port, timeout, verbose):
    """1 MB response over HTTP/3 — exercises multi-chunk DATA frame reassembly."""
    if not _AIOQUIC_AVAILABLE:
        print('[SKIP] HTTP/3 Large: aioquic not installed')
        return None
    proxy_host, proxy_port = _h3_proxy_parts(h3_proxy_url)
    try:
        status, _, body = _h3_forward_request(
            proxy_host, proxy_port,
            'GET', 'http', f'{target_host}:{target_port}', '/large',
            timeout=timeout,
        )
        expected = 1024 * 1024
        if (status == 200 and len(body) == expected
                and all(b == 0x41 for b in body)):
            print('[PASS] HTTP/3 Large (1 MB)')
            return True
        print(f'[FAIL] HTTP/3 Large: status={status} len={len(body)}')
        return False
    except Exception as e:
        print(f'[FAIL] HTTP/3 Large: {e}')
        if verbose:
            traceback.print_exc()
        return False


def test_http3_session_events(h3_proxy_url, http_proxy_url, target_host, target_port,
                              timeout, verbose):
    """Verify the proxy records HTTP/3 exchanges with version=HTTP/3 in the
    session log, confirming the H3 listener uses the same session pipeline."""
    if not _AIOQUIC_AVAILABLE:
        print('[SKIP] HTTP/3 Session Events: aioquic not installed')
        return None
    if not http_proxy_url:
        print('[SKIP] HTTP/3 Session Events: requires --http-proxy for admin API')
        return None
    proxy_host, proxy_port = _h3_proxy_parts(h3_proxy_url)
    sentinel = f'h3-sess-{int(time.time())}'
    try:
        status, _, _ = _h3_forward_request(
            proxy_host, proxy_port,
            'GET', 'http', f'{target_host}:{target_port}',
            f'/?sentinel={sentinel}',
            timeout=timeout,
        )
        if status != 200:
            print(f'[FAIL] HTTP/3 Session Events: request failed with status {status}')
            return False

        def _h3_proto(exchange):
            """Return the downstream protocol label for an exchange."""
            # Stored in downstream_protocol at the top level; protocol_context
            # is the structured variant.  request.version is not populated for H3.
            proto = exchange.get('downstream_protocol') or ''
            if not proto:
                proto = (exchange.get('protocol_context') or {}).get('downstream') or ''
            return proto.upper()

        def matches(exchange):
            req = exchange.get('request') or {}
            uri = req.get('uri') or ''
            return sentinel in uri and 'HTTP/3' in _h3_proto(exchange)

        exchange = _wait_for_recorded_session(
            http_proxy_url, matches, timeout, include_bodies=True
        )
        proto = _h3_proto(exchange)
        if 'HTTP/3' in proto:
            print('[PASS] HTTP/3 Session Events')
            return True
        print(f'[FAIL] HTTP/3 Session Events: downstream_protocol={proto!r}')
        if verbose:
            print(json.dumps(exchange, indent=2, default=str)[:2000])
        return False
    except Exception as e:
        print(f'[FAIL] HTTP/3 Session Events: {e}')
        if verbose:
            traceback.print_exc()
        return False


# ----------------------------------------------------------------------
# Main inner test runner
# ----------------------------------------------------------------------
def run_tests():
    parser = argparse.ArgumentParser(description="Comprehensive proxy tester (inner)")
    parser.add_argument('--http-proxy', default=None)
    parser.add_argument('--socks-proxy', default=None)
    parser.add_argument('--h3-proxy', default=None,
        help='HTTP/3 (QUIC) proxy URL, e.g. h3://localhost:8443. '
             'Derived from --http-proxy host + port 8443 when omitted.')
    parser.add_argument('--target-host', default='127.0.0.1')
    parser.add_argument('--timeout', type=int, default=10)
    parser.add_argument('--verbose', action='store_true')
    parser.add_argument('--run-tests', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()

    http_proxy  = args.http_proxy
    socks_proxy = args.socks_proxy
    h3_proxy    = args.h3_proxy
    target_host = args.target_host
    timeout     = args.timeout
    verbose     = args.verbose

    # Derive h3_proxy from the HTTP proxy host + default H3 port 8443 when the
    # caller did not set --h3-proxy explicitly.  The H3 listener is gated behind
    # the `http3` Cargo feature and OPROXY_HTTP3_ENABLED, so tests skip
    # gracefully when the proxy is unreachable or aioquic is absent.
    if not h3_proxy and http_proxy:
        _pa = urlparse(http_proxy)
        h3_proxy = f'h3://{_pa.hostname}:8443'

    if not http_proxy and not socks_proxy:
        print("Error: Provide at least --http-proxy or --socks-proxy.")
        sys.exit(1)

    print(f"Starting test servers on {target_host}...")
    global server_manager
    server_manager = ServerManager(target_host)

    http1_port = server_manager.start_http1()
    http1_base_url = f"http://{target_host}:{http1_port}"

    https1_port = server_manager.start_https1()

    http2_port = server_manager.start_http2()
    http2_base_url = f"https://{target_host}:{http2_port}"

    ws_port = server_manager.start_websocket()
    ws_url = f"ws://{target_host}:{ws_port}"

    grpc_port = server_manager.start_grpc()

    time.sleep(1)

    all_results = []

    if http_proxy:
        print("\n=== Testing HTTP proxy:", http_proxy)
        def http_add(name, result):
            all_results.append(("HTTP", name, result))

        http_add("HTTP/1.1 GET", test_http1_basic(http_proxy, f"{http1_base_url}/", timeout, verbose))
        http_add("HTTP/1.1 gzip", test_http1_encoding(http_proxy, http1_base_url, "/gzip", "gzip", b"Hello gzip", timeout, verbose))
        http_add("HTTP/1.1 deflate", test_http1_encoding(http_proxy, http1_base_url, "/deflate", "deflate", b"Hello deflate", timeout, verbose))
        http_add("HTTP/1.1 brotli", test_http1_encoding(http_proxy, http1_base_url, "/brotli", "br", b"Hello brotli", timeout, verbose))
        http_add("HTTP/1.1 zstd", test_http1_encoding(http_proxy, http1_base_url, "/zstd", "zstd", b"Hello zstd", timeout, verbose))
        http_add("HTTP/1.1 JSON", test_http1_json(http_proxy, http1_base_url, timeout, verbose))
        http_add("HTTP/1.1 Proto", test_http1_proto(http_proxy, http1_base_url, timeout, verbose))
        http_add("HTTP/1.1 Chunked", test_http1_chunked(http_proxy, http1_base_url, timeout, verbose))
        http_add("HTTP/1.1 Keep-Alive", test_http1_keepalive(http_proxy, http1_base_url, timeout, verbose))
        http_add("HTTP/1.1 Large", test_http1_large(http_proxy, http1_base_url, timeout, verbose))
        http_add("HTTP/1.1 POST JSON", test_http1_post_json(http_proxy, http1_base_url, timeout, verbose))
        http_add("HTTP/1.1 POST Proto", test_http1_post_proto(http_proxy, http1_base_url, timeout, verbose))

        http_add("HTTP/2 GET", test_http2_basic(http_proxy, f"{http2_base_url}/", timeout, verbose))
        http_add("HTTP/2 gzip", test_http2_encoding(http_proxy, http2_base_url, "/gzip", "gzip", b"Hello gzip", timeout, verbose))
        http_add("HTTP/2 deflate", test_http2_encoding(http_proxy, http2_base_url, "/deflate", "deflate", b"Hello deflate", timeout, verbose))
        http_add("HTTP/2 brotli", test_http2_encoding(http_proxy, http2_base_url, "/brotli", "br", b"Hello brotli", timeout, verbose))
        http_add("HTTP/2 zstd", test_http2_encoding(http_proxy, http2_base_url, "/zstd", "zstd", b"Hello zstd", timeout, verbose))
        http_add("HTTP/2 JSON", test_http2_json(http_proxy, http2_base_url, timeout, verbose))
        http_add("HTTP/2 Proto", test_http2_proto(http_proxy, http2_base_url, timeout, verbose))
        http_add("HTTP/2 POST JSON", test_http2_post_json(http_proxy, http2_base_url, timeout, verbose))
        http_add("HTTP/2 Chunked", test_http2_chunked(http_proxy, http2_base_url, timeout, verbose))
        http_add("HTTP/2 Large", test_http2_large(http_proxy, http2_base_url, timeout, verbose))

        http_add("HTTPS CONNECT tunnel", test_https_connect(http_proxy, target_host, https1_port, timeout, verbose))
        http_add("HTTP/1.1 Session Events", test_http1_session_events(http_proxy, http1_base_url, timeout, verbose))

        http_add("WebSocket Text", test_websocket_text(http_proxy, ws_url, timeout, verbose))
        http_add("WebSocket Binary", test_websocket_binary(http_proxy, ws_url, timeout, verbose))
        http_add("WebSocket Close", test_websocket_close(http_proxy, ws_url, timeout, verbose))
        http_add("WebSocket Session Events", test_websocket_session_events(http_proxy, ws_url, timeout, verbose))
        http_add("gRPC", test_grpc(http_proxy, target_host, grpc_port, timeout, verbose,
                                   grpc_server_ca_pem=server_manager.grpc_ca_pem,
                                   session_admin_proxy=http_proxy))

        # Robustness / regression / security
        http_add("HTTP/1.1 Status Passthrough", test_status_passthrough(http_proxy, http1_base_url, timeout, verbose))
        http_add("HTTP/2 Status Passthrough", test_status_passthrough(http_proxy, http2_base_url, timeout, verbose, label='HTTP/2', http2=True))
        http_add("HTTP/1.1 Multi Set-Cookie", test_multi_set_cookie(http_proxy, http1_base_url, timeout, verbose))
        http_add("HTTP/2 Multi Set-Cookie", test_multi_set_cookie(http_proxy, http2_base_url, timeout, verbose, label='HTTP/2', http2=True))
        http_add("HTTP/1.1 Methods", test_http1_methods(http_proxy, http1_base_url, timeout, verbose))
        http_add("HTTP/1.1 Large Upload", test_large_request_upload(http_proxy, http1_base_url, timeout, verbose))
        http_add("HTTP/1.1 SSE Streaming", test_sse_streaming(http_proxy, http1_base_url, timeout, verbose))
        http_add("Internal Header Stripping", test_internal_headers_stripped(http_proxy, http1_base_url, timeout, verbose))
        http_add("Concurrent Requests", test_concurrent_requests(http_proxy, http1_base_url, timeout, verbose))

        # Manipulation features (admin API → behavior). Mock/Access/Rewrite run
        # over both HTTP/1.1 and HTTP/2 since they share one middleware pipeline.
        http_add("HTTP/1.1 Mock Rule", test_mock_rule(http_proxy, http1_base_url, timeout, verbose))
        http_add("HTTP/2 Mock Rule", test_mock_rule(http_proxy, http2_base_url, timeout, verbose, http2=True, label='HTTP/2'))
        http_add("HTTP/1.1 Access Block", test_access_block(http_proxy, http1_base_url, timeout, verbose))
        http_add("HTTP/2 Access Block", test_access_block(http_proxy, http2_base_url, timeout, verbose, http2=True, label='HTTP/2'))
        http_add("HTTP/1.1 Rewrite Rule", test_rewrite_rule(http_proxy, http1_base_url, timeout, verbose))
        http_add("HTTP/2 Rewrite Rule", test_rewrite_rule(http_proxy, http2_base_url, timeout, verbose, http2=True, label='HTTP/2'))
        http_add("WebSocket Mock", test_websocket_mock(http_proxy, ws_url, timeout, verbose))
        http_add("Map Local", test_map_local(http_proxy, target_host, http1_port, timeout, verbose))
        http_add("DNS Override", test_dns_override(http_proxy, target_host, http1_port, timeout, verbose))
        http_add("Lua Script", test_lua_script(http_proxy, target_host, http1_port, timeout, verbose))
        http_add("Breakpoint", test_breakpoint(http_proxy, target_host, http1_port, timeout, verbose))
        http_add("JWT Inspector", test_jwt_inspector(http_proxy, http1_base_url, timeout, verbose))
        http_add("GraphQL Inspector", test_graphql_inspector(http_proxy, http1_base_url, timeout, verbose))
        http_add("Capture Filter", test_capture_filter(http_proxy, http1_base_url, timeout, verbose))
        # Throttling is global, so run it last (it always restores its config).
        http_add("Throttling", test_throttling(http_proxy, http1_base_url, timeout, verbose))

    if socks_proxy:
        print("\n=== Testing SOCKS5 proxy:", socks_proxy)
        def socks_add(name, result):
            all_results.append(("SOCKS5", name, result))

        socks_add("SOCKS5 TCP", test_socks5_tcp(socks_proxy, target_host, http1_port, timeout, verbose))
        socks_add("SOCKS5 Session Events", test_socks5_session_events(
            socks_proxy, http_proxy, target_host, http1_port, timeout, verbose
        ))

        udp_port = free_port()
        def udp_echo_server():
            sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            sock.bind(('127.0.0.1', udp_port))
            while not _shutdown_event.is_set():
                try:
                    data, addr = sock.recvfrom(1024)
                    sock.sendto(data, addr)
                except socket.timeout:
                    continue
                except:
                    break
            sock.close()
        udp_thread = threading.Thread(target=udp_echo_server, daemon=True)
        udp_thread.start()
        server_manager._add_server("UDP Echo", udp_port, lambda: None)
        time.sleep(0.2)
        socks_add("SOCKS5 UDP", test_socks5_udp(socks_proxy, target_host, udp_port, timeout, verbose))

    if h3_proxy:
        print("\n=== Testing HTTP/3 proxy:", h3_proxy)
        if not _AIOQUIC_AVAILABLE:
            print("  (aioquic not installed — all HTTP/3 tests will be skipped)")
        def h3_add(name, result):
            all_results.append(("HTTP/3", name, result))

        # H3 tests target the HTTP/1.1 fixture server; the proxy forwards the
        # absolute-URI H3 request to it via its normal reqwest pipeline.
        h3_add("HTTP/3 GET",          test_http3_basic(h3_proxy, target_host, http1_port, timeout, verbose))
        h3_add("HTTP/3 JSON",         test_http3_json(h3_proxy, target_host, http1_port, timeout, verbose))
        h3_add("HTTP/3 POST JSON",    test_http3_post_json(h3_proxy, target_host, http1_port, timeout, verbose))
        h3_add("HTTP/3 Large",        test_http3_large(h3_proxy, target_host, http1_port, timeout, verbose))
        h3_add("HTTP/3 Session Events", test_http3_session_events(
            h3_proxy, http_proxy, target_host, http1_port, timeout, verbose
        ))

    # Summary
    print("\n" + "="*40)
    print("FINAL TEST SUMMARY")
    print("-"*40)
    passed = sum(1 for _, _, ok in all_results if ok is True)
    skipped = sum(1 for _, _, ok in all_results if ok is None)
    total = len(all_results) - skipped
    for proxy_type, name, ok in all_results:
        status = "PASS" if ok is True else ("SKIP" if ok is None else "FAIL")
        print(f"  [{proxy_type}] {name}: {status}")
    print(f"\n{passed}/{total} tests passed" + (f" ({skipped} skipped)" if skipped else ""))
    if passed != total:
        sys.exit(1)

    server_manager.stop_all()

if __name__ == "__main__":
    run_tests()
