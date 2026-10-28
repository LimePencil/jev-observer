#!/usr/bin/env python3
"""Fault injection for retention-soak failure evidence; no Observer process or HTTP."""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location('soak', Path(__file__).with_name('check-retention-soak.py'))
soak = importlib.util.module_from_spec(spec)
spec.loader.exec_module(soak)
CANARY = 'private-exception-text-must-not-be-saved'


def signal_child(directory):
    """Reach the real idle wait without opening a database or HTTP connection."""
    directory = Path(directory)

    class App:
        def __init__(self, args):
            self.process = None
            self.log = None
            self.phase = 'setup'
            self.health_reads = 0

        def start(self, binary, extra):
            self.origin = 'http://127.0.0.1:1'
            self.token = CANARY
            self.token_path = Path(str(self.database) + '.token')
            self.process = SimpleNamespace(pid=os.getpid())

        def stop(self):
            self.process = None
            (directory / 'stopped').write_text('yes')
            time.sleep(.2)

        def api(self, route):
            count = int(self.health_reads > 0)
            self.health_reads += 1
            return {key: count for key in ('forwarded', 'captured', 'persisted')} | {
                key: 0 for key in ('queue_depth', 'active_captures', 'dropped', 'truncated',
                                  'write_failures', 'maintenance_failures')}

    class Connection:
        status = 200
        def __init__(self, *args, **kwargs): pass
        def connect(self): pass
        def request(self, *args): pass
        def getresponse(self): return self
        def read(self): return soak.upgrade.RESPONSE
        def close(self): pass

    server = SimpleNamespace(server_port=1, serve_forever=lambda: None,
                             shutdown=lambda: None, server_close=lambda: None)
    arguments = ['soak', '--binary', __file__, '--output', str(directory / 'report.json'),
                 '--active-state', str(directory / 'state.json'), '--cycles', '1',
                 '--seconds', '1', '--rate', '1']
    with mock.patch.object(soak.upgrade, 'Upgrade', App), \
         mock.patch.object(soak, 'ThreadingHTTPServer', return_value=server), \
         mock.patch.object(soak.http.client, 'HTTPConnection', Connection), \
         mock.patch('sys.argv', arguments):
        return soak.main()


@unittest.skipIf(os.name == 'nt', 'Windows terminate() does not deliver a catchable SIGTERM')
class SignalTests(unittest.TestCase):
    def test_sigterm_checkpoints_incomplete_cycle_and_stops_owned_process(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            child = subprocess.Popen([sys.executable, __file__, '--signal-child', temporary],
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    if (directory / 'state.json').exists():
                        state = json.loads((directory / 'state.json').read_text())
                        if state['progress']['subphase'] == 'idle_retention_wait':
                            break
                    self.assertIsNone(child.poll(), 'signal fixture exited before its idle wait')
                    time.sleep(.01)
                else:
                    self.fail('signal fixture did not reach its idle wait')
                child.send_signal(signal.SIGTERM)
                deadline = time.monotonic() + 5
                while not (directory / 'stopped').exists() and time.monotonic() < deadline:
                    self.assertIsNone(child.poll(), 'termination skipped child cleanup')
                    time.sleep(.01)
                self.assertTrue((directory / 'stopped').exists())
                child.send_signal(signal.SIGTERM)
                stdout, stderr = child.communicate(timeout=10)
                self.assertEqual(child.returncode, 128 + signal.SIGTERM, stderr)
                report = json.loads((directory / 'report.json').read_text())
                state = json.loads((directory / 'state.json').read_text())
                self.assertFalse(report['passed'])
                self.assertEqual(report['cycles'], [])
                self.assertEqual(report['total_offered'], 1)
                self.assertEqual(report['failure']['signal'], 'SIGTERM')
                self.assertEqual(report['failure']['progress']['subphase'], 'idle_retention_wait')
                self.assertEqual(report['failure']['progress']['completed'], 1)
                self.assertIn('finished_at', report)
                self.assertEqual(state['phase'], 'stopped')
                self.assertEqual(state['failure'], report['failure'])
                self.assertTrue((directory / 'stopped').exists())
                self.assertNotIn(CANARY, json.dumps(report) + json.dumps(state) + stdout + stderr)
            finally:
                if child.poll() is None:
                    child.kill()
                    child.communicate(timeout=5)


class DiagnosticsTests(unittest.TestCase):
    def exercise(self, stage, cleanup_failure=False, workspace_failure=False, diagnostic_failure=False):
        class App:
            def __init__(self, args):
                self.process = None
                self.log = None
                self.phase = 'setup'

            def start(self, binary, extra):
                self.origin = 'http://127.0.0.1:1'
                self.token = CANARY
                self.token_path = Path(str(self.database) + '.token')
                self.process = SimpleNamespace(pid=os.getpid())

            def stop(self):
                self.process = None
                if cleanup_failure:
                    raise RuntimeError(CANARY)

            def api(self, route):
                if stage == 'baseline_health':
                    raise TimeoutError(CANARY)
                return {'forwarded': 0, 'persisted': 0, 'queue_depth': 0, 'active_captures': 0,
                        'maintenance_healthy': True, 'unexpected_private_field': CANARY}

        class Connection:
            def __init__(self, *args, **kwargs):
                self.status = 200

            def fail(self, operation):
                if stage == operation:
                    raise TimeoutError(CANARY)

            def connect(self):
                self.fail('connect')

            def request(self, *args):
                self.fail('request_write')

            def getresponse(self):
                self.fail('response_headers')
                return self

            def read(self):
                self.fail('response_body')
                return soak.upgrade.RESPONSE

            def close(self):
                pass

        temporary_directory = tempfile.TemporaryDirectory

        class Workspace(temporary_directory):
            def __exit__(self, *args):
                super().__exit__(*args)
                if workspace_failure:
                    raise OSError(CANARY)

        server = SimpleNamespace(server_port=1, serve_forever=lambda: None,
                                 shutdown=lambda: None, server_close=lambda: None)
        with temporary_directory() as directory:
            output = Path(directory) / 'report.json'
            state = Path(directory) / 'state.json'
            write_text = Path.write_text

            def guarded_write_text(path, text, *args, **kwargs):
                if diagnostic_failure and path == output.with_name(output.name + '.tmp') and 'failure' in json.loads(text):
                    raise OSError(CANARY)
                return write_text(path, text, *args, **kwargs)

            arguments = ['soak', '--binary', __file__, '--output', str(output), '--active-state', str(state),
                         '--cycles', '1', '--seconds', '1', '--rate', '1']
            with contextlib.ExitStack() as patches:
                patches.enter_context(mock.patch.object(soak.upgrade, 'Upgrade', App))
                patches.enter_context(mock.patch.object(soak, 'ThreadingHTTPServer', return_value=server))
                patches.enter_context(mock.patch.object(soak.http.client, 'HTTPConnection', Connection))
                patches.enter_context(mock.patch.object(soak.tempfile, 'TemporaryDirectory', Workspace))
                patches.enter_context(mock.patch.object(Path, 'write_text', guarded_write_text))
                patches.enter_context(mock.patch('sys.argv', arguments))
                stdout = patches.enter_context(contextlib.redirect_stdout(io.StringIO()))
                self.assertEqual(soak.main(), 1)
            report = json.loads(stdout.getvalue().splitlines()[-1]) if diagnostic_failure else json.loads(output.read_text())
            state_value = json.loads(state.read_text())
            self.assertNotIn(CANARY, output.read_text() + state.read_text() + stdout.getvalue())
            self.assertEqual(report['failure']['type'], 'TimeoutError')
            self.assertFalse(report['passed'])
            self.assertEqual(state_value['phase'], 'stopped')
            self.assertEqual(state_value['failure'], report['failure'])
            for location in report['failure']['locations']:
                self.assertEqual(set(location), {'file', 'function', 'line'})
                self.assertNotIn('/', location['file'])
            return report

    def test_load_timeout_identifies_http_stage_without_private_data(self):
        for stage in ('connect', 'request_write', 'response_headers', 'response_body'):
            with self.subTest(stage=stage):
                report = self.exercise(stage)
                self.assertEqual(report['failure']['request']['stage'], stage)
                self.assertEqual(report['failure']['request']['index'], 0)
                self.assertEqual(report['failure']['progress']['submitted'], 1)
                self.assertEqual(report['failure']['progress']['completed'], 0)
                self.assertEqual(report['failure']['last_health']['values']['queue_depth'], 0)
                self.assertIn('resources', report['failure']['request'])
                self.assertNotIn('cleanup_failure', report)

    def test_api_timeout_names_subphase_without_making_a_failure_time_request(self):
        report = self.exercise('baseline_health')
        self.assertEqual(report['failure']['progress']['subphase'], 'baseline_health')
        self.assertEqual(report['total_offered'], 0)
        self.assertNotIn('request', report['failure'])

    def test_shutdown_failure_keeps_the_original_timeout(self):
        report = self.exercise('response_headers', cleanup_failure=True)
        self.assertEqual(report['cleanup_failure']['type'], 'RuntimeError')
        self.assertEqual(report['cleanup_failure']['stage'], 'observer_stop')

    def test_workspace_cleanup_failure_keeps_the_original_timeout(self):
        report = self.exercise('response_body', workspace_failure=True)
        self.assertEqual(report['cleanup_failure']['type'], 'OSError')
        self.assertEqual(report['cleanup_failure']['stage'], 'workspace_cleanup')

    def test_report_write_failure_keeps_timeout_in_state_and_stdout(self):
        report = self.exercise('response_headers', diagnostic_failure=True)
        self.assertEqual(report['cleanup_failure']['type'], 'OSError')
        self.assertEqual(report['cleanup_failure']['stage'], 'save_failure')


if __name__ == '__main__':
    if len(sys.argv) == 3 and sys.argv[1] == '--signal-child':
        raise SystemExit(signal_child(sys.argv[2]))
    unittest.main()
