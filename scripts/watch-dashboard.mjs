#!/usr/bin/env node
// Optional real-browser companion for a running local load benchmark.
import { chromium } from '../ui/node_modules/playwright/index.mjs';
import fs from 'node:fs/promises';
import { constants } from 'node:fs';
import path from 'node:path';
import { performance } from 'node:perf_hooks';

const args = Object.fromEntries(process.argv.slice(2).map(value => {
  const match = /^--(origin|seconds|output|access-token-file)=(.+)$/.exec(value);
  if (!match) throw new Error('Use --origin=URL --access-token-file=PATH [--seconds=N] [--output=PATH]');
  return [match[1], match[2]];
}));
const origin = new URL(args.origin ?? 'http://127.0.0.1:8765');
if (origin.protocol !== 'http:' || origin.hostname !== '127.0.0.1' || origin.username || origin.password || origin.pathname !== '/' || origin.search || origin.hash) throw new Error('Use a loopback Observer origin without credentials, path, query or fragment');
const seconds = Number(args.seconds ?? 60);
if (!Number.isFinite(seconds) || seconds < 1) throw new Error('Use a positive duration');
const output = path.resolve(args.output ?? 'reports/benchmarks/browser.json');
if (!args['access-token-file']) throw new Error('Supply --access-token-file=PATH using the workspace token file printed by Observer or the benchmark');
const tokenFile = await fs.open(args['access-token-file'], constants.O_RDONLY | (constants.O_NONBLOCK ?? 0));
let token;
try {
  if (!(await tokenFile.stat()).isFile()) throw new Error('The access-token file must be a regular file');
  // One byte beyond the expected length rejects trailing content without
  // allocating for an arbitrarily large malformed token file.
  const bytes = Buffer.alloc(75);
  const { bytesRead } = await tokenFile.read(bytes, 0, bytes.length, 0);
  token = bytes.subarray(0, bytesRead).toString('utf8');
  if (!/^jo_access_[0-9a-fA-F]{64}$/.test(token)) throw new Error('The access-token file is invalid');
} finally { await tokenFile.close(); }
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, httpCredentials: { username: 'observer', password: token, origin: origin.origin } });
const report = { started_at: new Date().toISOString(), origin: origin.origin, seconds, snapshots: 0, first_requests: null, last_requests: null, maximum_answers: 0, latencies_ms: [], errors: [] };
const starts = new WeakMap();
const pending = new Set();
page.on('request', request => starts.set(request, performance.now()));
page.on('pageerror', error => report.errors.push(error.message));
page.on('console', message => { if (message.type() === 'error') report.errors.push(message.text()); });
page.on('response', response => {
  if (new URL(response.url()).pathname !== '/api/dashboard') return;
  const work = (async () => {
    try {
      if (response.status() !== 200) throw new Error(`Dashboard HTTP ${response.status()}`);
      const data = await response.json();
      report.snapshots++;
      report.first_requests ??= data.summary.request_count;
      report.last_requests = data.summary.request_count;
      report.maximum_answers = Math.max(report.maximum_answers, data.summary.answer_count);
      report.latencies_ms.push(performance.now() - starts.get(response.request()));
    } catch (error) { report.errors.push(String(error)); }
  })().finally(() => pending.delete(work));
  pending.add(work);
});
try {
  const health = await page.request.get(origin.origin + '/api/health');
  if (health.status() === 401) throw new Error('Observer rejected the access token; use the token file for this workspace');
  if (!health.ok()) throw new Error(`Observer health returned HTTP ${health.status()}`);
  const response = await page.goto(origin.origin);
  if (response?.status() === 401) throw new Error('Observer rejected the access token; use the token file for this workspace');
  await page.getByRole('heading', { name: 'Request stream', exact: true }).waitFor();
  await new Promise(resolve => {
    const timer = setTimeout(resolve, seconds * 1000);
    const stop = () => { clearTimeout(timer); report.stopped_early = true; resolve(); };
    process.once('SIGINT', stop);
    process.once('SIGTERM', stop);
  });
  report.visible_requests = await page.locator('.metrics').innerText();
  report.page_overflow = await page.evaluate(() => document.documentElement.scrollWidth > innerWidth);
  await page.getByRole('heading', { name: 'Recurring questions', exact: true }).waitFor();
  await page.getByRole('heading', { name: 'Request activity', exact: true }).waitFor();
  await fs.mkdir(path.dirname(output), { recursive: true });
  await page.screenshot({ path: output.replace(/\.json$/, '.png'), fullPage: true });
  await Promise.all(pending);
  report.complete = report.snapshots > 1 && report.last_requests > report.first_requests && !report.page_overflow && !report.errors.length;
  await fs.writeFile(output, JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify({ output, snapshots: report.snapshots, first_requests: report.first_requests, last_requests: report.last_requests, complete: report.complete, errors: report.errors }));
  if (!report.complete) process.exitCode = 1;
} finally { await browser.close(); }
