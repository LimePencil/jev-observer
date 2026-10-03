#!/usr/bin/env python3
"""Run synthetic decisions through Observer to an existing local System One model.

The model server must already be running on loopback without a provider key.
This test uses real model inference, a disposable encrypted Observer workspace,
and no paid service. It never downloads models or starts the model server.
"""
import argparse
import base64
import copy
from datetime import datetime, timezone
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import secrets
import signal
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, target):
        return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--upstream', default='http://127.0.0.1:8000/v1/systemone')
    parser.add_argument('--provider', default='laya')
    parser.add_argument('--model', default='english')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    url = urllib.parse.urlsplit(args.upstream)
    host = url.hostname or ''
    loopback = host == 'localhost'
    try:
        loopback = loopback or ipaddress.ip_address(host).is_loopback
    except ValueError:
        pass
    if not loopback or url.scheme not in {'http', 'https'} or url.username or url.password or url.query or url.fragment:
        parser.error('upstream must be a loopback HTTP(S) endpoint without credentials or query')
    binary = args.binary.resolve(strict=True)
    report = {'checked_at': datetime.now(timezone.utc).isoformat(), 'scope': 'Real local model inference through Observer; synthetic inputs',
              'provider': args.provider, 'requested_model': args.model, 'upstream': args.upstream,
              'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'checks': {}, 'responses': [], 'passed': False}
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    process = None
    access = ''

    def check(condition, name):
        report['checks'][name] = bool(condition)

    def call(path, method='GET', body=None, auth=None):
        headers = {'Authorization': auth or 'Basic ' + base64.b64encode(('observer:' + access).encode()).decode(),
                   'Accept-Encoding': 'identity', 'Content-Type': 'application/json', 'X-Observer-Source': 'local-model-smoke'}
        req = urllib.request.Request(origin + path, data=None if body is None else json.dumps(body).encode(), headers=headers, method=method)
        try:
            response = opener.open(req, timeout=180)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            return response.status, json.load(response)

    with tempfile.TemporaryDirectory(prefix='observer-local-model-') as temporary:
        directory = Path(temporary)
        env = {key: value for key, value in os.environ.items() if not key.startswith(('TYPESAFE_', 'OPENROUTER_')) and key.lower() not in {'http_proxy', 'https_proxy', 'all_proxy'}}
        env['JEV_OBSERVER_DB_KEY'] = secrets.token_hex(32)
        try:
            with (directory / 'observer.log').open('w+') as logfile:
                process = subprocess.Popen([str(binary), '--port', '0', '--db', str(directory / 'history.sqlite'), '--upstream', args.upstream,
                                            '--upstream-auth', 'none', '--provider', args.provider], env=env, stdout=logfile, stderr=logfile,
                                           creationflags=subprocess.CREATE_NEW_PROCESS_GROUP if os.name == 'nt' else 0)
                origin = None
                for _ in range(300):
                    logfile.seek(0)
                    for line in logfile.read().splitlines():
                        if line.startswith('Observer: http://'):
                            origin = line.split(' ', 1)[1]
                    if origin:
                        break
                    if process.poll() is not None:
                        raise RuntimeError('Observer startup failed')
                    time.sleep(.1)
                if not origin:
                    raise RuntimeError('Observer startup deadline')
                access = (directory / 'history.access-token').read_text().strip()
                body = {'model': args.model, 'state': 'A synthetic customer ticket: I was charged twice. Please refund the duplicate payment today.',
                        'questions': {'department': {'type': 'choice', 'instructions': 'Which team should handle this ticket?', 'criteria': {'billing': 'Payments', 'technical': 'Bugs', 'sales': 'Plans'}},
                                      'urgency': {'type': 'noul', 'instructions': 'Does this ticket request a response today?'},
                                      'frustration': {'type': 'score', 'instructions': 'How frustrated is the customer?', 'criteria': ['Calm', 'Frustrated', 'Very angry']}}}
                for i in range(3):
                    request = copy.deepcopy(body)
                    if i == 2:
                        request['questions']['department']['instructions'] += ' Treat duplicate charges as billing.'
                    started = time.monotonic()
                    status, response = call('/v1/systemone', 'POST', request, 'Bearer ' + access)
                    check(status == 200, f'live_response_{i}_success')
                    report['responses'].append({'status': status, 'duration_ms': round((time.monotonic() - started) * 1000, 2),
                                                'model': response.get('model'), 'answers': response.get('answers'), 'usage': response.get('usage')})
                _, before = call('/api/health')
                status, _ = call('/v1/systemone', 'POST', body, 'Bearer invalid-local-token')
                _, after = call('/api/health')
                check(status == 401 and after['forwarded'] == before['forwarded'], 'invalid_local_access_rejected')
                for _ in range(200):
                    _, health = call('/api/health')
                    if health['persisted'] == 3:
                        break
                    time.sleep(.05)
                _, dashboard = call('/api/dashboard?window=all')
                records = [call('/api/requests/' + row['id'])[1] for row in dashboard['requests']]
                check(len(records) == 3 and dashboard['summary']['answer_count'] == 9, 'all_requests_and_answers_recorded')
                check(len(dashboard['groups']) == 4, 'definition_versions_separate')
                check(all(record['provider'] == args.provider for record in records), 'provider_attribution')
                check(all(record['capture_complete'] and len(record['answers']) == 3 and all(answer['valid'] for answer in record['answers']) for record in records), 'all_primitives_valid')
                check(all(not record['state_retained'] and record['state'] is None for record in records), 'input_not_retained')
                check(all(record['cost_usd'] is None for record in records), 'no_invented_local_cost')
                check(all(health.get(key) == 0 for key in ('dropped', 'truncated', 'write_failures')), 'no_capture_gaps')
                check(access not in json.dumps(records), 'local_access_token_absent_from_history')
                for name in ('input_tokens', 'output_tokens'):
                    counts = [response.get('usage', {}).get(name) for response in report['responses']]
                    check(all(isinstance(value, int) for value in counts) and dashboard['summary'][name] == sum(value for value in counts if isinstance(value, int)), 'exact_' + name)
                report['summary'] = dashboard['summary']
                report['health'] = health
                report['warnings'] = [answer.get('warnings', []) for record in records for answer in record['answers']]
                report['invalid_answers'] = [answer for record in records for answer in record['answers'] if not answer['valid']]
        except Exception as error:
            report['error_type'] = type(error).__name__
        finally:
            if process is not None and process.poll() is None:
                process.send_signal(signal.CTRL_BREAK_EVENT if os.name == 'nt' else signal.SIGTERM)
                try:
                    process.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
            check(process is not None and process.returncode == 0, 'clean_shutdown')
            database = directory / 'history.sqlite'
            check(database.exists() and not database.read_bytes().startswith(b'SQLite format 3'), 'encrypted_history')
    report['passed'] = 'error_type' not in report and all(report['checks'].values())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(report, indent=2)
    if access:
        text = text.replace(access, '[REDACTED]')
    args.output.write_text(text + '\n')
    print(json.dumps({'passed': report['passed'], 'failed_checks': [name for name, result in report['checks'].items() if not result], 'error_type': report.get('error_type')}))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
