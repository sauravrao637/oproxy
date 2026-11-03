"""Private, bounded proxy lifecycle using the binary under test."""

import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from uuid import uuid4

import requests
import yaml


class IsolatedProxy:
    def __init__(self, insecure_upstream=None):
        self.insecure_upstream = insecure_upstream
        self.process = self.log = self.directory = None
        self.client = requests.Session()
        self.client.trust_env = False
        self.admin_token = uuid4().hex
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
            config = root / "config.yaml"
            config.write_text(yaml.safe_dump({
                "port": port, "bind_host": "127.0.0.1", "update_check": False,
                "storage_path": str(root / "storage"),
                "mitm": {"enabled": True, "root_ca_path": str(root / "certs")},
                "log": {"level": "debug", "dir": str(root), "file": "proxy.log"},
            }), encoding="utf-8")
            env = {k: v for k, v in os.environ.items() if not k.startswith("OPROXY_")}
            env.update(OPROXY_CONFIG=str(config), OPROXY_UPDATE_CHECK="false")
            env["OPROXY_ADMIN_TOKEN"] = self.admin_token
            if self.insecure_upstream is not None:
                env["OPROXY_INSECURE_UPSTREAM"] = self.insecure_upstream
            self.log = (root / "startup.log").open("w", encoding="utf-8")
            self.process = subprocess.Popen([str(binary)], cwd=root, env=env,
                                            stdout=self.log, stderr=subprocess.STDOUT)
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                if self.process.poll() is not None:
                    raise RuntimeError(f"Isolated proxy exited: {self.process.returncode}")
                try:
                    if self.client.get(self.url + "/health", timeout=.5).ok:
                        response = self.client.get(self.url + "/admin/ca", timeout=3)
                        response.raise_for_status()
                        self.ca_path = root / "client-ca.pem"
                        self.ca_path.write_text(response.text, encoding="utf-8")
                        return self
                except requests.RequestException:
                    pass
                time.sleep(.05)
            raise RuntimeError("Isolated proxy readiness timed out")
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def __exit__(self, *args):
        try:
            if self.process is not None:
                if self.process.poll() is None:
                    self.process.terminate()
                try:
                    self.process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=5)
        finally:
            self.client.close()
            if self.log is not None:
                self.log.close()
            if self.directory is not None:
                self.directory.cleanup()
