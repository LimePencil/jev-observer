#!/usr/bin/env python3
"""External numeric resource sampler for an active synthetic soak; no HTTP or token reads."""
import argparse
import importlib.util
import json
from pathlib import Path
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--harness', type=Path, required=True)
parser.add_argument('--state', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--seconds', type=int, default=36000)
parser.add_argument('--interval', type=float, default=5)
args = parser.parse_args()
if args.seconds < 1 or args.interval < 1:
    parser.error('Duration and interval must be at least one second')
spec = importlib.util.spec_from_file_location('soak', args.harness.resolve(strict=True))
soak = importlib.util.module_from_spec(spec)
spec.loader.exec_module(soak)
deadline = time.monotonic() + args.seconds
args.output.parent.mkdir(parents=True, exist_ok=True)
# Refuse to overwrite an earlier run's samples.
with args.output.open('x', encoding='utf-8') as output:
    while True:
        state = {}
        state_error = None
        try:
            state = json.loads(args.state.read_text())
        except (OSError, ValueError) as error:
            state_error = type(error).__name__
        pid = state.get('pid')
        harness_pid = state.get('harness_pid')
        pid = pid if isinstance(pid, int) and pid > 0 else None
        harness_pid = harness_pid if isinstance(harness_pid, int) and harness_pid > 0 else None
        sample = soak.resource_snapshot(pid, harness_pid)
        if harness_pid is None:
            sample['processes']['watcher'] = sample['processes'].pop('harness')
        sample['state_error'] = state_error
        sample['phase'] = state.get('phase') if state.get('phase') in {'running', 'restarting', 'stopping', 'stopped'} else None
        sample['generation'] = state.get('generation') if isinstance(state.get('generation'), int) else None
        sample['progress'] = {key: value for key, value in (state.get('progress') or {}).items()
                              if key in {'cycle', 'submitted', 'completed', 'pending', 'target', 'total_offered'}
                              and isinstance(value, (int, float))}
        output.write(json.dumps(sample) + '\n')
        output.flush()
        remaining = deadline - time.monotonic()
        if remaining <= 0 or sample['phase'] == 'stopped':
            break
        time.sleep(min(args.interval, remaining))
