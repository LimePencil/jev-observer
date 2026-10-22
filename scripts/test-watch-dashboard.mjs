// Browser-monitor authentication regression; synthetic loopback data only.
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { once } from 'node:events';
import fs from 'node:fs/promises';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';

const run = promisify(execFile);
const monitor = fileURLToPath(new URL('./watch-dashboard.mjs', import.meta.url));
const token = 'jo_access_' + 'a'.repeat(64);
const authorization = 'Basic ' + Buffer.from('observer:' + token).toString('base64');

test('dashboard monitor authenticates from a file and keeps credentials out of evidence', async t => {
  const temporary = await fs.mkdtemp(path.join(os.tmpdir(), 'observer-monitor-'));
  t.after(() => fs.rm(temporary, { recursive: true, force: true }));
  // Both spaces and equals signs must survive command argument parsing.
  const tokenPath = path.join(temporary, 'workspace = fixture.access-token');
  await fs.writeFile(tokenPath, token, { mode: 0o600 });
  let snapshots = 0;
  const server = http.createServer((request, response) => {
    if (request.headers.authorization !== authorization) {
      response.writeHead(401, { 'WWW-Authenticate': 'Basic realm="Observer"' });
      response.end('Authentication required');
      return;
    }
    response.setHeader('Cache-Control', 'no-store');
    if (request.url === '/api/health') {
      response.setHeader('Content-Type', 'application/json');
      response.end('{}');
    } else if (request.url === '/api/dashboard') {
      response.setHeader('Content-Type', 'application/json');
      response.end(JSON.stringify({ summary: { request_count: ++snapshots, answer_count: snapshots } }));
    } else {
      response.setHeader('Content-Type', 'text/html');
      response.end('<!doctype html><h1>Request stream</h1><h2>Recurring questions</h2><h2>Request activity</h2><div class="metrics">Synthetic requests</div><script>setInterval(() => fetch("/api/dashboard").then(response => response.json()), 150)</script>');
    }
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  t.after(() => { server.closeAllConnections(); server.close(); });
  const origin = `http://127.0.0.1:${server.address().port}`;
  const output = path.join(temporary, 'browser.json');
  const invoke = extra => run(process.execPath, [monitor, '--origin=' + origin, '--seconds=1', '--output=' + output, ...extra], { timeout: 30_000, maxBuffer: 100_000 });

  await t.test('observes authenticated snapshots', async () => {
    const result = await invoke(['--access-token-file=' + tokenPath]);
    const report = await fs.readFile(output, 'utf8');
    assert.equal(JSON.parse(report).complete, true);
    assert.ok(JSON.parse(report).snapshots > 1);
    for (const secret of [token, authorization]) assert.ok(!(result.stdout + result.stderr + report).includes(secret));
  });

  await t.test('fails clearly when the file is missing or invalid', async () => {
    await assert.rejects(invoke([]), error => error.stderr.includes('Supply --access-token-file=PATH'));
    await fs.writeFile(tokenPath, 'private-invalid-token');
    await assert.rejects(invoke(['--access-token-file=' + tokenPath]), error => {
      assert.ok(!error.stderr.includes('private-invalid-token'));
      return error.stderr.includes('The access-token file is invalid');
    });
    for (const suffix of ['\n', 'x']) {
      await fs.writeFile(tokenPath, token + suffix);
      await assert.rejects(invoke(['--access-token-file=' + tokenPath]), error => error.stderr.includes('The access-token file is invalid'));
    }
  });

  await t.test('rejects a different workspace token without printing it', async () => {
    const wrongToken = 'jo_access_' + 'b'.repeat(64);
    await fs.writeFile(tokenPath, wrongToken);
    await assert.rejects(invoke(['--access-token-file=' + tokenPath]), error => {
      assert.ok(!error.stderr.includes(wrongToken));
      return error.stderr.includes('Observer rejected the access token');
    });
  });
});
