#!/usr/bin/env python3
"""Fault injection for retention-soak failure evidence; no Observer process or HTTP."""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location('soak', Path(__file__).with_name('check-retention-soak.py'))
soak = importlib.util.module_from_spec(spec)
spec.loader.exec_module(soak)
CANARY = 'private-exception-text-must-not-be-saved'


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
    unittest.main()
