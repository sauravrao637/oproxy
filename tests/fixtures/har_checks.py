"""Verify imported session identity and response content in an isolated HAR export."""

from datetime import datetime, timezone
from uuid import uuid4

from fixtures.isolated_proxy import IsolatedProxy
from fixtures.session_checks import wait_for_ids


def check_har_export():
    with IsolatedProxy() as proxy:
        return verify_har_export(proxy)


def verify_har_export(proxy):
    sid = "har-" + uuid4().hex
    uri = "http://export.test/" + sid
    body = "exported-" + sid
    sample = {
        "id": sid,
        "timestamp": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "request": {"method": "GET", "uri": uri, "headers": {},
                    "body": "", "host": "export.test", "body_bytes": None},
        "response": {"status": 201, "headers": {"content-type": "text/plain"},
                     "body": body, "request_uri": uri, "session_id": sid,
                     "ttfb_ms": 5, "body_ms": 3, "body_bytes": None},
        "metrics": None, "ws_frames": [],
    }
    # This helper owns the isolated instance's entire session store.
    wait_for_ids(proxy, set())
    try:
        response = proxy.client.post(proxy.url + "/admin/sessions/import",
                                     json={"sessions": [sample], "merge": False}, timeout=3)
        response.raise_for_status()
        assert response.json().get("imported") == 1, "Import count must be one"
        wait_for_ids(proxy, {sid})
        response = proxy.client.get(proxy.url + "/admin/sessions/export/har", timeout=3)
        response.raise_for_status()
        log = response.json()["log"]
        assert log["version"] == "1.2", "Incorrect HAR version"
        entries = log["entries"]
        assert len(entries) == 1, f"Expected exactly one HAR entry, got {len(entries)}"
        entry = entries[0]
        assert entry["_oproxy_id"] == sid, "Wrong exported session identity"
        assert entry["request"]["method"] == "GET", "Wrong exported method"
        assert entry["request"]["url"] == uri, "Wrong exported destination"
        assert entry["response"]["status"] == 201, "Wrong exported status"
        assert entry["response"]["content"]["text"] == body, "Wrong exported body"
    finally:
        response = proxy.client.delete(proxy.url + "/admin/sessions", timeout=3)
        response.raise_for_status()
        wait_for_ids(proxy, set())
    return "isolated import and HAR export verified identity, method, destination, status and body"
