import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';
import type { CredentialStatus, Dashboard, Group, RequestRecord, Settings } from '../src/types';
import AxeBuilder from '@axe-core/playwright';

// These records are deliberately synthetic test fixtures. The application itself
// always reads the real local API and never swaps in a mock data source.
const now = Date.now();
const group: Group = { id: 'group_v1_1111111111', name: 'department', key: 'department', kind: 'choice', source: 'Test inbox', definition_id: 'def_v1_1111111111', presentation_id: 'presentation_v1_1111111111', definition: { type: 'choice', instructions: 'Choose a department', criteria: { technical: 'Bugs', billing: 'Invoices' } }, task_version: null, family_id: null, family_name: null, adapter: null, answer_count: 2, valid_count: 1, request_count: 2, last_seen: now, version_count: 2, distribution: [{ label: 'technical', count: 1 }], mean_value: null, mean_confidence: null };
const record: RequestRecord = { id: 'request-one', timestamp: now, source: 'Test inbox', provider: 'typesafe', model: 'test-model', requested_model: 'test-model', status: 200, duration_ms: 124, input_tokens: 100, output_tokens: 20, cost_usd: null, cost_basis: null, answer_count: 1, capture_complete: true, sample: false, state_retained: false, state: null, transport_error: null, source_event_id: null, import_format: null, actions: [], labels: [], answers: [{ key: 'department', kind: 'choice', group_id: group.id, definition_id: group.definition_id, presentation_id: group.presentation_id, candidate_id: null, task_version: null, definition: group.definition, value: 'technical', probabilities: { technical: .8, billing: .2 }, confidence: null, valid: true, error: null, family_id: null, family_name: null, adapter: null, instance_ref: null, mapping_reason: null, raw_answer: {} }] };

async function harness(page: Page, options: { demo?: boolean } = {}) {
  const state = {
    record: structuredClone(record),
    dashboard: { generated_at: now, sample: false, sources: ['Test inbox'], models: ['test-model'], summary: { request_count: 2, answer_count: 2, error_count: 1, p50_ms: 124, p95_ms: 124, input_tokens: 100, output_tokens: 20, cost_usd: null, cost_known_requests: 0 }, timeline: [{ timestamp: now, requests: 2, errors: 1, mean_latency_ms: 124, cost_usd: null }], groups: [structuredClone(group)], requests: [structuredClone(record), { ...structuredClone(record), id: 'request-failed', status: 500, model: null, input_tokens: null, output_tokens: null, cost_usd: null }], health: { forwarded: 2, captured: 2, persisted: 2, dropped: 0, truncated: 0, write_failures: 0, queue_depth: 0, queued_bytes: 0, last_persisted_at: now, last_gap_at: null, lag_ms: 0, capture_limit: 262144, capture_slots: 256 } } as Dashboard,
    deleted: false, importError: false, dashboardError: false, labelError: false,
    labelPending: null as Promise<void> | null, labelHeaders: null as Record<string, string> | null,
    dashboardPending: null as Promise<void> | null, deletePending: null as Promise<void> | null, importDashboard: null as Dashboard | null,
    historicalDashboard: null as Dashboard | null,
    credentials: { configured: false, storage: 'none' } as CredentialStatus,
    providerKey: '', clientToken: '',
  };
  // Export opens a new tab; its first request belongs to the browser context.
  await page.context().route('**/api/**', async route => {
    const request = route.request(), url = new URL(request.url());
    let body: unknown;
    if (url.pathname === '/api/dashboard') {
      await state.dashboardPending;
      if (state.dashboardError) return route.fulfill({ status: 503, json: { error: 'Storage unavailable' } });
      body = structuredClone(url.searchParams.get('window') === 'all' && state.historicalDashboard ? state.historicalDashboard : state.dashboard);
      if (url.searchParams.get('status') === 'error') { const data = body as Dashboard; data.requests = data.requests.filter(item => item.status != null && item.status >= 400); }
    } else if (url.pathname === '/api/health') body = state.dashboard.health;
    else if (url.pathname === '/api/settings') body = { demo: options.demo ?? false, capture_state: false, retention_days: 7, max_records: 1000000, capture_limit: 262144, upstream: 'https://api.typesafe.ai/v1/systemone', version: 'test' } satisfies Settings;
    else if (url.pathname === '/api/credentials') {
      if (request.method() === 'PUT') {
        const payload = request.postDataJSON();
        state.providerKey = payload.api_key;
        state.clientToken = 'jo_local_' + 'a'.repeat(64);
        state.credentials = { configured: true, storage: payload.persist ? 'system' : 'session' };
        body = { client_token: state.clientToken, storage: state.credentials.storage };
      } else if (request.method() === 'DELETE') {
        state.providerKey = ''; state.clientToken = '';
        state.credentials = { configured: false, storage: 'none' };
        body = { ok: true };
      } else body = state.credentials;
    }
    else if (url.pathname.endsWith('/label')) { await state.labelPending; if (state.labelError) return route.fulfill({ status: 503, json: { error: 'Review could not be stored' } }); const payload = request.postDataJSON(); state.record.labels = [{ key: payload.key, label: payload.label }]; state.labelHeaders = request.headers(); body = { ok: true }; }
    else if (url.pathname.startsWith('/api/requests/')) body = state.record;
    else if (url.pathname.startsWith('/api/groups/')) body = { group, versions: [group, { ...group, id: 'group_v1_2222222222', definition_id: 'def_v1_2222222222', presentation_id: 'presentation_v1_2222222222', definition: { instructions: 'Changed department rules' } }], timeline: state.dashboard.timeline, requests: state.dashboard.requests, answers: [], detail_limit: 100, total_answers: 2 };
    else if (url.pathname === '/api/import') { if (state.importError) return route.fulfill({ status: 400, json: { error: 'Invalid source record' } }); if (state.importDashboard) state.dashboard = structuredClone(state.importDashboard); body = { imported: 2, duplicates: 1 }; }
    else if (url.pathname === '/api/data') { await state.deletePending; state.deleted = true; state.dashboard = { ...state.dashboard, sources: [], models: [], summary: { ...state.dashboard.summary, request_count: 0, answer_count: 0, error_count: 0 }, requests: [], groups: [], timeline: [] }; body = { ok: true }; }
    else if (url.pathname === '/api/export') return route.fulfill({ status: 200, contentType: 'text/csv', headers: { 'Content-Disposition': 'attachment; filename="observer-test.csv"' }, body: 'id,status\nrequest-one,200\n' });
    else return route.fulfill({ status: 404, json: { error: 'Unknown test endpoint' } });
    return route.fulfill({ json: body });
  });
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Request stream' })).toBeVisible();
  return state;
}

test('keeps all three capabilities together and unknown cost explicit', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await harness(page);
  await expect(page.getByRole('heading', { name: 'Request activity' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Recurring questions' })).toBeVisible();
  await expect(page.locator('.metrics')).toContainText('Unknown');
  await expect(page.locator('.metrics')).toContainText('0 / 2 requests covered');
  await page.getByRole('button', { name: 'Failures', exact: true }).click();
  await expect(page.locator('.request-link')).toHaveCount(1);
  await expect(page.locator('.request-table')).toContainText('500');
  expect(errors).toEqual([]);
});

test('registers a provider key and shows only the local client token', async ({ page }) => {
  const state = await harness(page);
  await page.getByRole('button', { name: 'Connect an application' }).click();
  await expect(page.getByText('No key registered in Observer.')).toBeVisible();
  await page.getByLabel('TypeSafe API key').fill('invalid key');
  await expect(page.getByRole('button', { name: 'Register key' })).toBeDisabled();
  await expect(page.getByText('Use a key with printable ASCII characters and no spaces.')).toBeVisible();
  await page.getByLabel('TypeSafe API key').fill('provider-secret');
  await page.getByRole('button', { name: 'Register key' }).click();
  await expect(page.getByText('Copy this local client token now.')).toBeVisible();
  expect(state.providerKey).toBe('provider-secret');
  await expect(page.getByRole('dialog')).not.toContainText('provider-secret');
  await expect(page.getByRole('dialog')).toContainText(state.clientToken);
  await page.getByRole('button', { name: 'Remove key' }).click();
  await page.getByRole('button', { name: 'Confirm removal' }).click();
  await expect(page.getByText('No key registered in Observer.')).toBeVisible();
  await expect(page.getByRole('dialog')).not.toContainText('jo_local_');
});

test('sample mode explains how to start live collection without offering an unusable key form', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await harness(page, { demo: true });
  await expect(page.locator('.sample-notice')).toContainText('forwarding is disabled');
  await expect(page.getByRole('button', { name: 'See live setup' })).toBeVisible();
  await page.getByRole('button', { name: 'See live setup' }).click();
  await expect(page.getByRole('heading', { name: 'Start live collection.' })).toBeVisible();
  await expect(page.getByRole('dialog')).toContainText('JEV_OBSERVER_DB_KEY');
  await expect(page.getByLabel('TypeSafe API key')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Copy local base URL' })).toHaveCount(0);
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Open navigation' }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(page.getByRole('dialog')).toContainText('Synthetic records will be recreated');
});

test('an empty recent window points to retained older history', async ({ page }) => {
  const state = await harness(page);
  state.historicalDashboard = structuredClone(state.dashboard);
  state.dashboard.requests = [];
  state.dashboard.summary.request_count = 0;
  await page.getByRole('button', { name: 'Refresh dashboard' }).click();
  await expect(page.getByRole('heading', { name: 'No requests in this window' })).toBeVisible();
  await page.getByRole('button', { name: 'Show all history' }).click();
  await expect(page.getByLabel('Time window')).toHaveValue('all');
  await expect(page.getByLabel('Inspect request request-one')).toBeVisible();
});

test('pause holds the view while collecting new snapshots, then resumes', async ({ page }) => {
  const state = await harness(page);
  await page.getByRole('button', { name: 'Live updates', exact: true }).click();
  state.dashboard.summary.request_count = 3;
  state.dashboard.health.persisted = 3;
  state.dashboard.requests.unshift({ ...record, id: 'new-request' });
  await expect(page.locator('.pause-notice')).toContainText('1 newer records available');
  await expect(page.getByLabel('Inspect request new-request')).toHaveCount(0);
  await page.getByRole('button', { name: 'Resume live', exact: true }).click();
  await expect(page.getByLabel('Inspect request new-request')).toBeVisible();
});

for (const { label, value, catalog } of [
  { label: 'Filter by source', value: 'Test inbox', catalog: 'sources' as const },
  { label: 'Filter by model', value: 'test-model', catalog: 'models' as const },
]) {
  test(`${label} keeps its selected value during loading, failure, and catalog changes`, async ({ page }) => {
    await page.clock.install({ time: new Date(now) });
    await page.clock.pauseAt(new Date(now + 1000));
    const state = await harness(page);
    let release!: () => void;
    state.dashboardPending = new Promise(resolve => { release = resolve; });
    state.dashboardError = true;
    const filter = page.getByLabel(label);
    await filter.selectOption(value);
    await expect(page.getByRole('status')).toContainText('Loading your workspace');
    await expect(filter).toHaveValue(value);

    release();
    await expect(page.getByRole('alert')).toContainText('Storage unavailable');
    await expect(filter).toHaveValue(value);
    state.dashboardError = false;
    state.dashboardPending = null;
    state.dashboard[catalog] = [];
    state.dashboard.requests = [];
    await page.getByRole('button', { name: 'Retry', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'No requests match these filters' })).toBeVisible();
    await expect(filter).toHaveValue(value);
    await expect(filter.locator('option:checked')).toHaveText(value);
    await expect(page.getByRole('alert')).toHaveCount(0);
    await page.getByRole('button', { name: 'Clear filters', exact: true }).first().click();
    await expect(filter).toHaveValue('');
  });
}

for (const { name, latency, delay, failed } of [
  { name: 'counts query time toward the one-second refresh cadence', latency: 600, delay: 400, failed: false },
  { name: 'keeps slow refreshes sequential with a brief idle period', latency: 1600, delay: 100, failed: false },
  { name: 'keeps a full retry backoff after a delayed refresh failure', latency: 600, delay: 1000, failed: true },
]) {
  test(name, async ({ page }) => {
    await page.clock.install({ time: new Date(now) });
    await page.clock.pauseAt(new Date(now + 1000));
    const state = await harness(page);
    state.dashboard.requests.unshift({ ...record, id: 'after-delayed-refresh' });
    let release!: () => void;
    const pending = new Promise<void>(resolve => { release = resolve; });
    let started = 0;
    await page.route('**/api/dashboard?**', async route => {
      started += 1;
      await pending;
      await route.fulfill(failed
        ? { status: 503, json: { error: 'Storage unavailable' } }
        : { json: state.dashboard });
    });

    await page.getByRole('button', { name: 'Refresh dashboard' }).click();
    await expect.poll(() => started).toBe(1);
    await page.clock.runFor(latency);
    expect(started, 'A pending snapshot must not start another query').toBe(1);
    release();
    if (failed) await expect(page.getByRole('alert')).toContainText('Storage unavailable');
    else await expect(page.getByLabel('Inspect request after-delayed-refresh')).toBeVisible();

    await page.clock.runFor(delay - 1);
    expect(started, 'The next query must respect the remaining delay').toBe(1);
    await page.clock.runFor(1);
    await expect.poll(() => started).toBe(2);
  });
}

test('request labels persist through API and versions remain separate', async ({ page }) => {
  const state = await harness(page);
  await page.getByLabel('Inspect request request-one', { exact: true }).click();
  await page.getByLabel('Review department').selectOption('incorrect');
  await expect(page.getByLabel('Review department')).toHaveValue('incorrect');
  expect(state.labelHeaders?.['x-observer-request']).toBe('1');
  await expect(page.getByText('Not collected. An API response')).toBeVisible();
  await page.getByRole('button', { name: 'View recurring group' }).click();
  await page.getByLabel('Compare with').selectOption('group_v1_2222222222');
  await expect(page.getByText('Changed department rules')).toBeVisible();
  await expect(page.getByText('Presentation: 2222222222')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
});

test('storage failures leave independent capture gaps visible', async ({ page }) => {
  const state = await harness(page);
  state.dashboardError = true;
  state.dashboard.health.dropped = 14;
  state.dashboard.health.write_failures = 1;
  await expect(page.getByRole('alert')).toContainText('Storage unavailable');
  await expect(page.getByRole('status')).toContainText('14 dropped');
  await expect(page.getByRole('heading', { name: 'Request stream' })).toBeVisible();
  await expect(page.locator('.health-panel')).toContainText('History gaps detected');
});

test('a failed health refresh falls back to newer dashboard gap counters', async ({ page }) => {
  await page.clock.install({ time: new Date(now) });
  await page.clock.pauseAt(new Date(now + 1000));
  const state = await harness(page);
  let healthFailures = 0;
  let healthUnavailable = true;
  await page.route('**/api/health', async route => {
    if (healthUnavailable) {
      healthFailures += 1;
      await route.fulfill({ status: 503, json: { error: 'Temporarily unavailable' } });
    } else await route.fulfill({ json: state.dashboard.health });
  });
  state.dashboard.health.dropped = 14;
  state.dashboard.health.write_failures = 1;
  await page.getByRole('button', { name: 'Refresh dashboard' }).click();
  await page.clock.runFor(1000);
  await expect.poll(() => healthFailures).toBeGreaterThan(0);
  await expect(page.locator('.health-panel')).toContainText('History gaps detected');
  await expect(page.locator('.health-fields').getByText('14', { exact: true })).toBeVisible();
  await expect(page.getByRole('alert')).toHaveCount(0);

  state.dashboardError = true;
  state.dashboard.health.dropped = 17;
  healthUnavailable = false;
  await page.clock.runFor(1000);
  await expect(page.getByRole('alert')).toContainText('Storage unavailable');
  await expect(page.locator('.health-fields').getByText('17', { exact: true })).toBeVisible();
  healthUnavailable = true;
  const previousFailures = healthFailures;
  await page.clock.runFor(1000);
  await expect.poll(() => healthFailures).toBeGreaterThan(previousFailures);
  await expect(page.locator('.health-fields').getByText('17', { exact: true })).toBeVisible();
});

for (const delayed of ['dashboard', 'health', 'previous process'] as const) {
  test(`a delayed ${delayed} response cannot hide newer collection gaps`, async ({ page }) => {
    await page.clock.install({ time: new Date(now) });
    await page.clock.pauseAt(new Date(now + 1000));
    const state = await harness(page);
    const oldHealth = { ...state.dashboard.health, process_id: 'old-process', sample_sequence: 20 };
    const currentHealth = { ...oldHealth, dropped: 17, process_id: delayed === 'previous process' ? 'new-process' : 'old-process', sample_sequence: delayed === 'previous process' ? 1 : 21 };
    let release!: () => void;
    const pending = new Promise<void>(resolve => { release = resolve; });
    let delayedRequests = 0;
    if (delayed === 'health') {
      await page.route('**/api/health', async route => {
        delayedRequests += 1;
        await pending;
        await route.fulfill({ json: oldHealth });
      });
      await page.clock.runFor(1000);
      await expect.poll(() => delayedRequests).toBe(1);
      state.dashboard.health = currentHealth;
      await page.getByRole('button', { name: 'Refresh dashboard' }).click();
    } else {
      const oldDashboard = { ...structuredClone(state.dashboard), health: oldHealth };
      await page.route('**/api/dashboard?**', async route => {
        delayedRequests += 1;
        await pending;
        await route.fulfill({ json: oldDashboard });
      });
      await page.getByRole('button', { name: 'Refresh dashboard' }).click();
      await expect.poll(() => delayedRequests).toBe(1);
      await page.route('**/api/health', route => route.fulfill({ json: currentHealth }));
      await page.clock.runFor(1000);
    }
    await expect(page.locator('.health-fields').getByText('17', { exact: true })).toBeVisible();
    await page.route('**/api/health', route => route.fulfill({ status: 503, json: { error: 'Temporarily unavailable' } }));
    await page.clock.runFor(100);
    const response = page.waitForResponse(response => new URL(response.url()).pathname === (delayed === 'health' ? '/api/health' : '/api/dashboard'));
    release();
    await response;
    await expect(page.locator('.health-panel')).toContainText('History gaps detected');
    await expect(page.locator('.health-fields').getByText('17', { exact: true })).toBeVisible();
  });
}

test('import validation, export download and deliberate deletion work', async ({ page }) => {
  const state = await harness(page);
  await page.getByRole('button', { name: 'Import records', exact: true }).click();
  await page.getByLabel('Or paste records').fill('{"test": true}');
  state.importError = true;
  await page.getByRole('dialog').getByRole('button', { name: 'Import records', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Invalid source record');
  state.importError = false;
  await page.getByRole('dialog').getByRole('button', { name: 'Import records', exact: true }).click();
  await expect(page.getByRole('status')).toContainText('2 records imported');
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Export', exact: true }).click();
  const download = page.waitForEvent('download');
  await page.getByRole('menuitem', { name: 'CSV spreadsheet' }).click();
  expect((await download).suggestedFilename()).toMatch(/\.csv$/);
  await page.getByRole('button', { name: 'Open settings', exact: true }).click();
  await page.getByRole('button', { name: 'Delete history', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Permanently delete history' })).toBeDisabled();
  expect(state.deleted).toBe(false);
  await page.getByLabel('Type DELETE to confirm').fill('DELETE');
  await page.getByRole('button', { name: 'Permanently delete history' }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(state.deleted).toBe(true);
});

test('mobile and dark theme remain navigable without horizontal page overflow', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await harness(page);
  await page.getByRole('button', { name: 'Switch to dark theme' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.getByRole('button', { name: 'Open navigation' }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(page.getByRole('dialog')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Local by design.' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
});

for (const unavailable of ['missing', 'denied'] as const) {
  test(`copying the local URL handles ${unavailable} clipboard access without an uncaught error`, async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    await harness(page);
    await page.evaluate(unavailable => {
      Object.defineProperty(navigator, 'clipboard', {
        configurable: true,
        value: unavailable === 'missing' ? undefined : { writeText: () => Promise.reject(new Error('Permission denied')) },
      });
    }, unavailable);
    await page.getByRole('button', { name: 'Connect an application', exact: true }).click();
    await page.getByRole('button', { name: 'Copy local base URL' }).click();
    await expect(page.getByRole('status')).toContainText('Select the URL to copy it manually');
    await expect(page.getByRole('dialog').locator('.copy-field code')).toHaveText('http://127.0.0.1:8765');
    expect(errors).toEqual([]);
  });
}

test('a completed deletion refreshes history without closing a newly opened panel', async ({ page }) => {
  const state = await harness(page);
  let finish!: () => void;
  state.deletePending = new Promise(resolve => { finish = resolve; });
  await page.getByRole('button', { name: 'Open settings', exact: true }).click();
  await page.getByRole('button', { name: 'Delete history', exact: true }).click();
  await page.getByLabel('Type DELETE to confirm').fill('DELETE');
  await page.getByRole('button', { name: 'Permanently delete history' }).click();
  await expect(page.getByRole('button', { name: 'Deleting…' })).toBeDisabled();
  await page.getByRole('button', { name: 'Close detail panel' }).click();
  await page.getByRole('button', { name: 'Connect an application', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Your next request, visible.' })).toBeVisible();
  finish();
  await expect(page.locator('.toast')).toContainText('Local history deleted');
  await expect(page.getByRole('heading', { name: 'Your next request, visible.' })).toBeVisible();
  expect(state.deleted).toBe(true);
  await page.keyboard.press('Escape');
  await expect(page.getByRole('heading', { name: 'Your workspace is ready' })).toBeVisible();
});

test('imported actions and unknown transport facts stay distinct', async ({ page }) => {
  const state = await harness(page);
  state.record = { ...state.record, timestamp: null, imported_at: now, timestamp_basis: 'import', event_kind: 'application_action', status: null, duration_ms: null, model: null, input_tokens: null, output_tokens: null, cost_usd: null, actions: [{ kind: 'cache_reuse', source: 'import' }] };
  state.dashboard.requests = [state.record];
  state.dashboard.summary.action_count = 1;
  await page.getByRole('button', { name: 'Refresh dashboard' }).click();
  await expect(page.locator('.request-table')).toContainText('Action');
  await expect(page.locator('.request-table')).toContainText('Import time');
  await page.getByLabel('Inspect request request-one', { exact: true }).click();
  await expect(page.getByRole('dialog')).toContainText('Original event time unknown');
  await expect(page.getByRole('dialog').locator('.status-badge')).toContainText('Unknown');
  await expect(page.getByRole('dialog')).toContainText('cache_reuse');
});

test('light and dark dashboard accessibility and assets stay local', async ({ page }) => {
  const requests: string[] = []; page.on('request', request => requests.push(request.url()));
  await harness(page);
  await page.evaluate(() => document.fonts.ready);
  let result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
  expect(result.violations.map(item => ({ id: item.id, nodes: item.nodes.map(node => node.target) }))).toEqual([]);
  await page.getByRole('button', { name: 'Switch to dark theme' }).click();
  await page.waitForTimeout(200);
  result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
  expect(result.violations.map(item => ({ id: item.id, nodes: item.nodes.map(node => node.target) }))).toEqual([]);
  expect(requests.every(url => new URL(url).hostname === '127.0.0.1')).toBe(true);
});

test('mobile navigation contains focus, closes with Escape, and returns focus from details', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await harness(page);
  const trigger = page.getByRole('button', { name: 'Open navigation', includeHidden: true });
  await trigger.click();
  const navigation = page.getByRole('dialog', { name: 'Workspace navigation' });
  await expect(navigation).toBeVisible();
  await expect(trigger).toHaveAttribute('aria-expanded', 'true');
  await expect(navigation.getByRole('button', { name: 'Close navigation' })).toBeFocused();
  await page.keyboard.press('Shift+Tab');
  await expect(navigation.getByRole('button', { name: 'Connect an application' })).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(navigation.getByRole('button', { name: 'Close navigation' })).toBeFocused();
  const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
  expect(result.violations.map(item => item.id)).toEqual([]);
  await page.keyboard.press('Escape');
  await expect(navigation).toHaveCount(0);
  await expect(trigger).toBeFocused();
  await expect(trigger).toHaveAttribute('aria-expanded', 'false');
  await trigger.click();
  await navigation.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Local by design.' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(trigger).toBeFocused();
  await trigger.click();
  await page.setViewportSize({ width: 1440, height: 1000 });
  await expect(navigation).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Request stream' })).toBeVisible();
});

test('review saves keep focus and expanded context, and failures keep the stored label', async ({ page }) => {
  const state = await harness(page);
  await page.getByRole('button', { name: 'Live updates', exact: true }).click();
  await page.getByLabel('Inspect request request-one', { exact: true }).click();
  const detail = page.getByRole('dialog');
  const definition = detail.locator('.definition-details');
  await definition.locator('summary').click();
  const review = page.getByLabel('Review department');
  await review.focus();
  let release!: () => void;
  state.labelPending = new Promise(resolve => { release = resolve; });
  await review.selectOption('correct');
  await expect(detail.getByRole('status')).toHaveText('Saving…');
  await expect(review).toBeFocused();
  await expect(review).toHaveAttribute('aria-disabled', 'true');
  await expect(definition).toHaveAttribute('open', '');
  release();
  await expect(detail.getByRole('status')).toHaveText('Saved');
  await expect(review).toHaveValue('correct');
  await expect(review).toBeFocused();
  await expect(definition).toHaveAttribute('open', '');
  state.labelPending = null;
  state.labelError = true;
  await review.selectOption('incorrect');
  await expect(detail.getByRole('alert')).toContainText('Review could not be stored');
  await expect(review).toHaveValue('correct');
  await expect(review).toBeFocused();
  await expect(definition).toHaveAttribute('open', '');
  state.labelError = false;
  state.dashboard.summary.request_count = 3;
  state.dashboard.health.persisted = 3;
  state.dashboard.requests.unshift({ ...record, id: 'while-reviewing' });
  await review.selectOption('unknown');
  await expect(detail.getByRole('status')).toHaveText('Saved');
  await expect(review).toHaveValue('unknown');
  expect(state.record.labels[0].label).toBe('unknown');
  await page.keyboard.press('Escape');
  await expect(page.getByRole('button', { name: 'Resume live', exact: true })).toBeVisible();
  await expect(page.getByLabel('Inspect request while-reviewing', { exact: true })).toHaveCount(0);
  await expect(page.locator('.pause-notice')).toContainText('1 newer records available');
});

test('delete and import replace paused history without resuming live updates', async ({ page }) => {
  const state = await harness(page);
  const imported = structuredClone(state.dashboard);
  imported.requests = [{ ...record, id: 'imported-request' }];
  imported.summary.request_count = 1;
  await page.getByRole('button', { name: 'Live updates', exact: true }).click();
  await page.getByRole('button', { name: 'Open settings', exact: true }).click();
  await page.getByRole('button', { name: 'Delete history', exact: true }).click();
  await page.getByLabel('Type DELETE to confirm').fill('DELETE');
  let release!: () => void;
  state.dashboardPending = new Promise(resolve => { release = resolve; });
  await page.getByRole('button', { name: 'Permanently delete history' }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByLabel('Inspect request request-one', { exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Resume live', exact: true })).toBeVisible();
  state.dashboardPending = null;
  release();
  await expect(page.getByRole('heading', { name: 'Your workspace is ready' })).toBeVisible();
  state.importDashboard = imported;
  await page.getByRole('button', { name: 'Import records', exact: true }).first().click();
  await page.getByLabel('Or paste records').fill('{"schema_version":1}');
  await page.getByRole('dialog').getByRole('button', { name: 'Import records', exact: true }).click();
  await expect(page.getByRole('dialog').getByRole('status')).toContainText('2 records imported');
  await page.keyboard.press('Escape');
  await expect(page.getByLabel('Inspect request imported-request', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Resume live', exact: true })).toBeVisible();
});

test('an invalid replacement import file cannot submit the previous contents', async ({ page }) => {
  await harness(page);
  await page.getByRole('button', { name: 'Import records', exact: true }).click();
  const input = page.getByLabel('Choose import file');
  const text = page.getByLabel('Or paste records');
  const submit = page.getByRole('dialog').getByRole('button', { name: 'Import records', exact: true });
  await input.setInputFiles({ name: 'valid.jsonl', mimeType: 'application/json', buffer: Buffer.from('{"previous":true}') });
  await expect(text).toHaveValue('{"previous":true}');
  await input.setInputFiles({ name: 'too-large.jsonl', mimeType: 'application/json', buffer: Buffer.alloc(8 * 1024 * 1024 + 1, 32) });
  await expect(page.getByRole('alert')).toContainText('no larger than 8 MiB');
  await expect(text).toHaveValue('');
  await expect(submit).toBeDisabled();
  await text.fill('{"pasted":true}');
  await expect(page.getByRole('alert')).toHaveCount(0);
  await expect(submit).toBeEnabled();
  await page.evaluate(() => { File.prototype.text = () => Promise.reject(new Error('Unavailable file')); });
  await input.setInputFiles({ name: 'unreadable.jsonl', mimeType: 'application/json', buffer: Buffer.from('unreadable') });
  await expect(page.getByRole('alert')).toContainText('could not be read');
  await expect(text).toHaveValue('');
  await expect(submit).toBeDisabled();
});

test('late file reads cannot overwrite a newer file or pasted import text', async ({ page }) => {
  await harness(page);
  await page.getByRole('button', { name: 'Import records', exact: true }).click();
  await page.evaluate(() => {
    const pending: Record<string, (text: string) => void> = {};
    (window as unknown as { finishImportRead: (name: string, text: string) => void }).finishImportRead = (name, text) => pending[name](text);
    File.prototype.text = function () { return new Promise<string>(resolve => { pending[this.name] = resolve; }); };
  });
  const input = page.getByLabel('Choose import file');
  const text = page.getByLabel('Or paste records');
  const submit = page.getByRole('dialog').getByRole('button', { name: 'Import records', exact: true });
  for (const name of ['older.jsonl', 'newer.jsonl']) await input.setInputFiles({ name, mimeType: 'application/json', buffer: Buffer.from(name) });
  await expect(submit).toBeDisabled();
  await page.evaluate(() => (window as unknown as { finishImportRead: (name: string, text: string) => void }).finishImportRead('newer.jsonl', '{"newer":true}'));
  await expect(text).toHaveValue('{"newer":true}');
  await page.evaluate(() => (window as unknown as { finishImportRead: (name: string, text: string) => void }).finishImportRead('older.jsonl', '{"older":true}'));
  await expect(text).toHaveValue('{"newer":true}');
  await expect(page.getByRole('dialog').getByText('newer.jsonl', { exact: true })).toBeVisible();
  await input.setInputFiles({ name: 'pending.jsonl', mimeType: 'application/json', buffer: Buffer.from('pending') });
  await text.fill('{"manual":true}');
  await page.evaluate(() => (window as unknown as { finishImportRead: (name: string, text: string) => void }).finishImportRead('pending.jsonl', '{"stale":true}'));
  await expect(text).toHaveValue('{"manual":true}');
  await expect(submit).toBeEnabled();
});
