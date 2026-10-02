import { expect, test } from '@playwright/test';
import { latestHealth } from '../src/api';
import type { Health } from '../src/types';

const health: Health = { forwarded: 2, captured: 2, persisted: 2, dropped: 0, write_failures: 0, queue_depth: 0, queued_bytes: 0, last_persisted_at: null, last_gap_at: null, lag_ms: 0, capture_limit: 262144, capture_slots: 256 };

test('server health sample order wins over opposing request order within a process', () => {
  // A slow dashboard query can sample health after a later-started health call.
  const dashboard = { requestOrder: 1, data: { ...health, process_id: 'process-one', sample_sequence: 21, dropped: 17 } };
  const independent = { requestOrder: 2, data: { ...health, process_id: 'process-one', sample_sequence: 20, queue_depth: 3 } };
  expect(latestHealth(dashboard, independent)).toEqual(dashboard.data);
  expect(latestHealth(independent, dashboard)).toEqual(dashboard.data);
});

test('a restarted collector resets health counters without late old-process data restoring them', () => {
  const oldProcess = { requestOrder: 1, data: { ...health, process_id: 'process-one', sample_sequence: 200, dropped: 17 } };
  const restarted = { requestOrder: 2, data: { ...health, process_id: 'process-two', sample_sequence: 1 } };
  expect(latestHealth(oldProcess, restarted)).toEqual(restarted.data);
  expect(latestHealth(restarted, oldProcess)).toEqual(restarted.data);
});
