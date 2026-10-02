#!/usr/bin/env python3
"""Exercise PowerShell installs with real native packages and a loopback mirror."""
import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import io
import os
from pathlib import Path
import platform
import subprocess
import tempfile
import threading
import unittest
import zipfile

parser = argparse.ArgumentParser()
parser.add_argument('--assets', type=Path, required=True)
args = parser.parse_args()
root = Path(__file__).resolve().parent.parent
assets = args.assets.resolve()
manifest = (assets / 'SHA256SUMS').read_bytes()
archive = next(assets.glob('*.zip'))
archive_bytes = archive.read_bytes()
with zipfile.ZipFile(io.BytesIO(archive_bytes)) as package:
    executable_bytes = package.read('jev-observer.exe')
version = archive.name.split('-v', 1)[1].split('-' + ('aarch64' if platform.machine().lower() in ('arm64', 'aarch64') else 'x86_64'), 1)[0]

class Handler(BaseHTTPRequestHandler):
    payloads = {}
    def do_GET(self):
        body = self.payloads.get(self.path)
        if body is None:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *_):
        pass

server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
worker = threading.Thread(target=server.serve_forever, daemon=True)
worker.start()

class Installation(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='observer install with spaces ')
        self.addCleanup(self.temporary.cleanup)
        self.installation = Path(self.temporary.name) / 'bin with spaces'
        self.installation.mkdir()
        self.destination = self.installation / 'jev-observer.exe'
        self.destination.write_bytes(b'existing installation')
        self.reset_payloads(manifest, archive_bytes)

    def reset_payloads(self, checksums, package, name=None, release=None):
        Handler.payloads = {
            '/latest/download/SHA256SUMS': checksums,
            f'/download/v{release or version}/SHA256SUMS': checksums,
            f'/download/v{release or version}/{name or archive.name}': package,
        }

    def run_installer(self, expected_success=False, *extra):
        environment = os.environ.copy()
        environment['JEV_OBSERVER_RELEASE_BASE_URL'] = f'http://127.0.0.1:{server.server_port}'
        environment['JEV_OBSERVER_ALLOW_INSECURE_HTTP'] = '1'
        result = subprocess.run(['pwsh', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
            '-File', str(root / 'install.ps1'), '-InstallDir', str(self.installation), *extra],
            env=environment, capture_output=True, text=True, timeout=60)
        if expected_success:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(self.destination.read_bytes(), executable_bytes)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(self.destination.read_bytes(), b'existing installation')
        self.assertFalse(list(self.installation.glob('.jev-observer-*.exe')))

    def test_install_and_repeat_upgrade(self):
        self.destination.unlink()
        self.run_installer(True)
        self.run_installer(True)
        output = subprocess.check_output([str(self.destination), '--version'], text=True).strip()
        self.assertEqual(output, 'jev-observer ' + version)

    def test_version_pin(self):
        self.run_installer(True, '-Version', version)

    def test_bad_checksum_preserves_installation(self):
        self.reset_payloads((('0' * 64) + '  ' + archive.name + '\n').encode(), archive_bytes)
        self.run_installer()

    def test_duplicate_manifest_entry_preserves_installation(self):
        self.reset_payloads(manifest + manifest, archive_bytes)
        self.run_installer()

    def test_missing_archive_preserves_installation(self):
        del Handler.payloads[f'/download/v{version}/{archive.name}']
        self.run_installer()

    def test_extra_archive_member_preserves_installation(self):
        body = io.BytesIO()
        with zipfile.ZipFile(body, 'w') as package:
            package.writestr('jev-observer.exe', executable_bytes)
            package.writestr('../escaped.txt', b'should never be extracted')
        data = body.getvalue()
        checksum = f'{hashlib.sha256(data).hexdigest()}  {archive.name}\n'.encode()
        self.reset_payloads(checksum, data)
        self.run_installer()
        self.assertFalse((Path(self.temporary.name) / 'escaped.txt').exists())

    def test_wrong_executable_version_preserves_installation(self):
        other = '99.0.0'
        name = archive.name.replace('-v' + version + '-', '-v' + other + '-')
        checksum = f'{hashlib.sha256(archive_bytes).hexdigest()}  {name}\n'.encode()
        self.reset_payloads(checksum, archive_bytes, name=name, release=other)
        self.run_installer()

try:
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(Installation)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
finally:
    server.shutdown()
    server.server_close()
    worker.join(timeout=5)
if not result.wasSuccessful():
    raise SystemExit(1)
