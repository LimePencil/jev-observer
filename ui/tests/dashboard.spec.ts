import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { expect, test } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';
import type { CredentialStatus, Dashboard, Group, RequestRecord, Settings } from '../src/types';
import AxeBuilder from '@axe-core/playwright';

const catalog = JSON.parse(readFileSync(new URL('../../src/model_catalog.json', import.meta.url), 'utf8'));

// These records are deliberately synthetic test fixtures. The application itself
// always reads the real local API and never swaps in a mock data source.
const now = Date.now();
const group: Group = { id: 'group_v1_1111111111', name: 'department', key: 'department', kind: 'choice', source: 'Test inbox', definition_id: 'def_v1_1111111111', presentation_id: 'presentation_v1_1111111111', definition: { type: 'choice', instructions: 'Choose a department', criteria: { technical: 'Bugs', billing: 'Invoices' } }, task_version: null, family_id: null, family_name: null, adapter: null, answer_count: 2, valid_count: 1, request_count: 2, last_seen: now, version_count: 2, distribution: [{ label: 'technical', count: 1 }], mean_value: null, mean_confidence: null };
const record: RequestRecord = { id: 'request-one', timestamp: now, source: 'Test inbox', provider: 'typesafe', model: 'test-model', requested_model: 'test-model', status: 200, duration_ms: 124, input_tokens: 100, output_tokens: 20, cost_usd: null, cost_basis: null, answer_count: 1, capture_complete: true, sample: false, state_retained: false, state: null, transport_error: null, source_event_id: null, import_format: null, actions: [], labels: [], answers: [{ key: 'department', kind: 'choice', group_id: group.id, definition_id: group.definition_id, presentation_id: group.presentation_id, candidate_id: null, task_version: null, definition: group.definition, value: 'technical', probabilities: { technical: .8, billing: .2 }, confidence: null, valid: true, error: null, family_id: null, family_name: null, adapter: null, instance_ref: null, mapping_reason: null, raw_answer: {} }] };

async function largeTokenHarness(page: Page, input: string, output: string, summary: Record<string, unknown> = {}) {
  const state = await harness(page);
  // Keep the decimal lexemes out of JavaScript Numbers until the application
  // parses the HTTP response. A route.fulfill({ json }) fixture would pre-round.
  const inputMarker = '__RAW_TOKEN_INPUT__', outputMarker = '__RAW_TOKEN_OUTPUT__';
  const raw = (value: unknown) => {
    expect(input).toMatch(/^(null|[0-9]+)$/);
    expect(output).toMatch(/^(null|[0-9]+)$/);
    const body = JSON.stringify(value).replaceAll(JSON.stringify(inputMarker), input).replaceAll(JSON.stringify(outputMarker), output);
    expect(body).toContain(`"input_tokens":${input}`);
    expect(body).toContain(`"output_tokens":${output}`);
    return body;
  };
  const request = { ...record, input_tokens: inputMarker, output_tokens: outputMarker };
  const current = { ...group, answer_count: 1, valid_count: 1, request_count: 1, input_tokens: inputMarker, output_tokens: outputMarker,
    input_tokens_known_requests: input === 'null' ? 0 : 1, output_tokens_known_requests: output === 'null' ? 0 : 1 };
  const dashboard = { ...state.dashboard, requests: [request], groups: [current], timeline: [], summary: {
    ...state.dashboard.summary, request_count: 1, answer_count: 1, error_count: 0,
    input_tokens: inputMarker, output_tokens: outputMarker, input_tokens_overflow: false, output_tokens_overflow: false,
    input_tokens_known_requests: input === 'null' ? 0 : 1, output_tokens_known_requests: output === 'null' ? 0 : 1, ...summary,
  } };
  const other = { ...current, id: 'safe-comparison', request_count: 2, answer_count: 2, valid_count: 2,
    input_tokens: Number.MAX_SAFE_INTEGER, output_tokens: null, output_tokens_overflow: true,
    input_tokens_known_requests: 2, output_tokens_known_requests: 2 };
  await page.route('**/api/dashboard?*', route => route.fulfill({ contentType: 'application/json', body: raw(dashboard) }));
  await page.route('**/api/requests/*', route => route.fulfill({ contentType: 'application/json', body: raw(request) }));
  await page.route('**/api/groups/*', route => route.fulfill({ contentType: 'application/json', body: raw({ group: current, versions: [current, other], requests: [], timeline: [], answers: [] }) }));
  await page.reload();
  await expect(page.locator('.request-link')).toHaveCount(1);
}

for (const { literal, displayed, abbreviated } of [
  { literal: '9007199254740993', displayed: 'About 9,007,199,254,740,992', abbreviated: 'About 9007.2T' },
  { literal: '9223372036854775807', displayed: 'About 9,223,372,036,854,776,000', abbreviated: 'About 9,223,372T' },
]) {
  test(`large token counts mark parsed ${literal} approximate throughout the request view`, async ({ page }) => {
    await largeTokenHarness(page, literal, '0');
    const metric = page.getByRole('group', { name: 'Token usage' });
    await expect(metric.locator('.metric-value')).toHaveText(abbreviated);
    await expect(metric).toContainText(`${abbreviated} in / 0 out`);
    await expect(metric).toContainText('Input: 1 / 1 requests');
    await expect(page.locator('.request-table tbody tr').first().locator('td').nth(4)).toHaveText(displayed);
    if (literal === '9223372036854775807') {
      await page.setViewportSize({ width: 390, height: 844 });
      const containment = await metric.evaluate(element => [...element.querySelectorAll('.metric-value, .metric-foot')].map(child => ({
        width: child.clientWidth, contentWidth: child.scrollWidth,
      })));
      for (const { width, contentWidth } of containment) expect(contentWidth).toBeLessThanOrEqual(width + 1);
      await page.setViewportSize({ width: 1440, height: 1000 });
    }
    await page.locator('.request-link').first().click();
    const dialog = page.getByRole('dialog');
    await expect(dialog.locator('dt').filter({ hasText: /^Input tokens$/ }).locator('..').locator('dd')).toHaveText(displayed);
    await expect(dialog.locator('dt').filter({ hasText: /^Output tokens$/ }).locator('..').locator('dd')).toHaveText('0');
  });
}

test('large token computed totals mark unsafe sums of safe operands approximate', async ({ page }) => {
  await largeTokenHarness(page, '9007199254740991', '2');
  const metric = page.getByRole('group', { name: 'Token usage' });
  await expect(metric.locator('.metric-value')).toHaveText('About 9007.2T');
  await expect(metric.locator('.metric-foot').first()).toHaveText('9007.2T in / 2 out');
  await expect(page.locator('.request-table tbody tr').first().locator('td').nth(4)).toHaveText('About 9,007,199,254,740,992');
  await page.locator('.request-link').first().click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.locator('dt').filter({ hasText: /^Input tokens$/ }).locator('..').locator('dd')).toHaveText('9,007,199,254,740,991');
  await expect(dialog.locator('dt').filter({ hasText: /^Output tokens$/ }).locator('..').locator('dd')).toHaveText('2');
});

for (const { input, output, displayed, abbreviated } of [
  { input: '9007199254740991', output: '0', displayed: '9,007,199,254,740,991', abbreviated: '9007.2T' },
  { input: '0', output: '0', displayed: '0', abbreviated: '0' },
  { input: 'null', output: 'null', displayed: 'Unknown', abbreviated: 'Unknown' },
]) {
  test(`large token formatting preserves the ${input} safe or unknown control`, async ({ page }) => {
    await largeTokenHarness(page, input, output);
    const metric = page.getByRole('group', { name: 'Token usage' });
    await expect(metric.locator('.metric-value')).toHaveText(abbreviated);
    await expect(metric).not.toContainText('About');
    await expect(page.locator('.request-table tbody tr').first().locator('td').nth(4)).toHaveText(displayed);
    await page.locator('.request-link').first().click();
    const dialog = page.getByRole('dialog');
    await expect(dialog.locator('dt').filter({ hasText: /^Input tokens$/ }).locator('..').locator('dd')).toHaveText(displayed);
    await expect(dialog.locator('.detail-grid:not(.compact-fields)')).not.toContainText('About');
  });
}

test('large token approximation keeps backend overflow labels and coverage precedence', async ({ page }) => {
  await largeTokenHarness(page, 'null', '9007199254740993', { input_tokens_overflow: true, input_tokens_known_requests: 1 });
  const metric = page.getByRole('group', { name: 'Token usage' });
  await expect(metric.locator('.metric-value')).toHaveText('Total exceeds range');
  await expect(metric.locator('.metric-foot').first()).toHaveText('Exceeds range in / About 9007.2T out');
  await expect(metric).toContainText('Input: 1 / 1 requests');
  await expect(metric).toContainText('Output: 1 / 1 requests');
});

test('large token version comparisons distinguish approximation from safe counts and overflow', async ({ page }) => {
  await largeTokenHarness(page, '9007199254740993', '0');
  await page.getByRole('button', { name: 'Inspect group', exact: true }).click();
  await page.getByLabel('Compare with').selectOption('safe-comparison');
  const comparison = page.getByLabel('Version outcome comparison');
  const input = comparison.getByRole('row').filter({ has: page.getByRole('rowheader', { name: 'Reported input tokens', exact: true }) });
  const output = comparison.getByRole('row').filter({ has: page.getByRole('rowheader', { name: 'Reported output tokens', exact: true }) });
  await expect(input.getByRole('cell').nth(0)).toHaveText('About 9,007,199,254,740,9921 / 1 requests covered');
  await expect(input.getByRole('cell').nth(1)).toHaveText('9,007,199,254,740,9912 / 2 requests covered');
  await expect(output.getByRole('cell').nth(0)).toHaveText('01 / 1 requests covered');
  await expect(output.getByRole('cell').nth(1)).toHaveText('Total exceeds range2 / 2 requests covered');
});

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

test('malformed retained probabilities remain inspectable without numeric coercion', async ({ page }) => {
  // Generated by the real systemone-capture normalizer from a synthetic Choice
  // response. Invalid probability JSON is retained, including an object whose
  // toString property makes JavaScript numeric coercion throw.
  const malformed = JSON.parse(readFileSync(new URL('./fixtures/malformed-probabilities.json', import.meta.url), 'utf8'));
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  const state = await harness(page);
  state.record = { ...state.record, ...malformed };
  await page.getByLabel('Inspect request request-one', { exact: true }).click();
  const card = page.getByRole('dialog').locator('.answer-card');
  await expect(card.getByText('Invalid or missing answer', { exact: true })).toBeVisible();
  await expect(card).toContainText('Probabilities must be finite numbers between zero and one');
  const rows = card.locator('.probability-row');
  await expect(rows.locator(':scope > span:first-child')).toHaveText([
    'one', 'half', 'zero', 'object_value', 'string_value', 'boolean_value', 'null_value',
    'empty_array', 'numeric_array', 'negative', 'over_one', 'huge',
  ]);
  for (const [key, width, label] of [['one', '100%', '100.0%'], ['half', '50%', '50.0%'], ['zero', '0%', '0.0%']]) {
    const row = rows.filter({ has: page.locator(`span[title="${key}"]`) });
    await expect(row.locator('.mono')).toHaveText(label);
    await expect(row.locator('.probability-track > span')).toHaveAttribute('style', `width: ${width};`);
  }
  for (const key of ['object_value', 'string_value', 'boolean_value', 'null_value', 'empty_array', 'numeric_array', 'negative', 'over_one', 'huge']) {
    const row = rows.filter({ has: page.locator(`span[title="${key}"]`) });
    await expect(row.locator('.mono')).toHaveText('Invalid');
    await expect(row.locator('.probability-track > span')).toHaveCount(0);
  }
  await card.getByText('Inspect raw answer', { exact: true }).click();
  await expect(card.locator('.raw-answer-details .json-view')).toHaveText(JSON.stringify(malformed.answers[0].raw_answer, null, 2));
  await page.keyboard.press('Escape');
  await expect(page.getByLabel('Inspect request request-one', { exact: true })).toBeFocused();
  expect(errors).toEqual([]);
});

test('valid numeric probabilities keep their sorted bars and percentages', async ({ page }) => {
  await harness(page);
  await page.getByLabel('Inspect request request-one', { exact: true }).click();
  const card = page.getByRole('dialog').locator('.answer-card');
  await expect(card.locator('.answer-value')).toHaveText('technical');
  await expect(card.locator('.probability-row > span:first-child')).toHaveText(['technical', 'billing']);
  await expect(card.locator('.probability-row > .mono')).toHaveText(['80.0%', '20.0%']);
  await expect(card.locator('.probability-track > span').nth(0)).toHaveAttribute('style', 'width: 80%;');
  await expect(card.locator('.probability-track > span').nth(1)).toHaveAttribute('style', 'width: 20%;');
  await expect(card.getByText('Inspect raw answer', { exact: true })).toHaveCount(0);
});

test('JSON disclosures release closed content and retain keyboard focus across reviews', async ({ page }) => {
  const state = await harness(page);
  state.record.state_retained = true;
  state.record.state = { message: 'Synthetic retained input', nested: [null, { value: '한글' }] };
  state.record.answers[0] = { ...state.record.answers[0], valid: false, error: 'Synthetic invalid choice', raw_answer: { type: 'choice', choice: 'outside', evidence: [false, { value: '한글' }] } };
  const opener = page.getByLabel('Inspect request request-one', { exact: true });
  await opener.click();
  const drawer = page.getByRole('dialog');
  const definition = drawer.locator('.definition-details');
  const raw = drawer.locator('.raw-answer-details');
  const input = drawer.locator('details').filter({ has: page.getByText('Inspect retained input', { exact: true }) });
  for (const [disclosure, value] of [[definition, state.record.answers[0].definition], [raw, state.record.answers[0].raw_answer], [input, state.record.state]] as const) {
    const summary = disclosure.locator('summary');
    await expect(disclosure.locator('.json-view')).toHaveCount(0);
    await summary.focus();
    await page.keyboard.press('Enter');
    await expect(disclosure.locator('.json-view')).toHaveText(JSON.stringify(value, null, 2));
    await expect(summary).toBeFocused();
    await page.keyboard.press('Space');
    await expect(disclosure.locator('.json-view')).toHaveCount(0);
    await expect(summary).toBeFocused();
  }
  const review = page.getByLabel('Review department');
  await review.focus();
  await review.selectOption('correct');
  await expect(drawer.getByRole('status')).toHaveText('Saved');
  await expect(review).toBeFocused();
  await expect(drawer.locator('details .json-view')).toHaveCount(0);
  await raw.locator('summary').click();
  await review.focus();
  await review.selectOption('incorrect');
  await expect(drawer.getByRole('status')).toHaveText('Saved');
  await expect(review).toBeFocused();
  await expect(raw).toHaveAttribute('open', '');
  await expect(raw.locator('.json-view')).toHaveText(JSON.stringify(state.record.answers[0].raw_answer, null, 2));
  await expect(definition.locator('.json-view')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(opener).toBeFocused();
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

for (const succeeds of [true, false]) {
  for (const reopenBeforeCompletion of [true, false]) {
    test(`credential registration ${succeeds ? 'success' : 'failure'} survives ${reopenBeforeCompletion ? 'reopening while pending' : 'completion while closed'}`, async ({ page }) => {
      const state = await harness(page);
      let release!: () => void;
      const pending = new Promise<void>(resolve => { release = resolve; });
      let writes = 0;
      await page.route('**/api/credentials', async route => {
        if (route.request().method() !== 'PUT') return route.fallback();
        writes += 1;
        await pending;
        return succeeds ? route.fallback() : route.fulfill({ status: 503, json: { error: 'Credential store is locked' } });
      });
      try {
        await page.getByRole('button', { name: 'Connect an application' }).click();
        await page.getByLabel('TypeSafe API key').fill('provider-secret');
        await page.getByRole('button', { name: 'Register key' }).click();
        await expect.poll(() => writes).toBe(1);
        await page.keyboard.press('Escape');
        await expect(page.getByRole('dialog')).toHaveCount(0);
        if (reopenBeforeCompletion) {
          await page.getByRole('button', { name: 'Connect an application' }).click();
          await expect(page.getByLabel('TypeSafe API key')).toBeDisabled();
          await expect(page.getByRole('button', { name: 'Working…' })).toBeDisabled();
        }
        const response = page.waitForResponse(value => value.url().endsWith('/api/credentials') && value.request().method() === 'PUT');
        release();
        await response;
        if (!reopenBeforeCompletion) await page.getByRole('button', { name: 'Connect an application' }).click();
        if (succeeds) {
          await expect(page.getByText('Copy this local client token now.')).toBeVisible();
          await expect(page.getByRole('dialog')).toContainText(state.clientToken);
          await expect(page.getByText('Registered key: this session only')).toBeVisible();
          await expect(page.getByLabel('TypeSafe API key')).toHaveValue('');
        } else {
          await expect(page.getByText('Credential store is locked')).toBeVisible();
          await expect(page.getByText('No key registered in Observer.')).toBeVisible();
          await expect(page.getByRole('dialog')).not.toContainText('jo_local_');
        }
        await expect(page.getByLabel('TypeSafe API key')).toBeEnabled();
        expect(writes).toBe(1);
        expect(await page.evaluate(() => [JSON.stringify(localStorage), JSON.stringify(sessionStorage)].join(' '))).not.toMatch(/provider-secret|jo_local_/);
      } finally { release(); }
    });
  }
}

for (const status of ['absent', 'configured', 'failed'] as const) {
  test(`credential status after reopening reconciles ${status} status with the remembered token`, async ({ page }, testInfo) => {
    const state = await harness(page);
    let reads = 0, writes = 0, laterReads = 0, delivered = 0, reopening = false;
    let release!: () => void;
    const pending = new Promise<void>(resolve => { release = resolve; });
    await page.route('**/api/credentials', async route => {
      if (route.request().method() !== 'GET') { writes += 1; return route.fallback(); }
      reads += 1;
      if (!reopening) return route.fallback();
      laterReads += 1;
      await pending;
      delivered += 1;
      if (status === 'failed') return route.fulfill({ status: 503, json: { error: 'Credential status temporarily unavailable' } });
      return route.fallback();
    });
    try {
      await page.getByRole('button', { name: 'Connect an application' }).click();
      await expect(page.getByText('No key registered in Observer.')).toBeVisible();
      await page.getByLabel('TypeSafe API key').fill('synthetic-status-key');
      await page.getByRole('button', { name: 'Register key', exact: true }).click();
      await expect(page.getByText('Copy this local client token now.')).toBeVisible();
      await expect(page.locator('.credential-token .copy-field code')).toHaveText(state.clientToken);
      await expect(page.getByText('Key available until Observer stops.', { exact: true })).toBeVisible();
      const originalToken = state.clientToken;
      expect(originalToken).not.toBe('');
      expect(writes).toBe(1);
      expect(reads).toBe(1);
      await page.keyboard.press('Escape');
      await expect(page.getByRole('dialog')).toHaveCount(0);
      // Models another dashboard removing the registration, or a session-only
      // credential disappearing on restart while this browser remains loaded.
      if (status === 'absent') {
        state.credentials = { configured: false, storage: 'none' };
        state.providerKey = ''; state.clientToken = '';
      }
      reopening = true;
      await page.getByRole('button', { name: 'Connect an application' }).click();
      await expect.poll(() => laterReads).toBe(1);
      await expect(page.locator('.credential-token .copy-field code')).toHaveText(originalToken);
      release();
      if (status === 'absent') await expect(page.getByText('No key registered in Observer.')).toBeVisible();
      else if (status === 'failed') await expect(page.getByRole('alert')).toContainText('Credential status temporarily unavailable');
      else await expect(page.getByText('Registered key: this session only')).toBeVisible();
      await expect.poll(() => delivered).toBe(1);
      expect(reads).toBe(2);
      expect(writes).toBe(1);
      const observation = { status, reads, writes, laterReads,
        rememberedTokenVisible: await page.locator('.credential-token').count() === 1,
        priorSuccessVisible: await page.getByText('Key available until Observer stops.', { exact: true }).count() === 1,
      };
      await testInfo.attach('credential-status-reconciliation.json', { body: JSON.stringify(observation, null, 2), contentType: 'application/json' });
      if (status === 'absent') {
        await expect(page.locator('.credential-token')).toHaveCount(0);
        await expect(page.getByText('Key available until Observer stops.', { exact: true })).toHaveCount(0);
      } else await expect(page.locator('.credential-token .copy-field code')).toHaveText(originalToken);
    } finally { release(); }
  });
}

test('a failed replacement keeps the valid token and blocks overlapping mutations after reopening', async ({ page }) => {
  const state = await harness(page);
  await page.getByRole('button', { name: 'Connect an application' }).click();
  await page.getByLabel('TypeSafe API key').fill('original-key');
  await page.getByRole('button', { name: 'Register key' }).click();
  await expect(page.getByText('Copy this local client token now.')).toBeVisible();
  const originalToken = state.clientToken;
  let release!: () => void;
  const pending = new Promise<void>(resolve => { release = resolve; });
  const writes: string[] = [];
  await page.route('**/api/credentials', async route => {
    if (route.request().method() === 'GET') return route.fallback();
    writes.push(route.request().method());
    await pending;
    return route.fulfill({ status: 503, json: { error: 'Replacement could not be saved' } });
  });
  try {
    await page.getByLabel('TypeSafe API key').fill('replacement-key');
    await page.getByRole('button', { name: 'Replace registered key' }).click();
    await expect.poll(() => writes).toEqual(['PUT']);
    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: 'Connect an application' }).click();
    await expect(page.getByLabel('TypeSafe API key')).toBeDisabled();
    await expect(page.getByRole('button', { name: 'Remove key' })).toBeDisabled();
    await expect(page.getByRole('dialog')).toContainText(originalToken);
    release();
    await expect(page.getByText('Replacement could not be saved')).toBeVisible();
    await expect(page.getByRole('dialog')).toContainText(originalToken);
    await expect(page.getByRole('button', { name: 'Remove key' })).toBeEnabled();
    expect(state.providerKey).toBe('original-key');
    expect(writes).toEqual(['PUT']);
  } finally { release(); }
});

test('a failed credential replacement keeps the original token until the page reloads', async ({ page }) => {
  const state = await harness(page);
  await page.getByRole('button', { name: 'Connect an application' }).click();
  await page.getByLabel('TypeSafe API key').fill('original-key');
  await page.getByRole('button', { name: 'Register key' }).click();
  await expect(page.getByText('Copy this local client token now.')).toBeVisible();
  const originalToken = state.clientToken;
  await page.route('**/api/credentials', route => route.request().method() === 'PUT'
    ? route.fulfill({ status: 503, json: { error: 'Replacement could not be saved' } }) : route.fallback());
  await page.getByLabel('TypeSafe API key').fill('replacement-key');
  await page.getByRole('button', { name: 'Replace registered key' }).click();
  await expect(page.getByText('Replacement could not be saved')).toBeVisible();
  await expect(page.locator('.credential-token .copy-field code')).toHaveText(originalToken);
  expect(state.providerKey).toBe('original-key');
  await page.reload();
  await page.getByRole('button', { name: 'Connect an application' }).click();
  await expect(page.getByText('Registered key: this session only')).toBeVisible();
  await expect(page.getByRole('dialog')).not.toContainText(originalToken);
});

for (const succeeds of [true, false]) {
  test(`credential removal ${succeeds ? 'success' : 'failure'} survives reopening and preserves the correct token`, async ({ page }) => {
    const state = await harness(page);
    await page.getByRole('button', { name: 'Connect an application' }).click();
    await page.getByLabel('TypeSafe API key').fill('original-key');
    await page.getByRole('button', { name: 'Register key' }).click();
    await expect(page.getByText('Copy this local client token now.')).toBeVisible();
    const originalToken = state.clientToken;
    let release!: () => void;
    const pending = new Promise<void>(resolve => { release = resolve; });
    const writes: string[] = [];
    await page.route('**/api/credentials', async route => {
      if (route.request().method() === 'GET') return route.fallback();
      writes.push(route.request().method());
      await pending;
      return succeeds ? route.fallback() : route.fulfill({ status: 503, json: { error: 'Removal could not be saved' } });
    });
    try {
      await page.getByRole('button', { name: 'Remove key' }).click();
      await page.getByRole('button', { name: 'Confirm removal' }).click();
      await expect.poll(() => writes).toEqual(['DELETE']);
      await page.keyboard.press('Escape');
      await page.getByRole('button', { name: 'Connect an application' }).click();
      await expect(page.getByLabel('TypeSafe API key')).toBeDisabled();
      await expect(page.getByRole('button', { name: 'Remove key' })).toBeDisabled();
      release();
      if (succeeds) {
        await expect(page.getByText('Registered provider key removed.')).toBeVisible();
        await expect(page.getByText('No key registered in Observer.')).toBeVisible();
        await expect(page.getByRole('dialog')).not.toContainText(originalToken);
      } else {
        await expect(page.getByText('Removal could not be saved')).toBeVisible();
        await expect(page.getByRole('dialog')).toContainText(originalToken);
        await expect(page.getByRole('button', { name: 'Remove key' })).toBeEnabled();
      }
      await expect(page.getByLabel('TypeSafe API key')).toBeEnabled();
      expect(writes).toEqual(['DELETE']);
    } finally { release(); }
  });
}

test('a credential status read started before registration cannot replace its successful result', async ({ page }) => {
  const state = await harness(page);
  let release!: () => void;
  const pending = new Promise<void>(resolve => { release = resolve; });
  let statusReads = 0;
  await page.route('**/api/credentials', async route => {
    if (route.request().method() !== 'GET') return route.fallback();
    statusReads += 1;
    await pending;
    return route.fulfill({ json: { configured: false, storage: 'none' } });
  });
  try {
    await page.getByRole('button', { name: 'Connect an application' }).click();
    await expect.poll(() => statusReads).toBe(1);
    await page.getByLabel('TypeSafe API key').fill('provider-secret');
    await page.getByLabel('Save in this computer’s credential store').check();
    const cancelled = page.waitForEvent('requestfailed', request => request.url().endsWith('/api/credentials') && request.method() === 'GET');
    await page.getByRole('button', { name: 'Register key' }).click();
    await cancelled;
    await expect(page.getByText('Registered key: saved in system credential store')).toBeVisible();
    release();
    await expect(page.getByRole('dialog')).toContainText(state.clientToken);
    await expect(page.getByText('Registered key: saved in system credential store')).toBeVisible();
    await expect(page.getByText('No key registered in Observer.')).toHaveCount(0);
    await expect(page.getByLabel('Save in this computer’s credential store')).toBeChecked();
  } finally { release(); }
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
      await page.clock.runFor(endpoint === 'dashboard' ? 44_999 : 14_999);
      expect(started).toBe(1);
      await page.clock.runFor(1);
      if (endpoint === 'dashboard') {
        await expect(page.getByRole('alert')).toContainText('Request timed out after 45 seconds');
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

function manyDecisionRecord(id = 'request-one') {
  return { ...structuredClone(record), id, answer_count: 53,
    answers: Array.from({ length: 53 }, (_, index) => ({ ...structuredClone(record.answers[0]), key: `decision-${index + 1}` })),
  };
}

test('decision pages bound mounted cards and keep every answer reachable by keyboard', async ({ page }) => {
  const state = await harness(page);
  state.record = manyDecisionRecord();
  let detailReads = 0;
  await page.route('**/api/requests/*', route => { detailReads += 1; return route.fulfill({ json: state.record }); });
  await page.getByLabel('Inspect request request-one', { exact: true }).click();
  await expect(page.locator('.answer-card')).toHaveCount(50);
  const pages = page.getByRole('navigation', { name: 'Decision pages' });
  await expect(pages).toContainText('1–50 of 53');
  await expect(pages.getByRole('button', { name: 'Previous decisions' })).toBeDisabled();
  const readsBeforeNavigation = detailReads;
  await pages.getByRole('button', { name: 'Next decisions' }).focus();
  await page.keyboard.press('Enter');
  await expect(pages).toContainText('51–53 of 53');
  await expect(page.locator('.answer-card')).toHaveCount(3);
  await expect(page.getByRole('heading', { name: 'decision-53', exact: true })).toBeVisible();
  await expect(pages.getByRole('button', { name: 'Next decisions' })).toBeDisabled();
  await expect(pages.getByRole('button', { name: 'Next decisions' })).toBeFocused();
  await page.keyboard.press('Enter');
  await page.keyboard.press('Space');
  await pages.getByRole('button', { name: 'Next decisions' }).click({ force: true });
  await expect(pages).toContainText('51–53 of 53');
  await page.locator('.answer-card').last().getByText('Definition and context', { exact: true }).click();
  await expect(page.locator('.answer-card').last().locator('.json-view')).toContainText('Choose a department');
  await pages.getByRole('button', { name: 'Previous decisions' }).focus();
  await page.keyboard.press('Enter');
  await expect(pages).toContainText('1–50 of 53');
  await expect(page.getByRole('heading', { name: 'decision-1', exact: true })).toBeVisible();
  await expect(pages.getByRole('button', { name: 'Previous decisions' })).toBeFocused();
  await page.keyboard.press('Enter');
  await page.keyboard.press('Space');
  await pages.getByRole('button', { name: 'Previous decisions' }).click({ force: true });
  await expect(pages).toContainText('1–50 of 53');
  expect(detailReads).toBe(readsBeforeNavigation);
  await pages.getByRole('button', { name: 'Next decisions' }).click();
  await expect(pages).toContainText('51–53 of 53');
  await page.keyboard.press('Escape');
  await expect(page.getByLabel('Inspect request request-one', { exact: true })).toBeFocused();
  state.record = manyDecisionRecord('request-failed');
  await page.getByLabel('Inspect request request-failed', { exact: true }).click();
  await expect(pages).toContainText('1–50 of 53');
  await expect(page.getByRole('heading', { name: 'decision-1', exact: true })).toBeVisible();
});

test('decision paging waits for every pending review and preserves saved labels after failures', async ({ page }) => {
  const state = await harness(page);
  state.record = manyDecisionRecord();
  let releaseFirst!: () => void, releaseSecond!: () => void;
  const first = new Promise<void>(resolve => { releaseFirst = resolve; });
  const second = new Promise<void>(resolve => { releaseSecond = resolve; });
  let failSecond = true, saves = 0;
  await page.route('**/api/requests/*/label', async route => {
    saves += 1;
    const payload = route.request().postDataJSON();
    await (payload.key === 'decision-1' ? first : second);
    if (payload.key === 'decision-2' && failSecond) return route.fulfill({ status: 503, json: { error: 'Second decision review failed' } });
    state.record.labels = [...state.record.labels.filter(item => item.key !== payload.key), payload];
    await route.fulfill({ json: { ok: true } });
  });
  try {
    await page.getByLabel('Inspect request request-one', { exact: true }).click();
    const pages = page.getByRole('navigation', { name: 'Decision pages' });
    const firstCard = page.locator('.answer-card').filter({ has: page.getByRole('heading', { name: 'decision-1', exact: true }) });
    const secondCard = page.locator('.answer-card').filter({ has: page.getByRole('heading', { name: 'decision-2', exact: true }) });
    await page.getByLabel('Review decision-1', { exact: true }).selectOption('correct');
    await page.getByLabel('Review decision-2', { exact: true }).selectOption('incorrect');
    await expect.poll(() => saves).toBe(2);
    await expect(pages.getByRole('button', { name: 'Next decisions' })).toBeDisabled();
    await pages.getByRole('button', { name: 'Next decisions' }).focus();
    await page.keyboard.press('Enter');
    await page.keyboard.press('Space');
    await pages.getByRole('button', { name: 'Next decisions' }).click({ force: true });
    await expect(pages).toContainText('1–50 of 53');
    releaseFirst();
    await expect(firstCard.getByRole('status')).toHaveText('Saved');
    await expect(pages.getByRole('button', { name: 'Next decisions' })).toBeDisabled();
    releaseSecond();
    await expect(secondCard.getByRole('alert')).toContainText('Second decision review failed');
    await expect(pages.getByRole('button', { name: 'Next decisions' })).toBeEnabled();
    await expect(page.getByLabel('Review decision-1', { exact: true })).toHaveValue('correct');
    failSecond = false;
    await page.getByLabel('Review decision-2', { exact: true }).selectOption('correct');
    await expect(secondCard.getByRole('status')).toHaveText('Saved');
    await expect(secondCard.getByRole('alert')).toHaveCount(0);
    await pages.getByRole('button', { name: 'Next decisions' }).click();
    await expect(pages).toContainText('51–53 of 53');
    await pages.getByRole('button', { name: 'Previous decisions' }).click();
    await expect(page.getByLabel('Review decision-1', { exact: true })).toHaveValue('correct');
    await expect(page.getByLabel('Review decision-2', { exact: true })).toHaveValue('correct');
    expect(saves).toBe(3);
  } finally { releaseFirst(); releaseSecond(); }
});

test('request labels persist through API and versions remain separate', async ({ page }) => {
  const state = await harness(page);
  await page.getByLabel('Inspect request request-one', { exact: true }).click();
  await page.getByLabel('Review department').selectOption('incorrect');
  await expect(page.getByLabel('Review department')).toHaveValue('incorrect');
  expect(state.labelHeaders?.['x-observer-request']).toBe('1');
  await expect(page.getByText('Not collected. An API response')).toBeVisible();
  await page.getByRole('button', { name: 'View recurring group' }).click();
  const definition = page.getByRole('dialog').locator('details').filter({ has: page.getByText('Inspect definition', { exact: true }) });
  await expect(definition.locator('.json-view')).toHaveCount(0);
  await definition.locator('summary').focus();
  await page.keyboard.press('Enter');
  await expect(definition.locator('.json-view')).toHaveText(JSON.stringify(group.definition, null, 2));
  await page.keyboard.press('Space');
  await expect(definition.locator('.json-view')).toHaveCount(0);
  await expect(definition.locator('summary')).toBeFocused();
  await page.getByLabel('Compare with').selectOption('group_v1_2222222222');
  const comparison = page.locator('.version-comparison details');
  await expect(comparison.locator('.json-view')).toHaveCount(0);
  await page.getByText('Compare definitions and context', { exact: true }).click();
  await expect(comparison.locator('.json-view')).toHaveText([JSON.stringify(group.definition, null, 2), JSON.stringify({ instructions: 'Changed department rules' }, null, 2)]);
  await expect(page.getByText('Changed department rules')).toBeVisible();
  await expect(page.getByText('Presentation: 2222222222')).toBeVisible();
  await page.keyboard.press('Space');
  await expect(comparison.locator('.json-view')).toHaveCount(0);
  await expect(comparison.locator('summary')).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
});

for (const paused of [false, true]) {
  test(`a 23-second dashboard query stays sequential and completes with the visible view ${paused ? 'paused' : 'live'}`, async ({ page }) => {
    await page.clock.install({ time: new Date(now) });
    await page.clock.pauseAt(new Date(now + 1000));
    const state = await harness(page);
    await expect(page.getByLabel('Inspect request request-one', { exact: true })).toBeVisible();
    if (paused) await page.getByRole('button', { name: 'Live updates', exact: true }).click();
    let finish!: () => void, queries = 0, healthReads = 0;
    const pending = new Promise<void>(resolve => { finish = resolve; });
    const updated = { ...structuredClone(state.dashboard), requests: [{ ...record, id: 'after-slow-query' }] };
    await page.route('**/api/dashboard?**', async route => { queries += 1; await pending; await route.fulfill({ json: updated }); });
    await page.route('**/api/health', route => { healthReads += 1; return route.fulfill({ json: { ...state.dashboard.health, dropped: healthReads } }); });
    const notice = page.getByRole('status').filter({ hasText: 'History query is still running' });
    try {
      await page.getByRole('button', { name: 'Refresh dashboard' }).click();
      await expect.poll(() => queries).toBe(1);
      await page.clock.runFor(4999);
      await expect(notice).toHaveCount(0);
      await page.clock.runFor(1);
      await expect(notice).toBeVisible();
      await expect(notice).toContainText('narrow the time window or filters');
      await expect.poll(() => healthReads).toBeGreaterThan(0);
      const observedHealthReads = healthReads;
      // Let the real response finish before advancing its next virtual poll.
      await expect(page.locator('.health-fields').getByText(String(observedHealthReads), { exact: true })).toBeVisible();
      await page.clock.runFor(1000);
      await expect.poll(() => healthReads).toBeGreaterThan(observedHealthReads);
      await page.clock.runFor(17_000);
      expect(queries, 'A useful slow query should neither time out nor start an overlapping retry').toBe(1);
      expect(healthReads).toBeGreaterThan(1);
      await expect(page.getByRole('alert')).toHaveCount(0);
      finish();
      await expect(notice).toHaveCount(0);
      if (paused) {
        await expect(page.locator('.pause-notice')).toContainText('visible dashboard is paused');
        await expect(page.getByLabel('Inspect request after-slow-query')).toHaveCount(0);
        await page.getByRole('button', { name: 'Resume live', exact: true }).click();
      }
      await expect(page.getByLabel('Inspect request after-slow-query')).toBeVisible();
      expect(queries).toBe(1);
    } finally { finish(); }
  });
}

test('a slow dashboard query clears its notice on scope change and ignores the old response', async ({ page }) => {
  await page.clock.install({ time: new Date(now) });
  await page.clock.pauseAt(new Date(now + 1000));
  const state = await harness(page);
  let finish!: () => void, slowReads = 0;
  const pending = new Promise<void>(resolve => { finish = resolve; });
  await page.route('**/api/dashboard?**', async route => {
    if (new URL(route.request().url()).searchParams.get('window') === 'all') {
      slowReads += 1; await pending;
      return route.fulfill({ json: { ...state.dashboard, requests: [{ ...record, id: 'stale-slow-scope' }] } });
    }
    return route.fulfill({ json: { ...state.dashboard, requests: [{ ...record, id: 'current-short-scope' }] } });
  });
  const notice = page.getByRole('status').filter({ hasText: 'History query is still running' });
  try {
    await page.getByLabel('Time window').selectOption('all');
    await expect.poll(() => slowReads).toBe(1);
    await page.clock.runFor(5000);
    await expect(notice).toBeVisible();
    const canceled = page.waitForEvent('requestfailed', request => new URL(request.url()).pathname === '/api/dashboard' && new URL(request.url()).searchParams.get('window') === 'all');
    await page.getByLabel('Time window').selectOption('1h');
    await canceled;
    await expect(notice).toHaveCount(0);
    await expect(page.getByLabel('Inspect request current-short-scope')).toBeVisible();
    finish();
    await page.clock.runFor(5000);
    await expect(notice).toHaveCount(0);
    await expect(page.getByLabel('Inspect request stale-slow-scope')).toHaveCount(0);
    await expect(page.getByRole('alert')).toHaveCount(0);
  } finally { finish(); }
});

test('slow dashboard notices reset on manual refresh, hidden tabs, and request failure', async ({ page }) => {
  await page.clock.install({ time: new Date(now) });
  await page.clock.pauseAt(new Date(now + 1000));
  const state = await harness(page);
  let finish!: () => void, queries = 0, fail = false;
  const pending = new Promise<void>(resolve => { finish = resolve; });
  await page.route('**/api/dashboard?**', async route => {
    queries += 1; await pending;
    await route.fulfill(fail ? { status: 503, json: { error: 'History unavailable for this control' } } : { json: state.dashboard });
  });
  const notice = page.getByRole('status').filter({ hasText: 'History query is still running' });
  try {
    await page.getByRole('button', { name: 'Refresh dashboard' }).click();
    await expect.poll(() => queries).toBe(1);
    await page.clock.runFor(5000);
    await expect(notice).toBeVisible();
    await page.getByRole('button', { name: 'Refresh dashboard' }).click();
    await expect.poll(() => queries).toBe(2);
    await expect(notice).toHaveCount(0);
    await page.clock.runFor(4999);
    await expect(notice).toHaveCount(0);
    await page.evaluate(() => { Object.defineProperty(document, 'hidden', { configurable: true, value: true }); document.dispatchEvent(new Event('visibilitychange')); });
    await page.clock.runFor(5001);
    await expect(notice).toHaveCount(0);
    expect(queries).toBe(2);
    await page.evaluate(() => { Object.defineProperty(document, 'hidden', { configurable: true, value: false }); document.dispatchEvent(new Event('visibilitychange')); });
    await expect.poll(() => queries).toBe(3);
    await page.clock.runFor(5000);
    await expect(notice).toBeVisible();
    fail = true; finish();
    await expect(page.getByRole('alert')).toContainText('History unavailable for this control');
    await expect(notice).toHaveCount(0);
  } finally { finish(); }
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

for (const reopenBeforeCompletion of [false, true]) {
  test(`closed deletion preserves failure when Settings reopens ${reopenBeforeCompletion ? 'before' : 'after'} completion`, async ({ page }) => {
    const state = await harness(page);
    let finish!: () => void, writes = 0;
    const pending = new Promise<void>(resolve => { finish = resolve; });
    await page.route('**/api/data', async route => {
      writes += 1;
      await pending;
      return route.fulfill({ status: 503, json: { error: 'History could not be deleted' } });
    });
    try {
      await page.getByRole('button', { name: 'Open settings', exact: true }).click();
      await page.getByRole('button', { name: 'Delete history', exact: true }).click();
      await page.getByLabel('Type DELETE to confirm').fill('DELETE');
      await page.getByRole('button', { name: 'Permanently delete history' }).click();
      await expect(page.getByRole('button', { name: 'Deleting…' })).toBeDisabled();
      await page.keyboard.press('Escape');
      if (reopenBeforeCompletion) {
        await page.getByRole('button', { name: 'Open settings', exact: true }).click();
        await expect(page.getByRole('button', { name: 'Deleting…' })).toBeDisabled();
        await expect(page.getByLabel('Type DELETE to confirm')).toHaveCount(0);
      }
      const finished = page.waitForEvent('requestfinished', request => new URL(request.url()).pathname === '/api/data');
      finish(); await finished;
      if (!reopenBeforeCompletion) await page.getByRole('button', { name: 'Open settings', exact: true }).click();
      await expect(page.getByRole('dialog').getByRole('alert')).toContainText('History could not be deleted');
      await expect(page.getByRole('button', { name: 'Delete history', exact: true })).toBeEnabled();
      expect(writes).toBe(1); expect(state.deleted).toBe(false);
      await page.keyboard.press('Escape');
      await expect(page.getByLabel('Inspect request request-one', { exact: true })).toBeVisible();
    } finally { finish(); }
  });
}

test('reopened pending deletion prevents another write and preserves the new Settings panel on success', async ({ page }) => {
  const state = await harness(page);
  let finish!: () => void, writes = 0;
  state.deletePending = new Promise(resolve => { finish = resolve; });
  await page.route('**/api/data', route => { writes += 1; return route.fallback(); });
  try {
    await page.getByRole('button', { name: 'Live updates', exact: true }).click();
    await page.getByRole('button', { name: 'Open settings', exact: true }).click();
    await page.getByRole('button', { name: 'Delete history', exact: true }).click();
    await page.getByLabel('Type DELETE to confirm').fill('DELETE');
    await page.getByRole('button', { name: 'Permanently delete history' }).click();
    await expect.poll(() => writes).toBe(1);
    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: 'Open settings', exact: true }).click();
    const pendingButton = page.getByRole('button', { name: 'Deleting…' });
    await expect(pendingButton).toBeDisabled();
    await pendingButton.evaluate(button => (button as HTMLButtonElement).click());
    await expect(page.getByLabel('Type DELETE to confirm')).toHaveCount(0);
    expect(writes).toBe(1);
    finish();
    await expect(page.locator('.toast')).toContainText('Local history deleted');
    await expect(page.getByRole('heading', { name: 'Local by design.' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Delete history', exact: true })).toBeEnabled();
    expect(writes).toBe(1);
    await page.keyboard.press('Escape');
    await expect(page.getByRole('heading', { name: 'Your workspace is ready' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Resume live', exact: true })).toBeVisible();
  } finally { finish(); }
});

test('deletion retry clears the previous failure and refreshes once after rapid confirmation', async ({ page }) => {
  const state = await harness(page);
  let finish!: () => void, writes = 0, detailReads = 0;
  const pending = new Promise<void>(resolve => { finish = resolve; });
  await page.route('**/api/data', async route => {
    writes += 1;
    if (writes === 1) return route.fulfill({ status: 503, json: { error: 'History could not be deleted' } });
    await pending;
    return route.fallback();
  });
  await page.route('**/api/requests/request-one', route => {
    detailReads += 1;
    return state.deleted ? route.fulfill({ status: 404, json: { error: 'Record not found' } }) : route.fulfill({ json: state.record });
  });
  try {
    await page.getByRole('button', { name: 'Live updates', exact: true }).click();
    await page.getByRole('button', { name: 'Open settings', exact: true }).click();
    await page.getByRole('button', { name: 'Delete history', exact: true }).click();
    await page.getByLabel('Type DELETE to confirm').fill('DELETE');
    await page.getByRole('button', { name: 'Permanently delete history' }).click();
    await expect(page.getByRole('alert')).toContainText('History could not be deleted');
    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: 'Open settings', exact: true }).click();
    await expect(page.getByRole('alert')).toContainText('History could not be deleted');
    await page.getByRole('button', { name: 'Delete history', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Permanently delete history' })).toBeDisabled();
    await page.getByLabel('Type DELETE to confirm').fill('DELETE');
    await page.getByRole('button', { name: 'Permanently delete history' }).evaluate(button => {
      (button as HTMLButtonElement).click(); (button as HTMLButtonElement).click();
    });
    await expect(page.getByRole('button', { name: 'Deleting…' })).toBeDisabled();
    await expect(page.getByRole('alert')).toHaveCount(0);
    await expect.poll(() => writes).toBe(2);
    await page.keyboard.press('Escape');
    await page.getByLabel('Inspect request request-one', { exact: true }).click();
    await expect(page.locator('.request-detail-id')).toBeVisible();
    const initialReads = detailReads;
    finish();
    await expect(page.getByRole('dialog').getByRole('alert')).toContainText('Record not found');
    expect(detailReads).toBe(initialReads + 1); expect(writes).toBe(2);
    await expect(page.locator('.toast')).toContainText('Local history deleted');
    await page.keyboard.press('Escape');
    await expect(page.getByRole('button', { name: 'Resume live', exact: true })).toBeVisible();
    await expect(page.getByLabel('Inspect request request-one', { exact: true })).toHaveCount(0);
  } finally { finish(); }
});

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

for (const detail of ['request', 'group'] as const) {
  test(`a completed deletion invalidates a newly opened ${detail} while paused`, async ({ page }) => {
    const state = await harness(page, { path: '/?source=Test+inbox' });
    const path = detail === 'request' ? '/api/requests/request-one' : `/api/groups/${group.id}`;
    let reads = 0, finish!: () => void;
    await page.route(`**${path}*`, route => {
      reads += 1;
      return state.deleted ? route.fulfill({ status: 404, json: { error: 'Record not found' } }) : route.fulfill({ json: detail === 'request' ? state.record : { group, versions: [group], requests: [], answers: [], timeline: [] } });
    });
    await page.getByRole('button', { name: 'Live updates', exact: true }).click();
    state.deletePending = new Promise(resolve => { finish = resolve; });
    await page.getByRole('button', { name: 'Open settings', exact: true }).click();
    await page.getByRole('button', { name: 'Delete history', exact: true }).click();
    await page.getByLabel('Type DELETE to confirm').fill('DELETE');
    await page.getByRole('button', { name: 'Permanently delete history' }).click();
    await expect(page.getByRole('button', { name: 'Deleting…' })).toBeDisabled();
    await page.keyboard.press('Escape');
    await page.getByLabel(detail === 'request' ? 'Inspect request request-one' : 'Inspect group department', { exact: true }).click();
    const drawer = page.getByRole('dialog');
    await expect(drawer.locator(detail === 'request' ? '.request-detail-id' : '.group-detail-meta')).toBeVisible();
    const initialReads = reads;
    finish();
    await expect(drawer.getByRole('alert')).toContainText('Record not found');
    expect(reads).toBe(initialReads + 1);
    await expect(drawer.getByRole('button', { name: 'Retry' })).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.getByRole('button', { name: 'Resume live', exact: true })).toBeVisible();
    await expect(page.getByLabel('Filter by source')).toHaveValue('Test inbox');
    await expect(page).toHaveURL(/source=Test\+inbox/);
    await expect(page.locator('.request-link')).toHaveCount(0);
  });
}

for (const succeeds of [false, true]) {
  test(`a ${succeeds ? 'successful' : 'failed'} delayed import ${succeeds ? 'refreshes' : 'preserves'} a newly opened group`, async ({ page }) => {
    await harness(page, { path: '/?source=Test+inbox' });
    let finish!: () => void, imported = false, reads = 0;
    const pending = new Promise<void>(resolve => { finish = resolve; });
    await page.route('**/api/import', async route => {
      await pending;
      imported = succeeds;
      await route.fulfill(succeeds ? { json: { imported: 7, duplicates: 0 } } : { status: 400, json: { error: 'Invalid source record' } });
    });
    await page.route(`**/api/groups/${group.id}*`, route => {
      reads += 1;
      const current = imported ? { ...group, valid_count: 8, answer_count: 9, request_count: 9 } : group;
      return route.fulfill({ json: { group: current, versions: [current], requests: [], answers: [], timeline: [] } });
    });
    await page.getByRole('button', { name: 'Live updates', exact: true }).click();
    await page.getByRole('button', { name: 'Import records', exact: true }).click();
    await page.getByLabel('Or paste records').fill('{"synthetic":true}');
    await page.getByRole('dialog').getByRole('button', { name: 'Import records', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Importing records…' })).toBeDisabled();
    await page.keyboard.press('Escape');
    await page.getByLabel('Inspect group department', { exact: true }).click();
    const metric = page.getByRole('dialog').locator('.detail-grid > div').filter({ has: page.getByText('Valid answers', { exact: true }) });
    await expect(metric).toContainText('1 / 2');
    const initialReads = reads;
    const finished = page.waitForEvent('requestfinished', request => new URL(request.url()).pathname === '/api/import');
    finish();
    await finished;
    if (succeeds) {
      await expect(metric).toContainText('8 / 9');
      expect(reads).toBe(initialReads + 1);
    } else {
      // Flush the completed fetch's React update before checking no invalidation.
      await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
      await expect(metric).toContainText('1 / 2');
      expect(reads).toBe(initialReads);
    }
    await page.keyboard.press('Escape');
    await expect(page.getByRole('button', { name: 'Resume live', exact: true })).toBeVisible();
    await expect(page.getByLabel('Filter by source')).toHaveValue('Test inbox');
  });
}

for (const outcome of ['success', 'failure'] as const) {
  test(`closed import ${outcome} retains operation ownership and completion feedback on reopening`, async ({ page }) => {
    await harness(page);
    let finish!: () => void, writes = 0, groupReads = 0, imported = false;
    const pending = new Promise<void>(resolve => { finish = resolve; });
    const payloads: { text: string; format: string }[] = [];
    await page.route('**/api/import', async route => {
      writes += 1;
      payloads.push(route.request().postDataJSON());
      if (writes === 1) {
        await pending;
        if (outcome === 'failure') return route.fulfill({ status: 400, json: { error: 'Synthetic import rejected atomically' } });
        imported = true;
        return route.fulfill({ json: { imported: 7, duplicates: 2 } });
      }
      return route.fulfill({ json: { imported: 1, duplicates: 0 } });
    });
    await page.route(`**/api/groups/${group.id}*`, route => {
      groupReads += 1;
      return route.fulfill({ json: { group: { ...group, answer_count: imported ? 9 : 2 }, versions: [group], requests: [], answers: [], timeline: [] } });
    });
    const openImport = page.getByRole('button', { name: 'Import records', exact: true }).first();
    await openImport.click();
    const drawer = page.getByRole('dialog');
    await page.getByLabel('Source format').selectOption('systemone-capture');
    await page.getByLabel('Or paste records').fill('{"first":true}');
    // Rapid duplicate activation must still own exactly one pending write.
    await drawer.getByRole('button', { name: 'Import records', exact: true }).evaluate(button => { (button as HTMLButtonElement).click(); (button as HTMLButtonElement).click(); });
    await expect(page.getByRole('button', { name: 'Importing records…' })).toBeDisabled();
    await expect.poll(() => writes).toBe(1);
    await page.keyboard.press('Escape');
    await openImport.click();
    await expect(page.getByLabel('Source format')).toHaveValue('systemone-capture');
    for (const field of ['Source format', 'Or paste records', 'Choose import file']) await expect(page.getByLabel(field)).toBeDisabled();
    await expect(page.getByLabel('Or paste records')).toHaveValue('');
    await expect(page.getByRole('button', { name: 'Importing records…' })).toBeDisabled();
    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: 'Inspect group', exact: true }).click();
    await expect(drawer.locator('.detail-grid').first()).toContainText('1 / 2');
    const previousReads = groupReads;
    const finished = page.waitForEvent('requestfinished', request => new URL(request.url()).pathname === '/api/import');
    finish(); await finished;
    await expect(drawer.locator('.detail-grid').first()).toContainText(outcome === 'success' ? '1 / 9' : '1 / 2');
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    expect(groupReads).toBe(previousReads + (outcome === 'success' ? 1 : 0));
    await page.keyboard.press('Escape');
    await openImport.click();
    await expect(drawer.getByText(outcome === 'success' ? '7 records imported.' : 'Synthetic import rejected atomically', { exact: true })).toBeVisible();
    if (outcome === 'success') await expect(drawer.getByRole('status')).toContainText('2 duplicates skipped');
    await expect(page.getByLabel('Source format')).toHaveValue('systemone-capture');
    await expect(page.getByLabel('Or paste records')).toHaveValue('');
    await expect(page.getByLabel('Or paste records')).toBeEnabled();
    expect(writes).toBe(1);

    // A deliberate new input clears settled feedback, then a new operation
    // replaces it without restoring the previous completion on another reopen.
    await page.getByLabel('Or paste records').fill('{"second":true}');
    await expect(drawer.getByRole('alert')).toHaveCount(0);
    await expect(drawer.locator('.success-notice')).toHaveCount(0);
    await page.getByLabel('Source format').selectOption('jevrouter-receipt');
    await drawer.getByRole('button', { name: 'Import records', exact: true }).click();
    await expect(drawer.getByRole('status')).toContainText('1 records imported.');
    await page.keyboard.press('Escape');
    await openImport.click();
    await expect(drawer.getByRole('status')).toContainText('1 records imported.');
    await expect(drawer).not.toContainText('7 records imported.');
    await expect(drawer.getByRole('alert')).toHaveCount(0);
    await expect(page.getByLabel('Source format')).toHaveValue('jevrouter-receipt');
    await expect(page.getByLabel('Or paste records')).toHaveValue('');
    expect(payloads).toEqual([{ text: '{"first":true}', format: 'systemone-capture' }, { text: '{"second":true}', format: 'jevrouter-receipt' }]);
  });
}

test('a closed drawer file read cannot replace a newer import input or completion', async ({ page }) => {
  await harness(page);
  await page.evaluate(() => {
    (window as unknown as { finishOldImportRead: (text: string) => void }).finishOldImportRead = () => {};
    File.prototype.arrayBuffer = function () { return new Promise<ArrayBuffer>(resolve => {
      (window as unknown as { finishOldImportRead: (text: string) => void }).finishOldImportRead = text => resolve(new TextEncoder().encode(text).buffer);
    }); };
  });
  const openImport = page.getByRole('button', { name: 'Import records', exact: true }).first();
  await openImport.click();
  await page.getByLabel('Choose import file').setInputFiles({ name: 'old.jsonl', mimeType: 'application/json', buffer: Buffer.from('{"old":true}') });
  await expect(page.getByRole('dialog').getByRole('status')).toContainText('Reading old.jsonl');
  await page.keyboard.press('Escape');
  await openImport.click();
  await page.getByLabel('Or paste records').fill('{"new":true}');
  await page.getByRole('dialog').getByRole('button', { name: 'Import records', exact: true }).click();
  await expect(page.getByRole('dialog').getByRole('status')).toContainText('2 records imported.');
  await page.evaluate(() => (window as unknown as { finishOldImportRead: (text: string) => void }).finishOldImportRead('{"old":true}'));
  await expect(page.getByLabel('Or paste records')).toHaveValue('{"new":true}');
  await expect(page.getByRole('dialog').getByRole('status')).toContainText('2 records imported.');
  await expect(page.getByRole('dialog')).not.toContainText('old.jsonl');
});

for (const removed of [false, true]) {
  for (const dismiss of ['Escape', 'close button'] as const) {
    test(`request drawer focus survives ${removed ? 'deletion of' : 'return to'} its opener after ${dismiss}`, async ({ page }, testInfo) => {
      const state = await harness(page);
      let finishDelete!: () => void, writes = 0, missingReads = 0;
      const deleting = new Promise<void>(resolve => { finishDelete = resolve; });
      if (removed) {
        state.deletePending = deleting;
        await page.route('**/api/data', async route => {
          if (route.request().method() === 'DELETE') writes += 1;
          await route.fallback();
        });
        await page.route('**/api/requests/request-one', async route => {
          if (!state.deleted) return route.fallback();
          missingReads += 1;
          await route.fulfill({ status: 404, json: { error: 'Record not found' } });
        });
      }
      try {
        if (removed) {
          await page.getByRole('button', { name: 'Open settings', exact: true }).click();
          await page.getByRole('button', { name: 'Delete history', exact: true }).click();
          await page.getByLabel('Type DELETE to confirm').fill('DELETE');
          await page.getByRole('button', { name: 'Permanently delete history' }).click();
          await expect(page.getByRole('button', { name: 'Deleting…' })).toBeDisabled();
          await expect.poll(() => writes).toBe(1);
          await page.keyboard.press('Escape');
        }
        const opener = page.getByLabel('Inspect request request-one', { exact: true });
        const originalOpener = await opener.elementHandle();
        expect(originalOpener).not.toBeNull();
        await opener.focus();
        await page.keyboard.press('Enter');
        const drawer = page.getByRole('dialog');
        await expect(drawer.locator('.request-detail-id')).toContainText('request-one');
        if (removed) {
          finishDelete();
          await expect(drawer.getByRole('alert')).toContainText('Record not found');
          await expect.poll(() => originalOpener!.evaluate(element => element.isConnected)).toBe(false);
          await expect(opener).toHaveCount(0);
          expect(missingReads).toBe(1);
          expect(writes).toBe(1);
        }
        const scrollBeforeClose = await page.evaluate(() => window.scrollY);
        if (dismiss === 'Escape') await page.keyboard.press('Escape');
        else await page.getByRole('button', { name: 'Close detail panel' }).click();
        await expect(drawer).toHaveCount(0);
        await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
        const focused = () => page.evaluate(() => {
          const element = document.activeElement as HTMLElement | null;
          return {
            tag: element?.tagName ?? null,
            role: element?.getAttribute('role') ?? null,
            label: element?.getAttribute('aria-label') ?? null,
            text: element?.textContent?.trim().slice(0, 120) ?? '',
            usable: Boolean(element && element !== document.body && element !== document.documentElement
              && element.isConnected && element.getClientRects().length && !element.closest('[aria-hidden="true"], [inert]')),
          };
        });
        const afterClose = await focused();
        if (!removed) await expect(opener).toBeFocused();
        else {
          await expect(page.getByRole('main')).toBeFocused();
          expect(await page.evaluate(() => window.scrollY)).toBe(scrollBeforeClose);
        }
        await page.keyboard.press('Tab');
        const observation = { removed, dismiss, writes, missingReads,
          originalOpenerConnected: await originalOpener!.evaluate(element => element.isConnected),
          afterClose, afterTab: await focused() };
        await testInfo.attach('drawer-opener-focus.json', { body: JSON.stringify(observation, null, 2), contentType: 'application/json' });
        expect(afterClose.usable, JSON.stringify(observation)).toBe(true);
        if (removed) await expect(page.getByRole('button', { name: 'Export', exact: true })).toBeFocused();
      } finally { finishDelete(); }
    });
  }
}

for (const mobileOpener of [true, false]) {
  test(`closing a drawer after its ${mobileOpener ? 'mobile' : 'desktop'} opener becomes hidden restores dashboard focus`, async ({ page }, testInfo) => {
    await page.setViewportSize(mobileOpener ? { width: 390, height: 844 } : { width: 1440, height: 1000 });
    await harness(page);
    const opener = page.getByRole('button', { name: mobileOpener ? 'Open navigation' : 'Open settings', exact: true, includeHidden: true });
    await opener.click();
    if (mobileOpener) await page.getByRole('dialog', { name: 'Workspace navigation' }).getByRole('button', { name: 'Settings', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Local by design.' })).toBeVisible();
    await page.setViewportSize(mobileOpener ? { width: 1440, height: 1000 } : { width: 390, height: 844 });
    await expect(opener).toBeHidden();
    expect(await opener.evaluate(element => element.isConnected)).toBe(true);
    await page.keyboard.press('Escape');
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const observation = { mobileOpener,
      active: await page.evaluate(() => ({ tag: document.activeElement?.tagName, id: document.activeElement?.id })),
      opener: await opener.evaluate(element => ({ connected: element.isConnected, visible: Boolean(element.getClientRects().length) })),
    };
    await testInfo.attach('hidden-drawer-opener-focus.json', { body: JSON.stringify(observation, null, 2), contentType: 'application/json' });
    await expect(page.getByRole('main')).toBeFocused();
    await page.keyboard.press('Tab');
    await expect(page.getByRole('button', { name: 'Export', exact: true })).toBeFocused();
  });
}

test('reopening a panel before the close frame keeps focus inside the new panel', async ({ page }) => {
  await harness(page);
  const opener = page.getByLabel('Inspect request request-one', { exact: true });
  await opener.click();
  await expect(page.getByRole('dialog').locator('.request-detail-id')).toContainText('request-one');
  // Two user-facing button events in one task leave the close frame pending
  // until after the new panel has been requested.
  await page.evaluate(() => {
    document.querySelector<HTMLButtonElement>('[aria-label="Close detail panel"]')!.click();
    document.querySelector<HTMLButtonElement>('[aria-label="Open settings"]')!.click();
  });
  await expect(page.getByRole('heading', { name: 'Local by design.' })).toBeVisible();
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  await expect.poll(() => page.getByRole('dialog').evaluate(element => element.contains(document.activeElement))).toBe(true);
  await expect(opener).not.toBeFocused();
  await expect(page.getByRole('main', { includeHidden: true })).not.toBeFocused();
});

test('a completed deletion discards an older in-flight detail response', async ({ page }) => {
  const state = await harness(page);
  let finishDelete!: () => void, finishDetail!: () => void, oldReads = 0, newReads = 0;
  state.deletePending = new Promise(resolve => { finishDelete = resolve; });
  const pendingDetail = new Promise<void>(resolve => { finishDetail = resolve; });
  await page.route('**/api/requests/request-one', async route => {
    if (state.deleted) {
      newReads += 1;
      await route.fulfill({ status: 404, json: { error: 'Record not found' } });
    } else {
      oldReads += 1;
      await pendingDetail;
      await route.fulfill({ json: state.record });
    }
  });
  await page.getByRole('button', { name: 'Open settings', exact: true }).click();
  await page.getByRole('button', { name: 'Delete history', exact: true }).click();
  await page.getByLabel('Type DELETE to confirm').fill('DELETE');
  await page.getByRole('button', { name: 'Permanently delete history' }).click();
  await expect(page.getByRole('button', { name: 'Deleting…' })).toBeDisabled();
  await page.keyboard.press('Escape');
  await page.getByLabel('Inspect request request-one', { exact: true }).click();
  await expect.poll(() => oldReads).toBeGreaterThan(0);
  finishDelete();
  await expect(page.getByRole('dialog').getByRole('alert')).toContainText('Record not found');
  expect(newReads).toBe(1);
  finishDetail();
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  await expect(page.getByRole('dialog').getByRole('alert')).toContainText('Record not found');
  await expect(page.getByRole('dialog').locator('.request-detail-id')).toHaveCount(0);
});

for (const outcome of ['success', 'failure'] as const) {
  for (const timing of ['before', 'during', 'after'] as const) {
    test(`review ${outcome} ${timing} a history refresh retains its outcome on the same decision page`, async ({ page }) => {
      const state = await harness(page);
      state.record.answers = Array.from({ length: 60 }, (_, index) => ({ ...structuredClone(record.answers[0]), key: `q${String(index).padStart(4, '0')}` }));
      state.record.labels = [{ key: 'q0050', label: 'unknown' }];
      let finishImport!: () => void, finishReview!: () => void, finishDetail!: () => void;
      const importing = new Promise<void>(resolve => { finishImport = resolve; });
      const reviewing = new Promise<void>(resolve => { finishReview = resolve; });
      const refreshing = new Promise<void>(resolve => { finishDetail = resolve; });
      let imported = false, refreshReads = 0, reviewWrites = 0;
      await page.route('**/api/import', async route => { await importing; imported = true; await route.fulfill({ json: { imported: 1, duplicates: 0 } }); });
      await page.route('**/api/requests/request-one', async route => {
        const snapshot = structuredClone(state.record);
        if (imported && ++refreshReads === 1) await refreshing;
        await route.fulfill({ json: snapshot });
      });
      await page.route('**/api/requests/request-one/label', async route => {
        reviewWrites += 1;
        await reviewing;
        if (outcome === 'failure') await route.fulfill({ status: 503, json: { error: 'Review could not be stored' } });
        else { state.record.labels = [{ key: 'q0050', label: 'correct' }]; await route.fulfill({ json: { ok: true } }); }
      });
      await page.getByRole('button', { name: 'Import records', exact: true }).click();
      await page.getByLabel('Or paste records').fill('{"synthetic":true}');
      await page.getByRole('dialog').getByRole('button', { name: 'Import records', exact: true }).click();
      await expect(page.getByRole('button', { name: 'Importing records…' })).toBeDisabled();
      await page.keyboard.press('Escape');
      await page.getByLabel('Inspect request request-one', { exact: true }).click();
      const drawer = page.getByRole('dialog'), pages = drawer.getByRole('navigation', { name: 'Decision pages' });
      await pages.getByRole('button', { name: 'Next decisions' }).press('Enter');
      const review = drawer.getByLabel('Review q0050', { exact: true });
      await review.selectOption('correct');
      await expect(review).toHaveAttribute('aria-disabled', 'true');
      async function completeReview() {
        const saved = page.waitForEvent('requestfinished', request => new URL(request.url()).pathname.endsWith('/label'));
        finishReview(); await saved;
      }
      if (timing === 'before') {
        await completeReview();
        await expect(drawer.getByText(outcome === 'success' ? 'Saved' : 'Review could not be stored', { exact: true })).toBeVisible();
      }
      finishImport();
      await expect.poll(() => refreshReads).toBe(1);
      await expect(drawer.getByRole('status')).toContainText('Loading request');
      if (timing === 'after') {
        finishDetail();
        await expect(review).toHaveAttribute('aria-disabled', 'true');
        await expect(pages.getByRole('button', { name: 'Previous decisions' })).toHaveAttribute('aria-disabled', 'true');
      }
      if (timing !== 'before') await completeReview();
      finishDetail();
      await expect(pages).toContainText('Decisions 51–60 of 60');
      await expect(review).toHaveValue(outcome === 'success' ? 'correct' : 'unknown');
      await expect(review).toHaveAttribute('aria-disabled', 'false');
      await expect(drawer.getByText(outcome === 'success' ? 'Saved' : 'Review could not be stored', { exact: true })).toBeVisible();
      expect(reviewWrites).toBe(1);
      expect(refreshReads).toBe(outcome === 'success' && timing !== 'before' ? 2 : 1);
      await pages.getByRole('button', { name: 'Previous decisions' }).press('Enter');
      await pages.getByRole('button', { name: 'Next decisions' }).press('Enter');
      await expect(review).toHaveValue(outcome === 'success' ? 'correct' : 'unknown');
      await expect(drawer.getByText(outcome === 'success' ? 'Saved' : 'Review could not be stored', { exact: true })).toBeVisible();
    });
  }
}

test('prototype-named answer keys retain enabled review controls and independent outcomes', async ({ page }) => {
  const state = await harness(page);
  const keys = ['__proto__', 'constructor', 'toString'];
  state.record.answers = keys.map(key => ({ ...structuredClone(record.answers[0]), key }));
  await page.getByLabel('Inspect request request-one', { exact: true }).click();
  const drawer = page.getByRole('dialog');
  for (const key of keys) await expect(drawer.getByLabel(`Review ${key}`, { exact: true })).toHaveAttribute('aria-disabled', 'false');
  for (const key of keys) {
    const card = drawer.locator('.answer-card').filter({ has: page.getByRole('heading', { name: key, exact: true }) });
    let finish!: () => void;
    state.labelPending = new Promise(resolve => { finish = resolve; });
    state.labelError = true;
    await card.getByLabel(`Review ${key}`, { exact: true }).selectOption('incorrect');
    await expect(card.getByLabel(`Review ${key}`, { exact: true })).toHaveAttribute('aria-disabled', 'true');
    finish();
    await expect(card.getByRole('alert')).toContainText('Review could not be stored');
    await expect(card.getByLabel(`Review ${key}`, { exact: true })).toHaveAttribute('aria-disabled', 'false');
    state.labelError = false;
    await card.getByLabel(`Review ${key}`, { exact: true }).selectOption('correct');
    await expect(card.getByText('Saved', { exact: true })).toBeVisible();
    await expect(card.getByRole('alert')).toHaveCount(0);
    await expect(card.getByLabel(`Review ${key}`, { exact: true })).toHaveValue('correct');
  }
});

test('parallel reviews survive refresh, retry independently, and stay isolated after closing the request', async ({ page }) => {
  const state = await harness(page);
  state.record.answers = Array.from({ length: 60 }, (_, index) => ({ ...structuredClone(record.answers[0]), key: `q${String(index).padStart(4, '0')}` }));
  let finishImport!: () => void, finishFirst!: () => void, finishSecond!: () => void, finishClosed!: () => void;
  const importing = new Promise<void>(resolve => { finishImport = resolve; });
  const first = new Promise<void>(resolve => { finishFirst = resolve; });
  const second = new Promise<void>(resolve => { finishSecond = resolve; });
  const closed = new Promise<void>(resolve => { finishClosed = resolve; });
  const writes: Record<string, number> = {};
  let failSecond = true, imported = false, refreshReads = 0;
  await page.route('**/api/import', async route => { await importing; imported = true; await route.fulfill({ json: { imported: 1, duplicates: 0 } }); });
  await page.route('**/api/requests/request-one', route => { if (imported) refreshReads += 1; return route.fulfill({ json: state.record }); });
  await page.route('**/api/requests/request-failed', route => route.fulfill({ json: { ...state.record, id: 'request-failed', labels: [] } }));
  await page.route('**/api/requests/request-one/label', async route => {
    const { key, label } = route.request().postDataJSON();
    writes[key] = (writes[key] ?? 0) + 1;
    await (key === 'q0050' ? first : key === 'q0051' ? second : closed);
    if (key === 'q0051' && failSecond) return route.fulfill({ status: 503, json: { error: 'Review could not be stored' } });
    state.record.labels = [...state.record.labels.filter(item => item.key !== key), { key, label }];
    return route.fulfill({ json: { ok: true } });
  });
  await page.getByRole('button', { name: 'Import records', exact: true }).click();
  await page.getByLabel('Or paste records').fill('{"synthetic":true}');
  await page.getByRole('dialog').getByRole('button', { name: 'Import records', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Importing records…' })).toBeDisabled();
  await page.keyboard.press('Escape');
  await page.getByLabel('Inspect request request-one', { exact: true }).click();
  const drawer = page.getByRole('dialog'), pages = drawer.getByRole('navigation', { name: 'Decision pages' });
  await pages.getByRole('button', { name: 'Next decisions' }).click();
  await drawer.getByLabel('Review q0050', { exact: true }).selectOption('correct');
  await drawer.getByLabel('Review q0051', { exact: true }).selectOption('incorrect');
  finishImport();
  await expect.poll(() => refreshReads).toBe(1);
  await expect(drawer.getByLabel('Review q0050', { exact: true })).toHaveAttribute('aria-disabled', 'true');
  await expect(drawer.getByLabel('Review q0051', { exact: true })).toHaveAttribute('aria-disabled', 'true');
  finishFirst();
  await expect(drawer.getByLabel('Review q0050', { exact: true })).toHaveValue('correct');
  await expect(drawer.getByLabel('Review q0050', { exact: true })).toHaveAttribute('aria-disabled', 'false');
  await expect(pages.getByRole('button', { name: 'Previous decisions' })).toHaveAttribute('aria-disabled', 'true');
  finishSecond();
  await expect(drawer.getByRole('alert')).toContainText('Review could not be stored');
  await expect(pages.getByRole('button', { name: 'Previous decisions' })).toHaveAttribute('aria-disabled', 'false');
  failSecond = false;
  await drawer.getByLabel('Review q0051', { exact: true }).selectOption('correct');
  await expect(drawer.getByRole('alert')).toHaveCount(0);
  await expect(drawer.getByText('Saved', { exact: true })).toHaveCount(2);
  await drawer.getByLabel('Review q0052', { exact: true }).selectOption('correct');
  await page.keyboard.press('Escape');
  await page.getByLabel('Inspect request request-failed', { exact: true }).click();
  await expect(pages).toContainText('Decisions 1–50 of 60');
  const saved = page.waitForEvent('requestfinished', request => new URL(request.url()).pathname.endsWith('/label'));
  finishClosed(); await saved;
  await expect(drawer.locator('.request-detail-id')).toContainText('request-failed');
  await expect(drawer.getByText('Saved', { exact: true })).toHaveCount(0);
  await page.keyboard.press('Escape');
  await page.getByLabel('Inspect request request-one', { exact: true }).click();
  await pages.getByRole('button', { name: 'Next decisions' }).click();
  for (const key of ['q0050', 'q0051', 'q0052']) await expect(drawer.getByLabel(`Review ${key}`, { exact: true })).toHaveValue('correct');
  expect(writes).toEqual({ q0050: 1, q0051: 2, q0052: 1 });
});

for (const outcome of ['success', 'failure'] as const) {
  test(`reopened pending review ${outcome} retains ownership, feedback, and focus`, async ({ page }) => {
    const state = await harness(page);
    state.record.labels = [{ key: 'department', label: 'unknown' }];
    let finish!: () => void, writes = 0;
    const pending = new Promise<void>(resolve => { finish = resolve; });
    await page.route('**/api/requests/request-one/label', async route => {
      writes += 1;
      if (writes === 1) {
        await pending;
        if (outcome === 'failure') return route.fulfill({ status: 503, json: { error: 'Review could not be stored' } });
      }
      const { key, label } = route.request().postDataJSON();
      state.record.labels = [{ key, label }];
      return route.fulfill({ json: { ok: true } });
    });
    const opener = page.getByLabel('Inspect request request-one', { exact: true });
    await opener.click();
    const drawer = page.getByRole('dialog'), review = drawer.getByLabel('Review department');
    await review.selectOption('correct');
    await expect(review).toHaveAttribute('aria-disabled', 'true');
    await page.keyboard.press('Escape');
    await expect(opener).toBeFocused();
    await opener.press('Enter');
    await expect(review).toHaveAttribute('aria-disabled', 'true');
    await expect(review).toHaveValue('correct');
    await expect(drawer.getByText('Saving…', { exact: true })).toBeVisible();
    await review.focus();
    await review.press('ArrowDown');
    await expect(review).toHaveValue('correct');
    // Exercise a stale change event too; Playwright refuses aria-disabled selects.
    await review.evaluate(select => { (select as HTMLSelectElement).value = 'incorrect'; select.dispatchEvent(new Event('change', { bubbles: true })); });
    expect(writes).toBe(1);
    finish();
    await expect(drawer.getByText(outcome === 'success' ? 'Saved' : 'Review could not be stored', { exact: true })).toBeVisible();
    await expect(review).toHaveValue(outcome === 'success' ? 'correct' : 'unknown');
    await expect(review).toHaveAttribute('aria-disabled', 'false');
    await expect(review).toBeFocused();
    if (outcome === 'failure') {
      await review.selectOption('correct');
      await expect(drawer.getByText('Saved', { exact: true })).toBeVisible();
      await expect(drawer.getByRole('alert')).toHaveCount(0);
      expect(writes).toBe(2);
    }
    // Settled feedback expires on leaving, while the persisted label remains.
    await page.keyboard.press('Escape');
    await opener.press('Enter');
    await expect(review).toHaveValue('correct');
    await expect(drawer.getByText('Saved', { exact: true })).toHaveCount(0);
    await expect(drawer.getByRole('alert')).toHaveCount(0);
  });
}

test('reopened pending review aborts a pre-save detail snapshot before it can replace the committed label', async ({ page }) => {
  const state = await harness(page);
  state.record.labels = [{ key: 'department', label: 'unknown' }];
  let finishReview!: () => void, finishDetail!: () => void;
  const reviewing = new Promise<void>(resolve => { finishReview = resolve; });
  const reading = new Promise<void>(resolve => { finishDetail = resolve; });
  let reopened = false, staleReads = 0, freshReads = 0, committed = false, writes = 0;
  await page.route('**/api/requests/request-one', async route => {
    const snapshot = structuredClone(state.record);
    if (reopened && !committed) { staleReads += 1; await reading; }
    else if (committed) freshReads += 1;
    await route.fulfill({ json: snapshot });
  });
  await page.route('**/api/requests/request-one/label', async route => {
    writes += 1; await reviewing;
    state.record.labels = [{ key: 'department', label: 'correct' }]; committed = true;
    await route.fulfill({ json: { ok: true } });
  });
  const opener = page.getByLabel('Inspect request request-one', { exact: true });
  await opener.click();
  await page.getByLabel('Review department').selectOption('correct');
  await page.keyboard.press('Escape');
  reopened = true;
  await opener.click();
  await expect.poll(() => staleReads).toBeGreaterThan(0);
  await expect(page.getByRole('dialog').getByRole('status')).toContainText('Loading request');
  finishReview();
  await expect.poll(() => freshReads).toBe(1);
  await expect(page.getByLabel('Review department')).toHaveValue('correct');
  await expect(page.getByRole('dialog').getByText('Saved', { exact: true })).toBeVisible();
  finishDetail();
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  await expect(page.getByLabel('Review department')).toHaveValue('correct');
  expect(writes).toBe(1);
});

for (const outcome of ['success', 'failure'] as const) {
  test(`closed review ${outcome} leaves another request independently editable and drops inactive feedback`, async ({ page }) => {
    const state = await harness(page);
    state.record.labels = [{ key: 'department', label: 'unknown' }];
    const other = { ...structuredClone(state.record), id: 'request-failed' };
    let finish!: () => void, firstWrites = 0, otherWrites = 0;
    const pending = new Promise<void>(resolve => { finish = resolve; });
    await page.route('**/api/requests/request-failed', route => route.fulfill({ json: other }));
    await page.route('**/api/requests/request-failed/label', async route => {
      otherWrites += 1; other.labels = [{ key: 'department', label: 'incorrect' }];
      await route.fulfill({ json: { ok: true } });
    });
    await page.route('**/api/requests/request-one/label', async route => {
      firstWrites += 1; await pending;
      if (outcome === 'failure') return route.fulfill({ status: 503, json: { error: 'First review could not be stored' } });
      state.record.labels = [{ key: 'department', label: 'correct' }];
      await route.fulfill({ json: { ok: true } });
    });
    await page.getByLabel('Inspect request request-one', { exact: true }).click();
    await page.getByLabel('Review department').selectOption('correct');
    await page.keyboard.press('Escape');
    await page.getByLabel('Inspect request request-failed', { exact: true }).click();
    const drawer = page.getByRole('dialog'), review = drawer.getByLabel('Review department');
    await expect(review).toHaveAttribute('aria-disabled', 'false');
    await review.selectOption('incorrect');
    await expect(drawer.getByText('Saved', { exact: true })).toBeVisible();
    const settled = page.waitForEvent('requestfinished', request => new URL(request.url()).pathname === '/api/requests/request-one/label');
    finish(); await settled;
    await expect(review).toHaveValue('incorrect');
    await expect(drawer.getByRole('alert')).toHaveCount(0);
    await expect(drawer.locator('.request-detail-id')).toContainText('request-failed');
    await page.keyboard.press('Escape');
    await page.getByLabel('Inspect request request-one', { exact: true }).click();
    await expect(review).toHaveValue(outcome === 'success' ? 'correct' : 'unknown');
    await expect(review).toHaveAttribute('aria-disabled', 'false');
    await expect(drawer.getByText('Saved', { exact: true })).toHaveCount(0);
    await expect(drawer.getByRole('alert')).toHaveCount(0);
    expect([firstWrites, otherWrites]).toEqual([1, 1]);
  });
}

test('reopened review completion preserves a deleted request error without resurrecting the row', async ({ page }) => {
  const state = await harness(page);
  let finish!: () => void, writes = 0, deleted = false;
  const pending = new Promise<void>(resolve => { finish = resolve; });
  await page.route('**/api/requests/request-one', route => deleted ? route.fulfill({ status: 404, json: { error: 'Record not found' } }) : route.fulfill({ json: state.record }));
  await page.route('**/api/requests/request-one/label', async route => { writes += 1; await pending; await route.fulfill({ json: { ok: true } }); });
  const opener = page.getByLabel('Inspect request request-one', { exact: true });
  await opener.click();
  await page.getByLabel('Review department').selectOption('correct');
  await page.keyboard.press('Escape');
  deleted = true; // Another client deleted history after the save committed but before its response arrived.
  await opener.click();
  const drawer = page.getByRole('dialog');
  await expect(drawer.getByRole('alert')).toContainText('Record not found');
  const settled = page.waitForEvent('requestfinished', request => new URL(request.url()).pathname.endsWith('/label'));
  finish(); await settled;
  await expect(drawer.getByRole('alert')).toContainText('Record not found');
  await expect(drawer.locator('.answer-card')).toHaveCount(0);
  expect(writes).toBe(1);
  await page.keyboard.press('Escape');
  await expect(opener).toBeFocused();
});

test('reopened review completion preserves a detail timeout and Retry reads the saved label without retrying the write', async ({ page }) => {
  const state = await harness(page);
  let finish!: () => void, finishRead!: () => void, writes = 0, hang = false, delayedReads = 0;
  const pending = new Promise<void>(resolve => { finish = resolve; });
  const reading = new Promise<void>(resolve => { finishRead = resolve; });
  await page.route('**/api/requests/request-one', async route => { if (hang) { delayedReads += 1; await reading; } await route.fulfill({ json: state.record }); });
  await page.route('**/api/requests/request-one/label', async route => { writes += 1; await pending; state.record.labels = [{ key: 'department', label: 'correct' }]; await route.fulfill({ json: { ok: true } }); });
  const opener = page.getByLabel('Inspect request request-one', { exact: true });
  await opener.click();
  await page.getByLabel('Review department').selectOption('correct');
  await page.keyboard.press('Escape');
  await page.clock.install();
  hang = true;
  await opener.click();
  await page.clock.runFor(500);
  await expect.poll(() => delayedReads).toBeGreaterThan(0);
  await page.clock.runFor(15_001);
  const drawer = page.getByRole('dialog');
  await expect(drawer.getByRole('alert')).toContainText('Request timed out');
  const settled = page.waitForEvent('requestfinished', request => new URL(request.url()).pathname.endsWith('/label'));
  finish(); await settled;
  await expect(drawer.getByRole('alert')).toContainText('Request timed out');
  expect(writes).toBe(1);
  hang = false;
  await drawer.getByRole('button', { name: 'Retry' }).click();
  await expect(page.getByLabel('Review department')).toHaveValue('correct');
  await expect(drawer.getByText('Saved', { exact: true })).toBeVisible();
  finishRead();
  expect(writes).toBe(1);
});

test('a committed old review response cannot mark a deleted and reimported raw identity Saved', async ({ page }, testInfo) => {
  const state = await harness(page);
  const raw = { id: 'synthetic-review-event', source: 'Test inbox', provider: 'typesafe', timestamp: now, status: 200, capture_complete: true,
    request: { questions: { department: group.definition, untouched: group.definition } },
    response: { answers: { department: { type: 'choice', choice: 'technical', probabilities: { technical: .8, billing: .2 } }, untouched: { type: 'choice', choice: 'technical', probabilities: { technical: .8, billing: .2 } } } } };
  // This is CaptureId::raw_import's canonical original tuple, not a fresh ID
  // invented on reimport. The API fixture models committed storage separately
  // from response delivery, as a slow network may delay an already saved POST.
  const id = 'capture_v1_' + createHash('sha256').update(JSON.stringify([raw.source, raw.provider, raw.id])).digest('hex');
  const imported = { ...structuredClone(record), id, source_event_id: id, import_format: 'systemone-capture',
    answers: [structuredClone(record.answers[0]), { ...structuredClone(record.answers[0]), key: 'untouched' }] };
  const dashboard = structuredClone(state.dashboard);
  let imports = 0, writes = 0, committed = false, detailReads = 0, release!: () => void;
  const delivery = new Promise<void>(resolve => { release = resolve; });
  state.dashboard.requests = [];
  state.dashboard.summary.request_count = 0;
  state.deleted = true;
  await page.route('**/api/import', async route => {
    const payload = route.request().postDataJSON();
    expect(payload.format).toBe('systemone-capture');
    expect(JSON.parse(payload.text)).toEqual(raw);
    imports += 1;
    state.record = structuredClone(imported);
    state.deleted = false;
    state.dashboard = { ...structuredClone(dashboard), requests: [state.record], summary: { ...dashboard.summary, request_count: 1, answer_count: 2 } };
    await route.fulfill({ json: { imported: 1, duplicates: 0 } });
  });
  await page.route(`**/api/requests/${id}`, async route => {
    detailReads += 1;
    await route.fulfill(state.deleted ? { status: 404, json: { error: 'Record not found' } } : { json: state.record });
  });
  await page.route(`**/api/requests/${id}/label`, async route => {
    writes += 1;
    const payload = route.request().postDataJSON();
    state.record.labels = [{ key: payload.key, label: payload.label }];
    committed = true;
    await delivery;
    // Do not write again when delivering this old success response.
    await route.fulfill({ json: { ok: true } });
  });
  async function importOriginal() {
    await page.getByRole('complementary', { name: 'Workspace navigation' }).getByRole('button', { name: 'Import records', exact: true }).click();
    await page.getByLabel('Source format').selectOption('systemone-capture');
    await page.getByLabel('Or paste records').fill(JSON.stringify(raw));
    await page.getByRole('dialog').getByRole('button', { name: 'Import records', exact: true }).click();
    await expect(page.getByRole('dialog').getByRole('status')).toContainText('1 records imported.');
    await page.keyboard.press('Escape');
  }
  await page.getByRole('button', { name: 'Refresh dashboard' }).click();
  await importOriginal();
  await page.getByLabel(`Inspect request ${id}`, { exact: true }).click();
  await page.getByLabel('Review department', { exact: true }).selectOption('correct');
  await expect.poll(() => committed).toBe(true);
  expect(state.record.labels).toEqual([{ key: 'department', label: 'correct' }]);
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Open settings', exact: true }).click();
  await page.getByRole('button', { name: 'Delete history', exact: true }).click();
  await page.getByLabel('Type DELETE to confirm').fill('DELETE');
  await page.getByRole('button', { name: 'Permanently delete history' }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(state.deleted).toBe(true);
  await importOriginal();
  expect(state.record.labels).toEqual([]);
  await page.getByLabel(`Inspect request ${id}`, { exact: true }).click();
  const drawer = page.getByRole('dialog');
  await expect(drawer.getByLabel('Review department', { exact: true })).toHaveAttribute('aria-disabled', 'true');
  const untouched = drawer.locator('.answer-card').filter({ has: page.getByRole('heading', { name: 'untouched', exact: true }) });
  await expect(untouched.getByLabel('Review untouched', { exact: true })).toHaveValue('');
  await expect(untouched.getByRole('status')).toBeEmpty();
  const priorReads = detailReads;
  const finished = page.waitForEvent('requestfinished', request => new URL(request.url()).pathname.endsWith('/label'));
  release(); await finished;
  await expect.poll(() => detailReads).toBeGreaterThan(priorReads);
  await expect(drawer.getByLabel('Review department', { exact: true })).toHaveValue('');
  await expect(drawer.getByLabel('Review department', { exact: true })).toHaveAttribute('aria-disabled', 'false');
  expect(imports).toBe(2); expect(writes).toBe(1); expect(state.record.labels).toEqual([]);
  const observed = { imports, writes, detailReads, storedLabels: state.record.labels,
    selectValue: await drawer.getByLabel('Review department', { exact: true }).inputValue(),
    statusText: await drawer.locator('.answer-card').first().getByRole('status').textContent() };
  await testInfo.attach('same-identity-review-observation', { body: JSON.stringify(observed, null, 2), contentType: 'application/json' });
  await testInfo.attach('same-identity-review', { body: await page.screenshot(), contentType: 'image/png' });
  await expect(drawer.locator('.answer-card').first().getByRole('status')).toBeEmpty();
});

for (const refreshedLabel of ['correct', 'incorrect'] as const) {
  test(`completed review feedback follows an authoritative ${refreshedLabel} label after refresh`, async ({ page }) => {
    const state = await harness(page);
    let finishImport!: () => void, imported = false, refreshed = 0, writes = 0;
    const importing = new Promise<void>(resolve => { finishImport = resolve; });
    await page.route('**/api/import', async route => {
      await importing; imported = true;
      await route.fulfill({ json: { imported: 1, duplicates: 0 } });
    });
    await page.route('**/api/requests/request-one', async route => {
      if (imported) refreshed += 1;
      await route.fulfill({ json: state.record });
    });
    await page.route('**/api/requests/request-one/label', async route => {
      writes += 1;
      const { key, label } = route.request().postDataJSON();
      state.record.labels = [{ key, label }];
      await route.fulfill({ json: { ok: true } });
    });
    await page.getByRole('button', { name: 'Import records', exact: true }).click();
    await page.getByLabel('Or paste records').fill('{"synthetic":true}');
    await page.getByRole('dialog').getByRole('button', { name: 'Import records', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Importing records…' })).toBeDisabled();
    await page.keyboard.press('Escape');
    await page.getByLabel('Inspect request request-one', { exact: true }).click();
    const drawer = page.getByRole('dialog'), review = drawer.getByLabel('Review department', { exact: true });
    await review.selectOption('correct');
    await expect(drawer.getByText('Saved', { exact: true })).toBeVisible();
    // An independent reviewer may replace the label before an unrelated
    // import completes and refreshes this still-open detail snapshot.
    state.record.labels = [{ key: 'department', label: refreshedLabel }];
    finishImport();
    await expect.poll(() => refreshed).toBe(1);
    await expect(review).toHaveValue(refreshedLabel);
    await expect(drawer.locator('.review-status')).toHaveText(refreshedLabel === 'correct' ? 'Saved' : '');
    expect(writes).toBe(1);
  });
}

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

async function paginatedSearchHarness(page: Page) {
  const state = await harness(page);
  await page.route('**/api/dashboard?**', async route => {
    const params = new URL(route.request().url()).searchParams;
    const requestPage = params.get('request_cursor') || 'first', groupPage = params.get('group_cursor') || 'first';
    const next = (cursor: string) => cursor === 'first' ? 'second' : cursor === 'second' ? 'third' : null;
    await route.fulfill({ json: {
      ...state.dashboard,
      requests: [{ ...record, id: `request-${requestPage}` }],
      groups: [{ ...group, id: `group-${groupPage}`, name: `department ${groupPage}` }],
      feed_next_cursor: next(requestPage), group_next_cursor: next(groupPage),
    } });
  });
  await page.getByRole('button', { name: 'Refresh dashboard' }).click();
  await expect(page.getByLabel('Inspect request request-first', { exact: true })).toBeVisible();
  await expect(page.locator('.group-row')).toContainText('department first');
}

async function openThirdPage(page: Page, section: 'requests' | 'groups') {
  for (const cursor of ['second', 'third']) {
    await page.getByRole('button', { name: `Older ${section}`, exact: true }).click();
    if (section === 'requests') await expect(page.getByLabel(`Inspect request request-${cursor}`, { exact: true })).toBeVisible();
    else await expect(page.locator('.group-row')).toContainText(`department ${cursor}`);
  }
}

for (const section of ['requests', 'groups'] as const) {
  test(`undoing a ${section === 'requests' ? 'request' : 'group'} search draft preserves the previous page`, async ({ page }) => {
    await page.clock.install({ time: new Date(now) });
    await page.clock.pauseAt(new Date(now + 1000));
    await paginatedSearchHarness(page);
    await openThirdPage(page, section);
    const search = page.getByLabel(section === 'requests' ? 'Search requests' : 'Find a question group');
    await search.fill('x');
    await search.fill('');
    await page.clock.runFor(300);
    // No search was committed, so this remains the third page of the same scope.
    const params = new URL(page.url()).searchParams;
    expect(params.get(section === 'requests' ? 'request_cursor' : 'group_cursor')).toBe('third');
    expect(params.has(section === 'requests' ? 'search' : 'group_search')).toBe(false);
    await page.getByRole('button', { name: `Newer ${section}`, exact: true }).click();
    if (section === 'requests') await expect(page.getByLabel('Inspect request request-second', { exact: true })).toBeVisible();
    else await expect(page.locator('.group-row')).toContainText('department second');
  });
}

test('committing a request search resets both page scopes', async ({ page }) => {
  await page.clock.install({ time: new Date(now) });
  await page.clock.pauseAt(new Date(now + 1000));
  await paginatedSearchHarness(page);
  await openThirdPage(page, 'requests');
  await openThirdPage(page, 'groups');
  await page.getByLabel('Search requests').fill('department');
  await page.clock.runFor(300);
  await expect(page.getByLabel('Inspect request request-first', { exact: true })).toBeVisible();
  await expect(page.locator('.group-row')).toContainText('department first');
  const params = new URL(page.url()).searchParams;
  expect(params.get('search')).toBe('department');
  expect(params.has('request_cursor')).toBe(false);
  expect(params.has('group_cursor')).toBe(false);
  await page.getByRole('button', { name: 'Older requests', exact: true }).click();
  await expect(page.getByLabel('Inspect request request-second', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Newer requests', exact: true }).click();
  await expect(page.getByLabel('Inspect request request-first', { exact: true })).toBeVisible();
});

test('committing a group search preserves request pagination history', async ({ page }) => {
  await page.clock.install({ time: new Date(now) });
  await page.clock.pauseAt(new Date(now + 1000));
  await paginatedSearchHarness(page);
  await openThirdPage(page, 'requests');
  await openThirdPage(page, 'groups');
  await page.getByLabel('Find a question group').fill('department');
  await page.clock.runFor(300);
  await expect(page.locator('.group-row')).toContainText('department first');
  const params = new URL(page.url()).searchParams;
  expect(params.get('group_search')).toBe('department');
  expect(params.has('group_cursor')).toBe(false);
  expect(params.get('request_cursor')).toBe('third');
  await page.getByRole('button', { name: 'Newer requests', exact: true }).click();
  await expect(page.getByLabel('Inspect request request-second', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Older groups', exact: true }).click();
  await expect(page.locator('.group-row')).toContainText('department second');
  await page.getByRole('button', { name: 'Newer groups', exact: true }).click();
  await expect(page.locator('.group-row')).toContainText('department first');
});

for (const section of ['requests', 'groups'] as const) {
  for (const direction of ['Older', 'Newer'] as const) {
    test(`group search during pending ${direction} ${section} preserves request history and resets groups`, async ({ page }) => {
      await page.clock.install({ time: new Date(now) });
      await page.clock.pauseAt(new Date(now + 1000));
      await paginatedSearchHarness(page);
      for (const current of ['requests', 'groups'] as const) {
        await page.getByRole('button', { name: `Older ${current}`, exact: true }).click();
        await expectHistoryPage(page, current, 'second');
        if (current !== section || direction === 'Newer') {
          await page.getByRole('button', { name: `Older ${current}`, exact: true }).click();
          await expectHistoryPage(page, current, 'third');
        }
      }
      const target = direction === 'Older' ? 'third' : 'second';
      let release!: () => void, started = false;
      const pending = new Promise<void>(resolve => { release = resolve; });
      await page.route('**/api/dashboard?**', async route => {
        const params = new URL(route.request().url()).searchParams;
        if (!started && !params.has('group_search') && params.get(section === 'requests' ? 'request_cursor' : 'group_cursor') === target) {
          started = true;
          await pending;
        }
        return route.fallback();
      });
      try {
        await page.getByRole('button', { name: `${direction} ${section}`, exact: true }).click();
        await expect.poll(() => started).toBe(true);
        await page.getByLabel('Find a question group').fill('department');
        await page.clock.runFor(300);
        await expectHistoryPage(page, 'requests', section === 'requests' ? target : 'third');
        await expectHistoryPage(page, 'groups', 'first');
        await expect(page.getByRole('button', { name: 'Newer groups', exact: true })).toBeDisabled();
        release();
        await page.clock.runFor(100);
        // The superseded response cannot restore the old group page or cursor stack.
        const previousRequests = section === 'requests' && direction === 'Newer' ? ['first'] : ['second', 'first'];
        for (const cursor of previousRequests) {
          await page.getByRole('button', { name: 'Newer requests', exact: true }).click();
          await expectHistoryPage(page, 'requests', cursor);
        }
        await page.getByRole('button', { name: 'Older groups', exact: true }).click();
        await expectHistoryPage(page, 'groups', 'second');
        await page.getByRole('button', { name: 'Newer groups', exact: true }).click();
        await expectHistoryPage(page, 'groups', 'first');
        await expect(page.getByRole('alert')).toHaveCount(0);
      } finally { release(); }
    });
  }
}

async function failingPageHarness(page: Page) {
  await page.clock.install({ time: new Date(now) });
  await page.clock.pauseAt(new Date(now + 1000));
  const state = await harness(page, { path: '/?source=Test+inbox' });
  const failures = new Set<string>(), reads: string[] = [];
  await page.route('**/api/dashboard?**', async route => {
    const params = new URL(route.request().url()).searchParams;
    const requests = params.get('request_cursor') || 'first', groups = params.get('group_cursor') || 'first';
    reads.push(`${requests}/${groups}`);
    if (failures.has(`requests:${requests}`) || failures.has(`groups:${groups}`)) return route.fulfill({ status: 503, json: { error: 'Selected history page unavailable' } });
    const next = (cursor: string) => cursor === 'first' ? 'second' : cursor === 'second' ? 'third' : null;
    return route.fulfill({ json: { ...state.dashboard,
      requests: [{ ...record, id: `request-${requests}` }],
      groups: [{ ...group, id: `group-${groups}`, name: `department ${groups}` }],
      feed_next_cursor: next(requests), group_next_cursor: next(groups),
    } });
  });
  await page.getByRole('button', { name: 'Refresh dashboard', exact: true }).click();
  await expect(page.getByLabel('Inspect request request-first', { exact: true })).toBeVisible();
  return { failures, reads };
}

async function expectHistoryPage(page: Page, section: 'requests' | 'groups', cursor: string) {
  if (section === 'requests') await expect(page.getByLabel(`Inspect request request-${cursor}`, { exact: true })).toBeVisible();
  else await expect(page.locator('.group-row')).toContainText(`department ${cursor}`);
  await expect.poll(() => new URL(page.url()).searchParams.get(section === 'requests' ? 'request_cursor' : 'group_cursor') || 'first').toBe(cursor);
}

for (const direction of ['Older', 'Newer'] as const) {
  test(`failed pending ${direction} requests after group search rolls back and retries in the new group scope`, async ({ page }) => {
    const { failures } = await failingPageHarness(page);
    await page.getByRole('button', { name: 'Older requests', exact: true }).click();
    await expectHistoryPage(page, 'requests', 'second');
    if (direction === 'Newer') {
      await page.getByRole('button', { name: 'Older requests', exact: true }).click();
      await expectHistoryPage(page, 'requests', 'third');
    }
    await openThirdPage(page, 'groups');
    const target = direction === 'Older' ? 'third' : 'second';
    const previous = direction === 'Older' ? 'second' : 'third';
    let release!: () => void, started = false;
    const pending = new Promise<void>(resolve => { release = resolve; });
    await page.route('**/api/dashboard?**', async route => {
      const params = new URL(route.request().url()).searchParams;
      if (!started && !params.has('group_search') && params.get('request_cursor') === target) {
        started = true;
        await pending;
      }
      return route.fallback();
    });
    try {
      failures.add(`requests:${target}`);
      await page.getByRole('button', { name: `${direction} requests`, exact: true }).click();
      await expect.poll(() => started).toBe(true);
      await page.getByLabel('Find a question group').fill('department');
      await page.clock.runFor(300);
      await expect(page.getByRole('alert')).toContainText('Showing the last successfully loaded page');
      await expectHistoryPage(page, 'requests', previous);
      await expectHistoryPage(page, 'groups', 'first');
      await expect(page.getByRole('status').filter({ hasText: 'Loading history…' })).toHaveCount(0);
      await expect(page.getByRole('button', { name: 'Newer groups', exact: true })).toBeDisabled();
      release();
      await page.clock.runFor(1000);
      await expect(page.getByRole('alert')).toContainText('Showing the last successfully loaded page');
      failures.clear();
      await page.getByRole('button', { name: 'Retry', exact: true }).click();
      await expectHistoryPage(page, 'requests', target);
      await expectHistoryPage(page, 'groups', 'first');
      expect(new URL(page.url()).searchParams.get('group_search')).toBe('department');
      await expect(page.getByRole('alert')).toHaveCount(0);
      for (const cursor of direction === 'Older' ? ['second', 'first'] : ['first']) {
        await page.getByRole('button', { name: 'Newer requests', exact: true }).click();
        await expectHistoryPage(page, 'requests', cursor);
      }
    } finally { release(); }
  });
}

for (const section of ['requests', 'groups'] as const) {
  test(`failed first ${section} page keeps the loaded view and an explicit retry`, async ({ page }) => {
    const { failures, reads } = await failingPageHarness(page);
    await page.getByRole('button', { name: 'Live updates', exact: true }).click();
    failures.add(`${section}:second`);
    await page.getByRole('button', { name: `Older ${section}`, exact: true }).click();
    await expect(page.getByRole('alert')).toContainText('Selected history page unavailable');
    await expectHistoryPage(page, section, 'first');
    await expect(page.locator('.requests-panel .section-heading p')).toContainText('Latest 1 records');
    await expect(page.getByRole('status').filter({ hasText: 'Loading history…' })).toHaveCount(0);
    await expect(page.getByRole('list', { name: 'Question groups' })).toHaveAttribute('aria-busy', 'false');
    await expect(page.getByRole('button', { name: `Older ${section}`, exact: true })).toBeEnabled();
    await expect(page.getByRole('button', { name: `Newer ${section}`, exact: true })).toBeDisabled();
    // Successful current-page polling must neither retry the bad page nor erase its notice.
    const failedReads = reads.filter(value => value.includes('second')).length;
    await page.clock.runFor(2000);
    await expect(page.getByRole('alert')).toContainText('Showing the last successfully loaded page');
    expect(reads.filter(value => value.includes('second'))).toHaveLength(failedReads);
    await expect(page.getByRole('button', { name: 'Resume live', exact: true })).toBeVisible();
    await expect(page.getByLabel('Filter by source')).toHaveValue('Test inbox');

    failures.clear();
    await page.getByRole('button', { name: 'Retry', exact: true }).click();
    await expectHistoryPage(page, section, 'second');
    await expect(page.getByRole('alert')).toHaveCount(0);
    await page.getByRole('button', { name: `Newer ${section}`, exact: true }).click();
    await expectHistoryPage(page, section, 'first');
  });

  test(`failed later ${section} page preserves both committed cursor stacks`, async ({ page }) => {
    const { failures, reads } = await failingPageHarness(page);
    const other = section === 'requests' ? 'groups' : 'requests';
    await openThirdPage(page, other);
    await page.getByRole('button', { name: `Older ${section}`, exact: true }).click();
    await expectHistoryPage(page, section, 'second');
    failures.add(`${section}:third`);
    await page.getByRole('button', { name: `Older ${section}`, exact: true }).click();
    await expect(page.getByRole('alert')).toContainText('Selected history page unavailable');
    await expectHistoryPage(page, section, 'second');
    // An immediate failed retry must make a new request without duplicating history.
    const attempts = reads.filter(value => value === 'third/third').length;
    await page.getByRole('button', { name: 'Retry', exact: true }).click();
    await expect.poll(() => reads.filter(value => value === 'third/third').length).toBeGreaterThan(attempts);
    await expect(page.getByRole('alert')).toContainText('Showing the last successfully loaded page');
    await expectHistoryPage(page, section, 'second');
    await page.getByRole('button', { name: `Newer ${section}`, exact: true }).click();
    await expectHistoryPage(page, section, 'first');
    await expect(page.getByRole('alert')).toHaveCount(0);
    for (const cursor of ['second', 'first']) {
      await page.getByRole('button', { name: `Newer ${other}`, exact: true }).click();
      await expectHistoryPage(page, other, cursor);
    }
  });

  test(`failed Newer ${section} navigation keeps its previous history and remains escapable`, async ({ page }) => {
    const { failures } = await failingPageHarness(page);
    await page.getByRole('button', { name: `Older ${section}`, exact: true }).click();
    await expectHistoryPage(page, section, 'second');
    failures.add(`${section}:first`);
    await page.getByRole('button', { name: `Newer ${section}`, exact: true }).click();
    await expect(page.getByRole('alert')).toContainText('Selected history page unavailable');
    await expectHistoryPage(page, section, 'second');
    if (section === 'requests') await expect(page.locator('.requests-panel .section-heading p')).toContainText('Older 1 records');
    // Move away while the latest page is still unavailable; the failed pop was never committed.
    await page.getByRole('button', { name: `Older ${section}`, exact: true }).click();
    await expectHistoryPage(page, section, 'third');
    failures.clear();
    for (const cursor of ['second', 'first']) {
      await page.getByRole('button', { name: `Newer ${section}`, exact: true }).click();
      await expectHistoryPage(page, section, cursor);
    }
  });
}

test('a timed-out history page rolls back and ignores its late response before explicit retry', async ({ page }) => {
  await failingPageHarness(page);
  let release!: () => void, started = false;
  const pending = new Promise<void>(resolve => { release = resolve; });
  await page.route('**/api/dashboard?**', async route => {
    if (new URL(route.request().url()).searchParams.get('request_cursor') && !started) {
      started = true;
      await pending;
    }
    return route.fallback();
  });
  try {
    await page.getByRole('button', { name: 'Older requests', exact: true }).click();
    await expect.poll(() => started).toBe(true);
    await page.clock.runFor(44_999);
    await expect(page.getByRole('status').filter({ hasText: 'Loading history…' })).toBeVisible();
    await page.clock.runFor(1);
    await expect(page.getByRole('alert')).toContainText('Request timed out after 45 seconds');
    await expectHistoryPage(page, 'requests', 'first');
    await expect(page.locator('.requests-panel .section-heading p')).toContainText('Latest 1 records');
    await expect(page.getByRole('button', { name: 'Older requests', exact: true })).toBeEnabled();
    release();
    await page.clock.runFor(1000);
    await expectHistoryPage(page, 'requests', 'first');
    await expect(page.getByRole('alert')).toContainText('Showing the last successfully loaded page');
    await page.getByRole('button', { name: 'Retry', exact: true }).click();
    await expectHistoryPage(page, 'requests', 'second');
    await expect(page.getByRole('alert')).toHaveCount(0);
  } finally { release(); }
});

for (const response of ['success', 'failure'] as const) {
  test(`a late pagination ${response} cannot restore abandoned filters or cursor history`, async ({ page }) => {
    await failingPageHarness(page);
    let release!: () => void, started = false;
    const pending = new Promise<void>(resolve => { release = resolve; });
    await page.route('**/api/dashboard?**', async route => {
      if (started || !new URL(route.request().url()).searchParams.get('request_cursor')) return route.fallback();
      started = true;
      await pending;
      if (response === 'failure') return route.fulfill({ status: 503, json: { error: 'Abandoned page failure' } });
      return route.fallback();
    });
    try {
      await page.getByRole('button', { name: 'Older requests', exact: true }).click();
      await expect.poll(() => started).toBe(true);
      await page.getByLabel('Filter by source').selectOption('');
      await expectHistoryPage(page, 'requests', 'first');
      release();
      await page.clock.runFor(1000);
      await expect(page.getByRole('alert')).toHaveCount(0);
      await expect(page.getByLabel('Filter by source')).toHaveValue('');
      await expectHistoryPage(page, 'requests', 'first');
      await page.getByRole('button', { name: 'Older requests', exact: true }).click();
      await expectHistoryPage(page, 'requests', 'second');
      await page.getByRole('button', { name: 'Newer requests', exact: true }).click();
      await expectHistoryPage(page, 'requests', 'first');
    } finally { release(); }
  });
}

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

for (const { timezoneId, locale, expected } of [
  { timezoneId: 'UTC', locale: 'en-US', expected: ['Oct 22, 2025, 03:00:00.123 GMT', 'Oct 22, 2026, 03:00:00.123 GMT', 'Dec 31, 2025, 23:30:00.456 GMT'] },
  { timezoneId: 'Asia/Seoul', locale: 'en-US', expected: ['Oct 22, 2025, 12:00:00.123 GMT+9', 'Oct 22, 2026, 12:00:00.123 GMT+9', 'Jan 1, 2026, 08:30:00.456 GMT+9'] },
  { timezoneId: 'America/Los_Angeles', locale: 'en-US', expected: ['Oct 21, 2025, 20:00:00.123 GMT-7', 'Oct 21, 2026, 20:00:00.123 GMT-7', 'Dec 31, 2025, 15:30:00.456 GMT-8'] },
  { timezoneId: 'UTC', locale: 'en-GB', expected: ['22 Oct 2025, 03:00:00.123 GMT', '22 Oct 2026, 03:00:00.123 GMT', '31 Dec 2025, 23:30:00.456 GMT'] },
]) {
  test.describe(`full date inspection in ${timezoneId} (${locale})`, () => {
    test.use({ timezoneId, locale });

    test('request and chart inspection distinguish years and local date rollover', async ({ page }) => {
      const state = await harness(page, { settings: { retention_days: 0 }, path: '/?window=all' });
      const timestamps = ['2025-10-22T03:00:00.123Z', '2026-10-22T03:00:00.123Z', '2025-12-31T23:30:00.456Z'].map(Date.parse);
      const records = timestamps.map((timestamp, index) => ({ ...structuredClone(record), id: `dated-${index}`, timestamp }));
      state.dashboard.requests = records;
      state.dashboard.timeline = [...timestamps].sort((a, b) => a - b).map(timestamp => ({ timestamp, requests: 1, errors: 0, mean_latency_ms: 1, cost_usd: null }));
      state.dashboard.timeline_meta = { start: timestamps[0], end: timestamps[1] + 86_400_000, bucket_width: 86_400_000, truncated: false };
      await page.getByRole('button', { name: 'Refresh dashboard' }).click();
      await expect(page.locator('.request-table tbody tr')).toHaveCount(3);
      const activity = page.locator('.activity-panel');
      await activity.getByText('View chart data', { exact: true }).click();
      for (const [index, item] of records.entries()) {
        const opener = page.getByRole('button', { name: `Inspect request ${item.id}`, exact: true });
        const row = page.locator('.request-table tbody tr').filter({ has: opener });
        const fullDate = await row.locator('td > span[title]').getAttribute('title');
        // ICU versions spell the zero offset as either GMT or GMT+0.
        expect(fullDate?.replace(/GMT\+0$/, 'GMT')).toBe(expected[index]);
        await expect(row.locator('td > span[title] > .sr-only')).toHaveText(fullDate!);
        await expect(row.locator('td > span[title] > [aria-hidden="true"]')).not.toContainText(/202[56]/);
        state.record = item;
        await opener.click();
        const date = page.getByRole('dialog').locator('.detail-field').filter({ has: page.getByText('Recorded', { exact: true }) }).locator('dd');
        await expect(date).toHaveText(fullDate!);
        await page.keyboard.press('Escape');
        await expect(opener).toBeFocused();
        const chartIndex = state.dashboard.timeline.findIndex(point => point.timestamp === item.timestamp);
        await expect(activity.locator('.chart-accessible tbody tr').nth(chartIndex).locator('td').first()).toHaveText(fullDate!);
        const point = activity.locator('.chart-bar').nth(chartIndex).locator('..');
        await expect(point.locator('title')).toHaveText(`${fullDate}: 1 requests, 0 failures`);
        await point.hover();
        await expect(activity.locator('.chart-tooltip > span').first()).toHaveText(fullDate!);
      }
      await expect(activity.locator('.chart-axis').last()).not.toContainText(/202[56]|GMT/);
      // Full inspection does not change local custom-range controls or their epoch scope.
      await page.getByRole('button', { name: 'Custom date range' }).click();
      await page.getByLabel('From date').fill('2025-12-31T23:30');
      const expectedFrom = await page.evaluate(() => String(new Date('2025-12-31T23:30').getTime()));
      await expect.poll(() => new URL(page.url()).searchParams.get('from')).toBe(expectedFrom);
      await expect(page.getByLabel('From date')).toHaveValue('2025-12-31T23:30');
    });
  });
}

test.describe('full dates at daylight-saving and legacy boundaries', () => {
  test.use({ timezoneId: 'America/Los_Angeles', locale: 'en-US' });

  test('repeated local hours carry distinct offsets in request details', async ({ page }) => {
    const state = await harness(page);
    for (const [timestamp, expected] of [
      ['2026-11-01T08:30:00Z', 'Nov 1, 2026, 01:30:00.000 GMT-7'],
      ['2026-11-01T09:30:00Z', 'Nov 1, 2026, 01:30:00.000 GMT-8'],
    ]) {
      state.record.timestamp = Date.parse(timestamp);
      await page.getByRole('button', { name: 'Inspect request request-one', exact: true }).click();
      await expect(page.getByRole('dialog').locator('.detail-field').filter({ has: page.getByText('Recorded', { exact: true }) }).locator('dd')).toHaveText(expected);
      await page.keyboard.press('Escape');
    }
  });

  test('legacy timestamp states and import-time fallback remain explicit', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    const state = await harness(page);
    for (const { timestamp, imported_at, expected } of [
      { timestamp: null, imported_at: null, expected: 'Unknown' },
      { timestamp: null, imported_at: Date.parse('2025-12-31T23:30:00.456Z'), expected: 'Dec 31, 2025, 15:30:00.456 GMT-8' },
      { timestamp: 1.5, imported_at: null, expected: 'Invalid timestamp' },
      { timestamp: 8_640_000_000_000_001, imported_at: null, expected: 'Date out of range' },
    ]) {
      Object.assign(state.record, { timestamp, imported_at });
      state.dashboard.requests = [structuredClone(state.record)];
      await page.getByRole('button', { name: 'Refresh dashboard' }).click();
      await expect(page.locator('.request-table td > span[title]')).toHaveAttribute('title', expected);
      const opener = page.getByRole('button', { name: 'Inspect request request-one', exact: true });
      await opener.click();
      const field = page.getByRole('dialog').locator('.detail-field').filter({ has: page.getByText(timestamp == null ? 'Imported at' : 'Recorded', { exact: true }) });
      await expect(field.locator('dd')).toHaveText(expected + (timestamp == null ? 'Original event time unknown' : ''));
      await page.keyboard.press('Escape');
      await expect(opener).toBeFocused();
    }
    expect(errors).toEqual([]);
  });
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

test.describe('responsive timeline geometry', () => {
  test.use({ timezoneId: 'UTC', locale: 'en-US' });

  async function readableChart(svg: Locator) {
    await expect.poll(() => svg.evaluate(element => {
      const scale = (element as SVGSVGElement).getScreenCTM();
      return scale ? Math.max(Math.abs(scale.a - 1), Math.abs(scale.d - 1)) : 1;
    })).toBeLessThan(.01);
    const geometry = await svg.evaluate(element => {
      const svg = element as SVGSVGElement, bounds = svg.getBoundingClientRect();
      const labels = [...svg.querySelectorAll<SVGTextElement>(':scope > .chart-axis')].map(label => {
        const rect = label.getBoundingClientRect();
        return { left: rect.left, right: rect.right, height: rect.height };
      });
      return { left: bounds.left, right: bounds.right, labels, finite: [...svg.querySelectorAll('rect')].every(rect => ['x', 'y', 'width', 'height'].every(name => Number.isFinite(Number(rect.getAttribute(name))))) };
    });
    expect(geometry.finite).toBe(true);
    for (const [index, label] of geometry.labels.entries()) {
      expect(label.height).toBeGreaterThanOrEqual(9);
      expect(label.left).toBeGreaterThanOrEqual(geometry.left - 1);
      expect(label.right).toBeLessThanOrEqual(geometry.right + 1);
      if (index) expect(label.left - geometry.labels[index - 1].right).toBeGreaterThan(8);
    }
  }

  for (const [metric, values] of [
    ['Volume', ['1', '2', '3']], ['Latency', ['10 ms', '20 ms', '30 ms']], ['Cost', ['$0.0100', '$0.0200', '$0.0300']],
  ] as const) {
    test(`${metric} keeps readable axes and exact data through phone desktop tablet resize`, async ({ page }, testInfo) => {
      const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
      await page.setViewportSize({ width: 390, height: 1000 });
      const state = await harness(page);
      const start = Date.UTC(2026, 9, 21), hour = 3_600_000;
      state.dashboard.timeline = [0, 6, 12].map((offset, index) => ({ timestamp: start + offset * hour, requests: index + 1, errors: index === 2 ? 1 : 0, mean_latency_ms: (index + 1) * 10, cost_usd: (index + 1) / 100 }));
      state.dashboard.timeline_meta = { start, end: start + 24 * hour, bucket_width: hour, truncated: false };
      await page.getByRole('button', { name: 'Refresh dashboard' }).click();
      const activity = page.locator('.activity-panel'), svg = activity.locator('.timeline-chart');
      await activity.getByRole('button', { name: metric, exact: true }).click();
      await expect(svg.locator('.chart-bar')).toHaveCount(3);
      await activity.getByText('View chart data', { exact: true }).click();
      await page.evaluate(() => document.fonts.ready);
      const table = activity.locator('.chart-accessible tbody');
      const originalRows = await table.locator('tr').allTextContents();
      for (let index = 0; index < 3; index += 1) {
        await expect(table.locator('tr').nth(index).locator('td').nth(1)).toHaveText(String(index + 1));
        await expect(table.locator('tr').nth(index).locator('td').nth(2)).toHaveText(index === 2 ? '1' : '0');
        await expect(table.locator('tr').nth(index).locator('td').nth(3)).toHaveText(`${(index + 1) * 10} ms`);
        await expect(table.locator('tr').nth(index).locator('td').nth(4)).toHaveText(`$0.0${index + 1}00`);
      }
      for (const width of [390, 1440, 768]) {
        await page.setViewportSize({ width, height: 1000 });
        await readableChart(svg);
        await expect(svg.locator(':scope > .chart-axis')).toHaveCount(width === 1440 ? 3 : 2);
        await expect(svg.locator('.chart-bar')).toHaveCount(3);
        await expect(svg.locator('.chart-error')).toHaveCount(metric === 'Volume' ? 1 : 0);
        const relativePositions = await svg.evaluate(element => {
          const ticks = element.querySelectorAll<SVGTextElement>(':scope > .chart-axis');
          const left = Number(ticks[0].getAttribute('x')), right = Number(ticks[ticks.length - 1].getAttribute('x'));
          return [...element.querySelectorAll<SVGRectElement>('.chart-bar')].map(bar => (Number(bar.getAttribute('x')) + Number(bar.getAttribute('width')) / 2 - left) / (right - left));
        });
        for (const [index, position] of relativePositions.entries()) expect(position).toBeCloseTo((index * 6 + .5) / 24);
        const heights = await svg.locator('.chart-bar').evaluateAll(bars => bars.map(bar => Number(bar.getAttribute('height'))));
        expect(heights[1] / heights[0]).toBeCloseTo(2);
        expect(heights[2] / heights[0]).toBeCloseTo(3);
        for (const [index, value] of values.entries()) {
          const date = await table.locator('tr').nth(index).locator('td').first().textContent();
          expect(date).toMatch(/Oct 21, 2026, (00|06|12):00:00\.000 GMT/);
          const point = svg.locator('.chart-bar').nth(index).locator('..');
          const suffix = metric === 'Volume' ? ` requests, ${index === 2 ? 1 : 0} failures` : '';
          await expect(point.locator('title')).toHaveText(`${date}: ${value}${suffix}`);
          await point.hover();
          await expect(activity.locator('.chart-tooltip > span').first()).toHaveText(date!);
          await expect(activity.locator('.chart-tooltip strong')).toHaveText(`${value}${metric === 'Volume' ? ' requests' : ''}`);
        }
        await expect(table.locator('tr')).toHaveText(originalRows);
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
        if (metric === 'Volume' && width !== 1440) await activity.screenshot({ path: testInfo.outputPath(`timeline-${width}.png`) });
      }
      expect(errors).toEqual([]);
    });
  }

  test('empty hidden and compact charts recover usable geometry', async ({ page }) => {
    const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
    await page.setViewportSize({ width: 390, height: 1000 });
    const state = await harness(page), activity = page.locator('.activity-panel');
    const timeline = state.dashboard.timeline;
    state.dashboard.timeline = [];
    await page.getByRole('button', { name: 'Refresh dashboard' }).click();
    await expect(activity.locator('.timeline-chart')).toHaveCount(0);
    await activity.evaluate(element => { element.style.display = 'none'; });
    state.dashboard.timeline = timeline;
    await page.getByRole('button', { name: 'Refresh dashboard' }).click();
    const svg = activity.locator('.timeline-chart');
    await expect(svg).toHaveCount(1);
    expect(await svg.getAttribute('viewBox')).not.toMatch(/NaN|Infinity/);
    await activity.evaluate(element => { element.style.display = ''; });
    await readableChart(svg);
    const geometry = await svg.getAttribute('viewBox');
    await activity.evaluate(element => { element.style.display = 'none'; });
    await page.setViewportSize({ width: 1440, height: 1000 });
    await expect(svg).toHaveAttribute('viewBox', geometry!);
    await activity.evaluate(element => { element.style.display = ''; });
    await readableChart(svg);
    await page.getByLabel('Inspect group department', { exact: true }).click();
    const compactChart = page.getByRole('dialog').locator('.timeline-chart');
    await readableChart(compactChart);
    await expect(compactChart.locator('.chart-bar')).toHaveCount(1);
    await page.keyboard.press('Escape');
    await expect(compactChart).toHaveCount(0);
    await page.setViewportSize({ width: 768, height: 1000 });
    await readableChart(svg);
    expect(errors).toEqual([]);
  });
});

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


test('the researched catalog keeps setup separate from active credentials and opens mapped capture import', async ({ page }, testInfo) => {
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
  await page.screenshot({ path: testInfo.outputPath('jev-model-catalog-mobile.png'), fullPage: true });
  await page.getByRole('button', { name: 'Import mapped decisions' }).click();
  await expect(page.getByLabel('Source format')).toHaveValue('systemone-capture');
  await expect(page.getByLabel('Or paste records')).toHaveAttribute('placeholder', /id, timestamp, source, provider/);
});

async function typedDecisionHarness(page: Page, answers: RequestRecord['answers'], current: Group = group) {
  const state = await harness(page);
  state.record = { ...state.record, protocol: 'openai-decisions', model_profile: 'openai-decisions', answers, answer_count: answers.length };
  state.dashboard.groups = [current];
  state.dashboard.requests = [{ ...state.record }];
  await page.route('**/api/groups/*', route => route.fulfill({ json: {
    group: current, versions: [current, { ...current, id: 'other-typed-version', definition_id: 'other-definition' }],
    timeline: [], requests: [state.record], total_answers: answers.length,
    answers: answers.map(answer => ({ ...answer, request_id: state.record.id, timestamp: now, source: state.record.source, label: null })),
  } }));
  await page.reload();
  await expect(page.locator('.request-link')).toHaveCount(1);
  return state;
}

const boxAnswer = (value: unknown, extra: Partial<RequestRecord['answers'][number]> = {}): RequestRecord['answers'][number] => ({
  ...record.answers[0], key: 'target', kind: 'box', definition: { type: 'box', instructions: 'Locate the target' },
  value: value as RequestRecord['answers'][number]['value'], probabilities: null, confidence: .75,
  raw_answer: { type: 'box', value, confidence: .75 }, ...extra,
});

for (const { name, value } of [
  { name: 'interior', value: { top: .125, bottom: .875, left: .25, right: .75 } },
  { name: 'full boundary', value: { top: 0, bottom: 1, left: 0, right: 1 } },
  { name: 'tiny nonzero', value: { top: .1234567890123, bottom: .1234567890124, left: .4, right: .4000000000001 } },
]) test(`typed decisions render ${name} Box geometry without fetching images`, async ({ page }, testInfo) => {
  const images: string[] = [];
  page.on('request', request => { if (request.resourceType() === 'image') images.push(request.url()); });
  await typedDecisionHarness(page, [boxAnswer(value)]);
  await page.locator('.request-link').click();
  const card = page.locator('.answer-card');
  await expect(card.locator('.box-coordinates dd')).toHaveText([value.top, value.bottom, value.left, value.right].map(String));
  const outline = card.locator('.box-outline');
  await expect(outline).toHaveAttribute('x', String(value.left * 100));
  await expect(outline).toHaveAttribute('y', String(value.top * 100));
  await expect(outline).toHaveAttribute('width', String((value.right - value.left) * 100));
  await expect(outline).toHaveAttribute('height', String((value.bottom - value.top) * 100));
  await expect(card.locator('.box-diagram')).toHaveAccessibleName(`Normalized box: top ${value.top}, bottom ${value.bottom}, left ${value.left}, right ${value.right}`);
  await expect(card).toContainText('Reported confidence 75.0% · not measured accuracy');
  await expect(card.locator('.probability-list')).toHaveCount(0);
  await card.getByText('Inspect raw answer', { exact: true }).click();
  await expect(card.locator('.raw-answer-details pre')).toHaveText(JSON.stringify(boxAnswer(value).raw_answer, null, 2));
  await card.getByText('Inspect raw answer', { exact: true }).click();
  await expect(card.locator('.raw-answer-details pre')).toHaveCount(0);
  if (name === 'interior') await testInfo.attach('box-desktop', { body: await page.screenshot(), contentType: 'image/png' });
  await page.setViewportSize({ width: 390, height: 844 });
  const widths = await card.evaluate(element => ({ width: element.clientWidth, scroll: element.scrollWidth }));
  expect(widths.scroll).toBeLessThanOrEqual(widths.width + 1);
  if (name === 'interior') await testInfo.attach('box-mobile', { body: await page.screenshot(), contentType: 'image/png' });
  expect((await new AxeBuilder({ page }).include('.answer-card').analyze()).violations).toEqual([]);
  expect(images).toEqual([]);
});

for (const { name, value } of [
  { name: 'reversed', value: { top: .8, bottom: .2, left: 0, right: 1 } },
  { name: 'out of range', value: { top: 0, bottom: 2, left: 0, right: 1 } },
  { name: 'string coordinate', value: { top: '0', bottom: 1, left: 0, right: 1 } },
  { name: 'missing coordinate', value: { top: 0, bottom: 1, left: 0 } },
]) test(`typed decisions keep ${name} Box geometry inspectable without drawing it`, async ({ page }) => {
  // Deliberately inconsistent valid=true checks that rendering never coerces or
  // invents geometry even if an external capture has malformed value metadata.
  await typedDecisionHarness(page, [boxAnswer(value)]);
  await page.locator('.request-link').click();
  const card = page.locator('.answer-card');
  await expect(card).toContainText('Invalid box geometry');
  await expect(card.locator('.box-diagram')).toHaveCount(0);
  await card.getByText('Inspect raw answer', { exact: true }).click();
  await expect(card.locator('.raw-answer-details pre')).toHaveText(JSON.stringify(boxAnswer(value).raw_answer, null, 2));
});

test('typed decisions distinguish H2O no-localization fallback from a full-image Box and preserve zero presence', async ({ page }) => {
  const sentinel = { top: 0, bottom: 1, left: 0, right: 1 };
  const absent = boxAnswer(sentinel, { box_found: false, box_present: 0, confidence: 0, raw_answer: { value: sentinel, found: false, present: 0 } });
  await typedDecisionHarness(page, [absent]);
  await page.locator('.request-link').click();
  const card = page.locator('.answer-card');
  await expect(card).toContainText('No box localized (reported)');
  await expect(card).toContainText('does not establish that the target is absent');
  await expect(card).toContainText('Reported presence 0.0%');
  await expect(card).toContainText('Reported confidence 0.0%');
  await expect(card.locator('.box-diagram')).toHaveCount(0);
  await card.getByText('Inspect raw answer', { exact: true }).click();
  await expect(card.locator('.raw-answer-details pre')).toHaveText(JSON.stringify(absent.raw_answer, null, 2));
  await card.getByLabel('Review target').selectOption('unknown');
  await expect(card.getByRole('status')).toHaveText('Saved');
  await page.keyboard.press('Escape');
  await expect(page.locator('.request-link')).toBeFocused();
});

test('typed decisions show Box group observations and comparisons without scalar statistics', async ({ page }) => {
  const value = { top: .1, bottom: .9, left: .2, right: .8 };
  const current: Group = { ...group, kind: 'box', name: 'target', definition: { type: 'box', instructions: 'Locate target' }, valid_count: 2, distribution: [], mean_value: null };
  await typedDecisionHarness(page, [boxAnswer(value, { box_found: true, box_present: .6 }), boxAnswer(value, { key: 'fallback', box_found: false, box_present: null })], current);
  await expect(page.locator('.selected-distribution')).toContainText('Box answers retain individual geometry');
  await page.getByRole('button', { name: 'Inspect group', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.locator('.box-value-compact').first()).toContainText('Reported presence 60.0%');
  await expect(dialog.locator('.box-value-compact').last()).toContainText('No box localized (reported)');
  await expect(dialog.locator('.box-value-compact').last()).not.toContainText('Reported presence');
  await expect(dialog.locator('.box-value-compact .box-diagram')).toHaveCount(0);
  await expect(dialog).not.toContainText('Mean score');
  await expect(dialog).not.toContainText('Mean yes-probability');
  await page.getByLabel('Compare with').selectOption('other-typed-version');
  const comparison = page.getByLabel('Version outcome comparison');
  await expect(comparison.getByRole('rowheader', { name: /^Mean (score|yes-probability)$/ })).toHaveCount(0);
  await expect(comparison.getByRole('rowheader', { name: 'Correct among judged answers' })).toBeVisible();
  await expect(comparison.locator('.box-distribution-note')).toHaveCount(2);
});

test('typed decisions retain Boolean and String choice identities in winners probabilities and distributions', async ({ page }) => {
  const values = [true, 'true', false, 'false', ''] as const;
  const definition = { type: 'choice', instructions: 'Choose an exact typed value', choices: values.map(value => ({ value, description: 'Synthetic option' })) };
  const key = (value: string | boolean) => `choice:v1:${JSON.stringify(value)}`;
  const probabilities = Object.fromEntries(values.map(value => [key(value), .2]));
  const answers = values.map((value, index) => ({ ...record.answers[0], key: `question:${index}`, definition, value, choice_key: key(value), probabilities, raw_answer: { type: 'choice', value } }));
  const current = { ...group, definition, valid_count: 5, distribution: values.map(value => ({ key: key(value), label: String(value), value, count: 1 })) };
  await typedDecisionHarness(page, answers, current);
  const labels = ['true (boolean)', '"true"', 'false (boolean)', '"false"', '""'];
  await expect(page.locator('.selected-distribution .distribution-label > span:first-child')).toHaveText(labels);
  await page.locator('.request-link').click();
  await expect(page.locator('.answer-value')).toHaveText(labels);
  await expect(page.locator('.answer-card').first().locator('.probability-row > span:first-child')).toHaveText(labels);
  await page.locator('.answer-card').first().getByText('Inspect raw answer', { exact: true }).click();
  await expect(page.locator('.answer-card').first().locator('.raw-answer-details pre')).toContainText('"value": true');
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Inspect group', exact: true }).click();
  await expect(page.locator('.box-value')).toHaveCount(0);
  await expect(page.getByRole('dialog').getByRole('row').filter({ has: page.getByRole('button', { name: 'question:0', exact: true }) }).locator('td').nth(1)).toHaveText('true (boolean)');
});

test('typed decisions derive old typed bin labels from native choices without parsing opaque keys', async ({ page }) => {
  const current = { ...group, definition: { choices: [{ value: false, description: 'False' }, { value: 'false', description: 'String false' }] }, distribution: [{ label: 'choice:v1:false', count: 2 }, { label: 'choice:v1:"false"', count: 1 }, { label: 'choice:v1:unrecognized', count: 1 }] };
  await typedDecisionHarness(page, [], current);
  await expect(page.locator('.selected-distribution .distribution-label > span:first-child')).toHaveText(['false (boolean)', '"false"', 'choice:v1:unrecognized']);
});

test('typed decisions connection uses OpenAI Decisions request shape and preserves provider identity', async ({ page }) => {
  await harness(page, { settings: { provider: 'openai', protocol: 'openai-decisions', model_profile: 'openai-decisions', upstream: 'https://api.openai.com/v1/decisions' } });
  await page.getByRole('button', { name: 'Connect an application', exact: true }).click();
  await page.getByLabel('Model name', { exact: true }).fill('gpt-6-luna');
  const snippet = page.getByLabel('SDK connection example');
  await expect(snippet).toContainText('from openai import OpenAI');
  await expect(snippet).toContainText('client.decisions.create');
  await expect(snippet).toContainText('/v1"');
  await expect(snippet).toContainText('"type": "predicate"');
  await expect(snippet).toContainText('model="gpt-6-luna"');
  await expect(snippet).toContainText('JEV_OBSERVER_CLIENT_TOKEN');
  await expect(page.getByLabel('openai API key', { exact: true })).toBeVisible();
  await expect(page.locator('.endpoint')).toHaveText('POST /v1/decisions');
  await expect(page.getByRole('dialog')).not.toContainText('typesafe-sdk');
  await page.getByRole('button', { name: 'JavaScript', exact: true }).click();
  await expect(snippet).toContainText('import OpenAI from "openai"');
  await expect(snippet).toContainText('questions: [{ type: "predicate"');
  await expect(snippet).toContainText('maxRetries: 0');
});

test('typed decisions connection keeps Cloudflare native response envelope in both examples', async ({ page }) => {
  await harness(page, { settings: { provider: 'cloudflare', protocol: 'cloudflare-systemone', model_profile: 'cf-jev', upstream: 'https://api.cloudflare.com/client/v4/accounts/example/ai/run/@typesafe/jev-1.14' } });
  await page.getByRole('button', { name: 'Connect an application', exact: true }).click();
  const snippet = page.getByLabel('SDK connection example');
  await expect(page.getByRole('heading', { name: 'Connect with native HTTP' })).toBeVisible();
  await expect(snippet).toContainText('from urllib.request import Request, urlopen');
  await expect(snippet).toContainText('envelope["result"]');
  await expect(snippet).toContainText('envelope.get("success") is not True');
  await expect(snippet).toContainText('/v1/systemone');
  await expect(snippet).not.toContainText('TypeSafeClient');
  await page.getByRole('button', { name: 'JavaScript', exact: true }).click();
  await expect(snippet).toContainText('await fetch(');
  await expect(snippet).toContainText('envelope.success !== true');
  await expect(snippet).toContainText('console.log(envelope.result)');
});

test('typed decisions catalog separates selected profile protocol and credential provider', async ({ page }) => {
  const models: NonNullable<Settings['model_catalog']> = { checked_at: '2026-10-22', models: [
    { id: 'profile-one', model_profile: 'profile-one', provider: 'billing-provider', protocol: 'openai-decisions', name: 'Synthetic Decisions', access: 'closed', integration: 'proxy', model_names: ['native-model'], upstream: 'https://example.com/v1/decisions', source_url: 'https://example.com/docs', note: 'Fixture', verification: 'source-reviewed', checked_at: '2026-10-22' },
    { id: 'offline', model_profile: 'offline', provider: 'local', protocol: 'systemone', name: 'Offline fixture', access: 'open', integration: 'capture', model_names: [], upstream: null, source_url: 'https://example.com/offline', note: 'Offline only', verification: 'source-reviewed' },
  ] };
  await harness(page, { settings: { provider: 'billing-provider', model_profile: 'profile-one', protocol: 'openai-decisions', model_catalog: models } });
  await page.getByRole('button', { name: 'Connect an application', exact: true }).click();
  await expect(page.getByLabel('Model integration')).toHaveValue('profile-one');
  await expect(page.getByLabel('Observer startup command')).toHaveText('jev-observer --upstream https://example.com/v1/decisions --provider billing-provider --protocol openai-decisions --model-profile profile-one');
  await expect(page.locator('#configured-model-names option')).toHaveAttribute('value', 'native-model');
  await page.getByLabel('Model integration').selectOption('offline');
  await expect(page.getByLabel('Observer startup command')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Import mapped decisions' })).toBeVisible();
  await expect(page.locator('.endpoint')).toHaveText('POST /v1/decisions');
  await page.getByRole('button', { name: 'Import mapped decisions' }).click();
  await expect(page.getByLabel('Source format')).toHaveValue('systemone-capture');
  await expect(page.getByRole('option', { name: 'Decision request/response capture' })).toHaveAttribute('value', 'systemone-capture');
  await expect(page.getByRole('dialog')).toContainText('include protocol: openai-decisions or cloudflare-systemone');
});
