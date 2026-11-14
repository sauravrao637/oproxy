"""Live primary-profile ownership, isolation and diagnostic checks."""

import contextlib
import io
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch
from urllib.parse import urlsplit

import requests
import yaml

import qa_process
from fixtures.isolated_proxy import IsolatedProxy


class ProfileOwnershipTests(unittest.TestCase):
    def test_occupied_listeners_fail_without_launch(self):
        for key, kind in (("port", socket.SOCK_STREAM), ("https_port", socket.SOCK_STREAM),
                          ("socks5_port", socket.SOCK_STREAM), ("http3_port", socket.SOCK_DGRAM)):
            with self.subTest(listener=key), socket.socket(socket.AF_INET, kind) as listener:
                listener.bind(("127.0.0.1", 0))
                config = {"bind_host": "127.0.0.1", key: listener.getsockname()[1],
                          "http3_enabled": key == "http3_port"}
                with self.assertRaises(OSError):
                    qa_process.check_listener_ports(config)

    def test_exited_process_cannot_use_foreign_health(self):
        proc = Mock(returncode=1)
        proc.poll.return_value = 1
        with patch.object(qa_process.requests, "Session") as session:
            self.assertFalse(qa_process.wait_for_health(1234, proc, "absent"))
            session.return_value.__enter__.return_value.get.assert_not_called()

    def test_foreign_ca_rejected_by_primary_readiness(self):
        with IsolatedProxy() as first, IsolatedProxy() as second:
            with self.assertRaisesRegex(RuntimeError, "does not belong"):
                qa_process.wait_for_health(urlsplit(first.url).port, first.process,
                                           second.ca_path, first.admin_token)

    def test_repeated_primary_runs_use_fresh_state_and_scrub_overrides(self):
        launch = subprocess.Popen
        roots, cas, children = [], [], []
        with tempfile.TemporaryDirectory() as directory:
            config_path = Path(directory, "source.yaml")
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                port = listener.getsockname()[1]
            config_path.write_text(yaml.safe_dump({"port": port, "bind_host": "127.0.0.1",
                "admin_token": "profile-token", "mitm": {"enabled": True},
                "max_sessions": 20}), encoding="utf-8")
            original = config_path.read_bytes()
            profile = {"config": str(config_path), "port": port,
                       "run": [{"script": "probe.py"}]}

            def spawn(*args, **kwargs):
                root = Path(kwargs["cwd"])
                roots.append(root)
                config = yaml.safe_load((root / "config.yaml").read_text(encoding="utf-8"))
                self.assertEqual(config["max_sessions"], 20)
                self.assertEqual(config["admin_token"], "profile-token")
                self.assertEqual(config["storage_path"], str(root / "storage"))
                for name in ("OPROXY_PORT", "OPROXY_ADMIN_TOKEN", "OPROXY_INSECURE_UPSTREAM"):
                    self.assertNotIn(name, kwargs["env"])
                child = launch(*args, **kwargs)
                children.append(child)
                return child

            def probe(*args):
                with requests.Session() as client:
                    client.trust_env = False
                    client.headers["x-oproxy-admin-token"] = "profile-token"
                    response = client.get(f"http://127.0.0.1:{port}/admin/ca", timeout=2)
                    response.raise_for_status()
                    cas.append(response.text)
                return 0

            with patch.object(qa_process, "build"), \
                 patch.object(qa_process.subprocess, "Popen", side_effect=spawn), \
                 patch.object(qa_process, "run_tester", side_effect=probe), \
                 patch.dict(os.environ, {"OPROXY_PORT": "1", "OPROXY_ADMIN_TOKEN": "foreign",
                                         "OPROXY_INSECURE_UPSTREAM": "1"}):
                for _ in range(2):
                    self.assertEqual(qa_process.run_profile("repeat", profile), {"probe.py": 0})
            self.assertEqual(config_path.read_bytes(), original)
        self.assertEqual(len(set(roots)), 2)
        self.assertEqual(len(set(cas)), 2)
        self.assertTrue(all(not root.exists() for root in roots))
        self.assertTrue(all(child.poll() is not None for child in children))

    def test_isolated_body_failure_prints_logs_before_cleanup(self):
        output = io.StringIO()
        with contextlib.redirect_stderr(output):
            with self.assertRaisesRegex(RuntimeError, "body fault"):
                with IsolatedProxy() as proxy:
                    root = proxy.root
                    proxy.log.write("diagnostic-body-marker\n")
                    raise RuntimeError("body fault")
        self.assertIn("diagnostic-body-marker", output.getvalue())
        self.assertFalse(root.exists())

    def test_isolated_startup_failure_prints_logs_before_cleanup(self):
        output = io.StringIO()
        proxy = IsolatedProxy()
        original = proxy._start

        def fail_after_start():
            original()
            proxy.log.write("diagnostic-startup-marker\n")
            raise RuntimeError("startup fault")

        with patch.object(proxy, "_start", side_effect=fail_after_start), \
             contextlib.redirect_stderr(output):
            with self.assertRaisesRegex(RuntimeError, "startup fault"):
                with proxy:
                    self.fail("Unexpected readiness")
        self.assertIn("diagnostic-startup-marker", output.getvalue())
        self.assertFalse(proxy.root.exists())
        self.assertIsNotNone(proxy.process.poll())


if __name__ == "__main__":
    unittest.main()
