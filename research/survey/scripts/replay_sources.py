#!/usr/bin/env python3
"""Retrieve the exact manifest files and verify hashes; never execute source.

Requires gh authentication. Uses the recorded commits instead of current HEAD.
Only source text normalized to UTF-8 by the original collector is hashed.
"""
import argparse
import base64
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import subprocess
from urllib.parse import quote


def fetch(job):
    repo, commit, item, cache = job
    destination = cache / repo.replace('/', '__') / 'files' / item['path']
    if destination.exists():
        content = destination.read_bytes().decode('utf-8')
    else:
        endpoint = f'repos/{repo}/contents/{quote(item["path"], safe="/")}?ref={commit}'
        result = subprocess.run(['gh', 'api', endpoint], capture_output=True, text=True, timeout=90)
        if result.returncode:
            raise RuntimeError(f'{repo}/{item["path"]}: {result.stderr[:300]}')
        response = json.loads(result.stdout)
        content = base64.b64decode(response['content']).decode('utf-8', errors='replace')
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(content)
    digest = hashlib.sha256(content.encode()).hexdigest()
    if digest != item['sha256']:
        raise ValueError(f'Hash mismatch: {destination}; use a fresh cache directory')
    return 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', type=Path, default=Path('research/survey/manifest.json'))
    parser.add_argument('--cache', type=Path, default=Path('/tmp/jev-observer-survey/cache'))
    parser.add_argument('--workers', type=int, default=4)
    args = parser.parse_args()
    rows = json.loads(args.manifest.read_text())
    jobs = [(r['repository'], r['commit'], f, args.cache) for r in rows for f in r['files']]
    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        count = sum(pool.map(fetch, jobs))
    print(f'Verified {count} source files across {len(rows)} repositories.')


if __name__ == '__main__':
    main()
