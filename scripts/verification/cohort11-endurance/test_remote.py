#!/usr/bin/env python3
"""Pure/mock controller controls: no services, subprocesses or network calls."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('remote_endurance', ROOT / 'remote.py')
r = importlib.util.module_from_spec(spec)
spec.loader.exec_module(r)


class Controls(unittest.TestCase):
    def setUp(self):
        binding = patch.multiple(r, RUN=1234, HEAD='a' * 40, ARTIFACT='synthetic-fixture-artifact')
        binding.start()
        self.addCleanup(binding.stop)

    def test_unbound_target_never_calls_api(self):
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, GITHUB_REPOSITORY=r.REPOSITORY), patch.dict(r.TARGET, status='awaiting_verified_artifact'), patch.object(r, 'api_json') as api:
            with self.assertRaisesRegex(r.Failure, 'reviewed_target_required'):
                r.verify_upstream(Path(directory))
            api.assert_not_called()

    def run_fixture(self):
        return {'id': r.RUN, 'head_sha': r.HEAD, 'run_attempt': 1, 'repository': {'full_name': r.REPOSITORY},
                'event': 'workflow_dispatch', 'path': '.github/workflows/ci.yml', 'status': 'completed', 'conclusion': 'success'}

    def test_run_requires_exact_successful_dispatch(self):
        r.validate_run(self.run_fixture())
        for field, value in [('head_sha', 'wrong'), ('run_attempt', 2), ('event', 'pull_request'), ('conclusion', 'failure'), ('status', 'in_progress')]:
            case = self.run_fixture(); case[field] = value
            with self.assertRaises(r.Failure):
                r.validate_run(case)

    def artifact(self, root):
        incoming = root / 'incoming'; incoming.mkdir()
        for name in (r.BINARY, r.PROBE):
            (incoming / name).write_bytes(('synthetic non-executable fixture ' + name).encode())
        value = {'schema': 1, 'source_commit': r.SOURCE, 'verification_commit': r.HEAD,
                 'production_manifest_sha256': r.PRODUCTION, 'excluded_only': '.github/workflows/ci.yml',
                 'run_id': str(r.RUN), 'run_attempt': '1', 'repository': r.REPOSITORY,
                 'runner_os': 'Linux', 'runner_arch': 'X64',
                 'binaries': {name: {'sha256': r.sha(incoming / name)} for name in (r.BINARY, r.PROBE)}}
        r.write(incoming / 'provenance.json', value)
        upstream = {'verified': True, 'run_id': r.RUN, 'verification_commit': r.HEAD,
                    'expected_provenance_sha256': r.sha(incoming / 'provenance.json'),
                    'expected_binary_sha256': r.sha(incoming / r.BINARY)}
        r.write(root / 'upstream.json', upstream)
        return incoming, value, upstream

    def test_provenance_rejects_wrong_source_or_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            incoming, value, upstream = self.artifact(Path(directory))
            r.validate_provenance(value, upstream)
            for field in ['source_commit', 'production_manifest_sha256', 'repository', 'runner_arch']:
                case = copy.deepcopy(value); case[field] = 'wrong'
                with self.assertRaises(r.Failure):
                    r.validate_provenance(case, upstream)
            case = copy.deepcopy(value); case['binaries'][r.BINARY]['sha256'] = 'wrong'
            with self.assertRaisesRegex(r.Failure, 'expected_binary_pin'):
                r.validate_provenance(case, upstream)

    def test_preparation_preserves_nine_inputs_and_exact_durations(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); self.artifact(root); work = r.prepare(root)
            pins = r.read(work / 'source-pins.json'); plan = r.read(work / 'plan.json')
            self.assertEqual(len(pins['copies']), 9)
            for name, pin in pins['copies'].items():
                self.assertEqual(r.sha(Path(name)), pin['sha256'])
            self.assertEqual(plan['runs']['retention']['cycles'], 15)
            self.assertEqual(plan['runs']['mixed']['cycles'], 22)
            self.assertEqual(plan['runs']['retention']['expected']['same_database_restarts'], 3)
            self.assertEqual(plan['runs']['mixed']['expected']['same_database_midpoint_restart_after_cycle'], 11)
            self.assertIn('325', plan['runs']['retention']['command'])
            self.assertIn('240', plan['runs']['mixed']['command'])
            self.assertIn("'audit.sqlite.token'", (work / 'mixed/harness.py').read_text())

    def test_downloaded_binary_tamper_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); incoming, _, _ = self.artifact(root)
            (incoming / r.BINARY).write_bytes(b'changed')
            with self.assertRaisesRegex(r.Failure, 'downloaded_binary_pin'):
                r.prepare(root)

    def test_symlink_artifact_rejected_without_reading_target(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); incoming, _, _ = self.artifact(root)
            canary = root / 'canary'; canary.write_bytes(b'untouched')
            (incoming / r.BINARY).unlink(); (incoming / r.BINARY).symlink_to(canary)
            with self.assertRaisesRegex(r.Failure, 'regular_owned_file'):
                r.prepare(root)
            self.assertEqual(canary.read_bytes(), b'untouched')

    def test_artifact_extra_file_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); incoming, _, _ = self.artifact(root)
            (incoming / 'unexpected').write_text('fixture')
            with self.assertRaisesRegex(r.Failure, 'artifact_file_set'):
                r.prepare(root)

    def test_upload_allowlist_excludes_database_key_token(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); private = root / 'private/tmp/owned'; private.mkdir(parents=True)
            for name in ['workspace.key', 'audit.sqlite.token', 'audit.sqlite', 'audit.sqlite-wal']:
                (private / name).write_text('private synthetic sentinel')
            r.write(root / 'summary.json', {'passed': False})
            destination = root / 'out'; r.collect(root, destination)
            self.assertEqual({str(x.relative_to(destination)) for x in destination.rglob('*') if x.is_file()}, {'summary.json', 'artifact-inventory.json'})
            self.assertEqual(len(list(private.iterdir())), 4)

    def test_upload_symlink_report_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); secret = root / 'workspace.key'; secret.write_text('synthetic sentinel')
            (root / 'summary.json').symlink_to(secret)
            with self.assertRaisesRegex(r.Failure, 'report_collection_failure'):
                r.collect(root, root / 'out')
            self.assertFalse((root / 'out/summary.json').exists())

    def test_upload_oversize_report_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with (root / 'summary.json').open('wb') as stream:
                stream.truncate(16 * 1024 * 1024 + 1)
            with self.assertRaisesRegex(r.Failure, 'report_collection_failure'):
                r.collect(root, root / 'out')

    def test_preparation_failure_remains_primary_through_cleanup_failure(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(r, 'prepare', side_effect=r.Failure('original_failure')), patch.object(r, 'cleanup', side_effect=OSError('not retained')), patch.object(r.subprocess, 'Popen') as popen:
            root = Path(directory)
            self.assertEqual(r.run(root), 1)
            report = r.read(root / 'summary.json')
            self.assertEqual(report['failure']['check'], 'original_failure')
            self.assertEqual(report['cleanup_failure'], {'type': 'OSError'})
            popen.assert_not_called()

    def test_cleanup_reaps_exited_supervisors(self):
        child = Mock(pid=12345); child.poll.return_value = 0
        with patch.object(r, 'processes', return_value={}):
            self.assertEqual(r.cleanup([child], {}), [])
        child.send_signal.assert_not_called()
        child.wait.assert_called_once_with(timeout=1)


if __name__ == '__main__':
    unittest.main()
