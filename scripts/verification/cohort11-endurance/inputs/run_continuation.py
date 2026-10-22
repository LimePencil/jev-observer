#!/usr/bin/env python3
"""Evidence-only pinned continuation launcher; never touches the old soak files."""
import argparse
import ctypes
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('guard', ROOT / 'process_guard.py')
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


class GuardFailure(Exception):
    pass


def require(condition, check):
    if not condition:
        raise GuardFailure(check)


def limits_check(sample, elapsed, limits):
    require(elapsed <= limits['seconds'], 'work_deadline')
    require(sample['available_kib'] >= limits['minimum_available_kib'], 'host_memory_guard')
    require(sample['group_rss_kib'] <= limits['maximum_group_rss_kib'], 'owned_memory_guard')
    require(sample['workspace_bytes'] <= limits['maximum_workspace_bytes'], 'owned_storage_guard')


def validate_report(report, run, pins):
    require(report.get('passed') is True, 'harness_passed')
    require(report.get('binary_sha256') == pins['binary_sha256'], 'reported_binary_pin')
    require(report.get('harness_sha256') == pins['copies'][run['harness']]['sha256'], 'reported_harness_pin')
    require(len(report['cycles']) == run['cycles'], 'all_cycles_completed')
    if run['kind'] == 'retention':
        require(report.get('helper_sha256') == pins['copies'][run['helper']]['sha256'], 'reported_helper_pin')
        require(report['total_offered'] == run['cycles'] * 3250, 'retention_total_offered')
        require(sum(item['restarted'] for item in report['cycles']) == run['expected']['same_database_restarts'], 'retention_expected_restarts')
        require(all(item['retained'] == 1000 for item in report['cycles']), 'retention_exact_cap')
    else:
        restarts = [item for item in report['cycles'] if 'restart' in item]
        require(len(restarts) == 1 and restarts[0]['restart']['canonical_equal'], 'mixed_same_database_restart')
        require(all(code == 0 for code in report['process_exits']), 'mixed_clean_exits')
        require(report.get('owned_workspace_removed') is True, 'mixed_workspace_removed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kind', choices=['retention', 'mixed'], required=True)
    args = parser.parse_args()
    require(sys.platform.startswith('linux') and __debug__, 'supported_supervisor_platform')
    plan = json.loads((ROOT / 'plan.json').read_text())
    require(plan.get('status') == 'ready_to_launch', 'verified_launch_plan_required')
    pins = json.loads((ROOT / 'source-pins.json').read_text())
    run = plan['runs'][args.kind]
    require(guard.sha(Path(pins['binary'])) == pins['binary_sha256'], 'binary_pin_before')
    for name, pin in pins['copies'].items():
        require(guard.sha(Path(name)) == pin['sha256'], 'source_pin_before')
    output = Path(run['supervisor_output'])
    report_path = Path(run['report'])
    workspace = Path(run['workspace_root'])
    require(not output.exists() and not report_path.parent.exists(), 'fresh_output_paths')
    require(not list(workspace.iterdir()), 'fresh_workspace_root')
    require(guard.available_kib() >= plan['limits']['minimum_available_kib'], 'startup_memory_guard')
    os.umask(0o077)
    output.mkdir(mode=0o700)
    summary = {'started_at': guard.utc(), 'kind': args.kind, 'passed': False,
               'binary_sha256': pins['binary_sha256'], 'source_pins_sha256': guard.sha(ROOT / 'source-pins.json'),
               'plan_sha256': guard.sha(ROOT / 'plan.json'), 'limits': plan['limits'], 'command': run['command']}
    process = None
    known = set()
    stage = 'setup'
    env = {name: value for name, value in os.environ.items()
           if not name.upper().startswith(('TYPESAFE_', 'OPENROUTER_', 'JEV_OBSERVER_'))
           and name.lower() not in ('http_proxy', 'https_proxy', 'all_proxy', 'no_proxy')}
    env.update(TMPDIR=str(workspace), SQLITE_TMPDIR=str(workspace), NO_PROXY='127.0.0.1,localhost', PYTHONUNBUFFERED='1')
    try:
        require(ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) == 0, 'subreaper_enabled')
        signal.signal(signal.SIGTERM, guard.requested_stop)
        signal.signal(signal.SIGINT, guard.requested_stop)
        with (output / 'harness.log').open('x') as log, (output / 'resources.jsonl').open('x') as samples:
            process = subprocess.Popen(run['command'], cwd=ROOT, env=env, stdout=log,
                                       stderr=subprocess.STDOUT, start_new_session=True)
            known.add(process.pid)
            summary.update(harness_pid=process.pid, owned_pgid=process.pid)
            (output / 'started.json').write_text(json.dumps(summary, indent=2) + '\n')
            started = time.monotonic()
            stage = 'run'
            while process.poll() is None:
                members = guard.group_members(process.pid)
                known.update(members)
                sample = {'at': guard.utc(), 'processes': members,
                          'available_kib': guard.available_kib(),
                          'group_rss_kib': sum(value.get('VmRSS', 0) for value in members.values()),
                          'workspace_bytes': guard.disk_bytes(workspace)}
                samples.write(json.dumps(sample) + '\n')
                samples.flush()
                require(guard.stop_signal is None, 'operator_stop')
                limits_check(sample, time.monotonic() - started, plan['limits'])
                time.sleep(5)
            stage = 'validate'
            require(process.returncode == 0, 'harness_exit_zero')
            validate_report(json.loads(report_path.read_text()), run, pins)
            require(guard.sha(Path(pins['binary'])) == pins['binary_sha256'], 'binary_pin_after')
            for name, pin in pins['copies'].items():
                require(guard.sha(Path(name)) == pin['sha256'], 'source_pin_after')
            require(not list(workspace.iterdir()), 'workspace_success_cleanup')
            guard.reap_orphans(process, known)
            require(not guard.group_members(process.pid), 'owned_group_empty_before_cleanup')
            require(guard.stop_signal is None, 'operator_stop')
            summary['passed'] = True
    except BaseException as error:
        summary['failure'] = {'stage': stage, 'type': type(error).__name__}
        if isinstance(error, GuardFailure):
            summary['failure']['check'] = str(error)
    finally:
        if process is not None:
            try:
                if process.poll() is None:
                    # Let each harness perform its own bounded cleanup first.
                    process.send_signal(signal.SIGINT)
                    try:
                        process.wait(timeout=45)
                    except subprocess.TimeoutExpired:
                        pass
                remaining = guard.stop_group(process, known, grace_seconds=10)
                summary.update(exit_code=process.returncode, remaining_owned_pids=sorted(remaining),
                               observed_owned_pids=sorted(known))
                require(not remaining, 'owned_group_empty')
            except BaseException as error:
                summary['passed'] = False
                summary['cleanup_failure'] = {'type': type(error).__name__}
        if guard.stop_signal is not None:
            summary['passed'] = False
            summary['stop_signal'] = guard.stop_signal
        # Failed encrypted workspaces stay private for diagnosis; never remove
        # them merely because a supervisor guard fired.
        try:
            summary['retained_workspace_entries'] = len(list(workspace.iterdir()))
        except OSError as error:
            summary['passed'] = False
            summary.setdefault('cleanup_failure', {'type': type(error).__name__})
        summary['finished_at'] = guard.utc()
        with (output / 'supervisor.json').open('x') as stream:
            json.dump(summary, stream, indent=2)
            stream.write('\n')
    print(json.dumps({'passed': summary['passed'], 'report': str(output / 'supervisor.json')}))
    return 0 if summary['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
