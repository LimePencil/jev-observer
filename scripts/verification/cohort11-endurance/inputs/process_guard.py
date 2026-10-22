#!/usr/bin/env python3
"""Session-only supervisor for the synthetic all-case stress run; no user data."""
import argparse, ctypes, datetime, hashlib, json, os, shutil, signal, stat, subprocess, sys, time
from pathlib import Path

ROOT = Path('/home/limepencil/dev/jev-observer')
EVIDENCE = ROOT / '.git/improvement-20261022'
stop_signal = None

def utc():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def requested_stop(signum, _frame):
    global stop_signal
    stop_signal = signum

def available_kib():
    return next(int(line.split()[1]) for line in Path('/proc/meminfo').read_text().splitlines() if line.startswith('MemAvailable:'))

def group_members(group):
    values = {}
    for entry in Path('/proc').iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            raw = (entry / 'stat').read_text()
            fields = raw[raw.rfind(')') + 2:].split()
            if int(fields[2]) != group or int(fields[3]) != group:
                continue
            numeric = {'start_ticks': int(fields[19]), 'zombie': fields[0] == 'Z'}
            for line in (entry / 'status').read_text().splitlines():
                key, _, value = line.partition(':')
                if key in ('VmRSS', 'VmHWM', 'VmSwap', 'Threads'):
                    numeric[key] = int(value.split()[0])
            values[int(entry.name)] = numeric
        except (OSError, ValueError, IndexError):
            continue
    return values

def reap_orphans(process, known):
    process.poll()  # Popen owns its direct child's exit status.
    for pid in known:
        if pid != process.pid:
            try:
                os.waitpid(pid, os.WNOHANG)
            except ChildProcessError:
                pass

def stop_group(process, known, grace_seconds=20):
    group = process.pid
    assert group != os.getpgrp()
    members = group_members(group)
    if members:
        try: os.killpg(group, signal.SIGTERM)
        except ProcessLookupError: pass
    end = time.monotonic() + grace_seconds
    while time.monotonic() < end:
        known.update(members)
        reap_orphans(process, known)
        members = group_members(group)
        if not members: break
        time.sleep(0.1)
    if members:
        try: os.killpg(group, signal.SIGKILL)
        except ProcessLookupError: pass
        end = time.monotonic() + 5
        while time.monotonic() < end:
            known.update(members)
            reap_orphans(process, known)
            members = group_members(group)
            if not members: break
            time.sleep(0.1)
    process.wait(timeout=5)
    return group_members(group)

def disk_bytes(root):
    size = 0
    for path in root.rglob('*'):
        try:
            info = path.lstat()
        except FileNotFoundError:
            continue  # SQLite WALs and per-case files can vanish during cleanup.
        if stat.S_ISLNK(info.st_mode):
            raise RuntimeError('owned_workspace_symlink')
        size += info.st_blocks * 512
    return size

def validate_report(report, binary_hash, harness_hash, module_hash):
    assert report['passed'] is True
    assert report['binary_sha256'] == binary_hash
    assert report['harness_sha256'] == harness_hash
    assert report['lock_helper_build_script_sha256'] == module_hash
    assert len(report['lock_helper_sha256']) == 64
    assert [case['name'] for case in report['cases']] == ['capture-budget', 'locked-writer', 'burst']
    for case, pressure, provider in zip(report['cases'], [1500, 1500, 2500], [1525, 1525, 5025]):
        assert case['passed'] is True and case['shutdown_exit'] == 0
        assert case['pressure']['offered'] == pressure
        assert case['warmup']['offered'] == 5 and case['recovery']['offered'] == 20
        assert case['proxy_upstream_attempts'] == pressure + 25
        assert case['upstream_attempts'] == provider
        assert case['final_health']['persisted'] + case['final_health']['dropped'] == pressure + 25
        assert all(case['checks'][key] is True for key in ['loss_accounting_exact', 'recovery_complete', 'persisted_accounting_exact', 'responses_preserved', 'health_independent_of_writer', 'live_database_not_plaintext'])

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--script', type=Path, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--sha256', required=True)
    parser.add_argument('--harness-sha256', required=True)
    parser.add_argument('--module-sha256', required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    assert sys.platform.startswith('linux') and __debug__
    assert args.script.is_absolute() and args.binary.is_absolute() and args.output_dir.is_absolute()
    module = args.script.with_name('stress-cargo.mjs')
    assert sha(args.binary) == args.sha256 and sha(args.script) == args.harness_sha256 and sha(module) == args.module_sha256
    assert available_kib() > 3 * 1024 * 1024
    assert not args.output_dir.exists()
    os.umask(0o077)
    args.output_dir.mkdir(mode=0o700)
    work = args.output_dir / 'work'
    work.mkdir(mode=0o700)
    output = args.output_dir / 'report.json'
    summary_path = args.output_dir / 'supervisor.json'
    log_path = args.output_dir / 'stress.log'
    samples_path = args.output_dir / 'resources.jsonl'
    env = {name: value for name, value in os.environ.items() if not name.upper().startswith('TYPESAFE_') and name.upper() != 'JEV_OBSERVER_DB_KEY' and name.lower() not in ('http_proxy', 'https_proxy', 'all_proxy', 'no_proxy')}
    env.update(CARGO_TARGET_DIR=str(ROOT / 'target'), CARGO_BUILD_JOBS='1', CARGO_NET_OFFLINE='true', TMPDIR=str(EVIDENCE / 'tmp'), NO_PROXY='127.0.0.1,localhost')
    command = ['node', str(args.script), '--binary=' + str(args.binary), '--case=all', '--burst-rate=500', '--burst-seconds=5', '--python=' + sys.executable, '--work-dir=' + str(work), '--output=' + str(output)]
    summary = {'started_at': utc(), 'passed': False, 'binary_sha256': args.sha256, 'harness_sha256': args.harness_sha256, 'module_sha256': args.module_sha256, 'command': command, 'limits': {'seconds': 600, 'minimum_available_kib': 1536 * 1024, 'maximum_group_rss_kib': 1024 * 1024, 'maximum_workspace_bytes': 256 * 1024 * 1024}, 'samples': 0, 'max_group_rss_kib': 0, 'max_workspace_bytes': 0, 'minimum_available_kib': None}
    process = None
    known = set()
    stage = 'subreaper'
    try:
        # Reap owned orphaned grandchildren after group shutdown, even if Node
        # exits before its helper. This supervisor creates no unrelated children.
        if ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) != 0:
            raise OSError('cannot_enable_child_subreaper')
        signal.signal(signal.SIGTERM, requested_stop)
        signal.signal(signal.SIGINT, requested_stop)
        stage = 'launch'
        with log_path.open('x') as log, samples_path.open('x') as samples:
            process = subprocess.Popen(command, cwd=args.script.parent.parent, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            started = time.monotonic()
            stage = 'run'
            while process.poll() is None:
                members = group_members(process.pid)
                known.update(members)
                rss = sum(value.get('VmRSS', 0) for value in members.values())
                available = available_kib()
                size = disk_bytes(work)
                samples.write(json.dumps({'at': utc(), 'processes': members, 'group_rss_kib': rss, 'available_kib': available, 'workspace_bytes': size}) + '\n')
                samples.flush()
                summary['samples'] += 1
                summary['max_group_rss_kib'] = max(summary['max_group_rss_kib'], rss)
                summary['max_workspace_bytes'] = max(summary['max_workspace_bytes'], size)
                summary['minimum_available_kib'] = min(summary['minimum_available_kib'] or available, available)
                if stop_signal is not None: raise InterruptedError('supervisor_signal')
                if time.monotonic() - started > 600: raise TimeoutError('run_deadline')
                if rss > 1024 * 1024: raise MemoryError('owned_group_rss_limit')
                if available < 1536 * 1024: raise MemoryError('available_memory_limit')
                if size > 256 * 1024 * 1024: raise RuntimeError('workspace_size_limit')
                time.sleep(0.5)
            summary['exit_code'] = process.returncode
            stage = 'validate'
            assert process.returncode == 0
            validate_report(json.loads(output.read_text()), args.sha256, args.harness_sha256, args.module_sha256)
            assert sha(args.binary) == args.sha256 and sha(args.script) == args.harness_sha256 and sha(module) == args.module_sha256
            reap_orphans(process, known)
            assert not group_members(process.pid)
            assert not list(work.iterdir())
            assert stop_signal is None
            summary['passed'] = True
    except BaseException as error:
        summary['failure'] = {'stage': stage, 'type': type(error).__name__}
    finally:
        summary['signal'] = stop_signal
        if stop_signal is not None:
            summary['passed'] = False
            summary.setdefault('failure', {'stage': 'signal', 'type': 'InterruptedError'})
        if process is not None:
            try:
                remaining = stop_group(process, known)
                summary['remaining_owned_pids'] = sorted(remaining)
                summary['exit_code'] = process.returncode
                if remaining: raise RuntimeError('owned_processes_remain')
            except BaseException as error:
                summary['passed'] = False
                summary['cleanup_failure'] = {'type': type(error).__name__}
        if not summary.get('cleanup_failure'):
            try:
                shutil.rmtree(work)
                summary['workspace_removed'] = True
            except BaseException as error:
                summary['passed'] = False
                summary['cleanup_failure'] = {'type': type(error).__name__}
        summary['finished_at'] = utc()
        summary['artifacts'] = {path.name: {'bytes': path.stat().st_size, 'sha256': sha(path)} for path in [Path(__file__), args.script, module, output, log_path, samples_path] if path.is_file()}
        with summary_path.open('x') as stream: json.dump(summary, stream, indent=2); stream.write('\n')
    print(json.dumps({'passed': summary['passed'], 'report': str(summary_path)}))
    return 0 if summary['passed'] else 1

if __name__ == '__main__':
    raise SystemExit(main())
