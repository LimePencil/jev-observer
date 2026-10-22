#!/usr/bin/env python3
"""Current SDK default encodings; synthetic loopback only, bounded owned workspace."""
from __future__ import annotations
import argparse
import base64
import hashlib
import importlib.metadata
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import httpx2
from typesafe_sdk import Image, Noul, RetryPolicy, TypeSafeClient

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('legacy_sdk_helpers', ROOT / 'scripts/sdk-compatibility.py')
helpers = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helpers)
KEY = 'current-sdk-placeholder'
STATE_SECRET = 'current-state-placeholder'
PNG = base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aPioAAAAASUVORK5CYII=')
RESPONSE = b'{ "model":"synthetic", "answers":{"q":{"type":"noul","noul":0.75}}, "echo":"current-sdk-placeholder", "ordinary":"retained" }'

class WorkDeadline(Exception):
    pass

def timeout(_signum, _frame):
    raise WorkDeadline()

class Mock(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    calls = []
    failures = []
    def log_message(self, *_args):
        pass
    def do_POST(self):
        self.connection.settimeout(5)
        try:
            assert self.path == '/v1/systemone'
            assert self.headers.get('Authorization') == 'Bearer ' + KEY
            assert self.headers.get('Accept-Encoding') == 'identity'
            assert not any(name.lower().startswith('x-observer-') for name in self.headers)
            body = bytearray()
            if 'chunked' in self.headers.get('Transfer-Encoding', '').lower():
                while True:
                    line = self.rfile.readline(128)
                    assert line.endswith(b'\r\n')
                    size = int(line.split(b';', 1)[0], 16)
                    assert 0 <= size <= 65536 - len(body)
                    if not size:
                        assert self.rfile.readline(128) == b'\r\n'
                        break
                    body.extend(self.rfile.read(size))
                    assert self.rfile.read(2) == b'\r\n'
            else:
                size = int(self.headers['Content-Length'])
                assert 0 <= size <= 65536
                body.extend(self.rfile.read(size))
                assert len(body) == size
            self.calls.append({'sha256': hashlib.sha256(body).hexdigest(),
                               'coding': self.headers.get('Content-Encoding', 'identity'),
                               'media': self.headers.get('Content-Type')})
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(RESPONSE)))
            self.send_header('X-Current-SDK-Fixture', 'preserved')
            self.end_headers()
            self.wfile.write(RESPONSE)
        except Exception as error:
            self.failures.append(type(error).__name__)
            self.close_connection = True

class Transport(httpx2.HTTPTransport):
    def __init__(self, origin, kind):
        super().__init__(trust_env=False, retries=0)
        self.origin, self.kind, self.calls = origin, kind, []
    def handle_request(self, request):
        assert str(request.url) == self.origin + '/v1/systemone'
        body = request.read()
        assert len(body) <= 65536
        coding = request.headers.get('content-encoding', 'identity')
        assert coding == ('zstd' if self.kind in ('large', 'multipart') else 'identity')
        media = request.headers.get('content-type')
        assert media.startswith('multipart/form-data;') == (self.kind == 'multipart')
        self.calls.append({'sha256': hashlib.sha256(body).hexdigest(), 'coding': coding, 'media': media})
        response = super().handle_request(request)
        response.read()
        assert response.status_code == 200 and response.content == RESPONSE
        assert response.headers['x-current-sdk-fixture'] == 'preserved'
        return response

def python_sdk(origin, token):
    assert importlib.metadata.version('typesafe-sdk') == '0.7.5'
    calls = []
    for kind in ('small', 'large', 'multipart', 'disabled'):
        transport = Transport(origin, kind)
        with TypeSafeClient(api_key=KEY, base_url=origin, model='synthetic',
                            retry=RetryPolicy(max_retries=0), timeout=5, transport=transport,
                            **({'compression': False} if kind == 'disabled' else {}),
                            headers={'x-observer-source': 'current-python-' + kind,
                                     'x-observer-access': token, 'Accept-Encoding': 'identity'}) as client:
            state = {'password': STATE_SECRET, 'padding': '' if kind == 'small' else 'x' * 2048}
            if kind == 'multipart':
                state['photo'] = Image(PNG)
            result = client.system_one(state=state, questions={'q': Noul(instructions='Evaluate the synthetic example')},
                                       extra_body={'echo': STATE_SECRET, 'ordinary': 'retained'})
            assert result.nouls['q'].noul == 0.75
        assert len(transport.calls) == 1
        calls.extend(transport.calls)
    return calls

def stop(process):
    if process is None:
        return None
    if process.poll() is None:
        process.terminate()
    try:
        return process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        return process.wait(timeout=5)

def records(origin, token):
    import urllib.request
    request = urllib.request.Request(origin + '/api/export?format=jsonl&window=all',
                                     headers={'Authorization': helpers.local_auth(token)})
    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=5) as response:
        raw = response.read(256 * 1024 + 1)
    assert len(raw) <= 256 * 1024
    return [json.loads(line) for line in raw.splitlines()]

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--node-sdk', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error('Result already exists')
    binary = args.binary.resolve(strict=True)
    report = {'passed': False, 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
              'python_sdk': '0.7.5', 'javascript_sdk': '0.7.0', 'generations': []}
    env = {k: v for k, v in os.environ.items() if not k.startswith(('TYPESAFE_', 'JEV_OBSERVER_', 'OBSERVER_SDK_'))
           and k.lower() not in {'http_proxy', 'https_proxy', 'all_proxy'}}
    env.update(NO_PROXY='127.0.0.1,localhost', TYPESAFE_API_KEY=KEY)
    stage = 'startup'
    process = None
    mock = None
    thread = None
    temporary = None
    signal.signal(signal.SIGALRM, timeout)
    signal.signal(signal.SIGTERM, timeout)
    signal.alarm(120)
    try:
        for capture_state in (False, True):
            Mock.calls, Mock.failures = [], []
            temporary = tempfile.TemporaryDirectory(prefix='jev-current-sdk-')
            directory = Path(temporary.name)
            db = directory / 'history.sqlite'
            generation = {'capture_state': capture_state}
            report['generations'].append(generation)
            mock = ThreadingHTTPServer(('127.0.0.1', 0), Mock)
            mock.daemon_threads = True
            thread = threading.Thread(target=mock.serve_forever, daemon=True)
            thread.start()
            origin = f'http://127.0.0.1:{helpers.free_port()}'
            env['JEV_OBSERVER_DB_KEY'] = os.urandom(32).hex()
            with (directory / 'observer.log').open('wb') as log:
                command = [str(binary), '--db', str(db), '--port', origin.rsplit(':', 1)[1],
                           '--upstream', f'http://127.0.0.1:{mock.server_port}/v1/systemone', '--capture-limit', '65536']
                if capture_state:
                    command.append('--capture-state')
                process = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT)
                generation['pid'] = process.pid
                for _ in range(100):
                    assert process.poll() is None, 'Observer exited during startup'
                    try:
                        token = Path(str(db) + '.token').read_text()
                        helpers.get(origin, '/api/health', token)
                        break
                    except (OSError, ValueError):
                        time.sleep(.05)
                else:
                    raise AssertionError('Startup deadline')
                stage = 'python_sdk'
                calls = python_sdk(origin, token)
                stage = 'javascript_sdk'
                node = subprocess.run(['node', str(ROOT / 'scripts/current-sdk-compatibility.mjs')],
                    env={**env, 'OBSERVER_SDK_PROXY': origin, 'OBSERVER_SDK_PACKAGE': str(args.node_sdk.resolve()),
                         'OBSERVER_SDK_ACCESS': token, 'OBSERVER_SDK_RESPONSE': base64.b64encode(RESPONSE).decode(),
                         'OBSERVER_SDK_IMAGE': base64.b64encode(PNG).decode()},
                    capture_output=True, timeout=30)
                assert node.returncode == 0, 'JavaScript SDK failed'
                calls.extend(json.loads(node.stdout)['calls'])
                stage = 'accounting_privacy'
                assert not Mock.failures and len(calls) == 8 and calls == Mock.calls
                for _ in range(100):
                    health = helpers.get(origin, '/api/health', token)
                    if health['persisted'] == 8 and health['active_captures'] == 0:
                        break
                    time.sleep(.05)
                assert all(health[name] == 8 for name in ('forwarded', 'captured', 'persisted'))
                assert all(health[name] == 0 for name in ('dropped', 'truncated', 'write_failures', 'active_captures', 'queue_depth'))
                rows = records(origin, token)
                assert len(rows) == 8
                for row in rows:
                    assert row['capture_complete'] and len(row['answers']) == 1 and row['answers'][0]['valid']
                    assert row['answers'][0]['value'] == .75
                    assert row['request_extra']['echo'] == '[REDACTED]' and row['response_extra']['echo'] == '[REDACTED]'
                    assert STATE_SECRET not in json.dumps(row) and KEY not in json.dumps(row)
                    assert row['request_extra']['ordinary'] == row['response_extra']['ordinary'] == 'retained'
                    if not capture_state:
                        assert row['state'] is None and 'assets' not in row['request_extra']
                    elif row['source'].endswith('-multipart'):
                        assets = row['request_extra']['assets']
                        assert len(assets) == 1
                        asset = next(iter(assets.values()))
                        assert asset['type'] == 'image' and isinstance(asset['part'], str) and asset['format'] == 'png'
                        assert base64.b64decode(asset['data_base64']) == PNG
                with db.open('rb') as database:
                    encrypted = database.read(16) != b'SQLite format 3\x00'
                generation.update(calls=8, default_zstd=4, identity=4, exact_wire=True, privacy=True,
                                  persisted=8, gaps=0, encrypted_header=encrypted)
                assert generation['encrypted_header']
                generation['exit_code'] = stop(process)
                process = None
                assert generation['exit_code'] == 0
            mock.shutdown()
            mock.server_close()
            thread.join(5)
            assert not thread.is_alive()
            mock, thread = None, None
            temporary.cleanup()
            temporary = None
            generation['workspace_removed'] = not directory.exists()
            assert generation['workspace_removed']
        report['passed'] = True
    except BaseException as error:
        report['failure'] = {'stage': stage, 'type': type(error).__name__}
    finally:
        signal.alarm(0)
        cleanup_errors = []
        try:
            if process:
                report['failure_exit_code'] = stop(process)
        except BaseException as error:
            cleanup_errors.append(type(error).__name__)
        try:
            if mock:
                if thread and thread.is_alive():
                    mock.shutdown()
                mock.server_close()
            if thread and thread.ident is not None:
                thread.join(5)
                assert not thread.is_alive()
        except BaseException as error:
            cleanup_errors.append(type(error).__name__)
        try:
            if temporary and (process is None or process.poll() is not None):
                temporary.cleanup()
        except BaseException as error:
            cleanup_errors.append(type(error).__name__)
        if cleanup_errors:
            report['passed'] = False
            report['cleanup_failures'] = cleanup_errors
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': report['passed'], 'calls': sum(g.get('calls', 0) for g in report['generations'])}))
    return 0 if report['passed'] else 1

if __name__ == '__main__':
    raise SystemExit(main())
