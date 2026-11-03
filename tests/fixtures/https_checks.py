"""Strict local HTTPS response and completed-capture assertions."""

import json
import shutil
import subprocess
import time
from urllib.parse import urlsplit
from uuid import uuid4

from .https_origin import HttpsOrigin
from .isolated_proxy import IsolatedProxy


def validate_response(result, path, nonce):
    assert result.returncode == 0, f"curl exit={result.returncode}: {result.stderr}"
    body, status = result.stdout.rsplit("\n", 1)
    assert status == "200", f"HTTPS status={status}: {body[:200]}"
    assert json.loads(body) == {"method": "GET", "path": path, "nonce": nonce}, \
        f"Unexpected HTTPS payload: {body[:200]}"


def completed_capture(session, url):
    target = urlsplit(url)
    request = session.get("request") or {}
    return (request.get("method") == "GET"
            and request.get("host") == target.netloc
            and request.get("uri") in (url, target.path + "?" + target.query)
            and (session.get("response") or {}).get("status") == 200
            and (session.get("metrics") or {}).get("status_code") == 200)


def wait_capture(proxy, url, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        response = proxy.client.get(proxy.url + "/admin/sessions", timeout=2)
        response.raise_for_status()
        payload = response.json()
        sessions = payload.get("sessions") if isinstance(payload, dict) else payload
        assert isinstance(sessions, list), "Malformed sessions API response"
        for session in sessions:
            assert isinstance(session, dict), "Malformed session entry"
            if completed_capture(session, url):
                return session
        time.sleep(.05)
    raise AssertionError(f"No completed GET capture for {url}")


def request_https(proxy, url):
    curl = shutil.which("curl")
    if not curl:
        raise RuntimeError("Missing required prerequisite: curl on PATH")
    version = subprocess.run([curl, "--version"], capture_output=True,
                             text=True, check=True, timeout=5).stdout
    args = [curl, "-sS", "--noproxy", "", "--http1.1", "--max-time", "8",
            "--proxy", proxy.url, "--cacert", str(proxy.ca_path),
            "--write-out", "\n%{http_code}"]
    if "Schannel" in version:
        args.append("--ssl-revoke-best-effort")
    return subprocess.run(args + [url], capture_output=True, text=True, timeout=12)


def check_local_https():
    if not shutil.which("curl"):
        raise RuntimeError("Missing required prerequisite: curl on PATH")
    with HttpsOrigin() as origin, IsolatedProxy("1") as proxy:
        nonce = uuid4().hex
        path = "/echo?nonce=" + nonce
        url = origin.url + path
        result = request_https(proxy, url)
        validate_response(result, path, nonce)
        session = wait_capture(proxy, url)
        return f"local TLS response=200, GET {url}, terminal capture={session.get('id')}"
