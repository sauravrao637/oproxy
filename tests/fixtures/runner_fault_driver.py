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
        path = "/echo?nonce=after-fault"
        url = origin.url + path
        if fault == "stopped-origin":
            origin.server.shutdown()
            origin.server.server_close()
        if fault == "suppressed-capture":
            proxy.client.post(proxy.url + "/admin/capture-filter", timeout=2,
                              json={"mode": "denylist", "hosts": ["127.0.0.1"]}).raise_for_status()
        result = request_https(proxy, url)
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
            "script": str(Path(__file__).resolve()), "args": ["child", fault], "timeout": 45}]}
        with patch.object(qa_process, "build"), patch.object(qa_process, "REPO_ROOT", directory), \
             patch.object(qa_runner, "load_matrix", return_value={"fault": profile}), \
             patch.object(sys, "argv", ["qa_runner.py", "fault"]):
            return qa_runner.main()


if __name__ == "__main__":
    mode, fault = sys.argv[1:]
    if mode == "runner":
        raise SystemExit(runner(fault))
    if fault.startswith("report-"):
        protocol_report(fault.removeprefix("report-"))
    else:
        https_fault(fault)
