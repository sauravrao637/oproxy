"""Local behavior and fault checks for required conditional protocol cases."""

import unittest
import socket
from unittest.mock import Mock, patch

import proxy_tester as suite
from fixtures.isolated_proxy import IsolatedProxy


class ConditionalProtocolTests(unittest.TestCase):
    def setUp(self):
        self.proxy = self.enterContext(IsolatedProxy("1"))
        self.enterContext(patch.object(suite, "ADMIN_TOKEN", self.proxy.admin_token))
        self.manager = suite.ServerManager("127.0.0.1")
        self.addCleanup(self.manager.stop_all)
        self.port = self.manager.start_http1()
        self.base = f"http://127.0.0.1:{self.port}"

    def test_fixture_listeners_close_and_cleanup_is_repeatable(self):
        tls_port = self.manager.start_https1()
        self.manager.stop_all()
        self.manager.stop_all()
        for port in (self.port, tls_port):
            with self.assertRaises(ConnectionRefusedError):
                socket.create_connection(('127.0.0.1', port), timeout=5)

    def test_rule_cleanup_positive_and_ignored_delete(self):
        for name, run in (
            ('mock', lambda: suite.test_mock_rule(self.proxy.url, self.base, 2, True)),
            ('access', lambda: suite.test_access_block(self.proxy.url, self.base, 2, True)),
            ('rewrite', lambda: suite.test_rewrite_rule(self.proxy.url, self.base, 2, True)),
            ('lua', lambda: suite.test_lua_script(self.proxy.url, '127.0.0.1', self.port, 2, True))):
            with self.subTest(rule=name):
                self.assertTrue(run())
                admin = suite._admin_request
                def ignore_delete(proxy, method, path, timeout, **kwargs):
                    if method == 'DELETE':
                        return Mock(raise_for_status=lambda: None)
                    return admin(proxy, method, path, timeout, **kwargs)
                with patch.object(suite, '_admin_request', side_effect=ignore_delete):
                    with self.assertRaisesRegex(RuntimeError, 'deleted rule still present'):
                        run()

    def test_capture_filter_positive_and_restoration(self):
        # Preserve a non-default setting instead of blindly disabling the filter.
        initial = {"mode": "denylist", "hosts": ["unrelated.invalid"]}
        self.proxy.client.post(self.proxy.url + "/admin/capture-filter", json=initial, timeout=2).raise_for_status()
        self.assertTrue(suite.test_capture_filter(self.proxy.url, self.base, 2, True))
        self.assertEqual(self.proxy.client.get(self.proxy.url + "/admin/capture-filter", timeout=2).json(), initial)

    def test_map_local_and_dns_cleanup_cannot_be_ignored(self):
        for name, run in (
            ('map', lambda: suite.test_map_local(self.proxy.url, '127.0.0.1', self.port, 2, True)),
            ('dns', lambda: suite.test_dns_override(self.proxy.url, '127.0.0.1', self.port, 2, True))):
            with self.subTest(feature=name):
                self.assertTrue(run())
                admin = suite._admin_request
                def ignore_delete(proxy, method, path, timeout, **kwargs):
                    if method == 'DELETE':
                        return Mock(raise_for_status=lambda: None)
                    return admin(proxy, method, path, timeout, **kwargs)
                with patch.object(suite, '_admin_request', side_effect=ignore_delete):
                    with self.assertRaises((AssertionError, RuntimeError)):
                        run()

    def test_throttling_restores_original_settings_and_detects_ignored_restore(self):
        initial = {'enabled': False, 'latency_ms': 31, 'bandwidth_limit_kbps': 123}
        self.proxy.client.put(self.proxy.url + '/admin/throttling', json=initial, timeout=2).raise_for_status()
        self.assertTrue(suite.test_throttling(self.proxy.url, self.base, 2, True))
        self.assertEqual(self.proxy.client.get(self.proxy.url + '/admin/throttling', timeout=2).json(), initial)
        admin = suite._admin_request
        def ignore_restore(proxy, method, path, timeout, **kwargs):
            if method == 'PUT' and path == '/admin/throttling' and kwargs['json'] == initial:
                return Mock(raise_for_status=lambda: None)
            return admin(proxy, method, path, timeout, **kwargs)
        with patch.object(suite, '_admin_request', side_effect=ignore_restore):
            with self.assertRaisesRegex(AssertionError, 'settings not restored'):
                suite.test_throttling(self.proxy.url, self.base, 2, True)

    def test_capture_filter_missing_recording_fails(self):
        with patch.object(suite, "_list_recorded_sessions", return_value=[]):
            self.assertFalse(suite.test_capture_filter(self.proxy.url, self.base, .3, False))

    def test_capture_filter_failed_restore_fails(self):
        admin = suite._admin_request
        def request(proxy, method, path, timeout, **kwargs):
            if method == 'POST' and path == '/admin/capture-filter' and kwargs['json']['mode'] != 'denylist':
                return Mock(raise_for_status=Mock(side_effect=RuntimeError('injected restore failure')))
            return admin(proxy, method, path, timeout, **kwargs)
        with patch.object(suite, "_admin_request", side_effect=request):
            self.assertFalse(suite.test_capture_filter(self.proxy.url, self.base, .5, False))

    def test_capture_filter_wrong_origin_response_fails(self):
        with patch.object(suite.requests, "get", return_value=Mock(status_code=200, content=b'wrong origin')):
            self.assertFalse(suite.test_capture_filter(self.proxy.url, self.base, .3, False))

    def test_capture_filter_ignored_exclusion_fails(self):
        admin = suite._admin_request
        def request(proxy, method, path, timeout, **kwargs):
            if method == 'POST' and path == '/admin/capture-filter' and kwargs['json']['mode'] == 'denylist':
                return Mock(raise_for_status=lambda: None)
            return admin(proxy, method, path, timeout, **kwargs)
        with patch.object(suite, '_admin_request', side_effect=request):
            self.assertFalse(suite.test_capture_filter(self.proxy.url, self.base, .5, False))

    def test_breakpoint_pause_and_release(self):
        original = self.proxy.client.get(self.proxy.url + '/admin/breakpoints', timeout=2).json()
        self.assertTrue(suite.test_breakpoint(self.proxy.url, '127.0.0.1', self.port, 2, True))
        self.assertEqual(self.proxy.client.get(self.proxy.url + '/admin/breakpoints', timeout=2).json(), original)
        self.assertEqual(self.proxy.client.get(self.proxy.url + '/admin/breakpoints/pending', timeout=2).json(), [])

    def test_unrelated_pending_request_does_not_count_as_paused(self):
        admin = suite._admin_request
        def request(proxy, method, path, timeout, **kwargs):
            if method == 'POST' and path == '/admin/breakpoints':
                return Mock(status_code=201, json=lambda: {'id': 'not-installed'})
            if method == 'GET' and path == '/admin/breakpoints/pending':
                return Mock(status_code=200, json=lambda: [{'id': 'foreign', 'bp_type': 'Request',
                    'context': {'Request': {'method': 'GET', 'uri': 'http://foreign.invalid/'}}}])
            if method == 'DELETE':
                return Mock(raise_for_status=lambda: None)
            if '/resolve' in path:
                self.fail('Attempted to release an unrelated request')
            return admin(proxy, method, path, timeout, **kwargs)
        with patch.object(suite, '_admin_request', side_effect=request):
            self.assertFalse(suite.test_breakpoint(self.proxy.url, '127.0.0.1', self.port, .3, False))

    def test_grpc_requires_decoded_capture(self):
        port = self.manager.start_grpc()
        self.assertTrue(suite.test_grpc(self.proxy.url, '127.0.0.1', port, 3, True,
            grpc_server_ca_pem=self.manager.grpc_ca_pem, session_admin_proxy=self.proxy.url))
        tunnel = {'request': {'method': 'CONNECT', 'uri': f'127.0.0.1:{port}'}}
        with patch.object(suite, '_list_recorded_sessions', return_value=[tunnel]):
            self.assertFalse(suite.test_grpc(self.proxy.url, '127.0.0.1', port, .5, False,
                grpc_server_ca_pem=self.manager.grpc_ca_pem, session_admin_proxy=self.proxy.url))


if __name__ == '__main__':
    unittest.main()
