#!/usr/bin/env python3
"""Check sustained collection, paging, idle retention and encrypted restarts.

Uses only synthetic requests to an in-process loopback provider and a temporary
workspace. No existing history, external provider or system credentials are used.
"""
import argparse
import concurrent.futures
import contextlib
import copy
from datetime import datetime, timezone
import hashlib
import http.client
from http.server import ThreadingHTTPServer
import importlib.util
import json
from pathlib import Path
import platform
import statistics
import tempfile
import threading
import time
from urllib.parse import urlsplit, urlencode

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('upgrade', ROOT / 'scripts/test-upgrade.py')
upgrade = importlib.util.module_from_spec(spec)
spec.loader.exec_module(upgrade)


def now():
    return datetime.now(timezone.utc).isoformat()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--cycles', type=int, default=6)
    parser.add_argument('--seconds', type=int, default=300)
    parser.add_argument('--rate', type=int, default=50)
    parser.add_argument('--max-records', type=int, default=1000)
    parser.add_argument('--restart-every', type=int, default=3)
    parser.add_argument('--active-state', type=Path,
                        help='Optional JSON status file for a separate read-only monitor; contains no token')
    args = parser.parse_args()
    if not 1 <= args.cycles <= 100 or not 1 <= args.seconds <= 3600:
        parser.error('Use 1–100 cycles and 1–3600 load seconds per cycle')
    if not 1 <= args.rate <= 200 or not 1 <= args.max_records <= 20000:
        parser.error('Use 1–200 requests/sec and a 1–20000 retained-record cap')
    if args.restart_every < 1:
        parser.error('Restart interval must be positive')
    if not __debug__:
        parser.error('Run without Python -O; this check requires assertions')
    args.binary = args.binary.resolve(strict=True)
    destinations = [args.output, args.output.with_name(args.output.name + '.tmp')]
    if args.active_state is not None:
        destinations.extend([args.active_state, args.active_state.with_name(args.active_state.name + '.tmp')])
    destinations = [path.resolve() for path in destinations]
    if args.binary in destinations or len(set(destinations)) != len(destinations):
        parser.error('Binary, report, optional status and their .tmp staging paths must be distinct')
    app = upgrade.Upgrade(args)
    report = {'started_at': now(), 'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest(),
              'harness_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'scope': __doc__.strip(), 'platform': platform.platform(),
              'helper_sha256': hashlib.sha256((ROOT / 'scripts/test-upgrade.py').read_bytes()).hexdigest(),
              'settings': {key: value for key, value in vars(args).items() if key not in ('binary', 'output', 'active_state')},
              'cycles': [], 'passed': False, 'notes': ['Synthetic loopback only; no provider or credential-store access.',
              'Shared host. Latencies describe this run and do not establish capacity.',
              'Each cycle waits for idle retention and verifies retained usage, answers, and export identities.',
              'At most 32 HTTP tasks are outstanding; overload fails the check rather than building an unbounded queue.',
              'RSS is sampled after each retention check on Linux, not a peak or a cross-platform metric.']}
    def save():
        args.output.parent.mkdir(parents=True, exist_ok=True)
        temporary = args.output.with_name(args.output.name + '.tmp')
        temporary.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
        temporary.replace(args.output)
    generation = 0
    def publish(phase):
        if args.active_state is not None:
            value = {'phase': phase, 'at': now(), 'generation': generation,
                     'binary_sha256': report['binary_sha256']}
            if phase == 'running':
                value.update(origin=app.origin, pid=app.process.pid,
                             token_path=str(app.database.with_suffix('.access-token')))
            args.active_state.parent.mkdir(parents=True, exist_ok=True)
            temporary = args.active_state.with_name(args.active_state.name + '.tmp')
            temporary.write_text(json.dumps(value) + '\n', encoding='utf-8')
            temporary.replace(args.active_state)
    def start_observer():
        nonlocal generation
        publish('restarting')
        app.start(args.binary, ['--max-records', str(args.max_records)])
        generation += 1
        publish('running')
    def stop_observer():
        try:
            publish('stopping')
        finally:
            try:
                app.stop()
            finally:
                # Popen can fail after the helper opens its log but before it
                # owns a process. Close that handle before removing the folder.
                if app.process is None and app.log is not None:
                    app.log.close()
                    app.log = None
    def cleanup_observer(error_type, error, traceback):
        try:
            stop_observer()
        except Exception as cleanup_error:
            report['cleanup_failure'] = {'type': type(cleanup_error).__name__}
            if isinstance(cleanup_error, AssertionError):
                report['cleanup_failure']['check'] = str(cleanup_error)
            # Keep the original failed invariant when shutdown also fails.
            if error is None:
                raise
        return False
    mock = ThreadingHTTPServer(('127.0.0.1', 0), upgrade.Mock)
    thread = threading.Thread(target=mock.serve_forever, daemon=True)
    thread.start()
    app.upstream = f'http://127.0.0.1:{mock.server_port}/v1/systemone'
    bodies = []
    for variant in range(5):
        request = copy.deepcopy(upgrade.REQUEST)
        for question in request['questions'].values():
            question['instructions'] += f' Synthetic variant {variant}.'
        request['request_extension']['authorization'] = 'Bearer ' + upgrade.KEY
        request['request_extension']['credential_echo'] = upgrade.KEY
        bodies.append(json.dumps(request).encode())
    total = 0
    try:
        with tempfile.TemporaryDirectory(prefix='observer-retention-soak-') as temporary, contextlib.ExitStack() as cleanup:
            # Stop the child before removing its open database/log files, also
            # on a failed assertion or Ctrl-C (required on Windows).
            cleanup.push(cleanup_observer)
            app.directory = Path(temporary)
            app.database = app.directory / 'soak.sqlite'
            app.phase = 'startup'
            start_observer()
            save()
            for cycle in range(args.cycles):
                app.phase = f'cycle_{cycle + 1}'
                before = app.api('/api/health')
                port = urlsplit(app.origin).port
                headers = {'Authorization': 'Bearer ' + upgrade.KEY,
                           'X-Observer-Access': app.token, 'X-Observer-Source': 'session-soak',
                           'Content-Type': 'application/json', 'Accept-Encoding': 'identity'}
                def send(index):
                    started = time.monotonic()
                    connection = http.client.HTTPConnection('127.0.0.1', port, timeout=10)
                    try:
                        selected_headers = {**headers, 'X-Observer-Source': f'session-soak-{index % 4}',
                                            'X-Observer-Task-Version': f'variant-{index % 3}'}
                        connection.request('POST', '/v1/systemone', bodies[index % 5], selected_headers)
                        response = connection.getresponse()
                        payload = response.read()
                        assert response.status == 200 and payload == upgrade.RESPONSE, 'forwarded_body_mismatch'
                        return (time.monotonic() - started) * 1000
                    finally:
                        connection.close()
                submitted = args.seconds * args.rate
                started = time.monotonic()
                pending = set()
                latencies = []
                with concurrent.futures.ThreadPoolExecutor(max_workers=16) as workers:
                    for index in range(submitted):
                        wait = started + index / args.rate - time.monotonic()
                        if wait > 0:
                            time.sleep(wait)
                        completed = {future for future in pending if future.done()}
                        for future in completed:
                            latencies.append(future.result())
                        pending -= completed
                        assert len(pending) < 32, 'load_generator_backlog_exceeded'
                        pending.add(workers.submit(send, index))
                        total += 1
                    for future in concurrent.futures.as_completed(pending):
                        latencies.append(future.result())
                load_seconds = time.monotonic() - started
                drain_until = time.monotonic() + 15
                while True:
                    health = app.api('/api/health')
                    if health['queue_depth'] == 0 and health['active_captures'] == 0:
                        break
                    assert time.monotonic() < drain_until, 'pipeline_did_not_drain'
                    time.sleep(.05)
                for key in ('forwarded', 'captured', 'persisted'):
                    assert health[key] - before[key] == submitted, key + '_accounting'
                for key in ('dropped', 'truncated', 'write_failures', 'maintenance_failures'):
                    assert health[key] == 0, key
                # The writer checks idle retention after 30 seconds of inactivity.
                time.sleep(35)
                health = app.api('/api/health')
                assert health['maintenance_failures'] == 0 and health['maintenance_healthy'], 'idle_maintenance_health'
                snapshot = app.api('/api/dashboard?window=all')
                expected = min(total, args.max_records)
                summary = snapshot['summary']
                assert summary['request_count'] == expected, 'idle_retention_count'
                assert summary['answer_count'] == expected * 3, 'retained_answers'
                assert summary['input_tokens'] == expected * 100, 'retained_input_usage'
                assert summary['output_tokens'] == expected * 12, 'retained_output_usage'
                exported = [json.loads(line) for line in app.request('/api/export?window=all').splitlines() if line.strip()]
                assert len(exported) == expected, 'export_count'
                identifiers = {record['id'] for record in exported}
                assert len(identifiers) == expected, 'export_unique_ids'
                assert all(record['state'] is None and not record['state_retained'] for record in exported), 'input_capture_opt_out'
                assert all(upgrade.KEY not in json.dumps(record) for record in exported), 'credential_echo_redaction'
                expected_groups = {answer['group_id'] for record in exported for answer in record['answers']}
                groups = {group['id']: group for group in snapshot['groups']}
                assert len(groups) == len(snapshot['groups']), 'duplicate_groups_within_first_page'
                cursor = snapshot['group_next_cursor']
                seen_cursors = set()
                while cursor:
                    assert cursor not in seen_cursors and len(seen_cursors) <= len(expected_groups) // 100, 'group_cursor_did_not_finish'
                    seen_cursors.add(cursor)
                    page = app.api('/api/dashboard?' + urlencode({'window': 'all', 'group_cursor': cursor}))
                    new = {group['id']: group for group in page['groups']}
                    assert new and len(new) == len(page['groups']), 'empty_or_duplicate_groups_within_page'
                    assert not (groups.keys() & new.keys()), 'duplicate_group_page'
                    groups.update(new)
                    cursor = page['group_next_cursor']
                assert groups.keys() == expected_groups, 'group_pages_match_retained_exports'
                assert sum(group['answer_count'] for group in groups.values()) == expected * 3, 'group_answer_counts'
                request_ids = {record['id'] for record in snapshot['requests']}
                assert len(request_ids) == len(snapshot['requests']), 'duplicate_requests_within_first_page'
                cursor = snapshot['feed_next_cursor']
                seen_cursors = set()
                while cursor:
                    assert cursor not in seen_cursors and len(seen_cursors) <= expected // 100, 'request_cursor_did_not_finish'
                    seen_cursors.add(cursor)
                    page = app.api('/api/dashboard?' + urlencode({'window': 'all', 'request_cursor': cursor}))
                    new_ids = {record['id'] for record in page['requests']}
                    assert new_ids and len(new_ids) == len(page['requests']), 'empty_or_duplicate_requests_within_page'
                    assert not (request_ids & new_ids), 'duplicate_request_page'
                    request_ids.update(new_ids)
                    cursor = page['feed_next_cursor']
                assert request_ids == identifiers, 'request_pages_match_retained_exports'
                sources = {record['source'] for record in exported}
                assert set(snapshot['sources']) == sources, 'source_options_match_retained_exports'
                for source in sources:
                    scoped = app.api('/api/dashboard?' + urlencode({'window': 'all', 'source': source}))
                    source_count = sum(record['source'] == source for record in exported)
                    assert scoped['summary']['request_count'] == source_count, 'source_request_count'
                    assert scoped['summary']['answer_count'] == source_count * 3, 'source_answer_count'
                    assert scoped['summary']['input_tokens'] == source_count * 100, 'source_token_count'
                with app.database.open('rb') as database:
                    assert database.read(16) != b'SQLite format 3\0', 'encrypted_storage'
                rss = None
                status = Path(f'/proc/{app.process.pid}/status')
                if status.exists():
                    for line in status.read_text().splitlines():
                        if line.startswith('VmRSS:'):
                            rss = int(line.split()[1])
                latencies.sort()
                item = {'cycle': cycle + 1, 'at': now(), 'offered': submitted, 'elapsed_load_seconds': load_seconds,
                        'latency_ms': {'median': statistics.median(latencies), 'p95': latencies[max(0, int(len(latencies) * .95) - 1)], 'maximum': latencies[-1]},
                        'retained': expected, 'groups': len(groups), 'sources': len(sources),
                        'database_bytes': app.database.stat().st_size, 'rss_kib': rss,
                        'health': health, 'restarted': False}
                if (cycle + 1) % args.restart_every == 0:
                    start_observer()
                    restarted = app.api('/api/dashboard?window=all')
                    assert restarted['summary'] == summary, 'restart_summary_changed'
                    after_export = [json.loads(line) for line in app.request('/api/export?window=all').splitlines() if line.strip()]
                    assert len(after_export) == len(exported), 'restart_record_count_changed'
                    assert {record['id']: record for record in after_export} == {record['id']: record for record in exported}, 'restart_records_changed'
                    assert app.api('/api/health')['forwarded'] == 0, 'restart_process_counters'
                    item['restarted'] = True
                report['cycles'].append(item)
                save()
                print(json.dumps({key: item[key] for key in ('cycle', 'at', 'offered', 'retained', 'database_bytes', 'rss_kib', 'restarted')}), flush=True)
            publish('stopping')
            app.stop()
        report['passed'] = True
    except (Exception, KeyboardInterrupt) as error:
        report['failure'] = {'phase': app.phase, 'type': type(error).__name__}
        if isinstance(error, AssertionError):
            report['failure']['check'] = str(error)
    finally:
        cleanup_error = None
        try:
            app.stop()
        except Exception as error:
            cleanup_error = error
            report['passed'] = False
            report['cleanup_failure'] = {'type': type(error).__name__}
        finally:
            try:
                mock.shutdown()
                mock.server_close()
                thread.join(timeout=5)
            finally:
                report['finished_at'] = now()
                report['total_offered'] = total
                save()
                publish('stopped')
    print(json.dumps({'passed': report['passed'], 'cycles': len(report['cycles']),
                      'total_offered': total, 'failure': report.get('failure'),
                      'cleanup_failure': report.get('cleanup_failure')}))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
