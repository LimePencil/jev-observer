#!/usr/bin/env python3
"""Measure bounded secret matching over authenticated loopback imports.

Synthetic data only; no provider calls or user credentials. Run the same script
against an old and new release binary. Timing is diagnostic, not a CI threshold.
"""
import argparse
import base64
import hashlib
import http.client
import json
import os
from pathlib import Path
import platform
import signal
import subprocess
import tempfile
import time


def compact(value):
    return json.dumps(value, separators=(',', ':'), ensure_ascii=False)


def fixture(event, secrets, strings):
    return {
        'id': event, 'source': 'fixture', 'provider': 'typesafe',
        'timestamp': int(time.time() * 1000), 'status': 200, 'capture_complete': True,
        'request': {'questions': {'q': {'type': 'noul', 'instructions': 'Is this eligible?'}},
                    'credentials': secrets, 'extension': strings},
        'response': {'answers': {'q': {'type': 'noul', 'noul': .8}}},
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve()
    report = {
        'harness_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        'binary_version': subprocess.check_output([str(binary), '--version'], text=True, timeout=5).strip(),
        'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
        'platform': platform.platform(), 'logical_cpus': os.cpu_count(),
        'load_average': list(os.getloadavg()) if hasattr(os, 'getloadavg') else None,
        'samples': [],
    }
    process = None
    try:
        with tempfile.TemporaryDirectory(prefix='observer-redaction-') as temporary:
            directory = Path(temporary)
            with (directory / 'observer.log').open('w+') as log:
                environment = dict(os.environ)
                environment.pop('TYPESAFE_API_KEY', None)
                process = subprocess.Popen(
                    [str(binary), '--demo', '--port', '0', '--db', str(directory / 'probe.sqlite')],
                    stdout=log, stderr=log, env=environment,
                )
                try:
                    port = None
                    deadline = time.monotonic() + 20
                    while time.monotonic() < deadline:
                        log.seek(0)
                        for line in log.read().splitlines():
                            if line.startswith('Observer: http://127.0.0.1:'):
                                port = int(line.split(':')[3].split()[0])
                        if port is not None:
                            break
                        if process.poll() is not None:
                            raise RuntimeError('Synthetic Observer failed to start')
                        time.sleep(.05)
                    if port is None:
                        raise RuntimeError('Synthetic Observer startup deadline exceeded')
                    # This benchmark compares old and new executables, each in
                    # its own fresh workspace. Prefer the complete-filename path
                    # and use the old layout only when the new file is absent.
                    token_path = directory / 'probe.demo.sqlite.token'
                    try:
                        token = token_path.read_text().strip()
                    except FileNotFoundError:
                        token_path = directory / 'probe.demo.access-token'
                        token = token_path.read_text().strip()
                    report['token_layout'] = 'current' if token_path.name.endswith('.token') else 'legacy'
                    authorization = 'Basic ' + base64.b64encode(('observer:' + token).encode()).decode()

                    def call(method, route, body=None):
                        connection = http.client.HTTPConnection('127.0.0.1', port, timeout=30)
                        try:
                            connection.request(method, route, body, {
                                'Authorization': authorization, 'X-Observer-Request': '1',
                                'Content-Type': 'application/json',
                            })
                            response = connection.getresponse()
                            return response.status, json.loads(response.read())
                        finally:
                            connection.close()

                    cases = [
                        (f'absent-{count}', fixture(f'absent-{count}',
                            [f'private-credential-{i:08}' for i in range(count)],
                            ['retained-public-field'] * count))
                        for count in [1000, 3000, 8000]
                    ]
                    cases.extend([
                        ('positive-work-limit', fixture('positive-work-limit',
                            ['a' * 32] + [f'absent-{i:08}' for i in range(8000)], ['a' * 32] * 8000)),
                        ('pattern-byte-limit', fixture('pattern-byte-limit', ['s' * (2 * 1024 * 1024)], [])),
                    ])
                    for name, record in cases:
                        text = compact(record)
                        body = compact({'format': 'systemone-capture', 'text': text})
                        started = time.monotonic()
                        status, result = call('POST', '/api/import', body.encode())
                        elapsed_ms = (time.monotonic() - started) * 1000
                        record_id = 'capture_v1_' + hashlib.sha256(compact(['fixture', 'typesafe', record['id']]).encode()).hexdigest()
                        detail_status, detail = call('GET', '/api/requests/' + record_id)
                        expected_strings = (['[REDACTED]'] * len(record['request']['extension'])
                                            if name == 'positive-work-limit' else record['request']['extension'])
                        sample = {
                            'case': name, 'secrets': len(record['request']['credentials']),
                            'retained_strings': len(record['request']['extension']),
                            'input_bytes': len(text.encode()), 'elapsed_ms': round(elapsed_ms, 2),
                            'status': status, 'result': result, 'stored': detail_status == 200,
                            'redacted': detail_status == 200 and detail['request_extra']['credentials'] == '[REDACTED]',
                            'saved_strings': len(detail.get('request_extra', {}).get('extension', [])),
                            'saved_strings_match': detail_status == 200 and detail.get('request_extra', {}).get('extension') == expected_strings,
                        }
                        report['samples'].append(sample)
                        print(json.dumps(sample), flush=True)
                finally:
                    if process.poll() is None:
                        process.send_signal(signal.SIGTERM)
                        try:
                            process.wait(timeout=20)
                        except subprocess.TimeoutExpired:
                            process.kill()
                            process.wait(timeout=5)

    finally:
        if process is not None and process.poll() is None:
            process.kill()
            process.wait(timeout=5)
        report['process_returncode'] = process.returncode if process is not None else None
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
