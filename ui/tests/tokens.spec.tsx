/** @jsxImportSource react */
import { expect, test } from '@playwright/test';
import { renderToStaticMarkup } from 'react-dom/server';
import { TokenMetric } from '../src/components';
import type { Summary } from '../src/types';

const base: Summary = {
  request_count: 2, answer_count: 2, error_count: 0, p50_ms: null, p95_ms: null,
  input_tokens: 100, output_tokens: 20, cost_usd: null, cost_known_requests: 0,
};

for (const { name, usage, label, total, coverage } of [
  { name: 'disjoint partial usage', usage: { input_tokens_known_requests: 1, output_tokens_known_requests: 1 }, label: 'Known tokens', total: '120', coverage: ['Input: 1 / 2 requests', 'Output: 1 / 2 requests'] },
  { name: 'complete usage', usage: { input_tokens_known_requests: 2, output_tokens_known_requests: 2 }, label: 'Reported tokens', total: '120', coverage: ['Input: 2 / 2 requests', 'Output: 2 / 2 requests'] },
  { name: 'one known direction', usage: { output_tokens: null, input_tokens_known_requests: 1, output_tokens_known_requests: 0 }, label: 'Known tokens', total: '100', coverage: ['100 in / Unknown out', 'Output: 0 / 2 requests'] },
  { name: 'known zero', usage: { input_tokens: 0, output_tokens: 0, input_tokens_known_requests: 2, output_tokens_known_requests: 2 }, label: 'Reported tokens', total: '0', coverage: ['0 in / 0 out', 'Input: 2 / 2 requests'] },
  { name: 'no reported usage', usage: { input_tokens: null, output_tokens: null, input_tokens_known_requests: 0, output_tokens_known_requests: 0 }, label: 'Known tokens', total: 'Unknown', coverage: ['Input: 0 / 2 requests', 'Output: 0 / 2 requests'] },
  { name: 'older API without coverage', usage: {}, label: 'Known tokens', total: '120', coverage: ['Usage coverage unavailable'] },
] satisfies { name: string; usage: Partial<Summary>; label: string; total: string; coverage: string[] }[]) {
  test(`token metric renders ${name} honestly`, () => {
    const html = renderToStaticMarkup(<TokenMetric summary={{ ...base, ...usage }} />);
    const text = html.replace(/<[^>]*>/g, '');
    expect(text).toContain(label);
    expect(html).toContain(`<span class="metric-value">${total}</span>`);
    for (const detail of coverage) expect(text).toContain(detail);
  });
}
