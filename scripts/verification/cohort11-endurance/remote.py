#!/usr/bin/env python3
"""Temporary synthetic endurance verification; never reads a local workspace."""
import argparse
import ctypes
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import stat
import subprocess
import sys
import time
import urllib.request

HERE = Path(__file__).resolve().parent
REPOSITORY = 'LimePencil/jev-observer'
TARGET = json.loads((HERE / 'expected-build.json').read_text())
RUN = TARGET['run_id']
HEAD = TARGET['verification_commit']
SOURCE = TARGET['source_commit']
PRODUCTION = TARGET['production_manifest_sha256']
ARTIFACT = TARGET['artifact_name']
BINARY = TARGET['binary_name']
PROBE = TARGET['probe_name']
STOP = None


class Failure(Exception):
    pass


def require(condition, name):
    if not condition:
        raise Failure(name)


def now():
    return datetime.now(timezone.utc).isoformat()


def sha(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + '.tmp')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(path)


def checked_file(path, maximum):
    value = path.lstat()
    require(stat.S_ISREG(value.st_mode) and value.st_nlink == 1 and value.st_uid == os.getuid(), 'regular_owned_file')
    require(value.st_size <= maximum, 'bounded_file')
    return value


def read(path, maximum=2 * 1024 * 1024):
    checked_file(path, maximum)
    return json.loads(path.read_text())


def expected_hash(name):
    value = TARGET.get(name)
    require(isinstance(value, str) and re.fullmatch('[0-9a-f]{64}', value) is not None, 'expected_hash_required')
    return value


def api_json(suffix):
    request = urllib.request.Request('https://api.github.com/repos/' + REPOSITORY + suffix,
                                    headers={'Authorization': 'Bearer ' + os.environ['GITHUB_TOKEN'],
                                             'Accept': 'application/vnd.github+json', 'X-GitHub-Api-Version': '2022-11-28'})
    with urllib.request.urlopen(request, timeout=20) as response:
        require(response.url.startswith('https://api.github.com/'), 'api_origin')
        data = response.read(2 * 1024 * 1024 + 1)
    require(len(data) <= 2 * 1024 * 1024, 'api_response_bound')
    return json.loads(data)


def validate_run(run):
    require(run['id'] == RUN and run['head_sha'] == HEAD and run['run_attempt'] == 1, 'upstream_run_identity')
    require(run['repository']['full_name'] == REPOSITORY and run['event'] == 'workflow_dispatch', 'upstream_repository_event')
    require(run['path'] == '.github/workflows/ci.yml' and run['status'] == 'completed'
            and run['conclusion'] == 'success', 'upstream_full_verification_success')


def verify_upstream(root):
    require(os.environ['GITHUB_REPOSITORY'] == REPOSITORY, 'repository_identity')
    require(TARGET['status'] == 'verified_ready' and isinstance(RUN, int) and RUN > 0
            and isinstance(HEAD, str) and re.fullmatch('[0-9a-f]{40}', HEAD)
            and isinstance(SOURCE, str) and re.fullmatch('[0-9a-f]{40}', SOURCE)
            and isinstance(ARTIFACT, str) and HEAD in ARTIFACT, 'reviewed_target_required')
    expected_hash('production_manifest_sha256')
    expected_hash('provenance_sha256')
    expected_hash('binary_sha256')
    run = api_json('/actions/runs/' + str(RUN))
    validate_run(run)
    listing = api_json('/actions/runs/' + str(RUN) + '/artifacts?per_page=100')
    require(listing['total_count'] <= 100, 'artifact_listing_bound')
    matches = [item for item in listing['artifacts'] if item['name'] == ARTIFACT]
    require(len(matches) == 1, 'single_expected_artifact')
    artifact = matches[0]
    require(not artifact['expired'] and 0 < artifact['size_in_bytes'] <= 140 * 1024 * 1024, 'artifact_size_expiry')
    require(artifact['workflow_run']['id'] == RUN and artifact['workflow_run']['head_sha'] == HEAD, 'artifact_run_identity')
    write(root / 'upstream.json', {'at': now(), 'verified': True, 'run_id': RUN, 'verification_commit': HEAD,
                                 'source_commit': SOURCE, 'artifact_id': artifact['id'], 'artifact_name': ARTIFACT,
                                 'artifact_bytes': artifact['size_in_bytes'], 'conclusion': run['conclusion'],
                                 'expected_provenance_sha256': expected_hash('provenance_sha256'),
                                 'expected_binary_sha256': expected_hash('binary_sha256')})
    with Path(os.environ['GITHUB_OUTPUT']).open('a') as stream:
        stream.write('artifact_id=' + str(int(artifact['id'])) + '\n')
        stream.write('run_id=' + str(RUN) + '\n')


def validate_provenance(value, upstream):
    require(value['schema'] == 1 and value['source_commit'] == SOURCE and value['verification_commit'] == HEAD, 'provenance_source')
    require(value['production_manifest_sha256'] == PRODUCTION and value['excluded_only'] == '.github/workflows/ci.yml', 'production_manifest')
    require(str(value['run_id']) == str(RUN) and str(value['run_attempt']) == '1'
            and value['repository'] == REPOSITORY, 'provenance_run')
    require(value['runner_os'] == 'Linux' and value['runner_arch'] == 'X64', 'runner_platform')
    require(set(value['binaries']) == {BINARY, PROBE}, 'binary_set')
    require(value['binaries'][BINARY]['sha256'] == upstream['expected_binary_sha256'], 'expected_binary_pin')


def prepare(root):
    upstream = read(root / 'upstream.json')
    require(upstream['verified'] and upstream['run_id'] == RUN and upstream['verification_commit'] == HEAD, 'verified_upstream_required')
    incoming = root / 'incoming'
    require(not incoming.is_symlink() and set(x.name for x in incoming.iterdir()) == {BINARY, PROBE, 'provenance.json'}, 'artifact_file_set')
    checked_file(incoming / 'provenance.json', 2 * 1024 * 1024)
    require(sha(incoming / 'provenance.json') == upstream['expected_provenance_sha256'], 'expected_provenance_pin')
    provenance = read(incoming / 'provenance.json')
    validate_provenance(provenance, upstream)
    for name in (BINARY, PROBE):
        checked_file(incoming / name, 64 * 1024 * 1024)
        require(sha(incoming / name) == provenance['binaries'][name]['sha256'], 'downloaded_binary_pin')
    (incoming / BINARY).chmod(0o500)
    inputs = read(HERE / 'inputs.json')['files']
    require(len(inputs) == 9, 'nine_inputs')
    work = root / 'private'
    work.mkdir(mode=0o700)
    copies = {}
    for name, digest in inputs.items():
        require(not Path(name).is_absolute() and '..' not in Path(name).parts, 'input_relative_path')
        original = HERE / 'inputs' / name
        checked_file(original, 256 * 1024)
        require(sha(original) == digest, 'input_pin')
        destination = work / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(original, destination)
        copies[str(destination)] = {'sha256': digest, 'source': str(original), 'source_sha256': digest}
    binary = str(incoming / BINARY)
    digest = upstream['expected_binary_sha256']
    runs = {}
    for kind, cycles in [('retention', 15), ('mixed', 22)]:
        report = work / (kind + '-run') / 'report.json'
        workspace = work / ('retention/tmp' if kind == 'retention' else 'tmp')
        workspace.mkdir(parents=True, mode=0o700)
        harness = work / ('retention/scripts/check-retention-soak.py' if kind == 'retention' else 'mixed/harness.py')
        command = [sys.executable, str(harness)]
        if kind == 'retention':
            command += ['--binary', binary, '--output', str(report), '--active-state', str(report.with_name('state.json')),
                        '--cycles', '15', '--seconds', '325', '--rate', '10', '--max-records', '1000', '--restart-every', '5']
        else:
            command += ['--run', '--binary', binary, '--sha256', digest, '--output', str(report.parent), '--cycles', '22', '--seconds', '240']
        runs[kind] = {'kind': kind, 'harness': str(harness), 'cycles': cycles, 'report': str(report),
                      'supervisor_output': str(work / (kind + '-supervisor')), 'workspace_root': str(workspace), 'command': command,
                      'expected': {'same_database_restarts': 3, 'same_database_midpoint_restart_after_cycle': 11}}
        if kind == 'retention':
            runs[kind]['helper'] = str(work / 'retention/scripts/test-upgrade.py')
    pins = {'binary': binary, 'binary_sha256': digest, 'copies': copies, 'integration_commit': SOURCE}
    plan = {'status': 'ready_to_launch', 'binary': binary, 'binary_sha256': digest, 'runs': runs,
            'limits': {'seconds': 7200, 'minimum_available_kib': 1572864,
                       'maximum_group_rss_kib': 1048576, 'maximum_workspace_bytes': 134217728}}
    write(work / 'source-pins.json', pins)
    write(work / 'plan.json', plan)
    write(root / 'preparation.json', {'at': now(), 'source_commit': SOURCE, 'binary_sha256': digest,
                                     'provenance_sha256': sha(incoming / 'provenance.json'), 'input_manifest_sha256': sha(HERE / 'inputs.json'),
                                     'controller_sha256': sha(Path(__file__)), 'plan_sha256': sha(work / 'plan.json'),
                                     'source_pins_sha256': sha(work / 'source-pins.json'),
                                     'scope': 'Only synthetic loopback workloads; fresh runner workspace. Existing interrupted local runs remain incomplete and are not read. Copied historical scope text does not establish completed prior endurance. Observer startup may inspect credential state on the disposable runner; workloads do not call credential mutation APIs.'})
    return work


def memory():
    return next(int(line.split()[1]) for line in Path('/proc/meminfo').read_text().splitlines() if line.startswith('MemAvailable:'))


def processes(known):
    table = {}
    for path in Path('/proc').iterdir():
        if not path.name.isdecimal():
            continue
        try:
            raw = (path / 'stat').read_text()
            fields = raw[raw.rfind(')') + 2:].split()
            rss = next((int(line.split()[1]) for line in (path / 'status').read_text().splitlines() if line.startswith('VmRSS:')), 0)
            table[int(path.name)] = {'ppid': int(fields[1]), 'start_ticks': int(fields[19]), 'rss_kib': rss}
        except (OSError, ValueError, IndexError):
            continue
    owned = {pid: row for pid, row in table.items() if known.get(pid) == row['start_ticks']}
    while True:
        added = {pid: row for pid, row in table.items() if pid != os.getpid() and pid not in owned and row['ppid'] in {os.getpid(), *owned}}
        if not added:
            break
        owned.update(added)
    known.update({pid: row['start_ticks'] for pid, row in owned.items()})
    return owned


def requested(signum, _frame):
    global STOP
    STOP = signum


def bounded_storage(work):
    total = 0
    for directory, dirs, files, fd in os.fwalk(work, follow_symlinks=False):
        for name in dirs + files:
            try:
                value = os.stat(name, dir_fd=fd, follow_symlinks=False)
            except FileNotFoundError:
                continue
            require(not stat.S_ISLNK(value.st_mode), 'owned_path_symlink')
            total += value.st_blocks * 512
    return total


def cleanup(children, known):
    for child in children:
        if child.poll() is None:
            child.send_signal(signal.SIGINT)
    for signum, seconds in [(None, 75), (signal.SIGTERM, 10), (signal.SIGKILL, 5)]:
        deadline = time.monotonic() + seconds
        while True:
            for child in children:
                child.poll()
            owned = processes(known)
            for pid, row in owned.items():
                if pid not in {child.pid for child in children}:
                    try:
                        os.waitpid(pid, os.WNOHANG)
                    except ChildProcessError:
                        pass
                if signum is not None:
                    try:
                        raw = Path('/proc', str(pid), 'stat').read_text()
                        if int(raw[raw.rfind(')') + 2:].split()[19]) == row['start_ticks']:
                            os.kill(pid, signum)
                    except (FileNotFoundError, ProcessLookupError):
                        pass
            if not processes(known):
                for child in children:
                    child.wait(timeout=1)
                return []
            if time.monotonic() >= deadline:
                break
            time.sleep(.1)
    return sorted(processes(known))


def run(root):
    children, logs, known = [], [], {}
    summary = {'started_at': now(), 'passed': False, 'source_commit': SOURCE, 'upstream_run': RUN, 'starts': []}
    stage = 'prepare'
    try:
        work = prepare(root)
        require(memory() >= 3 * 1024 * 1024, 'startup_memory')
        require(ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) == 0, 'subreaper')
        signal.signal(signal.SIGTERM, requested)
        signal.signal(signal.SIGINT, requested)
        started = time.monotonic()
        for kind in ('retention', 'mixed'):
            stage = kind + '_startup'
            require(memory() >= (3 if kind == 'retention' else 2) * 1024 * 1024, 'sequential_start_memory')
            log = (root / (kind + '-console.log')).open('xb')
            logs.append(log)
            env = {k: v for k, v in os.environ.items() if k not in ('GITHUB_TOKEN', 'GH_TOKEN')}
            child = subprocess.Popen([sys.executable, str(work / 'run_continuation.py'), '--kind', kind],
                                     cwd=work, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            children.append(child)
            deadline = time.monotonic() + 90
            while True:
                processes(known)
                require(STOP is None and child.poll() is None, 'startup_child_running')
                require(memory() >= 1572864 and time.monotonic() < deadline, 'startup_bound')
                state_path = work / (kind + '-run/state.json')
                if state_path.exists():
                    state = read(state_path)
                    ready = state.get('phase') == 'running' and state.get('pid') in processes(known)
                    if kind == 'retention':
                        ready = ready and state.get('last_health') is not None and state.get('progress', {}).get('subphase') == 'load_dispatch'
                    if ready:
                        summary['starts'].append({'kind': kind, 'at': now(), 'supervisor_pid': child.pid,
                                                  'observer_pid': state['pid'], 'binary_sha256': state['binary_sha256']})
                        break
                time.sleep(.25)
        stage = 'concurrent_run'
        with (root / 'resources.jsonl').open('x') as samples:
            while any(child.poll() is None for child in children):
                owned = processes(known)
                row = {'at': now(), 'available_kib': memory(), 'owned_rss_kib': sum(x['rss_kib'] for x in owned.values()),
                       'owned_processes': owned, 'allocated_bytes': bounded_storage(work)}
                samples.write(json.dumps(row) + '\n'); samples.flush()
                require(STOP is None and time.monotonic() - started < 6600, 'outer_work_deadline_or_stop')
                require(row['available_kib'] >= 1572864 and row['owned_rss_kib'] <= 2 * 1024 * 1024, 'outer_memory_guard')
                require(row['allocated_bytes'] <= 320 * 1024 * 1024, 'outer_storage_guard')
                require(bounded_storage(root) <= 512 * 1024 * 1024, 'all_artifact_storage_guard')
                for path in [root / 'resources.jsonl', *(root.glob('*-console.log')), *(work.glob('*-supervisor/harness.log'))]:
                    try:
                        checked_file(path, 16 * 1024 * 1024)
                    except FileNotFoundError:
                        pass
                require(all(child.poll() in (None, 0) for child in children), 'child_failure')
                time.sleep(5)
        stage = 'terminal_validation'
        require(all(child.returncode == 0 for child in children), 'supervisors_exit_zero')
        for kind in ('retention', 'mixed'):
            result = read(work / (kind + '-supervisor/supervisor.json'))
            require(result['passed'] is True and result['exit_code'] == 0 and not result['remaining_owned_pids'], 'supervisor_passed_clean')
        require(sha(root / 'incoming' / BINARY) == read(root / 'upstream.json')['expected_binary_sha256'], 'final_binary_pin')
        summary['passed'] = True
    except BaseException as error:
        summary['failure'] = {'stage': stage, 'type': type(error).__name__}
        if isinstance(error, Failure):
            summary['failure']['check'] = str(error)
    finally:
        try:
            remaining = cleanup(children, known)
            summary.update(remaining_owned_pids=remaining, supervisor_exit_codes=[p.returncode for p in children], observed_owned_pids=sorted(known))
            if remaining:
                summary['passed'] = False
        except BaseException as error:
            summary.update(passed=False, cleanup_failure={'type': type(error).__name__})
        for log in logs:
            log.close()
        if STOP is not None:
            summary.update(passed=False, stop_signal=STOP)
        summary['finished_at'] = now()
        write(root / 'summary.json', summary)
    print(json.dumps({'passed': summary['passed'], 'report': 'summary.json'}))
    return 0 if summary['passed'] else 1


def report_paths():
    result = ['upstream.json', 'preparation.json', 'summary.json', 'resources.jsonl', 'retention-console.log', 'mixed-console.log',
              'incoming/provenance.json', 'private/plan.json', 'private/source-pins.json']
    for kind in ('retention', 'mixed'):
        result += ['private/' + kind + '-run/' + name for name in ('report.json', 'state.json', 'resources.jsonl')]
        result += ['private/' + kind + '-supervisor/' + name for name in ('started.json', 'supervisor.json', 'resources.jsonl', 'harness.log')]
    return result


def collect(root, destination):
    destination.mkdir(mode=0o700)
    report = {'at': now(), 'scope': 'Explicit reports only; private database/key/token/workspace paths never traversed or uploaded.', 'files': {}, 'errors': []}
    total = 0
    for name in report_paths():
        path = root / name
        if not path.exists() and not path.is_symlink():
            continue
        try:
            require(not any(parent.is_symlink() for parent in path.parents if parent != root.parent), 'report_parent_symlink')
            value = checked_file(path, 16 * 1024 * 1024)
            require(total + value.st_size <= 64 * 1024 * 1024, 'report_total_bound')
            target = destination / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, target)
            require(sha(path) == sha(target), 'report_copy_pin')
            total += value.st_size
            report['files'][name] = {'bytes': value.st_size, 'sha256': sha(target)}
        except BaseException as error:
            report['errors'].append({'file': name, 'type': type(error).__name__})
    report['bytes'] = total
    write(destination / 'artifact-inventory.json', report)
    require(not report['errors'], 'report_collection_failure')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('verify-upstream', 'run', 'collect'))
    args = parser.parse_args()
    os.umask(0o077)
    root = Path(os.environ['RUNNER_TEMP']) / 'cohort11-endurance'
    require(not root.is_symlink(), 'root_symlink')
    root.mkdir(mode=0o700, exist_ok=True)
    if args.action == 'verify-upstream':
        verify_upstream(root)
    elif args.action == 'run':
        return run(root)
    else:
        collect(root, Path(os.environ['RUNNER_TEMP']) / 'cohort11-endurance-reports')
    return 0


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except Failure as error:
        print(json.dumps({'passed': False, 'check': str(error)}))
        raise SystemExit(1)
