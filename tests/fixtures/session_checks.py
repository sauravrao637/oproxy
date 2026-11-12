"""Session deletion assertions confined to an isolated storage instance."""

from datetime import datetime, timezone
import time
from uuid import uuid4

from fixtures.isolated_proxy import IsolatedProxy


def wait_for_ids(proxy, expected, timeout=5):
    deadline = time.monotonic() + timeout
    while True:
        response = proxy.client.get(proxy.url + "/admin/sessions", timeout=3)
        response.raise_for_status()
        data = response.json()
        sessions = data["sessions"] if isinstance(data, dict) else data
        actual = {session["id"] for session in sessions}
        if actual == expected:
            return
        assert time.monotonic() < deadline, (
            f"Expected session IDs {expected}, got {actual}")
        time.sleep(.05)


def check_session_deletion():
    with IsolatedProxy() as proxy:
        sid = "delete-" + uuid4().hex
        sample = {
            "id": sid,
            "timestamp": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
            "request": {"method": "GET", "uri": "http://delete.test/" + sid,
                        "headers": {}, "body": "", "host": "delete.test",
                        "body_bytes": None},
            "response": None, "metrics": None, "ws_frames": [],
        }
        response = proxy.client.post(proxy.url + "/admin/sessions/import",
                                     json={"sessions": [sample], "merge": False}, timeout=3)
        response.raise_for_status()
        assert response.json().get("imported") == 1, "Import count must be one"
        wait_for_ids(proxy, {sid})
        response = proxy.client.delete(proxy.url + "/admin/sessions/" + sid, timeout=3)
        method = "individual deletion"
        if response.status_code in (404, 405):
            method = "bulk deletion (individual endpoint unsupported)"
            response = proxy.client.delete(proxy.url + "/admin/sessions", timeout=3)
        response.raise_for_status()
        wait_for_ids(proxy, set())
        proxy.restart()
        wait_for_ids(proxy, set())
        return method + "; import verified before deletion; sessions remain absent after restart"
