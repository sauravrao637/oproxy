"""Live HAR export and fault detection with cleanup on every exit path."""

import json
import unittest
from unittest.mock import patch

import requests

from fixtures.har_checks import verify_har_export
from fixtures.isolated_proxy import IsolatedProxy
from fixtures.session_checks import wait_for_ids


class HarChecksTests(unittest.TestCase):
    def test_export_faults_and_cleanup(self):
        with IsolatedProxy() as proxy:
            request = proxy.client.request
            identities = []
            faults = (None, "empty", "duplicate", "identity", "method", "url",
                      "status", "body", "unavailable", "import-count", "interrupt")
            for fault in faults:
                def injected(method, url, **kwargs):
                    response = request(method, url, **kwargs)
                    if url.endswith("/sessions/import"):
                        identities.append(kwargs["json"]["sessions"][0]["id"])
                        if fault == "interrupt":
                            raise KeyboardInterrupt("after import")
                        if fault == "import-count":
                            response._content = b'{"imported":0}'
                    if url.endswith("/sessions/export/har"):
                        data = response.json()
                        entries = data["log"]["entries"]
                        if fault == "empty":
                            entries.clear()
                        elif fault == "duplicate":
                            entries.append(entries[0])
                        elif fault == "identity":
                            entries[0]["_oproxy_id"] = "unrelated"
                        elif fault in ("method", "url"):
                            entries[0]["request"][fault] = "incorrect"
                        elif fault == "status":
                            entries[0]["response"]["status"] = 500
                        elif fault == "body":
                            entries[0]["response"]["content"]["text"] = "incorrect"
                        elif fault == "unavailable":
                            response.status_code = 503
                        response._content = json.dumps(data).encode()
                    return response

                with self.subTest(fault=fault), patch.object(proxy.client, "request", injected):
                    if fault is None:
                        self.assertIn("HAR export verified", verify_har_export(proxy))
                    else:
                        expected = (KeyboardInterrupt if fault == "interrupt" else
                                    requests.HTTPError if fault == "unavailable" else AssertionError)
                        with self.assertRaises(expected):
                            verify_har_export(proxy)
                wait_for_ids(proxy, set())
            self.assertEqual(len(set(identities)), len(faults))


if __name__ == "__main__":
    unittest.main()
