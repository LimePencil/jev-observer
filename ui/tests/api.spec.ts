import { getEventListeners } from 'node:events';
import { createServer } from 'node:http';
import { mock } from 'node:test';
import { expect, test } from '@playwright/test';
import { api } from '../src/api';

test('read deadlines include unfinished success and error response bodies', async () => {
  const server = createServer((request, response) => {
    response.writeHead(request.url === '/error' ? 503 : 200, { 'Content-Type': 'application/json' });
    response.write('{"unfinished":');
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('Expected a TCP test server');
  try {
    const results = await Promise.allSettled(['success', 'error'].map(path => api(`http://127.0.0.1:${address.port}/${path}`)));
    for (const result of results) {
      expect(result.status).toBe('rejected');
      if (result.status === 'rejected') {
        expect(result.reason).toBeInstanceOf(Error);
        expect(result.reason.name).toBe('TimeoutError');
        expect(result.reason.message).toBe('Request timed out after 15 seconds');
      }
    }
  } finally { server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve())); }
});

test('reads preserve caller cancellation and remove forwarding listeners', async () => {
  const server = createServer((request, response) => {
    if (request.url === '/ready') response.end('{"ready":true}');
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('Expected a TCP test server');
  const controller = new AbortController();
  const reason = new Error('Details closed');
  try {
    await expect(api(`http://127.0.0.1:${address.port}/ready`, { signal: controller.signal })).resolves.toEqual({ ready: true });
    expect(getEventListeners(controller.signal, 'abort')).toHaveLength(0);
    const pending = api(`http://127.0.0.1:${address.port}/pending`, { signal: controller.signal });
    const cancelled = expect(pending).rejects.toBe(reason);
    controller.abort(reason);
    await cancelled;
    expect(getEventListeners(controller.signal, 'abort')).toHaveLength(0);
    await expect(api(`http://127.0.0.1:${address.port}/pending`, { signal: controller.signal })).rejects.toBe(reason);
    expect(getEventListeners(controller.signal, 'abort')).toHaveLength(0);
  } finally { server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve())); }
});

for (const outcome of ['success', 'http error', 'network error'] as const) {
  test(`a completed read clears its deadline after ${outcome}`, async () => {
    const caller = new AbortController();
    let requestSignal: AbortSignal | null | undefined;
    mock.timers.enable({ apis: ['setTimeout'] });
    mock.method(globalThis, 'fetch', async (_input: string | URL | Request, init?: RequestInit) => {
      requestSignal = init?.signal;
      if (outcome === 'network error') throw new Error('Network unavailable');
      return Response.json({ error: 'Storage unavailable' }, { status: outcome === 'success' ? 200 : 503 });
    });
    try {
      const result = api('/api/example', { signal: caller.signal });
      if (outcome === 'success') await expect(result).resolves.toEqual({ error: 'Storage unavailable' });
      else await expect(result).rejects.toThrow(outcome === 'http error' ? 'Storage unavailable' : 'Network unavailable');
      expect(getEventListeners(caller.signal, 'abort')).toHaveLength(0);
      mock.timers.tick(15_000);
      expect(requestSignal).toBeDefined();
      expect(requestSignal?.aborted).toBe(false);
    } finally { mock.timers.reset(); mock.restoreAll(); }
  });
}

test('caller cancellation during response decoding keeps its reason', async () => {
  const caller = new AbortController();
  const reason = new Error('A different request was selected');
  let decoding!: () => void;
  const started = new Promise<void>(resolve => { decoding = resolve; });
  mock.method(globalThis, 'fetch', async (_input: string | URL | Request, init?: RequestInit) => {
    const response = new Response();
    mock.method(response, 'json', () => {
      decoding();
      return new Promise((_, reject) => init?.signal?.addEventListener('abort', () => reject(new DOMException('The operation was aborted', 'AbortError')), { once: true }));
    });
    return response;
  });
  try {
    const result = api('/api/example', { signal: caller.signal });
    const cancelled = expect(result).rejects.toBe(reason);
    await started;
    caller.abort(reason);
    await cancelled;
    expect(getEventListeners(caller.signal, 'abort')).toHaveLength(0);
  } finally { mock.restoreAll(); }
});

test('a slow mutation is not timed out or retried', async () => {
  const caller = new AbortController();
  let release!: (response: Response) => void;
  const pending = new Promise<Response>(resolve => { release = resolve; });
  let requestSignal: AbortSignal | null | undefined;
  mock.timers.enable({ apis: ['setTimeout'] });
  const fetch = mock.method(globalThis, 'fetch', (_input: string | URL | Request, init?: RequestInit) => {
    requestSignal = init?.signal;
    return pending;
  });
  try {
    const result = api('/api/example', { method: 'POST', body: '{}', signal: caller.signal });
    mock.timers.tick(30_000);
    expect(requestSignal?.aborted).toBe(false);
    expect(fetch.mock.callCount()).toBe(1);
    release(Response.json({ ok: true }));
    await expect(result).resolves.toEqual({ ok: true });
  } finally { release(Response.json({ ok: true })); mock.timers.reset(); mock.restoreAll(); }
});

test('a longer read budget includes body decoding and still ends at its own deadline', async () => {
  let signal: AbortSignal | null | undefined, began!: () => void;
  const decoding = new Promise<void>(resolve => { began = resolve; });
  mock.timers.enable({ apis: ['setTimeout'] });
  mock.method(globalThis, 'fetch', async (_input: string | URL | Request, init?: RequestInit) => {
    signal = init?.signal;
    const response = new Response();
    mock.method(response, 'json', () => {
      began();
      return new Promise((_, reject) => signal?.addEventListener('abort', () => reject(signal?.reason), { once: true }));
    });
    return response;
  });
  try {
    const result = api('/api/dashboard', {}, 45_000).catch(error => error);
    await decoding;
    mock.timers.tick(44_999);
    expect(signal?.aborted).toBe(false);
    mock.timers.tick(1);
    const error = await result;
    expect(error).toBeInstanceOf(Error);
    expect(error).toMatchObject({ name: 'TimeoutError', message: 'Request timed out after 45 seconds' });
  } finally { mock.timers.reset(); mock.restoreAll(); }
});
