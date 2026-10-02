#!/usr/bin/env python3
"""Reproduce two next-release findings using disposable databases and a local mock.

Usage: python3 backend-repro.py /absolute/path/to/jev-observer
The script overwrites only temporary fixtures and makes no provider calls.
"""
import base64
import hashlib
import json
import os
import pathlib
import platform
import secrets
import sys
from datetime import datetime, timezone
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time
import urllib.request

binary = pathlib.Path(sys.argv[1]).resolve()
results = []
env = os.environ.copy()
env['JEV_OBSERVER_DB_KEY'] = secrets.token_hex(32)
env.pop('TYPESAFE_API_KEY', None)
for name in list(env):
    if name.lower() in {'http_proxy', 'https_proxy', 'all_proxy'}:
        env.pop(name)
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
with tempfile.TemporaryDirectory(prefix='jev-release-review-') as work:
    db = pathlib.Path(work) / 'future.sqlite'
    with sqlite3.connect(db) as conn:
        conn.executescript('CREATE TABLE schema_version(version INTEGER NOT NULL); INSERT INTO schema_version VALUES(999); CREATE TABLE sentinel(value TEXT); INSERT INTO sentinel VALUES("preserve-me");')
    before = db.read_bytes()
    result = subprocess.run([str(binary), '--db', str(db), '--port', '0'], env=env, text=True, capture_output=True, timeout=30)
    after = db.read_bytes()
    results.append({'case': 'unsupported_plaintext_schema', 'exit_code': result.returncode, 'rejected_schema': 'Unsupported database schema' in result.stderr, 'bytes_changed': before != after, 'before_plaintext': before.startswith(b'SQLite format 3\x00'), 'after_plaintext': after.startswith(b'SQLite format 3\x00'), 'stderr': result.stderr.replace(work, '<temporary-workspace>')})

    upstream = socket.socket()
    upstream.bind(('127.0.0.1', 0))
    upstream.listen(1)
    upstream_port = upstream.getsockname()[1]
    def interrupted_response():
        client, _ = upstream.accept()
        with client:
            request = b''
            while b'\r\n\r\n' not in request:
                request += client.recv(65536)
            headers, body = request.split(b'\r\n\r\n', 1)
            length = next(int(line.split(b':', 1)[1]) for line in headers.split(b'\r\n') if line.lower().startswith(b'content-length:'))
            while len(body) < length:
                body += client.recv(65536)
            client.sendall(b'HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{"answers":')
            time.sleep(0.15)
        upstream.close()
    threading.Thread(target=interrupted_response, daemon=True).start()
    db = pathlib.Path(work) / 'stream.sqlite'
    log = open(pathlib.Path(work) / 'observer.log', 'w+')
    proc = subprocess.Popen([str(binary), '--db', str(db), '--port', '0', '--upstream', f'http://127.0.0.1:{upstream_port}/v1/systemone'], env=env, stdout=log, stderr=log)
    try:
        origin = None
        for _ in range(150):
            log.seek(0)
            text = log.read()
            for line in text.splitlines():
                if line.startswith('Observer: http://'):
                    origin = line.split(' ', 1)[1]
            if origin:
                break
            time.sleep(0.1)
        assert origin, text
        token = db.with_suffix('.access-token').read_text()
        basic = 'Basic ' + base64.b64encode(('observer:' + token).encode()).decode()
        body = json.dumps({'questions': {'q': {'type': 'noul', 'instructions': 'Is this a test?'}}}).encode()
        req = urllib.request.Request(origin + '/v1/systemone', data=body, headers={'Authorization': 'Bearer dummy-provider-key', 'x-observer-access': token, 'Content-Type': 'application/json'})
        client_failed = False
        try:
            with opener.open(req, timeout=5) as response:
                response.read()
        except Exception as error:
            client_failed = True
        def get(path):
            request = urllib.request.Request(origin + path, headers={'Authorization': basic})
            with opener.open(request, timeout=5) as response:
                return json.load(response)
        for _ in range(50):
            dashboard = get('/api/dashboard?window=all')
            if dashboard['summary']['request_count']:
                break
            time.sleep(0.1)
        detail = get('/api/requests/' + dashboard['requests'][0]['id'])
        failures = get('/api/dashboard?window=all&status=error')
        results.append({'case': '200_response_body_abort', 'client_failed': client_failed, 'stored_status': detail['status'], 'transport_error': detail['transport_error'], 'capture_complete': detail['capture_complete'], 'total_request_count': dashboard['summary']['request_count'], 'failure_count': dashboard['summary']['error_count'], 'failure_filter_request_count': failures['summary']['request_count']})
    finally:
        proc.terminate()
        proc.wait(timeout=20)
        log.close()

print(json.dumps({"recorded_at": datetime.now(timezone.utc).isoformat(), "binary_version": subprocess.check_output([str(binary), "--version"], env=env, text=True).strip(), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "platform": platform.system(), "architecture": platform.machine(), "provider_calls": 0, "observations": results}, indent=2))
