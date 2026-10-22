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
import os
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


def resource_snapshot(pid=None, harness_pid=None):
    """Best-effort numeric Linux samples; never read commands or environments."""
    sample = {'at': now(), 'host': {}, 'processes': {}}
    for filename, names in (
        ('meminfo', {'MemTotal', 'MemAvailable', 'SwapTotal', 'SwapFree'}),
        ('vmstat', {'pswpin', 'pswpout', 'pgmajfault'}),
    ):
        with contextlib.suppress(OSError, ValueError):
            for line in Path('/proc', filename).read_text().splitlines():
                name, value, *_ = line.replace(':', '').split()
                if name in names:
                    sample['host'][name] = int(value)
    with contextlib.suppress(OSError, ValueError):
        sample['host']['load_average'] = [float(value) for value in Path('/proc/loadavg').read_text().split()[:3]]
    for name in ('cpu', 'memory', 'io'):
        with contextlib.suppress(OSError, ValueError):
            pressure = {}
            for line in Path('/proc/pressure', name).read_text().splitlines():
                kind, *values = line.split()
                if kind in ('some', 'full'):
                    pressure[kind] = {key: float(value) for key, value in (item.split('=') for item in values)
                                      if key in ('avg10', 'avg60', 'avg300', 'total')}
            sample['host'][name + '_pressure'] = pressure
    for name, process_id in (('harness', os.getpid() if harness_pid is None else harness_pid), ('observer', pid)):
        if process_id is not None:
            values = {'pid': process_id}
            with contextlib.suppress(OSError, ValueError):
                for line in Path(f'/proc/{process_id}/status').read_text().splitlines():
                    key, *fields = line.replace(':', '').split()
                    if key in ('VmRSS', 'VmHWM', 'VmSwap', 'Threads'):
                        values[key] = int(fields[0])
            sample['processes'][name] = values
    return sample


def error_locations(error):
    """Only code locations, never exception messages, source lines or locals."""
    locations = []
    trace = error.__traceback__
    while trace is not None:
        code = trace.tb_frame.f_code
        locations.append({'file': Path(code.co_filename).name, 'function': code.co_name, 'line': trace.tb_lineno})
        trace = trace.tb_next
    return {'type': type(error).__name__, 'locations': locations}


def health_snapshot(health):
    # A future server response must not accidentally turn diagnostics into a
    # copy of arbitrary response text. Existing counters and flags are enough.
    keys = ('sample_sequence', 'forwarded', 'captured', 'persisted', 'dropped', 'truncated',
            'write_failures', 'maintenance_failures', 'maintenance_healthy', 'last_maintenance_at',
            'queue_depth', 'queued_bytes', 'last_persisted_at', 'last_gap_at', 'lag_ms',
            'last_batch_lag_ms', 'active_captures', 'capture_limit', 'capture_slots', 'stopping')
    return {'at': now(), 'values': {key: health[key] for key in keys
                                  if key in health and (health[key] is None or isinstance(health[key], (int, float)))}}


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
              'RSS is sampled after each retention check on Linux, not a peak or a cross-platform metric.',
              'Progress and numeric Linux pressure samples update every five seconds during load dispatch; stalls can delay sampling and unavailable metrics are omitted.',
              'Failure traces contain code locations only. Last observed health is timestamped and is not a failure-time health request.']}
    def save():
        args.output.parent.mkdir(parents=True, exist_ok=True)
        temporary = args.output.with_name(args.output.name + '.tmp')
        temporary.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
        temporary.replace(args.output)
    generation = 0
    progress = {'subphase': 'setup', 'cycle': 0, 'submitted': 0, 'completed': 0, 'pending': 0}
    last_progress = 0
    original_error = None
    def sample_resources():
        return resource_snapshot(None if app.process is None else app.process.pid)
    def publish(phase):
        if args.active_state is not None:
            value = {'phase': phase, 'at': now(), 'generation': generation,
                     'harness_pid': os.getpid(),
                     'binary_sha256': report['binary_sha256'], 'progress': report.get('progress'),
                     'last_health': report.get('last_health'), 'resources': report.get('resources')}
            if 'failure' in report:
                value['failure'] = report['failure']
            if phase == 'running':
                value.update(origin=app.origin, pid=app.process.pid,
                             token_path=str(app.token_path))
            args.active_state.parent.mkdir(parents=True, exist_ok=True)
            temporary = args.active_state.with_name(args.active_state.name + '.tmp')
            temporary.write_text(json.dumps(value) + '\n', encoding='utf-8')
            temporary.replace(args.active_state)
    def update_progress(subphase, force=True, **counts):
        nonlocal last_progress
        progress.update(subphase=subphase, **counts)
        report['progress'] = {'phase': app.phase, 'at': now(), **progress}
        if force or time.monotonic() - last_progress >= 5:
            report['resources'] = sample_resources()
            if app.process is not None:
                publish('running')
            last_progress = time.monotonic()
    def observe_health():
        health = app.api('/api/health')
        report['last_health'] = health_snapshot(health)
        return health
    def failure_details(error):
        failure = {'phase': app.phase, 'at': now(), **error_locations(error),
                   'progress': copy.deepcopy(report.get('progress')),
                   'last_health': copy.deepcopy(report.get('last_health')),
                   'resources': sample_resources()}
        if hasattr(error, 'soak_request'):
            failure['request'] = error.soak_request
        # These are controlled invariant names from this harness/helper, never
        # HTTP or other exception messages. Other assertion sources get locations.
        trace = error.__traceback__
        while trace is not None and trace.tb_next is not None:
            trace = trace.tb_next
        if isinstance(error, AssertionError) and trace is not None and trace.tb_frame.f_code.co_filename in (__file__, upgrade.__file__):
            failure['check'] = str(error)
        return failure
    def capture_failure(error):
        nonlocal original_error
        if 'failure' not in report:
            original_error = error
            report['failure'] = failure_details(error)
    def capture_cleanup_failure(error, stage):
        report['passed'] = False
        report.setdefault('cleanup_failure', {'stage': stage, **error_locations(error)})
    def start_observer():
        nonlocal generation
        update_progress('observer_start')
        publish('restarting')
        app.start(args.binary, ['--max-records', str(args.max_records)])
        generation += 1
        update_progress('observer_ready')
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
        if error is not None:
            # Record the failure while the process still exists, before shutdown
            # or TemporaryDirectory cleanup can replace its exception/context.
            capture_failure(error)
            try:
                save()
            except Exception as save_error:
                capture_cleanup_failure(save_error, 'save_failure')
        try:
            stop_observer()
        except Exception as cleanup_error:
            capture_cleanup_failure(cleanup_error, 'observer_stop')
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
                progress.clear()
                update_progress('baseline_health', cycle=cycle + 1, submitted=0, completed=0, pending=0,
                                target=args.seconds * args.rate)
                before = observe_health()
                port = urlsplit(app.origin).port
                headers = {'Authorization': 'Bearer ' + upgrade.KEY,
                           'X-Observer-Access': app.token, 'X-Observer-Source': 'session-soak',
                           'Content-Type': 'application/json', 'Accept-Encoding': 'identity'}
                def send(index):
                    started = time.monotonic()
                    connection = http.client.HTTPConnection('127.0.0.1', port, timeout=10)
                    stage = 'connect'
                    try:
                        selected_headers = {**headers, 'X-Observer-Source': f'session-soak-{index % 4}',
                                            'X-Observer-Task-Version': f'variant-{index % 3}'}
                        connection.connect()
                        stage = 'request_write'
                        connection.request('POST', '/v1/systemone', bodies[index % 5], selected_headers)
                        stage = 'response_headers'
                        response = connection.getresponse()
                        stage = 'response_body'
                        payload = response.read()
                        stage = 'response_validation'
                        assert response.status == 200 and payload == upgrade.RESPONSE, 'forwarded_body_mismatch'
                        return (time.monotonic() - started) * 1000
                    except Exception as error:
                        error.soak_request = {'index': index, 'stage': stage, 'at': now(),
                                              'elapsed_seconds': time.monotonic() - started,
                                              'resources': sample_resources()}
                        raise
                    finally:
                        with contextlib.suppress(OSError):
                            connection.close()
                submitted = args.seconds * args.rate
                started = time.monotonic()
                pending = set()
                latencies = []
                update_progress('load_dispatch')
                with concurrent.futures.ThreadPoolExecutor(max_workers=16) as workers:
                    for index in range(submitted):
                        wait = started + index / args.rate - time.monotonic()
                        if wait > 0:
                            time.sleep(wait)
                        completed = {future for future in pending if future.done()}
                        update_progress('load_dispatch', force=False, submitted=index,
                                        completed=len(latencies), pending=len(pending), total_offered=total)
                        for future in completed:
                            latencies.append(future.result())
                        pending -= completed
                        assert len(pending) < 32, 'load_generator_backlog_exceeded'
                        pending.add(workers.submit(send, index))
                        total += 1
                    update_progress('load_completion', submitted=submitted, completed=len(latencies),
                                    pending=len(pending), total_offered=total)
                    for future in concurrent.futures.as_completed(pending):
                        latencies.append(future.result())
                        update_progress('load_completion', force=False, completed=len(latencies),
                                        pending=submitted - len(latencies))
                load_seconds = time.monotonic() - started
                drain_until = time.monotonic() + 15
                update_progress('pipeline_drain', completed=len(latencies), pending=0)
                while True:
                    health = observe_health()
                    if health['queue_depth'] == 0 and health['active_captures'] == 0:
                        break
                    assert time.monotonic() < drain_until, 'pipeline_did_not_drain'
                    time.sleep(.05)
                for key in ('forwarded', 'captured', 'persisted'):
                    assert health[key] - before[key] == submitted, key + '_accounting'
                for key in ('dropped', 'truncated', 'write_failures', 'maintenance_failures'):
                    assert health[key] == 0, key
                # The writer checks idle retention after 30 seconds of inactivity.
                update_progress('idle_retention_wait')
                time.sleep(35)
                update_progress('idle_retention_health')
                health = observe_health()
                assert health['maintenance_failures'] == 0 and health['maintenance_healthy'], 'idle_maintenance_health'
                update_progress('dashboard')
                snapshot = app.api('/api/dashboard?window=all')
                expected = min(total, args.max_records)
                summary = snapshot['summary']
                assert summary['request_count'] == expected, 'idle_retention_count'
                assert summary['answer_count'] == expected * 3, 'retained_answers'
                assert summary['input_tokens'] == expected * 100, 'retained_input_usage'
                assert summary['output_tokens'] == expected * 12, 'retained_output_usage'
                update_progress('export')
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
                update_progress('group_paging', group_pages=1)
                while cursor:
                    assert cursor not in seen_cursors and len(seen_cursors) <= len(expected_groups) // 100, 'group_cursor_did_not_finish'
                    seen_cursors.add(cursor)
                    update_progress('group_paging', group_pages=len(seen_cursors) + 1)
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
                update_progress('request_paging', request_pages=1)
                while cursor:
                    assert cursor not in seen_cursors and len(seen_cursors) <= expected // 100, 'request_cursor_did_not_finish'
                    seen_cursors.add(cursor)
                    update_progress('request_paging', request_pages=len(seen_cursors) + 1)
                    page = app.api('/api/dashboard?' + urlencode({'window': 'all', 'request_cursor': cursor}))
                    new_ids = {record['id'] for record in page['requests']}
                    assert new_ids and len(new_ids) == len(page['requests']), 'empty_or_duplicate_requests_within_page'
                    assert not (request_ids & new_ids), 'duplicate_request_page'
                    request_ids.update(new_ids)
                    cursor = page['feed_next_cursor']
                assert request_ids == identifiers, 'request_pages_match_retained_exports'
                sources = {record['source'] for record in exported}
                assert set(snapshot['sources']) == sources, 'source_options_match_retained_exports'
                for source_index, source in enumerate(sorted(sources)):
                    update_progress('source_dashboard', source_index=source_index)
                    scoped = app.api('/api/dashboard?' + urlencode({'window': 'all', 'source': source}))
                    source_count = sum(record['source'] == source for record in exported)
                    assert scoped['summary']['request_count'] == source_count, 'source_request_count'
                    assert scoped['summary']['answer_count'] == source_count * 3, 'source_answer_count'
                    assert scoped['summary']['input_tokens'] == source_count * 100, 'source_token_count'
                update_progress('encrypted_storage')
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
                    update_progress('restart_dashboard')
                    restarted = app.api('/api/dashboard?window=all')
                    assert restarted['summary'] == summary, 'restart_summary_changed'
                    update_progress('restart_export')
                    after_export = [json.loads(line) for line in app.request('/api/export?window=all').splitlines() if line.strip()]
                    assert len(after_export) == len(exported), 'restart_record_count_changed'
                    assert {record['id']: record for record in after_export} == {record['id']: record for record in exported}, 'restart_records_changed'
                    update_progress('restart_health')
                    assert observe_health()['forwarded'] == 0, 'restart_process_counters'
                    item['restarted'] = True
                update_progress('cycle_complete')
                report['cycles'].append(item)
                save()
                print(json.dumps({key: item[key] for key in ('cycle', 'at', 'offered', 'retained', 'database_bytes', 'rss_kib', 'restarted')}), flush=True)
            publish('stopping')
            app.stop()
        report['passed'] = 'failure' not in report and 'cleanup_failure' not in report
    except (Exception, KeyboardInterrupt) as error:
        if original_error is not None and error is not original_error:
            # A temporary-directory cleanup error can replace the original
            # exception after cleanup_observer has already captured it.
            capture_cleanup_failure(error, 'workspace_cleanup')
        capture_failure(error)
    finally:
        for stage, action in (
            ('observer_stop', stop_observer), ('mock_shutdown', mock.shutdown),
            ('mock_close', mock.server_close), ('mock_join', lambda: thread.join(timeout=5)),
        ):
            try:
                action()
            except Exception as error:
                capture_cleanup_failure(error, stage)
        report['finished_at'] = now()
        report['total_offered'] = total
        try:
            publish('stopped')
        except Exception as error:
            capture_cleanup_failure(error, 'publish_stopped')
        try:
            save()
        except Exception as error:
            capture_cleanup_failure(error, 'save_report')
    print(json.dumps({'passed': report['passed'], 'cycles': len(report['cycles']),
                      'total_offered': total, 'failure': report.get('failure'),
                      'cleanup_failure': report.get('cleanup_failure')}))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
