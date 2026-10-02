import { useCallback, useEffect, useRef, useState } from 'react';
import type { Dashboard, Filters, Health } from './types';

type HealthSample = { data: Health; requestOrder: number };
let nextHealthRequestOrder = 0;

export function latestHealth(first: HealthSample | null, second: HealthSample | null): Health | undefined {
  if (!first) return second?.data;
  if (!second) return first.data;
  const comparable = first.data.process_id && first.data.process_id === second.data.process_id
    && first.data.sample_sequence != null && second.data.sample_sequence != null;
  // Sample order survives delayed delivery. Request order is the fallback for
  // process changes and responses from older servers without sample metadata.
  const firstIsNewer = comparable
    ? first.data.sample_sequence! >= second.data.sample_sequence!
    : first.requestOrder >= second.requestOrder;
  return firstIsNewer ? first.data : second.data;
}

export function query(filters: Partial<Filters>): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(filters)) if (value) params.set(key, value);
  return params.toString();
}

export function parentScope(search: string): string {
  const params = new URLSearchParams(search);
  for (const field of ['request_cursor', 'group_cursor', 'group_search']) params.delete(field);
  params.sort();
  return params.toString();
}

export const defaultFilters: Filters = { source: '', model: '', window: '24h', status: '', search: '', group: '' };
export function readFilters(search = window.location.search): Filters {
  const params = new URLSearchParams(search);
  const filters: Filters = { ...defaultFilters };
  for (const key of ['source', 'model', 'status', 'search', 'group', 'request_cursor', 'group_cursor', 'group_search'] as const) {
    if (params.has(key)) filters[key] = params.get(key) ?? '';
  }
  if (filters.status !== 'error') filters.status = '';
  const windowValue = params.get('window');
  if (windowValue && ['1h', '24h', '7d', 'all'].includes(windowValue)) filters.window = windowValue;
  for (const key of ['from', 'to'] as const) {
    const value = params.get(key);
    if (value && /^-?\d+$/.test(value) && Number.isSafeInteger(Number(value)) && Number.isFinite(new Date(Number(value)).getTime())) filters[key] = value;
  }
  if (filters.from || filters.to) filters.window = 'all';
  return filters;
}

export async function api<T>(path: string, init: RequestInit = {}): Promise<T> {
  const response = await fetch(path, { ...init, headers: { ...((init.method && init.method !== 'GET') ? { 'Content-Type': 'application/json', 'X-Observer-Request': '1' } : {}), ...init.headers } });
  if (!response.ok) {
    let message = `Request failed (${response.status})`;
    try { const error = await response.json(); message = typeof error.error === 'string' ? error.error : typeof error.message === 'string' ? error.message : message; } catch { /* Do not expose arbitrary response bodies. */ }
    throw new Error(message);
  }
  return response.json();
}

export function useDashboard(filters: Filters) {
  const [snapshot, setSnapshot] = useState<{ data: Dashboard; scope: string; requestOrder: number } | null>(null);
  const [error, setError] = useState('');
  const [refresh, setRefresh] = useState(0);
  const generation = useRef(0);
  const scope = query(filters);
  const invalidate = useCallback(() => setRefresh(value => value + 1), []);
  const reset = useCallback(() => {
    generation.current += 1;
    setSnapshot(null); setError(''); setRefresh(value => value + 1);
  }, []);
  useEffect(() => {
    let stopped = false;
    let controller: AbortController | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let inFlight = false;
    async function tick() {
      if (stopped || inFlight || document.hidden) return;
      inFlight = true;
      controller = new AbortController();
      const currentGeneration = generation.current;
      const started = performance.now();
      const requestOrder = ++nextHealthRequestOrder;
      let succeeded = false;
      try {
        const data = await api<Dashboard>(`/api/dashboard?${scope}`, { signal: controller.signal });
        if (!stopped && !controller.signal.aborted && currentGeneration === generation.current) { setSnapshot({ data, scope, requestOrder }); setError(''); succeeded = true; }
      } catch (error) {
        if (!stopped && !controller.signal.aborted && currentGeneration === generation.current) setError(error instanceof Error ? error.message : 'Unable to reach Observer');
      } finally {
        inFlight = false;
        // Count query time toward the live cadence. A slow query still gets a
        // brief idle period, and failures retain the full retry backoff.
        const delay = succeeded ? Math.max(100, 1000 - (performance.now() - started)) : 1000;
        if (!stopped && !document.hidden) timer = setTimeout(tick, delay);
      }
    }
    const onVisibility = () => { if (timer) clearTimeout(timer); if (!document.hidden) void tick(); else controller?.abort(); };
    document.addEventListener('visibilitychange', onVisibility);
    void tick();
    return () => { stopped = true; if (timer) clearTimeout(timer); controller?.abort(); document.removeEventListener('visibilitychange', onVisibility); };
  }, [scope, refresh]);
  return { snapshot, error, invalidate, reset, scope };
}

export function useResource<T>(path: string | null) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState('');
  const [version, setVersion] = useState(0);
  const reload = useCallback(() => setVersion(value => value + 1), []);
  const update = useCallback((apply: (previous: T) => T) => setData(previous => previous === null ? null : apply(previous)), []);
  useEffect(() => {
    setData(null); setError('');
    if (!path) return;
    const controller = new AbortController();
    api<T>(path, { signal: controller.signal }).then(value => { if (!controller.signal.aborted) setData(value); }).catch(error => { if (!controller.signal.aborted) setError(error instanceof Error ? error.message : 'Unable to load details'); });
    return () => controller.abort();
  }, [path, version]);
  return { data, error, reload, update };
}

// Health is independent of database queries, so a failed writer cannot hide gaps.
export function useLiveHealth() {
  const [health, setHealth] = useState<HealthSample | null>(null);
  useEffect(() => {
    let stopped = false, busy = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let controller: AbortController | undefined;
    async function tick() {
      if (stopped || busy || document.hidden) return;
      busy = true; controller = new AbortController();
      const requestOrder = ++nextHealthRequestOrder;
      try { const data = await api<Health>('/api/health', { signal: controller.signal }); if (!stopped && !controller.signal.aborted) setHealth({ data, requestOrder }); }
      catch { /* Dashboard connection status communicates transport failures. */ }
      finally { busy = false; if (!stopped && !document.hidden) timer = setTimeout(tick, 1000); }
    }
    const visible = () => { if (timer) clearTimeout(timer); if (document.hidden) controller?.abort(); else void tick(); };
    document.addEventListener('visibilitychange', visible); void tick();
    return () => { stopped = true; if (timer) clearTimeout(timer); controller?.abort(); document.removeEventListener('visibilitychange', visible); };
  }, []);
  return health;
}

export function downloadExport(filters: Filters, format: 'jsonl' | 'csv') {
  // Let the browser stream Content-Disposition downloads directly to disk.
  // An HTTP error opens separately, preserving the current investigation.
  const link = document.createElement('a');
  link.href = `/api/export?${query(filters)}&format=${format}`;
  link.target = '_blank'; link.rel = 'noopener';
  link.click();
}
