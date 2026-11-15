"""Strict local HTTPS response and completed-capture assertions."""

import json
import shutil
import subprocess
import time
import tempfile
from pathlib import Path
from types import SimpleNamespace
from urllib.parse import urlsplit
from uuid import uuid4

import requests
import yaml

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


def request_https(proxy, url, *, ca_path=None):
    curl = shutil.which("curl")
    if not curl:
        raise RuntimeError("Missing required prerequisite: curl on PATH")
    version = subprocess.run([curl, "--version"], capture_output=True,
                             text=True, check=True, timeout=5).stdout
    args = [curl, "--disable", "-sS", "--noproxy", "", "--http1.1", "--max-time", "8",
            "--proxy", proxy.url if proxy else "", "--cacert",
            str(ca_path if ca_path is not None else proxy.ca_path),
            "--write-out", "\n%{http_code}"]
    if "Schannel" in version:
        args.append("--ssl-revoke-best-effort")
    return subprocess.run(args + [url], capture_output=True, text=True, timeout=12)


def validate_upstream_rejection(result):
    assert result.returncode == 0, f"curl exit={result.returncode}: {result.stderr}"
    body, status = result.stdout.rsplit("\n", 1)
    assert status == "502", f"Expected upstream rejection, status={status}: {body[:200]}"
    assert "TLS certificate verification failed" in body, \
        f"Not an upstream certificate rejection: {body[:300]}"


def check_upstream_policy(proxy, accept):
    """Distinguish upstream trust rejection from client TLS or origin failure."""
    with HttpsOrigin() as origin:
        nonce = uuid4().hex
        path = "/echo?nonce=" + nonce
        url = origin.url + path
        validate_response(request_https(None, url, ca_path=origin.ca_path), path, nonce)
        result = request_https(proxy, url)
        if accept:
            validate_response(result, path, nonce)
            wait_capture(proxy, url)
            return "primary upstream override: exact response and terminal GET capture"
        validate_upstream_rejection(result)
        return "primary upstream verification: certificate rejected (502); origin ready"


def primary_upstream_policy(port, explicit=None):
    if explicit is not None:
        return explicit == "accept"
    matrix = Path(__file__).resolve().parents[1] / "qa-matrix.yaml"
    profiles = yaml.safe_load(matrix.read_text(encoding="utf-8"))["profiles"]
    matches = [p for p in profiles.values() if p["port"] == port]
    if len(matches) != 1:
        raise RuntimeError("Unknown primary profile port; specify --upstream-policy reject or accept")
    value = matches[0].get("env", {}).get("OPROXY_INSECURE_UPSTREAM", "")
    return str(value).strip().lower() in ("1", "true", "yes")


def check_primary_https(url, token, accept):
    with tempfile.TemporaryDirectory(prefix="primary-https-") as directory, \
         requests.Session() as client:
        client.trust_env = False
        if token:
            client.headers["x-oproxy-admin-token"] = token
        response = client.get(url + "/admin/ca", timeout=3)
        response.raise_for_status()
        ca_path = Path(directory) / "client-ca.pem"
        ca_path.write_text(response.text, encoding="utf-8")
        proxy = SimpleNamespace(url=url, client=client, ca_path=ca_path)
        return check_upstream_policy(proxy, accept)


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
