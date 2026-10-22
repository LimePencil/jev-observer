import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { test } from 'node:test';

const moduleUrl = new URL('./stress-cargo.mjs', import.meta.url).href;
const artifact = executable => ({ reason: 'compiler-artifact', target: { name: 'hold_db_lock', kind: ['example'] }, executable, fresh: true });

async function fixture(run) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'observer-stress-cargo-'));
  try {
    const repository = path.join(root, 'repository');
    const bin = path.join(root, 'fake-bin');
    const marker = path.join(root, 'helper-ran');
    await fs.mkdir(repository); await fs.mkdir(bin);
    const cargo = path.join(bin, 'cargo');
    await fs.writeFile(cargo, `#!${process.execPath}
const fs = require('node:fs');
const args = process.argv.slice(2);
if (process.cwd() !== process.env.FAKE_REPOSITORY || JSON.stringify(args) !== JSON.stringify(['build','--release','--locked','--example','hold_db_lock','--message-format=json-render-diagnostics'])) process.exit(91);
process.stderr.write('synthetic compiler diagnostic\\n');
process.stdout.write(process.env.FAKE_CARGO_OUTPUT);
process.exit(Number(process.env.FAKE_CARGO_EXIT));
`, { mode: 0o700 });
    const env = { ...process.env, PATH: `${bin}${path.delimiter}${process.env.PATH}`, FAKE_REPOSITORY: repository, FAKE_CARGO_EXIT: '0' };
    delete env.CARGO_TARGET_DIR;
    const invoke = output => spawnSync(process.execPath, ['--input-type=module', '-e', `
import { buildLockHelper } from ${JSON.stringify(moduleUrl)};
import { execFileSync } from 'node:child_process';
try {
  const executable = buildLockHelper(process.env.FAKE_REPOSITORY);
  execFileSync(executable, [${JSON.stringify(marker)}], {timeout:1000});
  console.log(JSON.stringify({executable}));
} catch (error) { console.error(error.message); process.exitCode=1; }
`], { env: { ...env, FAKE_CARGO_OUTPUT: output }, encoding: 'utf8', timeout: 5000 });
    const helper = async executable => {
      await fs.mkdir(path.dirname(executable), { recursive: true });
      await fs.writeFile(executable, `#!${process.execPath}\nrequire('node:fs').writeFileSync(process.argv[2], 'discovered helper');\n`, { mode: 0o700 });
    };
    await run({ root, repository, marker, env, invoke, helper });
  } finally { await fs.rm(root, { recursive: true, force: true }); }
}

for (const kind of ['default', 'absolute custom', 'relative custom', 'configured target triple']) {
  test(`launches the reported executable: ${kind}`, async () => fixture(async f => {
    let target = path.join(f.repository, 'target');
    if (kind === 'absolute custom') f.env.CARGO_TARGET_DIR = target = path.join(f.root, 'custom target');
    if (kind === 'relative custom') {
      f.env.CARGO_TARGET_DIR = '../relative target';
      target = path.resolve(f.repository, f.env.CARGO_TARGET_DIR);
    }
    if (kind === 'configured target triple') target = path.join(f.root, 'configured', 'synthetic-target');
    const executable = path.join(target, 'release', 'examples', 'hold_db_lock');
    await f.helper(executable);
    const ignored = { ...artifact('/unrelated/not-launched'), target: { name: 'other_example', kind: ['example'] } };
    const result = f.invoke([ignored, artifact(executable), { reason: 'build-finished', success: true }].map(JSON.stringify).join('\n') + '\n');
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /synthetic compiler diagnostic/);
    assert.equal(JSON.parse(result.stdout).executable, executable);
    assert.equal(await fs.readFile(f.marker, 'utf8'), 'discovered helper');
  }));
}

for (const [name, output, reason] of [
  ['missing artifact', '', /did not report/],
  ['wrong target kind', JSON.stringify({ ...artifact('/unused'), target: { name: 'hold_db_lock', kind: ['lib'] } }), /did not report/],
  ['malformed JSON', '{not JSON}', /malformed build metadata/],
  ['missing executable', JSON.stringify(artifact(null)), /invalid lock-helper executable path/],
  ['relative executable', JSON.stringify(artifact('target/release/examples/hold_db_lock')), /invalid lock-helper executable path/],
  ['NUL executable', JSON.stringify(artifact('/invalid\0path')), /invalid lock-helper executable path/],
  ['duplicate artifact', `${JSON.stringify(artifact('/unused'))}\n${JSON.stringify(artifact('/unused'))}`, /multiple lock-helper executables/],
]) {
  test(`rejects ${name} before launching a helper`, async () => fixture(async f => {
    const result = f.invoke(output);
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, reason);
    await assert.rejects(fs.stat(f.marker), { code: 'ENOENT' });
  }));
}

test('a failed build cannot supply a usable executable, and diagnostics remain visible', async () => fixture(async f => {
  const executable = path.join(f.root, 'otherwise-valid-helper');
  await f.helper(executable);
  f.env.FAKE_CARGO_EXIT = '7';
  const result = f.invoke(JSON.stringify(artifact(executable)));
  assert.equal(result.status, 1, result.stderr);
  assert.match(result.stderr, /synthetic compiler diagnostic/);
  assert.match(result.stderr, /Command failed/);
  await assert.rejects(fs.stat(f.marker), { code: 'ENOENT' });
}));
