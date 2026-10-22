#!/usr/bin/env python3
"""Pure preparation controls: no subprocesses, sockets, services or key reads."""
import ast
import contextlib
import copy
import hashlib
import importlib.util
import io
import json
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parent


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main():
    checks = {}
    pins = json.loads((ROOT / 'source-pins.json').read_text())
    plan = json.loads((ROOT / 'plan.json').read_text())
    # Pure report controls need a nonempty stand-in before the real binary exists.
    if pins['binary_sha256'] is None:
        pins['binary_sha256'] = 'unbound-self-check-placeholder'
    checks['pending_plan_blocks_launch'] = plan['status'] != 'ready_to_launch'
    for path, pin in pins['copies'].items():
        checks['pin_' + str(Path(path).relative_to(ROOT))] = hashlib.sha256(Path(path).read_bytes()).hexdigest() == pin['sha256']
        if path.endswith('.py'):
            ast.parse(Path(path).read_text(), filename=path)
    checks['all_python_syntax'] = True
    mixed_path = ROOT / 'mixed/harness.py'
    original = Path(pins['copies'][str(mixed_path)]['source']).read_text()
    checks['mixed_exact_prior_copy'] = mixed_path.read_text() == original
    checks['mixed_one_token_expression'] = mixed_path.read_text().count("token_path = self.directory / 'audit.sqlite.token'") == 1
    helper_tree = ast.parse((ROOT / 'retention/scripts/test-upgrade.py').read_text())
    assignment = next(node for node in ast.walk(helper_tree) if isinstance(node, ast.Assign)
                      and any(isinstance(target, ast.Attribute) and target.attr == 'token_path' for target in node.targets))
    expression = compile(ast.Expression(assignment.value), '<token-path>', 'eval')
    for filename in ('history.sqlite', 'history.db', 'history', 'ends.in.dot.'):
        path = Path('/owned') / filename
        token = eval(expression, {'Path': Path, 'self': SimpleNamespace(database=path), 'legacy_token': False})
        checks['retention_full_suffix_' + filename] = token == Path(str(path) + '.token')
    mixed = load('mixed_copy', mixed_path)
    with contextlib.redirect_stdout(io.StringIO()) as output:
        mixed.self_check()
    checks['mixed_fixture_self_check'] = json.loads(output.getvalue())['self_check'] == 'passed'
    runner = load('continuation_runner', ROOT / 'run_continuation.py')
    limits = plan['limits']
    sample = {'available_kib': limits['minimum_available_kib'],
              'group_rss_kib': limits['maximum_group_rss_kib'],
              'workspace_bytes': limits['maximum_workspace_bytes']}
    runner.limits_check(sample, limits['seconds'], limits)
    checks['exact_resource_boundaries_accepted'] = True
    for field, value, expected in [
        ('available_kib', sample['available_kib'] - 1, 'host_memory_guard'),
        ('group_rss_kib', sample['group_rss_kib'] + 1, 'owned_memory_guard'),
        ('workspace_bytes', sample['workspace_bytes'] + 1, 'owned_storage_guard'),
        ('seconds', limits['seconds'] + 1, 'work_deadline'),
    ]:
        changed = {**sample, field: value}
        try:
            runner.limits_check(changed, value if field == 'seconds' else 0, limits)
        except runner.GuardFailure as error:
            checks['reject_' + field] = str(error) == expected
        else:
            checks['reject_' + field] = False
    for kind, run in plan['runs'].items():
        report = {'passed': True, 'binary_sha256': pins['binary_sha256'],
                  'harness_sha256': pins['copies'][run['harness']]['sha256'],
                  'cycles': [{} for _ in range(run['cycles'])]}
        if kind == 'retention':
            report.update(helper_sha256=pins['copies'][run['helper']]['sha256'], total_offered=run['expected']['live_offers'])
            report['cycles'] = [{'restarted': (index + 1) % 5 == 0, 'retained': 1000} for index in range(run['cycles'])]
        else:
            report.update(process_exits=[0], owned_workspace_removed=True)
            report['cycles'][run['expected']['same_database_midpoint_restart_after_cycle'] - 1]['restart'] = {'canonical_equal': True}
        runner.validate_report(report, run, pins)
        checks[kind + '_complete_report_accepted'] = True
        if kind == 'retention':
            bad = copy.deepcopy(report)
            next(item for item in bad['cycles'] if item['restarted'])['restarted'] = False
            try:
                runner.validate_report(bad, run, pins)
            except runner.GuardFailure as error:
                checks['retention_missing_restart_rejected'] = str(error) == 'retention_expected_restarts'
            else:
                checks['retention_missing_restart_rejected'] = False
        for change in ('passed', 'cycles', 'binary_sha256'):
            bad = copy.deepcopy(report)
            bad[change] = False if change == 'passed' else bad[change][:-1]
            try:
                runner.validate_report(bad, run, pins)
            except runner.GuardFailure:
                checks[kind + '_reject_' + change] = True
            else:
                checks[kind + '_reject_' + change] = False
    result = {'passed': all(checks.values()), 'checks': checks, 'native_runtime': False,
              'scope': 'Pure module/AST/fixture/guard-oracle controls; no process/signal cleanup runtime exercised.'}
    with (ROOT / 'self-check.json').open('x') as stream:
        json.dump(result, stream, indent=2)
        stream.write('\n')
    print(json.dumps({'passed': result['passed'], 'checks': len(checks)}))
    assert result['passed']


if __name__ == '__main__':
    main()
