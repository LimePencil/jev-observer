import { expect, test } from '@playwright/test';
import { spawn } from 'node:child_process';
import type { ChildProcess } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { once } from 'node:events';
import { mkdtemp, mkdir, readFile, rm } from 'node:fs/promises';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';

test('live workspace connects, captures, reviews and exports, then preserves history after process restart', async ({ browser }, testInfo) => {
  test.setTimeout(180_000);
  const binary = path.resolve(process.env.OBSERVER_BINARY ?? path.join('..', 'target', 'release', process.platform === 'win32' ? 'jev-observer.exe' : 'jev-observer'));
  const temporary = await mkdtemp(path.join(os.tmpdir(), 'observer-live-browser-'));
  const directory = path.join(temporary, '.jev-observer');
  await mkdir(directory, { mode: 0o700 });
  const providerKey = 'browser-fixture-provider-placeholder';
  const inputMarker = 'private-browser-fixture-input';
  const fixture = JSON.parse(await readFile(path.resolve('../fixtures/sdk/request.json'), 'utf8'));
  fixture.state = { message: inputMarker };
  const answer = JSON.parse(await readFile(path.resolve('../fixtures/sdk/response.json'), 'utf8'));
  answer.credential_echo = providerKey;
  const mockFailures: string[] = [];
  let upstreamCalls = 0;
  const mock = http.createServer((request, response) => {
    const chunks: Buffer[] = [];
    request.on('data', chunk => chunks.push(chunk));
    request.on('end', () => {
      upstreamCalls += 1;
      if (request.url !== '/v1/systemone' || request.headers.authorization !== `Bearer ${providerKey}` || request.headers['accept-encoding'] !== 'identity') mockFailures.push('Unexpected upstream route or headers');
      if (Buffer.concat(chunks).toString() !== JSON.stringify(fixture)) mockFailures.push('Request body changed');
      const body = JSON.stringify(answer);
      response.writeHead(200, { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(body) });
      response.end(body);
    });
  });
  mock.listen(0, '127.0.0.1');
  await once(mock, 'listening');
  const upstream = `http://127.0.0.1:${(mock.address() as net.AddressInfo).port}/v1/systemone`;
  const portProbe = net.createServer();
  portProbe.listen(0, '127.0.0.1');
  await once(portProbe, 'listening');
  const port = (portProbe.address() as net.AddressInfo).port;
  await new Promise<void>(resolve => portProbe.close(() => resolve()));
  const origin = `http://127.0.0.1:${port}`;
  const environment = Object.fromEntries(Object.entries(process.env).filter(([name]) => !['typesafe_api_key', 'http_proxy', 'https_proxy', 'all_proxy'].includes(name.toLowerCase())));
  environment.JEV_OBSERVER_DB_KEY = randomBytes(32).toString('hex');
  environment.NO_PROXY = '127.0.0.1,localhost';
  let processHandle: ChildProcess | undefined;
  let logs = '';
  let completed = false;
  async function stop() {
    if (processHandle && processHandle.exitCode === null && processHandle.pid) {
      const exited = once(processHandle, 'exit');
      processHandle.kill('SIGTERM');
      const timer = setTimeout(() => processHandle?.kill('SIGKILL'), 20_000);
      await exited;
      clearTimeout(timer);
    }
    processHandle = undefined;
  }
  async function start() {
    processHandle = spawn(binary, ['--port', String(port), '--db', path.join(directory, 'history.sqlite'), '--upstream', upstream, '--provider', 'browser-fixture'], { cwd: directory, env: environment, stdio: ['ignore', 'ignore', 'pipe'] });
    let spawnError: Error | undefined;
    processHandle.on('error', error => { spawnError = error; });
    processHandle.stderr?.on('data', data => { logs = (logs + data.toString()).slice(-8000); });
    let token = '';
    await expect.poll(async () => {
      if (spawnError) throw spawnError;
      if (processHandle?.exitCode !== null) throw new Error('Live Observer exited during startup');
      try {
        token = (await readFile(path.join(directory, 'history.access-token'), 'utf8')).trim();
        return (await fetch(origin + '/api/health', { headers: { Authorization: `Basic ${Buffer.from(`observer:${token}`).toString('base64')}` } })).status;
      } catch { return 0; }
    }, { timeout: 30_000 }).toBe(200);
    return token;
  }
  try {
    const accessToken = await start();
    const context = await browser.newContext({ httpCredentials: { username: 'observer', password: accessToken } });
    try {
      const page = await context.newPage();
      const browserErrors: string[] = [];
      page.on('pageerror', error => browserErrors.push(error.message));
      await page.goto(origin);
      await page.getByRole('button', { name: 'Connect an application', exact: true }).click();
      await page.getByLabel('browser-fixture API key', { exact: true }).fill(providerKey);
      await page.getByRole('button', { name: 'Register key', exact: true }).click();
      await expect(page.locator('.credential-token code').filter({ hasText: /^jo_local_/ })).toContainText('jo_local_');
      const localToken = (await page.locator('.credential-token code').filter({ hasText: /^jo_local_/ }).textContent())!;
      const captured = await fetch(origin + '/v1/systemone', { method: 'POST', headers: { Authorization: `Bearer ${localToken}`, 'Content-Type': 'application/json', 'Accept-Encoding': 'identity', 'X-Observer-Source': 'browser-live-fixture' }, body: JSON.stringify(fixture) });
      expect(captured.status).toBe(200);
      expect(await captured.json()).toEqual(answer);
      await expect(page.getByRole('region', { name: 'Connection verification' })).toContainText('1 captures saved', { timeout: 15_000 });
      await page.keyboard.press('Escape');
      const dashboard = await (await context.request.get(origin + '/api/dashboard?window=all')).json();
      expect(dashboard.summary.request_count).toBe(1);
      expect(dashboard.summary.answer_count).toBe(3);
      const id = dashboard.requests[0].id;
      await page.getByLabel(`Inspect request ${id}`, { exact: true }).click();
      await page.getByLabel('Review department', { exact: true }).selectOption('correct');
      await expect.poll(async () => (await (await context.request.get(origin + '/api/requests/' + id)).json()).labels.find((label: { key: string }) => label.key === 'department')?.label).toBe('correct');
      await page.keyboard.press('Escape');
      await page.getByRole('button', { name: 'Export', exact: true }).click();
      const pending = page.waitForEvent('download');
      await page.getByRole('menuitem', { name: 'JSONL records' }).click();
      const download = await pending;
      const saved = await readFile((await download.path())!, 'utf8');
      const record = JSON.parse(saved.trim());
      expect(record.id).toBe(id);
      expect(record.answers.every((item: { valid: boolean }) => item.valid)).toBe(true);
      expect(record.labels[0].label).toBe('correct');
      for (const secret of [providerKey, localToken, accessToken, inputMarker]) expect(saved).not.toContain(secret);
      await stop();
      expect((await readFile(path.join(directory, 'history.sqlite'))).subarray(0, 16).equals(Buffer.from('SQLite format 3\0'))).toBe(false);
      expect(await start()).toBe(accessToken);
      await page.reload();
      await page.getByLabel(`Inspect request ${id}`, { exact: true }).click();
      await expect(page.getByLabel('Review department', { exact: true })).toHaveValue('correct');
      expect((await (await context.request.get(origin + '/api/credentials')).json()).configured).toBe(false);
      expect(upstreamCalls).toBe(1);
      expect(mockFailures).toEqual([]);
      expect(browserErrors).toEqual([]);
      completed = true;
    } finally { await context.close(); }
  } finally {
    await stop();
    mock.closeAllConnections();
    await new Promise<void>(resolve => mock.close(() => resolve()));
    if (!completed) await testInfo.attach('observer-log', { body: logs, contentType: 'text/plain' });
    await rm(temporary, { recursive: true, force: true });
  }
});
