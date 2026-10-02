import { expect, test } from '@playwright/test';
import { parentScope, query, readFilters } from '../src/api';
import { connectionSnippet } from '../src/connection';
import { costBasis, dateFilter, dateInput, storageHint } from '../src/format';

test('bookmark parsing preserves valid scope and rejects invalid dates or modes', () => {
  const filters = readFilters('?source=Inbox&window=7d&from=0&to=1760000000000&request_cursor=page-2&group_search=urgent');
  expect(filters.window).toBe('all');
  expect(filters.from).toBe('0');
  expect(readFilters(`?${query(filters)}`)).toEqual(filters);
  expect(readFilters('?from=Infinity&to=99999999999999999&window=bad&status=success')).toMatchObject({ window: '24h', status: '' });
  expect(readFilters('?from=Infinity').from).toBeUndefined();
  expect(parentScope(query(filters))).toBe(parentScope(query({ ...filters, request_cursor: 'another-page', group_search: 'another question' })));
  expect(parentScope(query(filters))).not.toBe(parentScope(query({ ...filters, source: 'Another app' })));
});

test('local date values round-trip without shifting the selected instant', () => {
  for (const local of ['2026-01-15T09:30', '2026-07-15T23:45']) expect(dateInput(dateFilter(local))).toBe(local);
  expect(dateInput('bad')).toBe('');
  expect(dateFilter('')).toBe('');
});

test('SDK examples keep credentials in environment variables and model strings escaped', () => {
  for (const language of ['python', 'javascript'] as const) {
    const local = connectionSnippet(language, 'http://127.0.0.1:8765', true, 'model"\\name');
    expect(local).toContain('JEV_OBSERVER_ACCESS_TOKEN');
    expect(local).toContain('"Accept-Encoding": "identity"');
    expect(local).toContain('model\\"\\\\name');
    expect(local).not.toContain('JEV_OBSERVER_CLIENT_TOKEN');
    expect(connectionSnippet(language, 'http://127.0.0.1:8765', false, '')).toContain('JEV_OBSERVER_CLIENT_TOKEN');
  }
  expect(costBasis('provider_reported')).toBe('Provider-reported charge');
  expect(costBasis('configured_estimate')).toBe('Configured estimate');
  expect(storageHint('storage_full')).toContain('Free disk space');
});
