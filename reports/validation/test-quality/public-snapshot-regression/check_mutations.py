#!/usr/bin/env python3
"""Compile exact store tests and prove each public transaction is required.

Only isolated source copies are mutated. Existing Cargo-built dependency rlibs
are reused; the workspace sources and release executable must remain unchanged.
"""
import datetime
import os
import hashlib
import json
import pathlib
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[4]
OUT = ROOT / '.jev-observer/independent-validation-rerun/test-quality/public-snapshot-regression'
OUT.mkdir(parents=True, exist_ok=True)
DEPS = pathlib.Path(os.environ.get('JEV_VALIDATION_DEPS', ROOT / 'target/debug/deps'))
SOURCE = ROOT / 'src/store.rs'
RELEASE = pathlib.Path(os.environ.get('JEV_VALIDATION_CURRENT', ROOT / 'target/release/jev-observer')).resolve()


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


source = SOURCE.read_text()
source_hash = digest(SOURCE)
release_hash = digest(RELEASE)
model = (ROOT / 'src/model.rs').read_bytes()
(OUT / 'src').mkdir(exist_ok=True)
(OUT / 'src/model.rs').write_bytes(model)
(OUT / 'src/dashboard_tests.rs').write_bytes((ROOT / 'src/dashboard_tests.rs').read_bytes())
(OUT / 'fixtures/model').mkdir(parents=True, exist_ok=True)
(OUT / 'fixtures/model/jevrouter-receipt.json').write_bytes(
    (ROOT / 'fixtures/model/jevrouter-receipt.json').read_bytes())

REQUEST = '''    pub fn request(&self, id: &str) -> Result<Option<Value>> {
        let mut conn = self.reader()?;
        let tx = conn.transaction()?;
        Self::request_from(&tx, id)
    }'''
REQUEST_MUTANT = '''    pub fn request(&self, id: &str) -> Result<Option<Value>> {
        let conn = self.reader()?;
        Self::request_from(&conn, id)
    }'''
EXPORT = '''        let mut connection = self.reader()?;
        let conn = connection.transaction()?;'''
SIGNATURES = [
    f"fn {name}(conn: &rusqlite::Transaction<'_>, id: &str)"
    for name in ('request_from', 'request_parent')
]
results = []
for name, remove_request, remove_export, expected_failed in [
    ('current', False, False, 0),
    ('without_request_transaction', True, False, 2),
    ('without_export_transaction', False, True, 3),
    ('without_both_transactions', True, True, 5),
]:
    isolated = source
    if remove_request or remove_export:
        for signature in SIGNATURES:
            assert isolated.count(signature) == 1
            isolated = isolated.replace(signature, signature.replace("&rusqlite::Transaction<'_>", '&Connection'))
    if remove_request:
        assert isolated.count(REQUEST) == 1
        isolated = isolated.replace(REQUEST, REQUEST_MUTANT)
    if remove_export:
        start = isolated.index('    pub fn export_page(')
        end = isolated.index('\n    pub fn export(', start)
        export = isolated[start:end]
        assert export.count(EXPORT) == 1
        isolated = isolated[:start] + export.replace(EXPORT, '        let conn = self.reader()?;') + isolated[end:]
    module = OUT / 'src' / (name + '.rs')
    module.write_text(isolated)
    harness = OUT / (name + '.harness.rs')
    harness.write_text('#![allow(dead_code)]\n'
                       '#[path = "src/model.rs"] mod model;\n'
                       f'#[path = "src/{name}.rs"] mod store;\n')
    binary = OUT / name
    command = ['rustc', '--edition=2024', '--test', '--crate-name', name,
               str(harness), '-o', str(binary), '-L', 'dependency=' + str(DEPS)]
    for crate in ('anyhow', 'chrono', 'rusqlite', 'serde', 'serde_json', 'sha2', 'tempfile'):
        libraries = list(DEPS.glob('lib' + crate + '-*.rlib'))
        assert len(libraries) == 1, libraries
        command += ['--extern', crate + '=' + str(libraries[0])]
    build = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    (OUT / (name + '.build.log')).write_text(build.stdout + build.stderr)
    assert build.returncode == 0, build.stderr
    test_command = [str(binary), 'parent_snapshot', '--nocapture', '--test-threads=5']
    tested = subprocess.run(test_command, cwd=ROOT, capture_output=True, text=True)
    log = tested.stdout + tested.stderr
    (OUT / (name + '.test.log')).write_text(log)
    expected = f'{5 - expected_failed} passed; {expected_failed} failed;'
    assert expected in log, log
    assert tested.returncode == (101 if expected_failed else 0), log
    failed = [line.strip() for line in tested.stdout.splitlines()
              if line.startswith('    store::tests::')]
    result = {
        'variant': name,
        'removed_request_transaction': remove_request,
        'removed_export_transaction': remove_export,
        'source_sha256': digest(module),
        'test_exit_code': tested.returncode,
        'passed': 5 - expected_failed,
        'failed': expected_failed,
        'failed_tests': failed,
        'build_command': command,
        'test_command': test_command,
        'test_log': str((OUT / (name + '.test.log')).relative_to(ROOT)),
    }
    results.append(result)
    print(json.dumps({k: v for k, v in result.items()
                      if k not in ('build_command', 'test_command')}), flush=True)

assert digest(SOURCE) == source_hash, 'Workspace source changed during validation'
assert digest(RELEASE) == release_hash, 'Release executable changed during validation'
manifest = {
    'date': datetime.date.today().isoformat(),
    'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
    'workspace_store_sha256': source_hash,
    'model_sha256': hashlib.sha256(model).hexdigest(),
    'release_sha256_before_and_after': release_hash,
    'workspace_source_unchanged': True,
    'release_binary_unchanged': True,
    'method': 'Exact current store.rs tests run in isolated rustc --test harnesses. Mutants remove only the indicated public transaction wrappers and relax request_from/request_parent arguments to &Connection. No workspace mutations.',
    'results': results,
}
(OUT / 'results.json').write_text(json.dumps(manifest, indent=2) + '\n')
