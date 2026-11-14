"""Subprocess fault injection for runner verdicts, never a product baseline.

The parent reuses an already built binary. Report scenarios stub protocol work
to isolate accounting; HTTPS scenarios exercise real local TLS and capture.
"""

import json
import os
from pathlib import Path
import socket
import sys
import tempfile
import time
from urllib.parse import urlsplit
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))


def protocol_report(fault):
    import proxy_tester as suite
    manager = Mock(grpc_ca_pem=b"unused")
    for name in ("start_http1", "start_https1", "start_http2", "start_websocket", "start_grpc"):
        getattr(manager, name).return_value = 12345
    original = suite._print_final_summary

    def report(results, required):
        if fault == "missing":
            results[:] = [r for r in results if r[1] != "HTTP/2 GET"]
        elif fault == "zero":
            results.clear()
        elif fault == "duplicate":
            results.append(results[0])
        elif fault == "skip":
            results[0] = (*results[0][:2], None)
        return original(results, required)

    replacements = {name: (lambda *args, **kwargs: True)
                    for name in vars(suite) if name.startswith("test_")}
    with patch.dict(vars(suite), replacements), \
         patch.object(suite, "ServerManager", return_value=manager), \
         patch.object(suite, "_print_final_summary", side_effect=report), \
         patch.object(suite.time, "sleep"), patch.object(suite.threading, "Thread"), \
         patch.object(sys, "argv", ["proxy_tester.py", "--run-tests", "--http-proxy",
                                  "http://127.0.0.1:1234", "--socks-proxy",
                                  "socks5://127.0.0.1:1235", "--h3-proxy", "h3://127.0.0.1:1236"]):
        suite.run_tests()


def https_fault(fault):
    if fault == "rule-cleanup":
        import proxy_tester as suite
        from fixtures.isolated_proxy import IsolatedProxy
        with IsolatedProxy("1") as proxy, patch.object(suite, 'ADMIN_TOKEN', proxy.admin_token):
            manager = suite.ServerManager('127.0.0.1')
            port = manager.start_http1()
            admin = suite._admin_request
            def ignore_delete(url, method, path, timeout, **kwargs):
                if method == 'DELETE':
                    return Mock(raise_for_status=lambda: None)
                return admin(url, method, path, timeout, **kwargs)
            try:
                with patch.object(suite, '_admin_request', side_effect=ignore_delete):
                    suite.test_mock_rule(proxy.url, f'http://127.0.0.1:{port}', 2, True)
            finally:
                manager.stop_all()
        raise AssertionError('Ignored cleanup was not detected')
    if fault == "missing-fixture-module":
        import builtins
        importing = builtins.__import__
        def missing(name, *args, **kwargs):
            if name == "trustme":
                raise ModuleNotFoundError("injected missing fixture prerequisite: trustme")
            return importing(name, *args, **kwargs)
        with patch.object(builtins, "__import__", side_effect=missing):
            from fixtures.https_origin import HttpsOrigin
        raise AssertionError("Missing fixture prerequisite was ignored")
    from fixtures.https_checks import check_local_https, request_https, validate_response, wait_capture
    from fixtures.https_origin import HttpsOrigin
    from fixtures.isolated_proxy import IsolatedProxy
    if fault == "missing-curl":
        with patch("fixtures.https_checks.shutil.which", return_value=None):
            check_local_https()
        return
    if fault == "missing-binary":
        with patch.dict(os.environ, {"OPROXY_TEST_BINARY": str(Path(tempfile.gettempdir()) / "absent-binary")}):
            check_local_https()
        return
    with HttpsOrigin() as origin, IsolatedProxy("1") as proxy:
        path = "/echo?nonce=runner-fault"
        url = origin.url + path
        result = request_https(proxy, url)
        validate_response(result, path, "runner-fault")
        wait_capture(proxy, url)
        if fault == "timeout":
            Path(os.environ["OPROXY_FAULT_STATE"]).write_text(json.dumps({
                "pid": proxy.process.pid, "url": proxy.url, "root": str(proxy.root),
                "scratch": tempfile.gettempdir()}), encoding="utf-8")
            time.sleep(60)
        path = "/echo?nonce=after-fault"
        url = origin.url + path
        if fault == "stopped-origin":
            origin.server.shutdown()
            origin.server.server_close()
        if fault == "suppressed-capture":
            proxy.client.post(proxy.url + "/admin/capture-filter", timeout=2,
                              json={"mode": "denylist", "hosts": ["127.0.0.1"]}).raise_for_status()
        result = request_https(proxy, url, ca_path=origin.ca_path if fault == "client-trust" else None)
        if fault == "client-trust":
            assert result.returncode == 60, f"Expected untrusted client exit60, got {result.returncode}: {result.stderr}"
            print("Untrusted client certificate rejected: curl exit60", flush=True)
        if fault == "wrong-response":
            result.stdout = json.dumps({"method": "GET", "path": path, "nonce": "wrong"}) + "\n200"
        validate_response(result, path, "after-fault")
        wait_capture(proxy, url, timeout=.3)


def runner(fault):
    import qa_process
    import qa_runner
    import yaml
    with tempfile.TemporaryDirectory(prefix="runner-fault-") as directory:
        root = Path(directory)
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        config = root / "config.yaml"
        config.write_text(yaml.safe_dump({"port": port, "bind_host": "127.0.0.1",
            "update_check": False, "storage_path": str(root / "storage"),
            "mitm": {"enabled": True, "root_ca_path": str(root / "certs")}}), encoding="utf-8")
        profile = {"config": str(config), "port": port, "run": [{
            "script": str(Path(__file__).resolve()), "args": ["child", fault],
            "timeout": 8 if fault == "timeout" else 45}]}
        state_path = root / "child-state.json"
        with patch.object(qa_process, "build"), patch.object(qa_process, "REPO_ROOT", directory), \
             patch.object(qa_runner, "load_matrix", return_value={"fault": profile}), \
             patch.dict(os.environ, {"OPROXY_FAULT_STATE": str(state_path)}), \
             patch.object(sys, "argv", ["qa_runner.py", "fault"]):
            code = qa_runner.main()
        if fault == "timeout":
            state = json.loads(state_path.read_text(encoding="utf-8"))
            assert not Path(state["scratch"]).exists(), "Tester scratch retained"
            assert not Path(state["root"]).exists(), "Isolated proxy secrets retained"
            try:
                connection = socket.create_connection(("127.0.0.1", urlsplit(state["url"]).port), timeout=5)
            except ConnectionRefusedError:
                pass
            else:
                connection.close()
                raise AssertionError("Timed-out proxy listener still running")
            if os.name == "nt":
                import ctypes
                from ctypes import wintypes
                kernel = ctypes.WinDLL("kernel32", use_last_error=True)
                kernel.OpenProcess.restype = wintypes.HANDLE
                kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
                kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
                kernel.CloseHandle.argtypes = [wintypes.HANDLE]
                handle = kernel.OpenProcess(0x100000, False, state["pid"])
                if handle:
                    try:
                        assert kernel.WaitForSingleObject(handle, 0) == 0, "Proxy process still alive"
                    finally:
                        kernel.CloseHandle(handle)
            print("Timeout cleanup verified: isolated listener stopped and private scratch removed")
        return code


if __name__ == "__main__":
    mode, fault = sys.argv[1:]
    if mode == "runner":
        raise SystemExit(runner(fault))
    if fault.startswith("report-"):
        protocol_report(fault.removeprefix("report-"))
    else:
        https_fault(fault)
