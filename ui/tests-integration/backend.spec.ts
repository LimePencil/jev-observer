import { expect, test } from '@playwright/test';
import { spawn } from 'node:child_process';
import type { ChildProcess } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, mkdir, readFile, rm } from 'node:fs/promises';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';

// This test serves the executable's embedded UI and uses real backend routes.
// Demo mode guarantees it cannot call a provider. There are no intercepted APIs.
test('packaged dashboard authenticates, saves reviews, imports, and exports real history', async ({ browser }, testInfo) => {
  const binary = path.resolve(process.env.OBSERVER_BINARY ?? path.join('..', 'target', 'release', process.platform === 'win32' ? 'jev-observer.exe' : 'jev-observer'));
  const temporary = await mkdtemp(path.join(os.tmpdir(), 'observer-browser-'));
  const directory = path.join(temporary, '.jev-observer');
  await mkdir(directory, { mode: 0o700 });
  const probe = net.createServer();
  probe.listen(0, '127.0.0.1');
  await once(probe, 'listening');
  const port = (probe.address() as net.AddressInfo).port;
  await new Promise<void>(resolve => probe.close(() => resolve()));
  const origin = `http://127.0.0.1:${port}`;
  let processHandle: ChildProcess | undefined;
  let logs = '';
  let completed = false;
  try {
    const environment = Object.fromEntries(Object.entries(process.env).filter(([name]) => !['typesafe_api_key', 'http_proxy', 'https_proxy', 'all_proxy', 'jev_observer_db_key'].includes(name.toLowerCase())));
    processHandle = spawn(binary, ['--demo', '--port', String(port), '--db', path.join(directory, 'history.sqlite')], { cwd: directory, env: environment, stdio: ['ignore', 'ignore', 'pipe'] });
    let spawnError: Error | undefined;
    processHandle.on('error', error => { spawnError = error; });
    processHandle.stderr?.on('data', data => { logs = (logs + data.toString()).slice(-8000); });
    let token = '';
    await expect.poll(async () => {
      if (spawnError) throw spawnError;
      if (processHandle?.exitCode !== null) throw new Error('Observer exited during startup');
      try {
        token = (await readFile(path.join(directory, 'history.demo.access-token'), 'utf8')).trim();
        const response = await fetch(origin + '/api/health', { headers: { Authorization: `Basic ${Buffer.from(`observer:${token}`).toString('base64')}` } });
        return response.status;
      } catch { return 0; }
    }, { timeout: 30_000 }).toBe(200);
    expect((await fetch(origin + '/api/dashboard')).status).toBe(401);
    const context = await browser.newContext({ httpCredentials: { username: 'observer', password: token } });
    try {
      const page = await context.newPage();
      const errors: string[] = [];
      page.on('pageerror', error => errors.push(error.message));
      await page.goto(origin + '/?window=all');
      await expect(page.getByRole('heading', { name: 'Request stream' })).toBeVisible();
      await expect(page.locator('.sample-notice')).toContainText('forwarding is disabled');
      const initial = await (await context.request.get(origin + '/api/dashboard?window=all')).json();
      expect(initial.summary.request_count).toBe(720);
      const firstId = initial.requests[0].id;
      await page.getByRole('button', { name: 'Older requests', exact: true }).click();
      await expect(page.getByLabel(`Inspect request ${firstId}`, { exact: true })).toHaveCount(0);
      await page.getByRole('button', { name: 'Newer requests', exact: true }).click();
      await expect(page.getByLabel(`Inspect request ${firstId}`, { exact: true })).toBeVisible();
      const row = initial.requests.slice(0, 12).find((record: { status: number }) => record.status === 200);
      expect(row).toBeTruthy();
      const detail = await (await context.request.get(origin + '/api/requests/' + row.id)).json();
      const question = detail.answers[0].key;
      await page.getByLabel(`Inspect request ${row.id}`, { exact: true }).click();
      await page.getByLabel(`Review ${question}`, { exact: true }).selectOption('incorrect');
      await expect.poll(async () => {
        const saved = await (await context.request.get(origin + '/api/requests/' + row.id)).json();
        return saved.labels.find((label: { key: string }) => label.key === question)?.label;
      }).toBe('incorrect');
      await page.keyboard.press('Escape');
      await page.reload();
      await page.getByLabel(`Inspect request ${row.id}`, { exact: true }).click();
      await expect(page.getByLabel(`Review ${question}`, { exact: true })).toHaveValue('incorrect');
      await page.keyboard.press('Escape');
      const imported = { ...detail, id: 'browser-integration-import', source: 'browser-integration', source_event_id: 'browser-integration-import', timestamp: Date.now(), sample: false, labels: [] };
      await page.getByRole('button', { name: 'Import records', exact: true }).click();
      await page.getByLabel('Or paste records').fill(JSON.stringify(imported));
      const importButton = page.getByRole('dialog').getByRole('button', { name: 'Import records', exact: true });
      await importButton.click();
      await expect(page.getByRole('dialog').getByRole('status')).toContainText('1 records imported');
      await importButton.click();
      await expect(page.getByRole('dialog').getByRole('status')).toContainText('1 duplicates skipped');
      await page.keyboard.press('Escape');
      const after = await (await context.request.get(origin + '/api/dashboard?window=all')).json();
      expect(after.summary.request_count).toBe(721);
      for (const [label, extension] of [['JSONL records', 'jsonl'], ['CSV spreadsheet', 'csv']] as const) {
        await page.getByRole('button', { name: 'Export', exact: true }).click();
        const pending = page.waitForEvent('download');
        await page.getByRole('menuitem', { name: label }).click();
        const download = await pending;
        expect(download.suggestedFilename()).toBe(`observer.${extension}`);
        const downloaded = await download.path();
        expect(downloaded).not.toBeNull();
        const contents = await readFile(downloaded!, 'utf8');
        expect(contents).toContain('browser-integration-import');
        expect(contents).not.toContain(token);
        if (extension === 'jsonl') expect(contents.trim().split('\n')).toHaveLength(721);
        else expect(contents).toMatch(/^id,timestamp,source,model,status,/);
      }
      const settings = await (await context.request.get(origin + '/api/settings')).json();
      expect(settings.model_catalog.models.length).toBe(38);
      await page.getByRole('button', { name: 'Connect an application', exact: true }).click();
      await expect(page.getByLabel('Model integration')).toBeVisible();
      await page.getByLabel('Model integration').selectOption('vercel');
      await expect(page.getByLabel('Observer startup command')).toContainText('/typesafe/v1/systemone');
      await page.getByLabel('Model integration').selectOption('kev');
      await expect(page.getByLabel('Observer startup command')).toContainText('--provider kev --upstream-auth none');
      await page.getByLabel('Model integration').selectOption('nanojev');
      await expect(page.getByLabel('Observer startup command')).toHaveCount(0);
      await page.getByRole('button', { name: 'Import mapped decisions' }).click();
      await expect(page.getByLabel('Source format')).toHaveValue('systemone-capture');
      const captured = {
        id: 'nanojev-browser-call', timestamp: Date.now(), source: 'NanoJev integration', provider: 'nanojev',
        status: 200, duration_ms: null, capture_complete: true, sample: true,
        request: { model: 'requested-checkpoint', state: 'omitted-private-state', images: ['omitted-private-image'], questions: {
          route: { type: 'choice', instructions: 'Route', criteria: { a: 'A', b: 'B' } },
          check: { type: 'noul', instructions: 'Check' },
        } },
        response: { model: 'nanojev-revision', answers: {
          route: { type: 'choice', choice: 'b', probabilities: { a: .2, b: .8 } },
          check: { type: 'noul', noul: .7 },
        } },
      };
      await page.getByLabel('Or paste records').fill(JSON.stringify(captured));
      await page.getByRole('dialog').getByRole('button', { name: 'Import records', exact: true }).click();
      await expect(page.getByRole('dialog').getByRole('status')).toContainText('1 records imported');
      await page.getByRole('dialog').getByRole('button', { name: 'Import records', exact: true }).click();
      await expect(page.getByRole('dialog').getByRole('status')).toContainText('1 duplicates skipped');
      await page.keyboard.press('Escape');
      await page.getByLabel('Filter by model').selectOption('nanojev-revision');
      await expect(page.locator('.request-link')).toHaveCount(1);
      await page.locator('.request-link').click();
      await expect(page.getByRole('dialog')).toContainText('Confidence was not reported; it remains unknown');
      const scoped = await (await context.request.get(origin + '/api/dashboard?window=all&model=nanojev-revision')).json();
      expect(scoped.summary.request_count).toBe(1);
      expect(scoped.summary.answer_count).toBe(2);
      expect(scoped.summary.cost_usd).toBeNull();
      const mappedDetail = await (await context.request.get(origin + '/api/requests/' + scoped.requests[0].id)).json();
      expect(mappedDetail.answers.every((answer: { valid: boolean }) => answer.valid)).toBe(true);
      expect(mappedDetail.provider).toBe('nanojev');
      expect(mappedDetail.requested_model).toBe('requested-checkpoint');
      expect(mappedDetail.sample).toBe(true);
      expect(JSON.stringify(mappedDetail)).not.toContain('omitted-private');
      const mappedExport = await (await context.request.get(origin + '/api/export?format=jsonl&window=all&model=nanojev-revision')).text();
      const mappedReimport = await context.request.post(origin + '/api/import', { headers: { 'X-Observer-Request': '1' }, data: { text: mappedExport, format: 'observer-jsonl' } });
      expect(mappedReimport.status()).toBe(200);
      expect((await mappedReimport.json()).duplicates).toBe(1);
      expect((await (await context.request.get(origin + '/api/health')).json()).forwarded).toBe(0);
      expect(errors).toEqual([]);
      completed = true;
    } finally {
      await context.close();
    }
  } finally {
    if (processHandle && processHandle.exitCode === null && processHandle.pid) {
      const exited = once(processHandle, 'exit');
      processHandle.kill('SIGTERM');
      const timer = setTimeout(() => processHandle?.kill('SIGKILL'), 20_000);
      await exited;
      clearTimeout(timer);
    }
    if (!completed) await testInfo.attach('observer-log', { body: logs, contentType: 'text/plain' });
    await rm(temporary, { recursive: true, force: true });
  }
});
