"""Bounded, private HTTP/1.1 TLS origin for native and shared-network testers."""

import json
import ssl
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

import trustme


class HttpsOrigin:
    def __enter__(self):
        self.directory = tempfile.TemporaryDirectory(prefix="https-origin-")
        self.server = None
        self.thread = None
        try:
            directory = Path(self.directory.name)
            ca = trustme.CA()
            leaf = ca.issue_cert("localhost", "127.0.0.1")
            self.ca_path = directory / "ca.pem"
            ca.cert_pem.write_to_path(self.ca_path)
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            leaf.configure_cert(context)
            context.set_alpn_protocols(["http/1.1"])

            class Handler(BaseHTTPRequestHandler):
                protocol_version = "HTTP/1.1"

                def log_message(self, *args):
                    pass

                def do_GET(self):
                    target = urlsplit(self.path)
                    nonce = parse_qs(target.query).get("nonce", [])
                    valid = target.path == "/echo" and len(nonce) == 1
                    payload = {"method": "GET", "path": self.path,
                               "nonce": nonce[0]} if valid else {"error": "not found"}
                    body = json.dumps(payload).encode()
                    self.send_response(200 if valid else 404)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(body)))
                    self.send_header("Connection", "close")
                    self.end_headers()
                    self.wfile.write(body)
                    self.close_connection = True

            class Server(ThreadingHTTPServer):
                def get_request(self):
                    connection, address = super().get_request()
                    connection.settimeout(2)
                    try:
                        return context.wrap_socket(connection, server_side=True), address
                    except BaseException:
                        connection.close()
                        raise

            self.server = Server(("127.0.0.1", 0), Handler)
            self.port = self.server.server_address[1]
            self.url = f"https://127.0.0.1:{self.port}"
            self.thread = threading.Thread(target=self.server.serve_forever,
                                           kwargs={"poll_interval": 0.05}, daemon=True)
            self.thread.start()
            return self
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def __exit__(self, *args):
        if self.thread is not None:
            self.server.shutdown()
            self.thread.join(timeout=5)
        if self.server is not None:
            self.server.server_close()
        self.directory.cleanup()
