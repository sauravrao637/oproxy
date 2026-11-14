"""Async protocol origins must release listeners, threads and private keys."""

import asyncio
import importlib
from pathlib import Path
import socket
import sys
import tempfile
import threading
import unittest
from unittest.mock import Mock, patch
from urllib.parse import urlsplit

import httpx
from websockets.sync.client import connect
import proxy_tester as suite
from fixtures.isolated_proxy import IsolatedProxy


class ServerLifecycleTests(unittest.TestCase):
    def run_websocket_section(self, port):
        with patch.object(sys, 'argv', ['e2e_protocol_test.py']):
            e2e = importlib.import_module('e2e_protocol_test')
        results = []
        run = e2e.run_test
        def tunnel_only(name, fn):
            if name == 'WebSocket CONNECT tunnel + upgrade via HTTP proxy':
                run(name, fn)
        with patch.object(e2e, 'PROXY_HOST', '127.0.0.1'), \
             patch.object(e2e, 'PROXY_PORT', port), \
             patch.object(e2e, 'RESULTS', results), \
             patch.object(e2e, 'run_test', side_effect=tunnel_only):
            e2e.section_websocket()
        self.assertEqual(len(results), 1, results)
        self.assertFalse(any(t.name.startswith('fixture-') for t in threading.enumerate()))
        return results[0]

    def test_e2e_websocket_repeated_success_and_connection_failure_cleanup(self):
        with IsolatedProxy('1') as proxy:
            for _ in range(2):
                result = self.run_websocket_section(urlsplit(proxy.url).port)
                self.assertEqual(result['status'], 'PASS', result)
        result = self.run_websocket_section(0)
        self.assertEqual(result['status'], 'FAIL', result)

    def test_e2e_websocket_startup_failure_is_reported_and_joined(self):
        with patch.object(suite.websockets, 'serve', side_effect=RuntimeError('injected WS startup failure')):
            result = self.run_websocket_section(0)
        self.assertEqual(result['status'], 'FAIL', result)
        self.assertIn('injected WS startup failure', result['detail'])

    def test_repeated_origins_serve_and_release_resources(self):
        with tempfile.TemporaryDirectory() as scratch:
            with patch.object(tempfile, 'tempdir', scratch):
                for _ in range(3):
                    manager = suite.ServerManager()
                    try:
                        h2 = manager.start_http2()
                        ws = manager.start_websocket()
                        with httpx.Client(http2=True, verify=False, trust_env=False) as client:
                            response = client.get(f'https://127.0.0.1:{h2}/', timeout=3)
                            self.assertEqual(response.http_version, 'HTTP/2')
                            self.assertEqual(response.text, 'Hello HTTP/2')
                        with connect(f'ws://127.0.0.1:{ws}', proxy=None) as client:
                            client.send('fixture echo')
                            self.assertEqual(client.recv(timeout=3), 'fixture echo')
                    finally:
                        manager.stop_all()
                    manager.stop_all()
                    for port in (h2, ws):
                        with socket.socket() as listener:
                            listener.bind(('127.0.0.1', port))
                            listener.listen()
                    self.assertEqual(list(Path(scratch).iterdir()), [])
                    self.assertFalse(any(t.name.startswith('fixture-') for t in threading.enumerate()))

    def test_startup_failure_releases_thread_and_certificates(self):
        async def broken(*args, **kwargs):
            raise RuntimeError('injected serve failure')
        with tempfile.TemporaryDirectory() as scratch:
            with patch.object(tempfile, 'tempdir', scratch), \
                 patch.object(suite, 'hypercorn_serve', side_effect=broken):
                with self.assertRaisesRegex(RuntimeError, 'injected serve failure'):
                    suite.ServerManager().start_http2()
            self.assertEqual(list(Path(scratch).iterdir()), [])
        self.assertFalse(any(t.name.startswith('fixture-') for t in threading.enumerate()))

    def test_shutdown_failure_is_not_success_and_loop_is_closed(self):
        async def broken_shutdown(stop):
            while not stop.is_set():
                await asyncio.sleep(.01)
            raise RuntimeError('injected shutdown failure')
        manager = suite.ServerManager()
        with patch.object(suite.socket, 'create_connection'):
            manager._start_async_server('failure', 0, broken_shutdown)
        with self.assertRaisesRegex(RuntimeError, 'injected shutdown failure'):
            manager.stop_all()
        self.assertFalse(any(t.name.startswith('fixture-') for t in threading.enumerate()))

    def test_startup_timeout_and_interrupt_close_loop(self):
        for interrupted in (False, True):
            with self.subTest(interrupted=interrupted):
                loops = []
                async def waiting(stop):
                    loops.append(asyncio.get_running_loop())
                    while not stop.is_set():
                        await asyncio.sleep(.01)
                ready = Mock() if interrupted else threading.Event()
                if interrupted:
                    ready.wait.side_effect = KeyboardInterrupt
                manager = suite.ServerManager()
                expected = KeyboardInterrupt if interrupted else TimeoutError
                with self.assertRaises(expected):
                    manager._start_async_server('unready', 0, waiting, ready)
                self.assertEqual(manager.servers, [])
                self.assertEqual(len(loops), 1)
                self.assertTrue(loops[0].is_closed())
                self.assertFalse(any(t.name.startswith('fixture-') for t in threading.enumerate()))

    def test_unresponsive_shutdown_cancels_task_and_closes_loop(self):
        loops = []
        cancelled = threading.Event()
        ready = threading.Event()
        async def unresponsive(stop):
            loops.append(asyncio.get_running_loop())
            ready.set()
            try:
                await asyncio.Event().wait()
            finally:
                cancelled.set()
        manager = suite.ServerManager()
        manager._start_async_server('unresponsive', 0, unresponsive, ready)
        with self.assertRaisesRegex(RuntimeError, 'Fixture cleanup failed'):
            manager.stop_all()
        self.assertTrue(cancelled.is_set())
        self.assertTrue(loops[0].is_closed())
        self.assertFalse(any(t.name.startswith('fixture-') for t in threading.enumerate()))


if __name__ == '__main__':
    unittest.main()
