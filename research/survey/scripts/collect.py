#!/usr/bin/env python3
"""Fetch pinned public source for a bounded, read-only Jev project survey.

Requires an authenticated GitHub CLI. Never installs or runs project code.
Source snapshots are cached outside the repository and are not redistributed.
"""
import argparse
import base64
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import subprocess
from urllib.parse import quote


def api(endpoint):
    result = subprocess.run(
        ['gh', 'api', endpoint], capture_output=True, text=True, timeout=90
    )
    if result.returncode:
        raise RuntimeError(result.stderr.strip()[:300])
    return json.loads(result.stdout)


def cached_api(directory, name, endpoint):
    path = directory / name
    if path.exists():
        return json.loads(path.read_text())
    result = api(endpoint)
    path.write_text(json.dumps(result))
    return result


def score_path(path, readme):
    lower = path.lower()
    score = 0
    for pattern, weight in [
        ('jev', 7), ('typesafe', 8), ('question', 8), ('decision', 7),
        ('classif', 6), ('client', 4), ('router', 4), ('scor', 4),
        ('guard', 3), ('triage', 3), ('rank', 3), ('compac', 3),
        ('policy', 2), ('src/', 2), ('lib/', 2),
    ]:
        if pattern in lower:
            score += weight
    if path in readme:
        score += 12
    if re.search(r'(test|spec|fixture|mock|generated)', lower):
        score -= 18
    if re.search(r'(types|schema|config|index)\.[^.]+$', lower):
        score -= 4
    return score


def collect(item, cache, file_count):
    repo = item['repository']
    directory = cache / repo.replace('/', '__')
    directory.mkdir(parents=True, exist_ok=True)
    meta = cached_api(directory, 'metadata.json', f'repos/{repo}')
    if meta.get('private'):
        raise RuntimeError('Private repository excluded')
    commit = cached_api(directory, 'commit.json', f'repos/{repo}/commits/{meta["default_branch"]}')
    sha = commit['sha']
    tree = cached_api(directory, 'tree.json', f'repos/{repo}/git/trees/{sha}?recursive=1')
    paths = [e['path'] for e in tree['tree'] if e['type'] == 'blob' and e.get('size', 0) < 250000]
    readmes = sorted((p for p in paths if re.search(r'(^|/)readme(\.en)?\.(md|rst|txt)$', p, re.I)), key=lambda p: (p.count('/'), len(p)))
    files = []

    def fetch(path, kind):
        local = directory / 'files' / path
        local.parent.mkdir(parents=True, exist_ok=True)
        if local.exists():
            content = local.read_bytes().decode('utf-8')
        else:
            obj = api(f'repos/{repo}/contents/{quote(path, safe="/")}?ref={sha}')
            content = base64.b64decode(obj['content']).decode('utf-8', errors='replace')
            local.write_text(content)
        files.append({
            'path': path, 'kind': kind,
            'url': f'https://github.com/{repo}/blob/{sha}/{quote(path, safe="/")}',
            'sha256': hashlib.sha256(content.encode()).hexdigest(),
            'lines': len(content.splitlines()),
        })
        return content

    readme = fetch(readmes[0], 'readme') if readmes else ''
    allowed = {'.py', '.ts', '.tsx', '.js', '.mjs', '.cjs', '.rs', '.go', '.rb', '.ex', '.exs', '.sh', '.c', '.cpp', '.h', '.sql', '.php', '.java', '.hs', '.R'}
    code = [p for p in paths if Path(p).suffix in allowed and not re.search(r'(^|/)(node_modules|vendor|dist|build|target|\.git|\.venv)/', p)]
    code.sort(key=lambda p: (-score_path(p, readme), len(p)))
    file_errors = []
    for path in code[:file_count]:
        try:
            fetch(path, 'implementation')
        except Exception as exc:
            file_errors.append({'path': path, 'error': str(exc)})
    return {
        **item, 'canonical_repository': meta['full_name'], 'public': True,
        'fork': meta['fork'], 'parent': meta.get('parent', {}).get('full_name'),
        'description': meta.get('description'), 'language': meta.get('language'),
        'commit': sha, 'commit_date': commit['commit']['committer']['date'],
        'retrieved_at': datetime.now(timezone.utc).isoformat(),
        'tree_truncated': tree.get('truncated', False),
        'files': files, 'file_errors': file_errors,
        'candidate_code_paths': code[:30],
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--sample', type=Path, default=Path('research/survey/sample.json'))
    parser.add_argument('--cache', type=Path, default=Path('/tmp/jev-observer-survey/cache'))
    parser.add_argument('--output', type=Path, default=Path('research/survey/manifest.json'))
    parser.add_argument('--workers', type=int, default=4)
    parser.add_argument('--files', type=int, default=4)
    args = parser.parse_args()
    rows = json.loads(args.sample.read_text())
    results = []
    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        jobs = {pool.submit(collect, r, args.cache, args.files): r for r in rows}
        for job in as_completed(jobs):
            item = jobs[job]
            try:
                results.append(job.result())
                print(f'{len(results):03}/{len(rows)} OK {item["repository"]}', flush=True)
            except Exception as exc:
                results.append({**item, 'error': str(exc)})
                print(f'{len(results):03}/{len(rows)} ERROR {item["repository"]}: {exc}', flush=True)
            args.output.write_text(json.dumps(sorted(results, key=lambda r: r['id']), indent=2) + '\n')


if __name__ == '__main__':
    main()
