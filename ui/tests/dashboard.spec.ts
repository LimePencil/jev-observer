import { readFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';
import type { CredentialStatus, Dashboard, Group, RequestRecord, Settings } from '../src/types';
import AxeBuilder from '@axe-core/playwright';

const catalog = JSON.parse(readFileSync(new URL('../../src/model_catalog.json', import.meta.url), 'utf8'));

// These records are deliberately synthetic test fixtures. The application itself
// always reads the real local API and never swaps in a mock data source.
const now = Date.now();
const group: Group = { id: 'group_v1_1111111111', name: 'department', key: 'department', kind: 'choice', source: 'Test inbox', definition_id: 'def_v1_1111111111', presentation_id: 'presentation_v1_1111111111', definition: { type: 'choice', instructions: 'Choose a department', criteria: { technical: 'Bugs', billing: 'Invoices' } }, task_version: null, family_id: null, family_name: null, adapter: null, answer_count: 2, valid_count: 1, request_count: 2, last_seen: now, version_count: 2, distribution: [{ label: 'technical', count: 1 }], mean_value: null, mean_confidence: null };
const record: RequestRecord = { id: 'request-one', timestamp: now, source: 'Test inbox', provider: 'typesafe', model: 'test-model', requested_model: 'test-model', status: 200, duration_ms: 124, input_tokens: 100, output_tokens: 20, cost_usd: null, cost_basis: null, answer_count: 1, capture_complete: true, sample: false, state_retained: false, state: null, transport_error: null, source_event_id: null, import_format: null, actions: [], labels: [], answers: [{ key: 'department', kind: 'choice', group_id: group.id, definition_id: group.definition_id, presentation_id: group.presentation_id, candidate_id: null, task_version: null, definition: group.definition, value: 'technical', probabilities: { technical: .8, billing: .2 }, confidence: null, valid: true, error: null, family_id: null, family_name: null, adapter: null, instance_ref: null, mapping_reason: null, raw_answer: {} }] };

async function harness(page: Page, options: { demo?: boolean; settings?: Partial<Settings>; path?: string } = {}) {
  const state = {
    record: structuredClone(record),
    dashboard: { generated_at: now, sample: false, sources: ['Test inbox'], models: ['test-model'], summary: { request_count: 2, answer_count: 2, error_count: 1, p50_ms: 124, p95_ms: 124, input_tokens: 100, output_tokens: 20, input_tokens_known_requests: 1, output_tokens_known_requests: 1, cost_usd: null, cost_known_requests: 0 }, timeline: [{ timestamp: now, requests: 2, errors: 1, mean_latency_ms: 124, cost_usd: null }], groups: [structuredClone(group)], requests: [structuredClone(record), { ...structuredClone(record), id: 'request-failed', status: 500, model: null, input_tokens: null, output_tokens: null, cost_usd: null }], health: { forwarded: 2, captured: 2, persisted: 2, dropped: 0, truncated: 0, write_failures: 0, queue_depth: 0, queued_bytes: 0, last_persisted_at: now, last_gap_at: null, lag_ms: 0, capture_limit: 262144, capture_slots: 256 } } as Dashboard,
    deleted: false, importError: false, dashboardError: false, labelError: false,
    labelPending: null as Promise<void> | null, labelHeaders: null as Record<string, string> | null,
    dashboardPending: null as Promise<void> | null, deletePending: null as Promise<void> | null, importDashboard: null as Dashboard | null,
    historicalDashboard: null as Dashboard | null,
    olderDashboard: null as Dashboard | null,
    groupCatalog: [structuredClone(group)],
    dashboardQueries: [] as string[],
    credentials: { configured: false, storage: 'none' } as CredentialStatus,
    providerKey: '', clientToken: '',
  };
  // Export opens a new tab; its first request belongs to the browser context.
  await page.context().route('**/api/**', async route => {
    const request = route.request(), url = new URL(request.url());
    let body: unknown;
    if (url.pathname === '/api/dashboard') {
      state.dashboardQueries.push(url.search);
      await state.dashboardPending;
      if (state.dashboardError) return route.fulfill({ status: 503, json: { error: 'Storage unavailable' } });
      body = structuredClone(url.searchParams.get('request_cursor') && state.olderDashboard ? state.olderDashboard : url.searchParams.get('window') === 'all' && state.historicalDashboard ? state.historicalDashboard : state.dashboard);
      if (url.searchParams.get('group_search')) (body as Dashboard).groups = state.groupCatalog.filter(item => item.name.toLowerCase().includes(url.searchParams.get('group_search')!.toLowerCase()));
      if (url.searchParams.get('status') === 'error') { const data = body as Dashboard; data.requests = data.requests.filter(item => item.failed || item.status != null && item.status >= 400); }
    } else if (url.pathname === '/api/health') body = state.dashboard.health;
    else if (url.pathname === '/api/settings') body = { demo: options.demo ?? false, capture_state: false, retention_days: 7, max_records: 1000000, capture_limit: 262144, upstream: 'https://api.typesafe.ai/v1/systemone', version: 'test', ...options.settings } satisfies Settings;
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
  await page.goto(options.path ?? '/');
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

test('token totals disclose independently missing usage and preserve known zero values', async ({ page }) => {
  const state = await harness(page);
  // Request A reports only input tokens; request B reports only output tokens.
  state.dashboard.requests[0] = { ...state.dashboard.requests[0], input_tokens: 100, output_tokens: null };
  state.dashboard.requests[1] = { ...state.dashboard.requests[1], input_tokens: null, output_tokens: 20 };
  const metric = page.getByRole('group', { name: 'Token usage', exact: true });
  await page.getByRole('button', { name: 'Refresh dashboard' }).click();
  await expect(metric.locator('.metric-label')).toHaveText('Known tokens');
  await expect(metric.locator('.metric-value')).toHaveText('120');
  await expect(metric).toContainText('Input: 1 / 2 requests');
  await expect(metric).toContainText('Output: 1 / 2 requests');

  Object.assign(state.dashboard.summary, { input_tokens: 0, output_tokens: null, input_tokens_known_requests: 1, output_tokens_known_requests: 0 });
  await page.getByRole('button', { name: 'Refresh dashboard' }).click();
  await expect(metric.locator('.metric-value')).toHaveText('0');
  await expect(metric).toContainText('0 in / Unknown out');
  await expect(metric).toContainText('Output: 0 / 2 requests');

  Object.assign(state.dashboard.summary, { input_tokens: null, output_tokens: null, input_tokens_known_requests: 0, output_tokens_known_requests: 0 });
  await page.getByRole('button', { name: 'Refresh dashboard' }).click();
  await expect(metric.locator('.metric-value')).toHaveText('Unknown');
  await expect(metric).toContainText('Input: 0 / 2 requests');

  Object.assign(state.dashboard.summary, { input_tokens: 0, output_tokens: 0, input_tokens_known_requests: 2, output_tokens_known_requests: 2 });
  await page.getByRole('button', { name: 'Refresh dashboard' }).click();
  await expect(metric.locator('.metric-label')).toHaveText('Reported tokens');
  await expect(metric.locator('.metric-value')).toHaveText('0');
  await expect(metric).toContainText('Input: 2 / 2 requests');
  await expect(metric).toContainText('Output: 2 / 2 requests');
});

test('overflowing token totals stay distinct from unknown and normal usage after refresh', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const state = await harness(page);
  const metric = page.getByRole('group', { name: 'Token usage', exact: true });
  for (const usage of [
    { input_tokens: null, output_tokens: 20, input_tokens_overflow: true, output_tokens_overflow: false, input_tokens_known_requests: 2, output_tokens_known_requests: 1, detail: 'Exceeds range in / 20 out' },
    { input_tokens: 100, output_tokens: null, input_tokens_overflow: false, output_tokens_overflow: true, input_tokens_known_requests: 1, output_tokens_known_requests: 2, detail: '100 in / Exceeds range out' },
    { input_tokens: null, output_tokens: null, input_tokens_overflow: true, output_tokens_overflow: true, input_tokens_known_requests: 2, output_tokens_known_requests: 2, detail: 'Exceeds range in / Exceeds range out' },
  ]) {
    const { detail, ...summary } = usage;
    Object.assign(state.dashboard.summary, summary);
    await page.getByRole('button', { name: 'Refresh dashboard' }).click();
    await expect(metric.locator('.metric-value')).toHaveText('Total exceeds range');
    await expect(metric).toContainText(detail);
    await expect(metric).toContainText(`Input: ${summary.input_tokens_known_requests} / 2 requests`);
    await expect(metric).toContainText(`Output: ${summary.output_tokens_known_requests} / 2 requests`);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  }
  Object.assign(state.dashboard.summary, { input_tokens_overflow: false, output_tokens_overflow: false, input_tokens_known_requests: 0, output_tokens_known_requests: 0 });
  await page.getByRole('button', { name: 'Refresh dashboard' }).click();
  await expect(metric.locator('.metric-value')).toHaveText('Unknown');
  await expect(metric).not.toContainText('Exceeds range');

  Object.assign(state.dashboard.summary, { input_tokens: 100, output_tokens: 20, input_tokens_known_requests: 2, output_tokens_known_requests: 2 });
  await page.getByRole('button', { name: 'Refresh dashboard' }).click();
  await expect(metric.locator('.metric-label')).toHaveText('Reported tokens');
  await expect(metric.locator('.metric-value')).toHaveText('120');
  await expect(metric).toContainText('100 in / 20 out');
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

for (const { storage, choice } of [
  { storage: 'system', choice: false },
  { storage: 'session', choice: true },
  { storage: 'system', choice: null },
] as const) {
  test(`delayed ${storage} credential status ${choice === null ? 'initializes an untouched storage choice' : `preserves the explicit ${choice ? 'system' : 'session'} storage choice`}`, async ({ page }) => {
    const state = await harness(page);
    state.credentials = { configured: true, storage };
    let release!: () => void;
    const pending = new Promise<void>(resolve => { release = resolve; });
    await page.route('**/api/credentials', async route => {
      if (route.request().method() !== 'GET') return route.fallback();
      await pending;
      await route.fulfill({ json: state.credentials });
    });
    try {
      await page.getByRole('button', { name: 'Connect an application' }).click();
      const checkbox = page.getByLabel('Save in this computer’s credential store');
      await expect(checkbox).toBeVisible();
      if (choice !== null) {
        await checkbox.setChecked(!choice);
        await checkbox.setChecked(choice);
      }
      release();
      await expect(page.getByText(`Registered key: ${storage === 'system' ? 'saved in system credential store' : 'this session only'}`)).toBeVisible();
      await page.getByLabel('TypeSafe API key').fill('provider-secret');
      const expected = choice ?? storage === 'system';
      await expect(checkbox).toBeChecked({ checked: expected });
      await page.getByRole('button', { name: 'Replace registered key' }).click();
      await expect(page.getByText('Copy this local client token now.')).toBeVisible();
      expect(state.credentials.storage).toBe(expected ? 'system' : 'session');
      await expect(checkbox).toBeChecked({ checked: expected });
    } finally { release(); }
  });
}

const detailModule = /\/(?:assets\/Details-[^/?]+\.js|src\/Details\.tsx)(?:\?.*)?$/;

test('a failed details module preserves the dashboard and recovers after an explicit page reload', async ({ page }) => {
  let attempts = 0;
  await page.route(detailModule, route => { attempts += 1; return route.abort('connectionreset'); });
  const state = await harness(page, { path: '/?window=all&source=Test+inbox' });
  const trigger = page.getByRole('button', { name: 'Connect an application' });
  await trigger.click();
  await expect(page.getByRole('dialog')).toContainText('Details unavailable');
  await expect(page.locator('#requests h2')).toHaveText('Request stream');
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(trigger).toBeFocused();
  state.dashboard.requests = [{ ...record, id: 'after-detail-load-failure' }];
  await page.getByRole('button', { name: 'Refresh dashboard' }).click();
  await expect(page.getByLabel('Inspect request after-detail-load-failure')).toBeVisible();

  await page.unroute(detailModule);
  await trigger.click();
  // React.lazy caches the rejection: closing/reopening must still offer recovery.
  await expect(page.getByRole('dialog')).toContainText('Details unavailable');
  expect(attempts).toBe(1);
  await page.getByRole('button', { name: 'Reload page', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Request stream' })).toBeVisible();
  await expect(page.getByLabel('Filter by source')).toHaveValue('Test inbox');
  await expect(page.getByLabel('Time window')).toHaveValue('all');
  await trigger.click();
  await expect(page.getByRole('heading', { name: 'Your next request, visible.' })).toBeVisible();
});

test('Escape cancels a loading details module without reopening after the download completes', async ({ page }) => {
  let release!: () => void;
  const pending = new Promise<void>(resolve => { release = resolve; });
  await page.route(detailModule, async route => { await pending; await route.continue(); });
  await harness(page);
  const trigger = page.getByRole('button', { name: 'Connect an application' });
  try {
    await trigger.click();
    await expect(page.getByRole('dialog')).toContainText('Opening details');
    await page.keyboard.press('Tab');
    await expect(page.getByRole('button', { name: 'Close detail panel' })).toBeFocused();
    await page.keyboard.press('Escape');
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(trigger).toBeFocused();
    const loaded = page.waitForResponse(detailModule);
    release();
    await loaded;
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await trigger.click();
    await expect(page.getByRole('heading', { name: 'Your next request, visible.' })).toBeVisible();
  } finally { release(); }
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

for (const endpoint of ['dashboard', 'health'] as const) {
  test(`a stalled ${endpoint} read times out and polling recovers independently`, async ({ page }) => {
    await page.clock.install({ time: new Date(now) });
    await page.clock.pauseAt(new Date(now + 1000));
    const state = await harness(page);
    state.dashboard.health = { ...state.dashboard.health, process_id: 'timeout-test', sample_sequence: 1 };
    const oldDashboard = structuredClone(state.dashboard);
    const newHealth = { ...state.dashboard.health, sample_sequence: 2, dropped: 11 };
    const recoveredDashboard = { ...structuredClone(state.dashboard), requests: [{ ...record, id: 'after-timeout' }] };
    let release!: () => void;
    const pending = new Promise<void>(resolve => { release = resolve; });
    let started = 0;
    await page.route(endpoint === 'dashboard' ? '**/api/dashboard?**' : '**/api/health', async route => {
      started += 1;
      if (started === 1) {
        await pending;
        await route.fulfill({ json: endpoint === 'dashboard' ? oldDashboard : oldDashboard.health });
      } else await route.fulfill({ json: endpoint === 'dashboard' ? recoveredDashboard : newHealth });
    });
    if (endpoint === 'dashboard') await page.route('**/api/health', route => route.fulfill({ json: newHealth }));
    else state.dashboard.requests = recoveredDashboard.requests;

    try {
      await page.clock.runFor(1000);
      await expect.poll(() => started).toBe(1);
      // The other endpoint continues updating while this request is stalled.
      if (endpoint === 'dashboard') await expect(page.locator('.health-fields').getByText('11', { exact: true })).toBeVisible();
      else await expect(page.getByLabel('Inspect request after-timeout')).toBeVisible();
      await page.clock.runFor(14_999);
      expect(started).toBe(1);
      await page.clock.runFor(1);
      if (endpoint === 'dashboard') {
        await expect(page.getByRole('alert')).toContainText('Request timed out after 15 seconds');
        await expect(page.getByLabel('Inspect request request-one', { exact: true })).toBeVisible();
      }
      await page.clock.runFor(1000);
      await expect.poll(() => started).toBe(2);
      if (endpoint === 'dashboard') {
        await expect(page.getByLabel('Inspect request after-timeout')).toBeVisible();
        await expect(page.getByRole('alert')).toHaveCount(0);
      } else await expect(page.locator('.health-fields').getByText('11', { exact: true })).toBeVisible();
      release();
      // A late response from the abandoned read must not replace recovered data.
      if (endpoint === 'dashboard') await expect(page.getByLabel('Inspect request request-one', { exact: true })).toHaveCount(0);
      else await expect(page.locator('.health-fields').getByText('11', { exact: true })).toBeVisible();
    } finally { release(); }
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
  await page.getByText('Compare definitions and context', { exact: true }).click();
  await expect(page.getByText('Changed department rules')).toBeVisible();
  await expect(page.getByText('Presentation: 2222222222')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
});

test('a stalled detail read offers Retry and recovers', async ({ page }) => {
  await page.clock.install({ time: new Date(now) });
  await harness(page);
  // Load the lazy drawer before freezing timers used by React Suspense.
  await page.getByLabel('Inspect request request-one', { exact: true }).click();
  await expect(page.getByLabel('Review department')).toBeVisible();
  await page.keyboard.press('Escape');
  await page.clock.pauseAt(new Date(await page.evaluate(() => Date.now() + 1000)));
  let release!: () => void;
  const pending = new Promise<void>(resolve => { release = resolve; });
  let stalled = true;
  await page.route('**/api/requests/request-one', async route => {
    if (stalled) await pending;
    await route.fulfill({ json: record });
  });
  try {
    await page.getByLabel('Inspect request request-one', { exact: true }).click();
    await expect(page.getByRole('dialog')).toContainText('Loading request');
    await page.clock.runFor(15_000);
    await expect(page.getByRole('dialog').getByRole('alert')).toContainText('Request timed out after 15 seconds');
    stalled = false;
    await page.getByRole('dialog').getByRole('button', { name: 'Retry' }).click();
    await expect(page.getByRole('dialog')).toContainText('Test inbox');
    await expect(page.getByRole('dialog').getByRole('alert')).toHaveCount(0);
  } finally { release(); }
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
    await expect(page.getByRole('status', { name: 'Copy status' })).toContainText('Select the URL to copy it manually');
    await expect(page.getByRole('dialog').locator('.copy-field code')).toHaveText(process.env.OBSERVER_UI_PREVIEW === '1' ? new URL(page.url()).origin : 'http://127.0.0.1:8765');
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

for (const format of ['jevrouter-receipt', 'observer-jsonl']) {
  test(`${format} application input stays distinct from absent request state`, async ({ page }) => {
    const state = await harness(page);
    state.record = { ...state.record, event_kind: 'application_action', import_format: format, source_event_id: 'synthetic-receipt', state_retained: false, state: null, answers: [], actions: [{ kind: 'policy_decision', source: 'jevrouter-receipt', decision: { input: 'synthetic retained input' } }] };
    await page.getByLabel('Inspect request request-one', { exact: true }).click();
    const dialog = page.getByRole('dialog');
    const input = dialog.locator('section').filter({ has: page.getByRole('heading', { name: 'Request input state', exact: true }) });
    await expect(input).toContainText('This record has no retained request state.');
    await expect(input).toContainText('Imported application data is shown under Application outcome.');
    await expect(input).not.toContainText('Raw input capture is off');
    await expect(dialog.getByText('Inspect retained input', { exact: true })).toHaveCount(0);
    const outcome = dialog.locator('section').filter({ has: page.getByRole('heading', { name: 'Application outcome', exact: true }) });
    await expect(outcome).toContainText('synthetic retained input');
    await expect(dialog).toContainText(format);
  });
}

for (const retained of [false, true]) {
  test(`ordinary request state copy describes ${retained ? 'retained' : 'absent'} input`, async ({ page }) => {
    const state = await harness(page);
    state.record = { ...state.record, state_retained: retained, state: retained ? { message: 'synthetic request state' } : null };
    await page.getByLabel('Inspect request request-one', { exact: true }).click();
    const input = page.getByRole('dialog').locator('section').filter({ has: page.getByRole('heading', { name: 'Request input state', exact: true }) });
    await expect(input).not.toContainText('Imported application data');
    if (retained) {
      await expect(input).toContainText('Captured with local opt-in and configured redaction.');
      await input.getByText('Inspect retained input', { exact: true }).click();
      await expect(input).toContainText('synthetic request state');
    } else {
      await expect(input).toContainText('This record has no retained request state.');
      await expect(input.getByText('Inspect retained input', { exact: true })).toHaveCount(0);
    }
  });
}

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

for (const dismiss of ['Escape', 'close button']) {
  test(`Import from the Export menu restores keyboard focus after ${dismiss}`, async ({ page }) => {
    await harness(page);
    const trigger = page.getByRole('button', { name: 'Export', exact: true });
    await trigger.focus();
    await page.keyboard.press('Enter');
    await page.keyboard.press('End');
    await expect(page.getByRole('menuitem', { name: 'Import local records' })).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('dialog')).toBeVisible();
    if (dismiss === 'Escape') await page.keyboard.press('Escape');
    else await page.getByRole('button', { name: 'Close detail panel' }).click();
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(trigger).toBeFocused();
    await page.keyboard.press('Tab');
    await expect(page.getByRole('button', { name: 'Live updates', exact: true })).toBeFocused();
  });
}

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

test('import files preserve valid Unicode and reject invalid UTF-8 without substituting bytes', async ({ page }) => {
  await harness(page);
  const imports: string[] = [];
  page.on('request', request => {
    if (new URL(request.url()).pathname === '/api/import') imports.push(request.postDataJSON().text);
  });
  await page.getByRole('button', { name: 'Import records', exact: true }).click();
  const input = page.getByLabel('Choose import file');
  const text = page.getByLabel('Or paste records');
  const submit = page.getByRole('dialog').getByRole('button', { name: 'Import records', exact: true });
  const unicode = JSON.stringify({ source: '한글 café 😀 �' });
  await input.setInputFiles({ name: 'unicode.jsonl', mimeType: 'application/json', buffer: Buffer.from('\uFEFF' + unicode) });
  await expect(text).toHaveValue(unicode);
  await submit.click();
  await expect(page.getByRole('dialog').getByRole('status')).toContainText('2 records imported');
  expect(imports).toEqual([unicode]);

  const invalid = Buffer.concat([Buffer.from('{"source":"'), Buffer.from([0xff]), Buffer.from('"}')]);
  await input.setInputFiles({ name: 'invalid.jsonl', mimeType: 'application/json', buffer: invalid });
  await expect(page.getByRole('alert')).toContainText('not valid UTF-8');
  await expect(text).toHaveValue('');
  await expect(submit).toBeDisabled();
  expect(imports).toEqual([unicode]);

  await input.setInputFiles({ name: 'unicode.jsonl', mimeType: 'application/json', buffer: Buffer.from(unicode) });
  await expect(text).toHaveValue(unicode);
  await expect(page.getByRole('alert')).toHaveCount(0);
  await expect(submit).toBeEnabled();
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
  await page.evaluate(() => { File.prototype.arrayBuffer = () => Promise.reject(new Error('Unavailable file')); });
  await input.setInputFiles({ name: 'unreadable.jsonl', mimeType: 'application/json', buffer: Buffer.from('unreadable') });
  await expect(page.getByRole('alert')).toContainText('could not be read');
  await expect(text).toHaveValue('');
  await expect(submit).toBeDisabled();
});

test('late file reads cannot overwrite a newer file or pasted import text', async ({ page }) => {
  await harness(page);
  await page.getByRole('button', { name: 'Import records', exact: true }).click();
  await page.evaluate(() => {
    const pending: Record<string, (buffer: ArrayBuffer) => void> = {};
    (window as unknown as { finishImportRead: (name: string, text: string) => void }).finishImportRead = (name, text) => pending[name](new TextEncoder().encode(text).buffer);
    File.prototype.arrayBuffer = function () { return new Promise<ArrayBuffer>(resolve => { pending[this.name] = resolve; }); };
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

test('local model onboarding uses workspace authentication and identity SDK examples', async ({ page }) => {
  await harness(page, { settings: { provider: 'laya', upstream_auth: 'none', upstream: 'http://127.0.0.1:8080/v1/systemone' } });
  await page.getByRole('button', { name: 'Connect an application' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('http://127.0.0.1:8080/v1/systemone');
  await expect(dialog).toContainText('None · local server');
  await expect(page.getByRole('button', { name: 'Register key' })).toHaveCount(0);
  await page.getByLabel('Model name', { exact: true }).fill('my-local-laya');
  const snippet = page.getByLabel('SDK connection example');
  await expect(snippet).toContainText('JEV_OBSERVER_ACCESS_TOKEN');
  await expect(snippet).toContainText('"Accept-Encoding": "identity"');
  await expect(snippet).toContainText('model="my-local-laya"');
  await expect(snippet).toContainText('timeout=180,');
  await page.getByRole('button', { name: 'JavaScript', exact: true }).click();
  await expect(snippet).toContainText('apiKey: process.env.JEV_OBSERVER_ACCESS_TOKEN');
  await expect(snippet).toContainText('model: "my-local-laya"');
  await expect(snippet).toContainText('timeout: 180000,');
  await expect(dialog).toContainText('2 captures saved in this Observer session.');
});

test('key-protected Laya keeps client authentication and longer SDK timeouts', async ({ page }) => {
  await harness(page, { settings: { provider: 'laya', upstream_auth: 'bearer', upstream: 'http://127.0.0.1:8080/v1/systemone' } });
  await page.getByRole('button', { name: 'Connect an application' }).click();
  await expect(page.getByRole('button', { name: 'Register key' })).toBeVisible();
  const snippet = page.getByLabel('SDK connection example');
  await expect(snippet).toContainText('JEV_OBSERVER_CLIENT_TOKEN');
  await expect(snippet).not.toContainText('JEV_OBSERVER_ACCESS_TOKEN');
  await expect(snippet).toContainText('timeout=180,');
  await page.getByRole('button', { name: 'JavaScript', exact: true }).click();
  await expect(snippet).toContainText('apiKey: process.env.JEV_OBSERVER_CLIENT_TOKEN');
  await expect(snippet).toContainText('timeout: 180000,');
  await expect(page.getByRole('dialog')).toContainText('Python: seconds; JavaScript: milliseconds');
});

test('older request pages survive reloading a bookmarked view', async ({ page }) => {
  const state = await harness(page);
  state.dashboard.feed_next_cursor = 'older-page';
  state.olderDashboard = { ...structuredClone(state.dashboard), requests: [{ ...structuredClone(record), id: 'old-request' }], feed_next_cursor: null };
  await page.getByRole('button', { name: 'Refresh dashboard' }).click();
  await page.getByRole('button', { name: 'Older requests', exact: true }).click();
  await expect(page.getByLabel('Inspect request old-request')).toBeVisible();
  await expect(page).toHaveURL(/request_cursor=older-page/);
  await page.reload();
  await expect(page.getByLabel('Inspect request old-request')).toBeVisible();
  await page.getByRole('button', { name: 'Newer requests', exact: true }).click();
  await expect(page.getByLabel('Inspect request request-one', { exact: true })).toBeVisible();
});

test('custom date filters survive reloading a bookmarked view', async ({ page }) => {
  await harness(page);
  await page.getByRole('button', { name: 'Custom date range' }).click();
  await page.getByLabel('From date').fill('2026-10-01T09:00');
  await page.getByLabel('To date').fill('2026-10-02T09:00');
  await expect(page).toHaveURL(/from=\d+/);
  await expect(page).toHaveURL(/to=\d+/);
  await page.reload();
  await expect(page.getByLabel('From date')).toHaveValue('2026-10-01T09:00');
  await expect(page.getByLabel('To date')).toHaveValue('2026-10-02T09:00');
  await expect(page.getByLabel('Time window')).toHaveValue('all');
  await page.getByLabel('Time window').selectOption('24h');
  await expect(page.getByLabel('From date')).toHaveValue('');
  await expect(page).not.toHaveURL(/request_cursor|from=|to=/);
});

test.describe('date bookmarks in Asia/Seoul', () => {
  test.use({ timezoneId: 'Asia/Seoul' });

  test('valid epoch boundary bookmarks keep their scope without crashing the date picker', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    const boundary = '8640000000000000';
    const state = await harness(page, { path: `/?window=all&from=${boundary}&to=${boundary}` });
    // A local value beyond the HTML control's range stays in the URL/API scope.
    await expect(page.getByLabel('From date')).toHaveValue('');
    await expect(page.getByLabel('To date')).toHaveValue('');
    await expect(page.getByLabel('From date')).toHaveAccessibleDescription(/saved start date.*remains active/);
    await expect(page.getByLabel('To date')).toHaveAccessibleDescription(/saved end date.*remains active/);
    expect(state.dashboardQueries.some(query => {
      const params = new URLSearchParams(query);
      return params.get('from') === boundary && params.get('to') === boundary;
    })).toBe(true);
    await expect(page).toHaveURL(new RegExp(`from=${boundary}&to=${boundary}`));
    await page.reload();
    await expect(page.getByRole('heading', { name: 'Request stream' })).toBeVisible();
    expect(errors).toEqual([]);

    await page.getByLabel('From date').fill('10000-01-15T09:30');
    await page.getByLabel('To date').fill('10000-01-15T10:45');
    await expect(page.getByLabel('From date')).toHaveValue('10000-01-15T09:30');
    await expect(page.getByLabel('To date')).toHaveValue('10000-01-15T10:45');
    const expected = await page.evaluate(() => ({ from: String(new Date('+010000-01-15T09:30').getTime()), to: String(new Date('+010000-01-15T10:45').getTime()) }));
    await expect.poll(() => new URL(page.url()).searchParams.get('from')).toBe(expected.from);
    await expect.poll(() => new URL(page.url()).searchParams.get('to')).toBe(expected.to);
    await expect(page.locator('#from-date-range-note, #to-date-range-note')).toHaveCount(0);
  });
});

test('question search reaches server history and stays editable after no matches', async ({ page }) => {
  const state = await harness(page);
  state.groupCatalog.push({ ...structuredClone(group), id: 'older-question', name: 'historical-routing' });
  const search = page.getByLabel('Find a question group');
  await search.fill('historical');
  await expect(page.locator('.group-row')).toContainText('historical-routing');
  await expect(search).toBeFocused();
  expect(state.dashboardQueries.some(value => new URLSearchParams(value).get('group_search') === 'historical')).toBe(true);
  await search.fill('no-matching-question');
  await expect(page.getByRole('heading', { name: 'No questions match this search' })).toBeVisible();
  await expect(search).toBeFocused();
  await search.fill('department');
  await expect(page.locator('.group-row')).toContainText('department');
});

for (const { timezoneId, endDate, lastBucketDate } of [
  { timezoneId: 'UTC', endDate: 'Oct 3', lastBucketDate: 'Oct 2' },
  { timezoneId: 'America/Los_Angeles', endDate: 'Oct 2', lastBucketDate: 'Oct 1' },
]) {
  test.describe(`timeline calendar in ${timezoneId}`, () => {
    test.use({ timezoneId, locale: 'en-US' });

    test('timeline spacing preserves quiet periods and shows calendar dates', async ({ page }) => {
      const state = await harness(page);
      const hour = 3_600_000, start = Date.UTC(2026, 9, 1);
      state.dashboard.timeline = [0, 1, 24].map(offset => ({ timestamp: start + offset * hour, requests: 1, errors: 0, mean_latency_ms: 10, cost_usd: 0 }));
      state.dashboard.timeline_meta = { start, end: start + 48 * hour, bucket_width: hour, truncated: false };
      await page.getByRole('button', { name: 'Refresh dashboard' }).click();
      await expect(page.locator('.activity-panel .chart-bar')).toHaveCount(3);
      const positions = await page.locator('.activity-panel .chart-bar').evaluateAll(nodes => nodes.map(node => Number(node.getAttribute('x'))));
      expect((positions[2] - positions[1]) / (positions[1] - positions[0])).toBeCloseTo(23);
      await expect(page.locator('.activity-panel .chart-axis').last()).toContainText(endDate);
      await page.locator('.activity-panel').getByText('View chart data', { exact: true }).click();
      await expect(page.locator('.activity-panel .chart-accessible tbody tr').last().locator('td').first()).toContainText(lastBucketDate);
    });
  });
}

test('a partial timeline bucket keeps the selected exclusive range end', async ({ page }) => {
  const state = await harness(page);
  const start = Date.UTC(2026, 9, 1);
  state.dashboard.timeline = [{ timestamp: start, requests: 1, errors: 0, mean_latency_ms: 10, cost_usd: null }];
  state.dashboard.timeline_meta = { start, end: start + 30_000, bucket_width: 60_000, truncated: false };
  await page.getByRole('button', { name: 'Refresh dashboard' }).click();
  const expectedEnd = await page.evaluate(value => new Date(value).toLocaleString([], { hour: '2-digit', minute: '2-digit', hour12: false }), start + 30_000);
  await expect(page.locator('.activity-panel .chart-axis').last()).toHaveText(expectedEnd);
});

test('transport failures, provider charges and Score warnings remain visible', async ({ page }) => {
  const state = await harness(page);
  state.record = { ...state.record, failed: true, status: 200, cost_usd: .003, cost_basis: 'provider_reported', transport_error: 'Connection closed before the response finished.' };
  state.record.answers[0] = { ...state.record.answers[0], kind: 'score', value: 1.7, warnings: ['Score outside the supplied rubric range.'] };
  state.dashboard.requests[0] = state.record;
  await page.getByRole('button', { name: 'Refresh dashboard' }).click();
  await expect(page.locator('.request-table')).toContainText('Transport failed');
  await expect(page.locator('.request-table')).toContainText('Provider-reported charge');
  await page.getByRole('button', { name: 'Failures', exact: true }).click();
  await expect(page.getByLabel('Inspect request request-one', { exact: true })).toBeVisible();
  await page.getByLabel('Inspect request request-one', { exact: true }).click();
  await expect(page.getByRole('dialog')).toContainText('Score outside the supplied rubric range.');
  await expect(page.locator('.answer-value')).toContainText('1.7');
  await expect(page.getByRole('dialog')).toContainText('Provider-reported charge');
});

test('version comparisons show token coverage independently for partial unknown and zero usage', async ({ page }) => {
  await harness(page);
  const current = { ...group, request_count: 3, input_tokens: 100, output_tokens: 20, input_tokens_known_requests: 1, output_tokens_known_requests: 2 };
  const versions = [
    current,
    { ...group, id: 'unknown', input_tokens: null, output_tokens: null, input_tokens_known_requests: 0, output_tokens_known_requests: 0 },
    { ...group, id: 'zero', input_tokens: 0, output_tokens: 0, input_tokens_known_requests: 2, output_tokens_known_requests: 2 },
  ];
  await page.route('**/api/groups/**', route => route.fulfill({ json: {
    group: current, versions, timeline: [], requests: [], answers: [],
  } }));
  await page.getByRole('button', { name: 'Inspect group', exact: true }).click();
  await page.getByLabel('Compare with').selectOption('unknown');
  const comparison = page.getByLabel('Version outcome comparison');
  const input = comparison.getByRole('row').filter({ has: page.getByRole('rowheader', { name: 'Reported input tokens', exact: true }) });
  const output = comparison.getByRole('row').filter({ has: page.getByRole('rowheader', { name: 'Reported output tokens', exact: true }) });
  await expect(input.getByRole('cell').nth(0)).toHaveText('1001 / 3 requests covered');
  await expect(output.getByRole('cell').nth(0)).toHaveText('202 / 3 requests covered');
  await expect(input.getByRole('cell').nth(1)).toHaveText('Unknown0 / 2 requests covered');
  await expect(output.getByRole('cell').nth(1)).toHaveText('Unknown0 / 2 requests covered');

  await page.getByLabel('Compare with').selectOption('zero');
  await expect(input.getByRole('cell').nth(1)).toHaveText('02 / 2 requests covered');
  await expect(output.getByRole('cell').nth(1)).toHaveText('02 / 2 requests covered');
});

test('version comparisons preserve per-direction overflow and coverage within their selected scope', async ({ page }) => {
  await harness(page, { path: '/?window=all&group=selected-family&source=Test+inbox&model=test-model' });
  const current = { ...group, request_count: 3, input_tokens: null, output_tokens: 20, input_tokens_overflow: true, output_tokens_overflow: false, input_tokens_known_requests: 3, output_tokens_known_requests: 2 };
  const versions = [
    current,
    { ...group, id: 'output-overflow', input_tokens: 100, output_tokens: null, input_tokens_overflow: false, output_tokens_overflow: true, input_tokens_known_requests: 1, output_tokens_known_requests: 2 },
    { ...group, id: 'both-overflow', input_tokens: null, output_tokens: null, input_tokens_overflow: true, output_tokens_overflow: true, input_tokens_known_requests: 2, output_tokens_known_requests: 2 },
    { ...group, id: 'unknown', input_tokens: null, output_tokens: null, input_tokens_overflow: false, output_tokens_overflow: false, input_tokens_known_requests: 0, output_tokens_known_requests: 0 },
    { ...group, id: 'normal', input_tokens: 100, output_tokens: 0, input_tokens_overflow: false, output_tokens_overflow: false, input_tokens_known_requests: 2, output_tokens_known_requests: 2 },
  ];
  const queries: URL[] = [];
  await page.route('**/api/groups/**', route => {
    queries.push(new URL(route.request().url()));
    return route.fulfill({ json: { group: current, versions, timeline: [], requests: [], answers: [] } });
  });
  await page.getByRole('button', { name: 'Inspect group', exact: true }).click();
  await page.getByLabel('Compare with').selectOption('output-overflow');
  const comparison = page.getByLabel('Version outcome comparison');
  const input = comparison.getByRole('row').filter({ has: page.getByRole('rowheader', { name: 'Reported input tokens', exact: true }) });
  const output = comparison.getByRole('row').filter({ has: page.getByRole('rowheader', { name: 'Reported output tokens', exact: true }) });
  await expect(input.getByRole('cell').nth(0)).toHaveText('Total exceeds range3 / 3 requests covered');
  await expect(output.getByRole('cell').nth(0)).toHaveText('202 / 3 requests covered');
  await expect(input.getByRole('cell').nth(1)).toHaveText('1001 / 2 requests covered');
  await expect(output.getByRole('cell').nth(1)).toHaveText('Total exceeds range2 / 2 requests covered');
  await page.getByLabel('Compare with').selectOption('both-overflow');
  await expect(input.getByRole('cell').nth(1)).toHaveText('Total exceeds range2 / 2 requests covered');
  await expect(output.getByRole('cell').nth(1)).toHaveText('Total exceeds range2 / 2 requests covered');
  await page.getByLabel('Compare with').selectOption('unknown');
  await expect(input.getByRole('cell').nth(1)).toHaveText('Unknown0 / 2 requests covered');
  await expect(output.getByRole('cell').nth(1)).toHaveText('Unknown0 / 2 requests covered');
  await page.getByLabel('Compare with').selectOption('normal');
  await expect(input.getByRole('cell').nth(1)).toHaveText('1002 / 2 requests covered');
  await expect(output.getByRole('cell').nth(1)).toHaveText('02 / 2 requests covered');
  expect(queries.length).toBeGreaterThan(0);
  for (const url of queries) {
    expect(url.searchParams.get('group')).toBe('selected-family');
    expect(url.searchParams.get('source')).toBe('Test inbox');
    expect(url.searchParams.get('model')).toBe('test-model');
  }
});

test('version comparisons show observed outcomes and judged review coverage', async ({ page }) => {
  await harness(page);
  await page.route('**/api/groups/**', route => route.fulfill({ json: {
    group: { ...group, review_counts: { correct: 1, incorrect: 0, unknown: 0, unlabeled: 1 } },
    versions: [group, { ...group, id: 'other', review_counts: { correct: 0, incorrect: 1, unknown: 1, unlabeled: 0 }, distribution: [{ label: 'billing', count: 1 }] }],
    timeline: [], requests: [], answers: [],
  } }));
  await page.getByRole('button', { name: 'Inspect group', exact: true }).click();
  await page.getByLabel('Compare with').selectOption('other');
  const comparison = page.getByLabel('Version outcome comparison');
  await expect(comparison).toContainText('100.0% (1 / 1)');
  await expect(comparison).toContainText('0.0% (0 / 1)');
  await expect(comparison).toContainText('not a matched replay');
  await expect(comparison).toContainText('technical');
  await expect(comparison).toContainText('billing');
});

for (const parentGroup of ['selected-group', 'selected-family']) {
  test(`group details and version navigation retain the ${parentGroup} parent scope`, async ({ page }) => {
    const state = await harness(page, { path: `/?window=all&group=${parentGroup}&source=Test+inbox&model=test-model` });
    const scopedGroup = { ...structuredClone(group), answer_count: 1, valid_count: 1, request_count: 1 };
    const otherVersion = { ...scopedGroup, id: 'other-version', definition_id: 'other-definition' };
    const sharedRequest = { ...structuredClone(record), id: 'shared-request' };
    const outsideRequest = { ...structuredClone(record), id: 'outside-parent-scope' };
    state.dashboard.groups = [scopedGroup];
    state.dashboard.requests = [sharedRequest];
    state.record = sharedRequest;
    await page.getByRole('button', { name: 'Refresh dashboard' }).click();
    await expect(page.locator('.distribution-meta')).toContainText('1 valid / 1 answers');

    const detailQueries: URL[] = [];
    await page.route('**/api/groups/**', route => {
      const url = new URL(route.request().url());
      detailQueries.push(url);
      const scoped = url.searchParams.get('group') === parentGroup;
      const selected = url.pathname.endsWith(otherVersion.id) ? otherVersion : scopedGroup;
      const inScope = (version: Group) => ({ ...version, answer_count: scoped ? 1 : 2, valid_count: scoped ? 1 : 2, request_count: scoped ? 1 : 2 });
      return route.fulfill({ json: {
        group: inScope(selected), versions: [inScope(scopedGroup), inScope(otherVersion)],
        timeline: [], requests: scoped ? [sharedRequest] : [sharedRequest, outsideRequest], answers: [],
      } });
    });

    await page.getByRole('button', { name: 'Inspect group', exact: true }).click();
    const drawer = page.getByRole('dialog');
    await expect(drawer.locator('.detail-field').filter({ has: page.locator('dt', { hasText: 'Distinct requests' }) }).locator('dd')).toHaveText('1');
    await expect(drawer.getByLabel('Inspect request outside-parent-scope')).toHaveCount(0);
    await page.getByLabel('Compare with').selectOption(otherVersion.id);
    await expect(page.getByLabel('Version outcome comparison').getByRole('row', { name: 'Distinct requests 1 1', exact: true })).toBeVisible();

    await drawer.getByRole('button', { name: /Version 2/ }).click();
    await expect(drawer.locator('.version-item.selected')).toContainText('other-defi');
    await drawer.getByLabel('Inspect request shared-request', { exact: true }).click();
    await drawer.getByRole('button', { name: 'View recurring group' }).click();
    await expect(drawer.getByRole('heading', { name: 'department', exact: true })).toBeVisible();
    // Development StrictMode remounts effects, so each visit can issue an
    // aborted duplicate request. Assert the visited groups, not fetch counts.
    const visitedPaths = detailQueries.map(url => url.pathname).filter((path, index, paths) => index === 0 || path !== paths[index - 1]);
    expect(visitedPaths).toEqual([
      `/api/groups/${scopedGroup.id}`,
      `/api/groups/${otherVersion.id}`,
      `/api/groups/${scopedGroup.id}`,
    ]);
    for (const url of detailQueries) {
      expect(url.searchParams.get('group')).toBe(parentGroup);
      expect(url.searchParams.get('source')).toBe('Test inbox');
      expect(url.searchParams.get('model')).toBe('test-model');
      expect(url.searchParams.get('window')).toBe('all');
    }
  });
}


test('the researched catalog keeps setup separate from active credentials and opens mapped capture import', async ({ page }) => {
  await harness(page, { settings: { provider: 'kev', upstream_auth: 'none', upstream: 'http://127.0.0.1:8009/v1/systemone', model_catalog: catalog as Settings['model_catalog'] } });
  await page.getByRole('button', { name: 'Connect an application', exact: true }).click();
  await expect(page.getByLabel('Model integration')).toHaveValue('kev');
  await expect(page.getByLabel('Observer startup command')).toContainText('--provider kev --upstream-auth none');
  await page.getByLabel('Model integration').selectOption('milliseconds');
  await expect(page.getByLabel('Observer startup command')).toContainText('https://api.milliseconds.ai/v1/systemone');
  await expect(page.getByRole('dialog')).toContainText('None · local server');
  await expect(page.getByLabel('SDK connection example')).toContainText('JEV_OBSERVER_ACCESS_TOKEN');
  await page.getByLabel('Model integration').selectOption('autotrust');
  await expect(page.getByLabel('Observer startup command')).toHaveCount(0);
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.getByRole('button', { name: 'Import mapped decisions' })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  const accessibility = await new AxeBuilder({ page }).analyze();
  expect(accessibility.violations).toEqual([]);
  await page.screenshot({ path: '/tmp/jev-model-catalog-mobile.png', fullPage: true });
  await page.getByRole('button', { name: 'Import mapped decisions' }).click();
  await expect(page.getByLabel('Source format')).toHaveValue('systemone-capture');
  await expect(page.getByLabel('Or paste records')).toHaveAttribute('placeholder', /id, timestamp, source, provider/);
});
