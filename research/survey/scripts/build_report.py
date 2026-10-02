#!/usr/bin/env python3
"""Validate manual research annotations and render the 100-project evidence matrix."""
from collections import Counter
import csv
import io
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
PATTERNS = {
    'fixed': 'Fixed recurring definitions',
    'rules': 'User/configuration-defined rules',
    'indexed': 'Indexed or contextual question instances',
    'candidates': 'Changing candidate sets',
    'study': 'Controlled study or benchmark variants',
}


def main():
    sample = json.loads((ROOT / 'sample.json').read_text())
    manifest = json.loads((ROOT / 'manifest.json').read_text())
    with (ROOT / 'annotations.tsv').open() as stream:
        notes = list(csv.DictReader(stream, delimiter='\t'))
    assert len(sample) == len(manifest) == len(notes) == 100
    assert len({r['repository'].casefold() for r in sample}) == 100
    assert {r['id'] for r in sample} == set(range(1, 101))
    by_id = {r['id']: r for r in manifest}
    samples = {r['id']: r for r in sample}
    assert len(by_id) == 100
    assert {int(r['id']) for r in notes} == set(range(1, 101))
    rows = []
    for note in notes:
        assert None not in note and all(note.values()), note
        item = by_id[int(note['id'])]
        assert item['repository'] == samples[item['id']]['repository']
        assert item['public'] and not item.get('error')
        assert not item['tree_truncated'] and not item['file_errors']
        assert re.fullmatch('[0-9a-f]{40}', item['commit'])
        assert note['primary_pattern'] in PATTERNS
        files = {f['path']: f for f in item['files']}
        source = files[note['evidence_path']]
        assert f'/blob/{item["commit"]}/' in source['url']
        readme = next(f for f in item['files'] if f['kind'] == 'readme')
        rows.append({
            'id': item['id'], 'repository': item['repository'],
            'category': item['discovery_category'], **note,
            'evidence_url': source['url'], 'readme_url': readme['url'],
            'commit': item['commit'],
        })
    rows.sort(key=lambda r: int(r['id']))
    output = io.StringIO(newline='')
    writer = csv.DictWriter(output, fieldnames=list(rows[0]), lineterminator='\n')
    writer.writeheader()
    writer.writerows(rows)
    (ROOT / 'projects.csv').write_text(output.getvalue())
    counts = {
        'repositories': len(rows),
        'github_declared_forks': sum(r['fork'] for r in manifest),
        'collected_files': sum(len(r['files']) for r in manifest),
        'discovery_categories': dict(Counter(r['category'] for r in rows)),
        'primary_patterns': dict(Counter(r['primary_pattern'] for r in rows)),
    }
    (ROOT / 'counts.json').write_text(json.dumps(counts, indent=2) + '\n')
    lines = [
        '# Evidence matrix: 100 Jev repositories', '',
        'Reviewed September 22, 2026. [Findings and methodology](README.md). '
        '[CSV](projects.csv) · [pinned source manifest](manifest.json) · [manual annotations](annotations.tsv).', '',
        'Observed use and existing visibility are static-review findings from code excerpts and project documentation. '
        'Proposed help is our hypothesis, not a maintainer request or measured benefit. '
        'Each row links its principal implementation evidence and README; the manifest lists additional collected sources. '
        'Patterns describe the main grouping challenge observed, not every question in a repository. '
        'Rows 91–100 are evaluation-oriented projects; row 96 is an artifact/report study. '
        'No applications or benchmarks were run.', '',
    ]
    for category in counts['discovery_categories']:
        lines.extend([f'## {category}', '',
            '| # / project and evidence | Observed Jev use / primary pattern | Existing visibility | Proposed help | Integration limit |',
            '|---|---|---|---|---|'])
        for row in (r for r in rows if r['category'] == category):
            cells = [
                f'{row["id"]}. [{row["repository"]}]({row["readme_url"]}) · [source]({row["evidence_url"]})',
                f'{row["observed_use"]} **{PATTERNS[row["primary_pattern"]]}**.',
                row['existing_visibility'], row['proposed_help'], row['integration_limit'],
            ]
            lines.append('| ' + ' | '.join(c.replace('|', '\\|') for c in cells) + ' |')
        lines.append('')
    (ROOT / 'projects.md').write_text('\n'.join(lines))
    print(json.dumps(counts, indent=2))


if __name__ == '__main__':
    main()
