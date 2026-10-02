#!/usr/bin/env python3
"""Print bounded source excerpts for manual review; this does not classify projects."""
import argparse
import json
from pathlib import Path
import re

parser = argparse.ArgumentParser()
parser.add_argument('start', type=int)
parser.add_argument('end', type=int)
parser.add_argument('--cache', type=Path, default=Path('/tmp/jev-observer-survey/cache'))
args = parser.parse_args()
rows = json.loads(Path('research/survey/manifest.json').read_text())
pattern = re.compile(r'system_?one|systemOne|instructions|criteria|confidence|threshold|base_url|baseURL|api\.typesafe|/decisions|\.jsonl|telemetry|sqlite|usage\.|cost_usd', re.I)
for row in rows:
    if not args.start <= row['id'] <= args.end:
        continue
    print(f'\n=== {row["id"]:03} {row["repository"]} ===')
    if row.get('error'):
        print(row['error'])
        continue
    print('DESCRIPTION:', row['description'])
    directory = args.cache / row['repository'].replace('/', '__') / 'files'
    for entry in row['files']:
        lines = (directory / entry['path']).read_text().splitlines()
        print('FILE', entry['path'])
        if entry['kind'] == 'readme':
            intro = [line for line in lines[:70] if line.strip() and not line.startswith(('![', '[![', '<', '```', '|', '  '))]
            print('INTRO:', ' '.join(intro)[:400])
            hits = [(i + 1, line) for i, line in enumerate(lines) if re.search(r'dashboard|logg|\.jsonl|telemetry|ledger|trace|base_url|baseURL|TYPESAFE_BASE|history|threshold', line, re.I)]
            for i, line in hits[:3]:
                print(f'{i}: {line[:210]}')
        else:
            hits = [(i + 1, line) for i, line in enumerate(lines) if pattern.search(line)]
            for i, line in hits[:6]:
                print(f'{i}: {line[:180]}')
