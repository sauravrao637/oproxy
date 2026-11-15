"""Private, bounded proxy lifecycle using the binary under test."""

import os
from pathlib import Path
import socket
import ssl
import subprocess
import sys
import tempfile
import time
from uuid import uuid4
from urllib.parse import urlsplit

import requests
import yaml


class IsolatedProxy:
    def __init__(self, insecure_upstream=None, *, admin_token=None, open_admin=False):
        self.insecure_upstream = insecure_upstream
        self.process = self.log = self.directory = None
        self.client = requests.Session()
        self.client.trust_env = False
        self.admin_token = None if open_admin else (admin_token or uuid4().hex)
        if self.admin_token is not None:
            self.client.headers["x-oproxy-admin-token"] = self.admin_token

    def __enter__(self):
        try:
            default = Path(__file__).resolve().parents[2] / "target" / "debug"
            binary = Path(os.environ.get("OPROXY_TEST_BINARY", str(
                default / ("oproxy.exe" if os.name == "nt" else "oproxy")))).resolve()
            if not binary.is_file():
                raise RuntimeError(f"Missing tested binary: {binary}; set OPROXY_TEST_BINARY")
            self.directory = tempfile.TemporaryDirectory(prefix="isolated-proxy-")
            root = Path(self.directory.name)
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                port = listener.getsockname()[1]
            self.url = f"http://127.0.0.1:{port}"
            self.binary = binary
            self.root = root
            config = self.config_path = root / "config.yaml"
            config.write_text(yaml.safe_dump({
                "admin_token": self.admin_token,
                "port": port, "bind_host": "127.0.0.1", "update_check": False,
                "storage_path": str(root / "storage"),
                "mitm": {"enabled": True, "root_ca_path": str(root / "certs")},
                "log": {"level": "debug", "dir": str(root), "file": "proxy.log"},
            }), encoding="utf-8")
            return self.start()
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def configure(self, **settings):
        """Write startup settings without changing the running process."""
        config = yaml.safe_load(self.config_path.read_text(encoding="utf-8"))
        config.update(settings)
        self.config_path.write_text(yaml.safe_dump(config), encoding="utf-8")

    def restart(self):
        """Restart with the same config, storage, listener and certificate directory."""
        self.stop()
        config = yaml.safe_load(self.config_path.read_text(encoding="utf-8"))
        self.admin_token = config.get("admin_token")
        self.client.cookies.clear()
        self.client.headers.pop("x-oproxy-admin-token", None)
        if self.admin_token:
            self.client.headers["x-oproxy-admin-token"] = self.admin_token
        return self.start()

    def start(self):
        try:
            return self._start()
        except BaseException:
            self.print_diagnostic()
            raise

    def print_diagnostic(self):
        if self.log is not None and not self.log.closed:
            self.log.flush()
        if self.directory is None:
            return
        path = self.root / "startup.log"
        try:
            with path.open("rb") as log:
                log.seek(max(0, path.stat().st_size - 16384))
                print("Isolated proxy diagnostic:\n" +
                      log.read().decode("utf-8", errors="replace"), file=sys.stderr)
        except OSError:
            pass

    def _start(self):
        with socket.socket() as listener:
            if os.name == "nt":
                listener.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
            listener.bind(("127.0.0.1", urlsplit(self.url).port))
        env = {k: v for k, v in os.environ.items() if not k.startswith("OPROXY_")}
        env.update(OPROXY_CONFIG=str(self.config_path), OPROXY_UPDATE_CHECK="false")
        if self.insecure_upstream is not None:
            env["OPROXY_INSECURE_UPSTREAM"] = self.insecure_upstream
        self.log = (self.root / "startup.log").open("a", encoding="utf-8")
        self.process = subprocess.Popen([str(self.binary)], cwd=self.root, env=env,
                                        stdout=self.log, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError(f"Isolated proxy exited: {self.process.returncode}")
            try:
                if self.client.get(self.url + "/health", timeout=.5).ok:
                    response = self.client.get(self.url + "/admin/ca", timeout=3)
                    response.raise_for_status()
                    owned_ca = (self.root / "certs" / "root.crt").read_text(encoding="utf-8")
                    try:
                        same_ca = ssl.PEM_cert_to_DER_cert(response.text) == ssl.PEM_cert_to_DER_cert(owned_ca)
                    except ValueError:
                        same_ca = False
                    if not same_ca or self.process.poll() is not None:
                        raise RuntimeError("Readiness response does not belong to the isolated proxy")
                    self.ca_path = self.root / "client-ca.pem"
                    self.ca_path.write_text(response.text, encoding="utf-8")
                    return self
            except requests.RequestException:
                pass
            time.sleep(.05)
        raise RuntimeError("Isolated proxy readiness timed out")

    def stop(self):
        if self.process is not None:
            if self.process.poll() is None:
                self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        if self.log is not None:
            self.log.close()

    def __exit__(self, *args):
        try:
            self.stop()
        finally:
            if args and args[0] is not None:
                self.print_diagnostic()
            self.client.close()
            if self.log is not None:
                self.log.close()
            if self.directory is not None:
                self.directory.cleanup()
