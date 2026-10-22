#!/usr/bin/env python3
"""Evidence-only bounded mixed-operation endurance check; synthetic loopback only.

No run occurs on import. --self-check validates fixtures and pure comparisons;
--run requires an explicitly pinned binary and new evidence destination.
"""
import argparse
import base64
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor
import copy
import csv
from datetime import datetime, timezone
import hashlib
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import io
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import socket
import subprocess
import tempfile
import threading
import time
from urllib.parse import quote, urlencode

EVIDENCE = Path(__file__).resolve().parent.parent
PROVIDER_SECRET = 'synthetic-endurance-provider-7cf421f8'
STATE_SECRET = 'synthetic-endurance-state-93f74a01'
COLLISION_SECRETS = ('synthetic-endurance-alpha-341acf', 'synthetic-endurance-beta-72bc40')
LIVE_ERROR = 'Credential redaction caused an identifier or object key collision; request and response content were not retained'
IMPORT_ERROR = 'Import rejected because credential redaction makes distinct identifiers or object keys identical.'
MAX_PAYLOAD = 2048
MAX_RESPONSE = 32 * 1024 * 1024
MAX_WORKSPACE = 128 * 1024 * 1024
CSV_FIELDS = ['id', 'timestamp', 'source', 'model', 'status', 'duration_ms', 'input_tokens', 'output_tokens', 'cost_usd', 'cost_basis', 'answer_count', 'event_kind']
HEALTH_COUNTERS = ['forwarded', 'captured', 'persisted', 'dropped', 'truncated', 'write_failures', 'maintenance_failures', 'queue_depth', 'queued_bytes', 'active_captures']


class CheckFailure(Exception):
    """Only a static check name reaches the report; never response bodies."""


def require(condition, name):
    if not condition:
        raise CheckFailure(name)


def fallback_generated_id(identifier):
    return isinstance(identifier, str) and re.fullmatch(r'redacted_v1_[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}', identifier) is not None


def encode(value):
    return json.dumps(value, separators=(',', ':'), ensure_ascii=False, allow_nan=False).encode()


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False, allow_nan=False).encode()


def digest(value):
    return hashlib.sha256(canonical(value)).hexdigest()


def raw_import_identity(record):
    # The raw adapter stores this generated identity in both id and
    # source_event_id. The fixture uses ASCII provider names; lowercasing and
    # compact JSON match CaptureId::raw_import's explicit tuple convention.
    provider = record['provider']
    require(provider.isascii(), 'raw_fixture_provider_ascii')
    return 'capture_v1_' + digest([record['source'], provider.lower(), record['id']])


def now():
    return datetime.now(timezone.utc).isoformat()


def private_write(path, data):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'wb') as output:
        output.write(data)


def atomic_json(path, value):
    staging = path.with_name(path.name + '.tmp')
    data = encode(value) + b'\n'
    require(len(data) <= MAX_RESPONSE, 'report_size_limit')
    fd = os.open(staging, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, 'wb') as output:
        output.write(data)
    staging.replace(path)


def is_incomplete(index):
    return index % 100 == 99


def definition():
    return {'type': 'noul', 'instructions': 'Does the synthetic rule apply?'}


def request_fixture(index, token='', bad=None):
    bad = is_incomplete(index) if bad is None else bad
    request = {'model': 'mixed-model', 'questions': {'q': definition()}, 'fixture_index': index,
               'state': {'credentials': STATE_SECRET, 'private_input': 'synthetic-input-not-retained'},
               'echo': PROVIDER_SECRET, 'state_echo': STATE_SECRET, 'local_echo': token}
    if bad:
        request['credentials'] = list(COLLISION_SECRETS)
        request['questions'] = {'audit-' + key: definition() for key in COLLISION_SECRETS}
    return request


def response_fixture(index, token='', bad=None):
    bad = is_incomplete(index) if bad is None else bad
    keys = ['audit-' + key for key in reversed(COLLISION_SECRETS)] if bad else ['q']
    return {'model': 'mixed-model', 'answers': {key: {'type': 'noul', 'noul': .75} for key in keys},
            'usage': {'input_tokens': 5, 'output_tokens': 2},
            'echo': PROVIDER_SECRET, 'state_echo': STATE_SECRET, 'local_echo': token}


def raw_fixture(identifier, timestamp, bad=False):
    request, response = request_fixture(0, bad=bad), response_fixture(0, bad=bad)
    # Raw imports have no authenticated live provider/access headers. Keep the
    # state-discovered credential echo, without assuming an unknown echo is secret.
    for value in (request, response):
        value.pop('echo')
        value.pop('local_echo')
    return {'id': identifier, 'source': 'mixed-import-raw', 'provider': 'typesafe', 'timestamp': timestamp,
            'status': 200, 'capture_complete': True,
            'request': request, 'response': response}


def observer_fixture(identifier, timestamp, source='mixed-import-observer'):
    record = {key: None for key in ['requested_model', 'duration_ms', 'cost_usd', 'cost_basis', 'import_format', 'transport_error', 'state']}
    record.update(schema_version=1, id=identifier, timestamp=timestamp, source=source, provider='typesafe',
                  model='mixed-model', status=200, input_tokens=5, output_tokens=2,
                  source_event_id=identifier, capture_complete=True, state_retained=False, sample=False,
                  actions=[], labels=[], answers=[{'key': 'q', 'definition': definition(), 'raw_answer': {'type': 'noul', 'noul': .75}}])
    return record


def import_body(records, kind):
    body = encode({'format': kind, 'text': '\n'.join(encode(record).decode() for record in records)})
    require(len(body) <= MAX_PAYLOAD, 'fixture_payload_limit')
    return body


def read_bounded(response, maximum=MAX_RESPONSE):
    output = bytearray()
    while True:
        chunk = response.read1(min(65536, maximum + 1 - len(output)))
        if not chunk:
            require(response.length in (None, 0), 'response_body_incomplete')
            return bytes(output)
        output.extend(chunk)
        require(len(output) <= maximum, 'response_size_limit')


def parse_jsonl(payload):
    rows = [json.loads(line) for line in payload.splitlines() if line]
    require(len(rows) < 5000, 'history_record_bound')
    require(len({row['id'] for row in rows}) == len(rows), 'export_unique_ids')
    return rows


def csv_matches(payload, rows):
    reader = csv.DictReader(io.StringIO(payload.decode()))
    require(reader.fieldnames == CSV_FIELDS, 'csv_header')
    actual = list(reader)
    require(len(actual) == len(rows), 'csv_count')
    for got, row in zip(actual, rows):
        expected = {key: row.get(key) for key in CSV_FIELDS}
        expected['answer_count'] = len(row['answers'])
        expected['event_kind'] = row.get('event_kind', 'request')
        for key, value in expected.items():
            if value is None:
                require(got[key] == '', 'csv_null_field')
            elif isinstance(value, (float, int)):
                require(float(got[key]) == value, 'csv_numeric_field')
            else:
                require(got[key] == str(value), 'csv_text_field')


def numeric_resources(pid, directory):
    result = {'at': now(), 'pid': pid, 'harness_pid': os.getpid()}
    memory = Path('/proc/meminfo').read_text().splitlines()
    result['mem_available_kib'] = int(next(line.split()[1] for line in memory if line.startswith('MemAvailable:')))
    result['processes'] = {}
    for label, target in [('observer', pid), ('harness', os.getpid())]:
        status = Path(f'/proc/{target}/status').read_text().splitlines()
        values = {}
        for key in ('VmRSS', 'VmHWM', 'VmSize', 'Threads'):
            values[key] = int(next((line.split()[1] for line in status if line.startswith(key + ':')), '0'))
        stat = Path(f'/proc/{target}/stat').read_text().rsplit(')', 1)[1].split()
        values.update(utime_ticks=int(stat[11]), stime_ticks=int(stat[12]), fd_count=len(list(Path(f'/proc/{target}/fd').iterdir())))
        result['processes'][label] = values
    result['database_bytes'] = {suffix or 'database': (directory / ('audit.sqlite' + suffix)).stat().st_size if (directory / ('audit.sqlite' + suffix)).exists() else 0 for suffix in ('', '-wal', '-shm')}
    require(sum(result['database_bytes'].values()) <= MAX_WORKSPACE, 'workspace_disk_bound')
    require(result['mem_available_kib'] >= 1572864, 'memory_guard')
    return result


class Harness:
    def __init__(self, args):
        self.args = args
        self.lock = threading.RLock()
        self.http_slots = threading.BoundedSemaphore(6)
        self.stop = threading.Event()
        self.failure = None
        self.stats = defaultdict(lambda: {'attempted': 0, 'completed': 0, 'bytes': 0, 'elapsed_ms_sum': 0., 'elapsed_ms_max': 0.})
        self.active_http = self.peak_http = 0
        self.active_sockets = set()
        self.worker_threads = []
        self.provider_active = self.provider_peak = 0
        self.provider_condition = threading.Condition(self.lock)
        self.process = self.directory = self.log_thread = None
        self.token = self.auth = self.key = None
        self.port = None
        self.workspace_index = -1
        self.process_generation = 0
        self.live_offered = self.live_accepted = self.process_live_start = 0
        self.expected_imports = {}
        self.provider_seen = set()
        self.expected_label = None
        self.workspace_tokens = set()
        self.export_slot_retries = 0
        self.checkpoint_bytes = None
        self.report = {'started_at': now(), 'binary_sha256': args.sha256,
                       'harness_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                       'settings': {'cycles': args.cycles, 'seconds': args.seconds, 'live_rate': 5, 'live_workers': 2, 'live_outstanding_limit': 4, 'active_http_limit': 6, 'max_records': 10000, 'replace_workspace_every': 4, 'payload_limit': MAX_PAYLOAD, 'checkpoint_bytes_limit': MAX_RESPONSE, 'workspace_bytes_limit': MAX_WORKSPACE},
                       'scope': 'Synthetic loopback workspace only. Mixed-operation coverage; existing cohort1 soak remains retention authority.',
                       'cycles': [], 'process_exits': [], 'passed': False}
        args.output.mkdir(mode=0o700)
        self.resource_output = (args.output / 'resources.jsonl').open('x', encoding='utf-8')
        self.provider = self.make_provider()
        self.provider_thread = threading.Thread(target=self.provider.serve_forever, daemon=True)
        self.provider_thread.start()

    def publish(self, phase):
        atomic_json(self.args.output / 'state.json', {'at': now(), 'phase': phase, 'pid': self.process.pid if self.process else None,
                    'harness_pid': os.getpid(), 'workspace_index': self.workspace_index, 'generation': self.process_generation,
                    'completed_cycles': len(self.report['cycles']), 'live_offered': self.live_offered, 'live_accepted': self.live_accepted,
                    'binary_sha256': self.args.sha256})
        with self.lock:
            self.report['operation_stats'] = copy.deepcopy(dict(self.stats))
        self.report['peak_active_http'] = self.peak_http
        atomic_json(self.args.output / 'report.json', self.report)

    def fail(self, error):
        with self.lock:
            if self.failure is None:
                self.failure = {'type': type(error).__name__, 'check': str(error) if isinstance(error, CheckFailure) else 'unexpected_exception'}
            self.stop.set()
            sockets = tuple(self.active_sockets)
        for owned_socket in sockets:
            try:
                owned_socket.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass

    def raise_failure(self):
        if self.failure:
            raise CheckFailure(self.failure['check'])

    def make_provider(self):
        owner = self

        class Provider(BaseHTTPRequestHandler):
            def do_POST(self):
                with owner.provider_condition:
                    owner.provider_active += 1
                    owner.provider_peak = max(owner.provider_peak, owner.provider_active)
                try:
                    self.connection.settimeout(10)
                    size = int(self.headers.get('Content-Length', '0'))
                    require(0 < size <= MAX_PAYLOAD, 'provider_request_size')
                    body = self.rfile.read(size)
                    index = json.loads(body)['fixture_index']
                    require(isinstance(index, int) and 0 <= index < 5000, 'provider_fixture_index')
                    require(body == encode(request_fixture(index, owner.token)), 'provider_request_exact_bytes')
                    with owner.lock:
                        require(index not in owner.provider_seen, 'provider_duplicate_offer')
                        owner.provider_seen.add(index)
                    payload = encode(response_fixture(index, owner.token))
                    self.send_response(200)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                except BaseException as error:
                    owner.fail(error)
                finally:
                    with owner.provider_condition:
                        owner.provider_active -= 1
                        owner.provider_condition.notify_all()

            def log_message(self, *unused):
                pass

        provider = ThreadingHTTPServer(('127.0.0.1', 0), Provider)
        provider.daemon_threads = True
        return provider

    def start_process(self):
        require(self.process is None, 'process_already_running')
        env = {key: value for key, value in os.environ.items() if not key.startswith(('TYPESAFE_', 'OPENROUTER_', 'JEV_OBSERVER_')) and key.lower() not in ('http_proxy', 'https_proxy', 'all_proxy')}
        env.update(JEV_OBSERVER_DB_KEY=self.key, TMPDIR=str(self.directory), SQLITE_TMPDIR=str(self.directory), RUST_LOG='warn')
        self.port = None
        ready = threading.Event()
        self.process = subprocess.Popen([str(self.args.binary), '--port', '0', '--db', str(self.directory / 'audit.sqlite'),
                         '--upstream', f'http://127.0.0.1:{self.provider.server_port}/v1/systemone', '--max-records', '10000'],
                         stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env)

        def consume_log():
            # Discard process text, including unexpected diagnostics. The report
            # never contains token/key/body strings or arbitrary process output.
            while True:
                line = self.process.stdout.readline(8192)
                if not line:
                    return
                match = re.fullmatch(rb'Observer: http://127\.0\.0\.1:(\d+)\s*', line)
                if match:
                    self.port = int(match.group(1))
                    ready.set()

        self.log_thread = threading.Thread(target=consume_log, daemon=True)
        self.log_thread.start()
        require(ready.wait(10), 'observer_startup_timeout')
        require(self.process.poll() is None, 'observer_startup_exit')
        token_path = self.directory / 'audit.sqlite.token'
        require(token_path.stat().st_mode & 0o077 == 0, 'token_file_permissions')
        self.token = token_path.read_text().strip()
        self.workspace_tokens.add(self.token)
        self.auth = 'Basic ' + base64.b64encode(('observer:' + self.token).encode()).decode()
        self.process_generation += 1
        self.process_live_start = self.live_accepted
        self.publish('running')

    def stop_process(self):
        if self.process is None:
            return
        process = self.process
        if process.poll() is None:
            process.send_signal(signal.SIGTERM)
            try:
                process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        if self.log_thread:
            self.log_thread.join(timeout=2)
        if process.stdout:
            process.stdout.close()
        self.report['process_exits'].append(process.returncode)
        self.process = self.log_thread = None
        require(process.returncode == 0, 'observer_unclean_exit')

    def new_workspace(self):
        if self.directory is not None:
            self.stop_process()
            shutil.rmtree(self.directory)
        self.workspace_index += 1
        self.directory = Path(tempfile.mkdtemp(prefix='mixed-endurance-', dir=EVIDENCE / 'tmp'))
        self.directory.chmod(0o700)
        self.key = secrets.token_hex(32)
        private_write(self.directory / 'workspace.key', self.key.encode())
        self.live_offered = self.live_accepted = self.process_live_start = 0
        self.provider_seen.clear()
        self.expected_imports.clear()
        self.workspace_tokens.clear()
        self.expected_label = None
        self.seed_id = f'mixed-seed-w{self.workspace_index}'
        self.start_process()
        seed = observer_fixture(self.seed_id, int(time.time() * 1000), 'mixed-seed')
        self.import_records([seed], 'observer-jsonl', 1, 0)
        self.expected_imports[self.seed_id] = 'mixed-seed'

    def request(self, role, method, path, body=None, live=False, disconnect=False):
        require(not self.stop.is_set(), 'operations_stopped')
        require(body is None or len(body) <= MAX_PAYLOAD, 'http_payload_limit')
        self.http_slots.acquire()
        if self.stop.is_set():
            self.http_slots.release()
            raise CheckFailure('operations_stopped')
        with self.lock:
            self.active_http += 1
            self.peak_http = max(self.peak_http, self.active_http)
            require(self.active_http <= 6, 'active_http_limit')
            self.stats[role]['attempted'] += 1
        start = time.monotonic()
        connection = http.client.HTTPConnection('127.0.0.1', self.port, timeout=10)
        timer = None
        expired = threading.Event()
        owned_socket = None
        try:
            headers = {'Authorization': 'Bearer ' + PROVIDER_SECRET if live else self.auth,
                       'X-Observer-Access': self.token, 'X-Observer-Request': '1', 'Content-Type': 'application/json', 'Accept-Encoding': 'identity'}
            if live:
                index = json.loads(body)['fixture_index']
                headers['X-Observer-Source'] = 'mixed-live-a' if index % 2 == 0 else 'mixed-live-b'
            def end_deadline():
                # A per-read socket timeout alone permits a continuously slow
                # response to keep a worker alive indefinitely. Shutdown wakes
                # the owned response reader even after HTTPConnection detaches it.
                expired.set()
                if owned_socket is not None:
                    try:
                        owned_socket.shutdown(socket.SHUT_RDWR)
                    except OSError:
                        pass

            timer = threading.Timer(15, end_deadline)
            timer.daemon = True
            timer.start()
            connection.connect()
            owned_socket = connection.sock
            with self.lock:
                self.active_sockets.add(owned_socket)
            require(not self.stop.is_set(), 'operations_stopped')
            require(not expired.is_set(), 'http_operation_deadline')
            connection.request(method, path, body, headers)
            response = connection.getresponse()
            if disconnect:
                payload = response.read(64)
            else:
                payload = read_bounded(response)
            require(not expired.is_set(), 'http_operation_deadline')
            with self.lock:
                self.stats[role]['completed'] += 1
                self.stats[role]['bytes'] += len(payload)
            if self.checkpoint_bytes is not None:
                self.checkpoint_bytes += len(payload)
                require(self.checkpoint_bytes <= MAX_RESPONSE, 'checkpoint_bytes_limit')
            return response.status, payload
        finally:
            # All callers, including deliberate early disconnects, close their
            # connection before releasing the global operation slot.
            connection.close()
            if timer is not None:
                timer.cancel()
                timer.join(timeout=1)
            elapsed = (time.monotonic() - start) * 1000
            with self.lock:
                self.active_sockets.discard(owned_socket)
                self.active_http -= 1
                self.stats[role]['elapsed_ms_sum'] += elapsed
                self.stats[role]['elapsed_ms_max'] = max(self.stats[role]['elapsed_ms_max'], elapsed)
            self.http_slots.release()

    def api(self, role, path, method='GET', body=None):
        status, payload = self.request(role, method, path, body)
        require(status == 200, role + '_http_status')
        return json.loads(payload)

    def import_records(self, records, kind, imported, duplicates):
        result = self.api('mutation', '/api/import', 'POST', import_body(records, kind))
        require(result == {'imported': imported, 'duplicates': duplicates}, 'import_counts')

    def live(self, index):
        status, payload = self.request('live', 'POST', '/v1/systemone', encode(request_fixture(index, self.token)), live=True)
        require(status == 200 and payload == encode(response_fixture(index, self.token)), 'live_forwarding_exact')
        with self.lock:
            self.live_accepted += 1

    def mutation(self, serial):
        stamp = int(time.time() * 1000)
        prefix = f'w{self.workspace_index}-m{serial}'
        raw = raw_fixture(prefix + '-raw', stamp)
        observer = observer_fixture(prefix + '-observer', stamp)
        recovery = raw_fixture(prefix + '-recovery', stamp)
        bad = raw_fixture(prefix + '-bad', stamp, bad=True)
        for record, kind in [(raw, 'systemone-capture'), (observer, 'observer-jsonl')]:
            self.import_records([record], kind, 1, 0)
            self.import_records([record], kind, 0, 1)
            event_id = raw_import_identity(record) if kind == 'systemone-capture' else record['source_event_id']
            self.expected_imports[event_id] = record['source']
        status, payload = self.request('mutation', 'POST', '/api/import', import_body([recovery, bad], 'systemone-capture'))
        require(status == 400 and json.loads(payload) == {'error': IMPORT_ERROR}, 'atomic_bad_batch_static_error')
        # This must import one: a partially committed good sibling would return
        # a duplicate and immediately fail, even amid unrelated live writes.
        self.import_records([recovery], 'systemone-capture', 1, 0)
        self.import_records([recovery], 'systemone-capture', 0, 1)
        self.expected_imports[raw_import_identity(recovery)] = recovery['source']
        label = ['correct', 'incorrect', 'unknown'][serial % 3]
        result = self.api('mutation', '/api/requests/' + quote(self.seed_id, safe='') + '/label', 'POST', encode({'key': 'q', 'label': label}))
        require(result.get('ok') is True, 'label_update_result')
        self.expected_label = label

    def reader(self, serial):
        path = '/api/dashboard?' + urlencode({'window': 'all', **({'search': 'mixed-live'} if serial % 2 else {})})
        snapshot = self.api('reader', path)
        require(isinstance(snapshot['summary']['request_count'], int), 'reader_summary_shape')
        if snapshot['groups']:
            group = snapshot['groups'][serial % len(snapshot['groups'])]['id']
            detail = self.api('reader', '/api/groups/' + quote(group, safe='') + '?window=all')
            require(detail['group']['id'] == group, 'reader_group_identity')

    def export(self, serial):
        kind, disconnect = [('jsonl', False), ('csv', True), ('csv', False), ('jsonl', True)][serial % 4]
        path = '/api/export?' + urlencode({'window': 'all', 'source': 'mixed-live-a', 'format': kind})
        status, payload = self.request('export_disconnect' if disconnect else 'export_complete', 'GET', path, disconnect=disconnect)
        require(status == 200, 'background_export_status')
        if not disconnect:
            if kind == 'jsonl':
                rows = parse_jsonl(payload)
                require(all(row['source'] == 'mixed-live-a' for row in rows), 'background_export_filter')
            else:
                reader = csv.DictReader(io.StringIO(payload.decode()))
                require(reader.fieldnames == CSV_FIELDS and all(row['source'] == 'mixed-live-a' for row in reader), 'background_csv_filter')

    def health_sample(self, unused):
        health = self.api('health', '/api/health')
        for field in ['dropped', 'write_failures', 'maintenance_failures']:
            require(health[field] == 0, 'health_' + field)
        require(health['maintenance_healthy'], 'maintenance_health')
        sample = numeric_resources(self.process.pid, self.directory)
        sample.update(workspace_index=self.workspace_index, generation=self.process_generation,
                      health={field: health[field] for field in HEALTH_COUNTERS})
        self.resource_output.write(json.dumps(sample) + '\n')
        self.resource_output.flush()
        self.publish('running')

    def cycle(self, number):
        self.raise_failure()
        start = time.monotonic()
        deadline = start + self.args.seconds
        local_stop = threading.Event()
        before_offered, before_accepted = self.live_offered, self.live_accepted
        rounds = Counter()

        def periodic(name, interval, operation):
            due = start
            try:
                while not self.stop.is_set() and not local_stop.is_set():
                    delay = due - time.monotonic()
                    if delay > 0 and local_stop.wait(delay):
                        break
                    if self.stop.is_set() or time.monotonic() >= deadline:
                        break
                    operation(rounds[name] + (number % 4) * max(1, self.args.seconds // interval))
                    rounds[name] += 1
                    # No backlog or burst of missed periodic operations.
                    due = max(due + interval, time.monotonic())
            except BaseException as error:
                self.fail(error)

        threads = [threading.Thread(target=periodic, args=item, daemon=True) for item in [
            ('reader', 2, self.reader), ('export', 10, self.export), ('mutation', 30, self.mutation), ('health', 5, self.health_sample)]]
        self.worker_threads = threads
        pending = set()
        maximum_pending, skipped_slots = 0, 0
        try:
            for thread in threads:
                thread.start()
            with ThreadPoolExecutor(max_workers=2, thread_name_prefix='mixed-live') as workers:
                due = start
                while time.monotonic() < deadline and not self.stop.is_set():
                    delay = due - time.monotonic()
                    if delay > 0:
                        self.stop.wait(delay)
                    if self.stop.is_set() or time.monotonic() >= deadline:
                        break
                    for future in tuple(pending):
                        if future.done():
                            future.result()
                            pending.remove(future)
                    require(len(pending) < 4, 'live_outstanding_bound')
                    index = self.live_offered
                    self.live_offered += 1
                    pending.add(workers.submit(self.live, index))
                    maximum_pending = max(maximum_pending, len(pending))
                    due += .2
                    if due < time.monotonic():
                        skipped_slots += 1
                        due = time.monotonic() + .2
                for future in pending:
                    future.result()
        except BaseException as error:
            self.fail(error)
        finally:
            local_stop.set()
            for thread in threads:
                if thread.ident is not None:
                    thread.join(timeout=45)
                if thread.is_alive():
                    self.fail(CheckFailure('worker_did_not_quiesce'))
            self.worker_threads = [thread for thread in threads if thread.is_alive()]
        self.raise_failure()
        require(self.active_http == 0, 'owned_http_not_closed')
        require(self.live_offered == self.live_accepted, 'offered_vs_accepted_live')
        require(self.live_offered - before_offered >= self.args.seconds * 5 * .95, 'offer_rate_shortfall')
        require(self.provider_seen == set(range(self.live_offered)), 'provider_offer_ledger')
        return {'cycle': number + 1, 'workspace_index': self.workspace_index, 'target_offers': self.args.seconds * 5,
                'offered': self.live_offered - before_offered, 'accepted': self.live_accepted - before_accepted,
                'pacing_skipped_slots': skipped_slots, 'max_live_outstanding': maximum_pending,
                'operations': dict(rounds), 'active_seconds': round(time.monotonic() - start, 3)}

    def drain(self):
        deadline = time.monotonic() + 20
        while True:
            health = self.api('checkpoint', '/api/health')
            if health['queue_depth'] == health['active_captures'] == health['queued_bytes'] == 0:
                break
            require(time.monotonic() < deadline, 'collector_drain_timeout')
            time.sleep(.05)
        expected = self.live_accepted - self.process_live_start
        expected_incomplete = sum(is_incomplete(index) for index in range(self.process_live_start, self.live_accepted))
        for field in ['forwarded', 'captured', 'persisted']:
            require(health[field] == expected, 'process_' + field + '_exact')
        require(health['truncated'] == expected_incomplete, 'process_incomplete_count')
        for field in ['dropped', 'write_failures', 'maintenance_failures']:
            require(health[field] == 0, 'checkpoint_' + field)
        require(health['last_gap_at'] is not None if expected_incomplete else health['last_gap_at'] is None, 'expected_gap_signal')
        return {field: health[field] for field in HEALTH_COUNTERS}

    def checked_export(self, kind='jsonl', source=None):
        path = '/api/export?' + urlencode({'window': 'all', 'format': kind, **({'source': source} if source else {})})
        deadline = time.monotonic() + 5
        while True:
            status, payload = self.request('checkpoint', 'GET', path)
            if status != 429:
                require(status == 200, 'checkpoint_export_status')
                return payload
            require(time.monotonic() < deadline, 'export_slot_not_recovered')
            self.export_slot_retries += 1
            time.sleep(.05)

    def checkpoint(self):
        self.checkpoint_bytes = 0
        try:
            counters = self.drain()
            rows = parse_jsonl(self.checked_export())
            expected_count = self.live_accepted + len(self.expected_imports)
            require(len(rows) == expected_count, 'history_count_exact')
            incomplete = [row for row in rows if not row['capture_complete']]
            expected_incomplete = sum(is_incomplete(index) for index in range(self.live_accepted))
            require(len(incomplete) == expected_incomplete, 'history_incomplete_exact')
            for row in incomplete:
                require(row['normalization_error'] == LIVE_ERROR and row['answers'] == [], 'minimal_incomplete_record')
                require(fallback_generated_id(row['id']), 'fallback_generated_id_shape')
            normal_live = [row for row in rows if row['source'] in ('mixed-live-a', 'mixed-live-b')]
            live_indexes = [row['request_extra']['fixture_index'] for row in normal_live]
            expected_indexes = [index for index in range(self.live_accepted) if not is_incomplete(index)]
            require(sorted(live_indexes) == expected_indexes, 'normal_live_nonce_ledger')
            for row in normal_live:
                index = row['request_extra']['fixture_index']
                require(row['source'] == ('mixed-live-a' if index % 2 == 0 else 'mixed-live-b'), 'normal_live_source')
            imported = [row for row in rows if row['source'].startswith('mixed-import-') or row['source'] == 'mixed-seed']
            require({row['source_event_id']: row['source'] for row in imported} == self.expected_imports and len(imported) == len(self.expected_imports), 'import_identity_ledger')
            for row in imported:
                if row['source'] == 'mixed-import-raw':
                    require(row['id'] == row['source_event_id'] and row['import_format'] == 'systemone-capture', 'raw_import_generated_identity')
            forbidden = [PROVIDER_SECRET, STATE_SECRET, *COLLISION_SECRETS, *self.workspace_tokens, self.key, 'synthetic-input-not-retained']
            for row in rows:
                serialized = encode(row)
                require(all(value.encode() not in serialized for value in forbidden), 'privacy_echo_absence')
                require(row['state'] is None and not row['state_retained'], 'input_opt_out')
                if row['capture_complete']:
                    require(row.get('normalization_error') is None and len(row['answers']) == 1, 'complete_answer_count')
                    answer = row['answers'][0]
                    require(answer['valid'] is True and answer['key'] == 'q' and answer['value'] == .75, 'complete_answer_semantics')
                labels = row['labels']
                if row['id'] == self.seed_id and self.expected_label is not None:
                    require(len(labels) == 1 and labels[0]['key'] == 'q' and labels[0]['label'] == self.expected_label and labels[0]['source'] == 'local-reviewer', 'review_label_exact')
                else:
                    require(labels == [], 'unexpected_labels')
            csv_matches(self.checked_export('csv'), rows)
            filtered = parse_jsonl(self.checked_export(source='mixed-live-a'))
            require(filtered == [row for row in rows if row['source'] == 'mixed-live-a'], 'filtered_jsonl_exact_order')
            csv_matches(self.checked_export('csv', 'mixed-live-a'), filtered)
            snapshot = self.api('checkpoint', '/api/dashboard?window=all')
            good = len(rows) - len(incomplete)
            for key, value in {'request_count': len(rows), 'answer_count': good, 'incomplete_count': len(incomplete), 'error_count': 0, 'input_tokens': good * 5, 'output_tokens': good * 2, 'cost_known_requests': 0, 'sample_count': 0}.items():
                require(snapshot['summary'][key] == value, 'summary_' + key)
            groups = {}
            for row in rows:
                for answer in row['answers']:
                    group = groups.setdefault(answer['group_id'], {'source': row['source'], 'count': 0})
                    require(group['source'] == row['source'], 'group_source_identity')
                    group['count'] += 1
            require(snapshot['group_next_cursor'] is None and {group['id'] for group in snapshot['groups']} == set(groups), 'dashboard_groups_exact')
            require(len(groups) <= 5, 'stable_definition_group_bound')
            for group in snapshot['groups']:
                count = groups[group['id']]['count']
                for key in ('answer_count', 'valid_count', 'request_count'):
                    require(group[key] == count, 'group_' + key)
                require(group['mean_value'] == .75 and sum(bin['count'] for bin in group['distribution']) == count, 'group_distribution_exact')
                detail = self.api('checkpoint', '/api/groups/' + quote(group['id'], safe='') + '?window=all')['group']
                require(all(detail[key] == group[key] for key in ('answer_count', 'valid_count', 'request_count', 'mean_value', 'distribution')), 'detail_overview_equivalence')
                seed_label = self.expected_label if groups[group['id']]['source'] == 'mixed-seed' else None
                require(detail['review_counts'] == {**{label: int(seed_label == label) for label in ('correct', 'incorrect', 'unknown')}, 'unlabeled': count - int(seed_label is not None)}, 'group_review_counts')
            request_ids = [row['id'] for row in snapshot['requests']]
            cursor = snapshot['feed_next_cursor']
            cursors = set()
            while cursor:
                require(cursor not in cursors and len(cursors) < 50, 'request_cursor_progress')
                cursors.add(cursor)
                page = self.api('checkpoint', '/api/dashboard?' + urlencode({'window': 'all', 'request_cursor': cursor}))
                request_ids.extend(row['id'] for row in page['requests'])
                cursor = page['feed_next_cursor']
            require(len(request_ids) == len(rows) and set(request_ids) == {row['id'] for row in rows}, 'request_pagination_identity')
            with (self.directory / 'audit.sqlite').open('rb') as database:
                require(not database.read(16).startswith(b'SQLite format 3'), 'encrypted_database_header')
            result = {'records': len(rows), 'normal_live': len(normal_live), 'incomplete': len(incomplete), 'imports': len(imported), 'groups': len(groups),
                      'canonical_export_sha256': digest(rows), 'unique_ids_sha256': digest(sorted(row['id'] for row in rows)), 'counters': counters, 'checkpoint_bytes': self.checkpoint_bytes, 'export_slot_retries_total': self.export_slot_retries}
            return result
        finally:
            self.checkpoint_bytes = None

    def run(self):
        try:
            for cycle in range(self.args.cycles):
                if cycle % 4 == 0:
                    self.new_workspace()
                outcome = self.cycle(cycle)
                self.publish('checkpoint')
                outcome['checkpoint'] = self.checkpoint()
                if cycle + 1 == max(1, self.args.cycles // 2):
                    before = outcome['checkpoint']['canonical_export_sha256']
                    self.stop_process()
                    self.start_process()
                    after = self.checkpoint()
                    require(after['canonical_export_sha256'] == before, 'restart_canonical_export_changed')
                    outcome['restart'] = {'canonical_equal': True, 'generation': self.process_generation, 'counters': after['counters']}
                self.report['cycles'].append(outcome)
                self.publish('running')
            self.report['passed'] = True
        except BaseException as error:
            self.fail(error)
            self.report['failure'] = self.failure
        finally:
            self.stop.set()

            def cleanup(operation, name):
                try:
                    operation()
                except BaseException as error:
                    self.report.setdefault('cleanup_failures', []).append({'type': type(error).__name__, 'check': name})
                    self.report['passed'] = False

            cleanup(self.stop_process, 'observer_stop')
            cleanup(self.provider.shutdown, 'provider_shutdown')
            cleanup(self.provider.server_close, 'provider_close')
            cleanup(lambda: self.provider_thread.join(timeout=5), 'provider_join')
            cleanup(lambda: require(not self.provider_thread.is_alive(), 'provider_thread_alive'), 'provider_thread_alive')
            with self.provider_condition:
                drained = self.provider_condition.wait_for(lambda: self.provider_active == 0, timeout=12)
            cleanup(lambda: require(drained, 'provider_requests_alive'), 'provider_requests_alive')
            for thread in self.worker_threads:
                cleanup(lambda: thread.join(timeout=20), 'worker_join')
                cleanup(lambda: require(not thread.is_alive(), 'worker_alive'), 'worker_alive')
            cleanup(lambda: require(self.active_http == 0 and not self.active_sockets, 'http_operations_alive'), 'http_operations_alive')
            if self.failure:
                self.report['failure'] = self.failure
                self.report['passed'] = False
            cleanup(self.resource_output.close, 'resource_output_close')
            if self.directory is not None:
                if self.report['passed']:
                    cleanup(lambda: shutil.rmtree(self.directory), 'workspace_remove')
                    self.report['owned_workspace_removed'] = not self.directory.exists()
                if self.directory.exists():
                    self.report['retained_private_workspace'] = str(self.directory)
            self.report['peak_provider_handlers'] = self.provider_peak
            self.report['finished_at'] = now()
            cleanup(lambda: self.publish('stopped'), 'final_report_publish')
            if self.report.get('cleanup_failures'):
                # Preserve the original invariant as well as cleanup problems,
                # even if the report filesystem itself became unavailable.
                print(json.dumps({'failure': self.report.get('failure'), 'cleanup_failures': self.report['cleanup_failures']}))
        return self.report['passed']


def self_check():
    require(fallback_generated_id('redacted_v1_0123456789ab4def8123456789abcdef'), 'self_fallback_generated_id')
    for invalid in ('01234567-89ab-4def-8123-456789abcdef', 'redacted_v1_' + '0' * 32, 'redacted_v1_0123456789ab4def8123456789abcdef-extra', None):
        require(not fallback_generated_id(invalid), 'self_fallback_id_negative_control')
    token = 'synthetic-workspace-token-' + 'a' * 64
    maximum = 0
    for index in (0, 1, 99, 100, 4799):
        for payload in (encode(request_fixture(index, token)), encode(response_fixture(index, token))):
            maximum = max(maximum, len(payload))
            require(len(payload) <= MAX_PAYLOAD, 'self_fixture_payload')
    stamp = 1_790_000_000_000
    prefix = 'w22-m31'
    raw = raw_fixture(prefix + '-raw', stamp)
    raw_id = raw_import_identity(raw)
    require(raw_id == 'capture_v1_c489e644f7b153aaab5f671eab096bfaa5527f8fc45e3cd0f8160c06b731c0dd', 'self_raw_identity_known_vector')
    require(raw_id != raw['id'], 'self_raw_identity_not_caller_id')
    require(raw_import_identity({**raw, 'provider': 'Typesafe'}) == raw_id, 'self_raw_identity_ascii_provider_case')
    variants = [{**raw, field: raw[field] + '-other'} for field in ('source', 'provider', 'id')]
    require(len({raw_id, *(raw_import_identity(value) for value in variants)}) == 4, 'self_raw_identity_tuple_distinct')
    for records, kind in [([raw_fixture(prefix + '-raw', stamp)], 'systemone-capture'),
                          ([observer_fixture(prefix + '-observer', stamp)], 'observer-jsonl'),
                          ([raw_fixture(prefix + '-recovery', stamp), raw_fixture(prefix + '-bad', stamp, bad=True)], 'systemone-capture')]:
        maximum = max(maximum, len(import_body(records, kind)))
    require(1200 * 4 + 8 * 3 * 4 + 1 == 4897, 'self_workspace_bound')
    require(sum(is_incomplete(index) for index in range(1200)) == 12, 'self_incomplete_count')
    print(json.dumps({'self_check': 'passed', 'maximum_fixture_payload_bytes': maximum, 'nominal_records_per_workspace': 4897, 'nominal_incomplete_per_cycle': 12, 'network_started': False}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--self-check', action='store_true')
    parser.add_argument('--run', action='store_true')
    parser.add_argument('--binary', type=Path)
    parser.add_argument('--sha256')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--cycles', type=int, default=90)
    parser.add_argument('--seconds', type=int, default=240)
    args = parser.parse_args()
    if args.self_check:
        require(not args.run, 'self_check_cannot_run_network')
        self_check()
        return
    if not args.run or not args.binary or not args.sha256 or not args.output:
        parser.error('Execution requires --run --binary --sha256 --output; preparation uses --self-check')
    require(1 <= args.cycles <= 90 and 30 <= args.seconds <= 240 and args.seconds % 30 == 0, 'configured_duration_bounds')
    require(not args.output.exists(), 'preserve_existing_evidence')
    args.binary = args.binary.resolve(strict=True)
    require(hashlib.sha256(args.binary.read_bytes()).hexdigest() == args.sha256, 'pinned_binary_sha256')
    available = int(next(line.split()[1] for line in Path('/proc/meminfo').read_text().splitlines() if line.startswith('MemAvailable:')))
    require(available >= 1572864, 'startup_memory_guard')
    def interrupted(unused_signal, unused_frame):
        raise KeyboardInterrupt()
    signal.signal(signal.SIGTERM, interrupted)
    require(Harness(args).run(), 'endurance_failed_see_report')


if __name__ == '__main__':
    main()
