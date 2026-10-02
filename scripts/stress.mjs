#!/usr/bin/env node
// Local failure injection only. No provider calls, installs, or real credentials.
import assert from 'node:assert/strict';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import fs from 'node:fs/promises';
import path from 'node:path';
import { spawn, execFileSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { performance, monitorEventLoopDelay } from 'node:perf_hooks';
import { fileURLToPath } from 'node:url';

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const args = Object.fromEntries(process.argv.slice(2).map(arg => {
  const match = /^--([a-z-]+)(?:=(.*))?$/.exec(arg);
  if (!match) throw new Error(`Use --name=value arguments: ${arg}`);
  return [match[1], match[2] ?? true];
}));
const allowed = new Set(['binary', 'output', 'work-dir', 'python', 'case', 'burst-rate', 'burst-seconds', 'keep-db', 'help']);
for (const key of Object.keys(args)) if (!allowed.has(key)) throw new Error(`Unknown option --${key}`);
if (args.help) {
  console.log('node scripts/stress.mjs [--binary=PATH] [--output=PATH] [--case=all|capture-budget|locked-writer|burst] [--burst-rate=2000] [--burst-seconds=5] [--python=python3] [--work-dir=PATH] [--keep-db]');
  process.exit(0);
}
const binary = path.resolve(String(args.binary ?? 'target/release/jev-observer'));
const output = path.resolve(String(args.output ?? 'reports/stress/latest.json'));
const python = String(args.python ?? 'python3');
const burstRate = Number(args['burst-rate'] ?? 2000), burstSeconds = Number(args['burst-seconds'] ?? 5);
assert(Number.isInteger(burstRate) && burstRate >= 1 && burstRate <= 10000, 'Burst rate must be an integer from 1 to 10000');
assert(Number.isFinite(burstSeconds) && burstSeconds >= 1 && burstSeconds <= 30 && burstRate * burstSeconds <= 100000, 'Burst must last 1–30 seconds and offer at most 100000 calls');
const selected = String(args.case ?? 'all');
assert(['all', 'capture-budget', 'locked-writer', 'burst'].includes(selected), 'Unknown case');
const fixture = {
  model: 'jev-stress-fixture', state: { text: 'Synthetic billing request, used only by this local stress harness.' },
  questions: {
    routing: { type: 'choice', instructions: 'Select a team', criteria: { technical: 'Software', billing: 'Payments' } },
    urgency: { type: 'noul', instructions: 'Is this urgent?' },
    quality: { type: 'score', instructions: 'Rate completeness', criteria: ['Low', 'Medium', 'High'] },
  },
};
const requestBytes = Buffer.from(JSON.stringify(fixture));
const responseBytes = Buffer.from(JSON.stringify({
  model: 'jev-stress-fixture-1',
  answers: {
    routing: { type: 'choice', choice: 'billing', probabilities: { technical: 0.2, billing: 0.8 }, confidence: 0.6 },
    urgency: { type: 'noul', noul: 0.8 },
    quality: { type: 'score', score: 1.6, probabilities: { 0: 0.1, 1: 0.2, 2: 0.7 }, legend: { 0: 'Low', 1: 'Medium', 2: 'High' }, confidence: 0.6 },
  }, usage: { input_tokens: 100, output_tokens: 10 }, extension: { preserved: true },
}));
const key = 'observer-local-stress-placeholder';
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const quantile = (values, fraction) => values.length ? [...values].sort((a, b) => a - b)[Math.max(0, Math.ceil(values.length * fraction) - 1)] : null;
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const cleanEnvironment = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith('TYPESAFE_') && !['http_proxy', 'https_proxy', 'all_proxy'].includes(name.toLowerCase())));
cleanEnvironment.NO_PROXY = '127.0.0.1,localhost';
if (args['keep-db'] && !process.env.JEV_OBSERVER_DB_KEY) throw new Error('--keep-db requires JEV_OBSERVER_DB_KEY so retained encrypted databases can be reopened');
cleanEnvironment.JEV_OBSERVER_DB_KEY = process.env.JEV_OBSERVER_DB_KEY ?? randomBytes(32).toString('hex');
let dashboardAuthorization = '';
let observerAccess = '';
const pythonRuntime = JSON.parse(execFileSync(python, ['-c', 'import json,os,platform,sqlite3; print(json.dumps({"version":platform.python_version(),"sqlite_version":sqlite3.sqlite_version,"clock_ticks_per_second":os.sysconf("SC_CLK_TCK") if hasattr(os,"sysconf") else None}))'], { encoding: 'utf8', timeout: 5000, env: cleanEnvironment }));

function waitForExit(child, milliseconds) {
  if (child.exitCode !== null) return Promise.resolve(child.exitCode);
  return new Promise(resolve => {
    const exited = code => { clearTimeout(timer); resolve(code); };
    const timer = setTimeout(() => { child.off('exit', exited); resolve('timeout'); }, milliseconds);
    child.once('exit', exited);
  });
}

async function freePort() {
  const probe = net.createServer();
  await new Promise((resolve, reject) => { probe.once('error', reject); probe.listen(0, '127.0.0.1', resolve); });
  const port = probe.address().port;
  await new Promise(resolve => probe.close(resolve));
  return port;
}

function exchange(port, route, agent, post = false, observerMetadata = true) {
  return new Promise(resolve => {
    const started = performance.now();
    let settled = false;
    const finish = value => { if (!settled) { settled = true; clearTimeout(deadline); resolve({ ...value, ms: performance.now() - started }); } };
    const request = http.request({ hostname: '127.0.0.1', port, path: route, method: post ? 'POST' : 'GET', agent,
      headers: post ? { authorization: `Bearer ${key}`, 'content-type': 'application/json', 'content-length': requestBytes.length, 'accept-encoding': 'identity', 'x-observer-access': observerAccess, ...(observerMetadata ? { 'x-observer-source': 'local-stress' } : {}) } : { authorization: dashboardAuthorization },
    }, response => {
      const chunks = [];
      response.on('data', chunk => chunks.push(chunk));
      response.on('end', () => finish({ status: response.statusCode, body: Buffer.concat(chunks) }));
      response.on('error', error => finish({ status: 0, error: error.message, body: Buffer.alloc(0) }));
    });
    const deadline = setTimeout(() => request.destroy(new Error('5 second client deadline')), 5000);
    request.on('error', error => finish({ status: 0, error: error.message, body: Buffer.alloc(0) }));
    request.end(post ? requestBytes : undefined);
  });
}

async function jsonGet(context, route) {
  const result = await exchange(context.port, route, context.readAgent);
  assert.equal(result.status, 200, `${route}: HTTP ${result.status}: ${result.error ?? result.body}`);
  return JSON.parse(result.body);
}

async function drain(context) {
  const started = performance.now();
  while (performance.now() - started < 15000) {
    const health = await jsonGet(context, '/api/health');
    if (health.queue_depth === 0 && health.active_captures === 0) return health;
    await sleep(25);
  }
  throw new Error('Capture pipeline did not drain within 15 seconds');
}

async function load(context, { rate, seconds, label }) {
  const count = Math.floor(rate * seconds), started = performance.now();
  const cpuStart = process.cpuUsage(), loopStart = performance.eventLoopUtilization();
  const loopDelay = monitorEventLoopDelay({ resolution: 10 }); loopDelay.enable();
  const pending = new Set(), latencies = [], lateness = [], failures = [];
  let submitted = 0, completedBeforeTwoSeconds = 0, peakInFlight = 0, stoppedReason = null;
  offering: while (submitted < count) {
    const due = Math.min(count, Math.floor((performance.now() - started) * rate / 1000) + 1);
    while (submitted < due) {
      if (pending.size >= 2048) {
        stoppedReason = 'Load generator reached its 2048-call concurrency limit';
        break offering;
      }
      const index = submitted++;
      lateness.push(Math.max(0, performance.now() - started - index * 1000 / rate));
      const work = exchange(context.port, '/v1/systemone', context.callAgent, true, !context.direct).then(result => {
        latencies.push(result.ms);
        if (performance.now() - started < 2000) completedBeforeTwoSeconds++;
        if (result.status !== 200 || !result.body.equals(responseBytes)) failures.push({ index, status: result.status, error: result.error ?? 'Response bytes differ' });
      }).finally(() => pending.delete(work));
      pending.add(work);
      peakInFlight = Math.max(peakInFlight, pending.size);
    }
    await sleep(1);
  }
  const offeringElapsed = (performance.now() - started) / 1000;
  await Promise.all(pending);
  loopDelay.disable();
  const elapsed = (performance.now() - started) / 1000, cpu = process.cpuUsage(cpuStart);
  const result = { label, target_rps: rate, duration_seconds: seconds, planned: count, offered: submitted, completed: latencies.length, errors: failures.length,
    offering_elapsed_seconds: offeringElapsed, stopped_reason: stoppedReason, peak_in_flight: peakInFlight,
    failures: failures.slice(0, 10), elapsed_seconds: elapsed, completed_before_two_seconds: completedBeforeTwoSeconds,
    generator: { cpu_percent_one_core: (cpu.user + cpu.system) / (elapsed * 10000), event_loop_utilization: performance.eventLoopUtilization(loopStart).utilization,
      event_loop_delay_ms: { p99: loopDelay.percentile(99) / 1e6, max: loopDelay.max / 1e6 } },
    latency_ms: { p50: quantile(latencies, 0.5), p95: quantile(latencies, 0.95), p99: quantile(latencies, 0.99), max: latencies.reduce((maximum, value) => Math.max(maximum, value), 0) },
    scheduling_lateness_ms: { p95: quantile(lateness, 0.95), p99: quantile(lateness, 0.99) } };
  console.log(JSON.stringify({ case: context.name, phase: label, ...result }));
  return result;
}

function checkLoad(result) {
  assert.equal(result.stopped_reason, null, result.stopped_reason ?? 'Unexpected admission stop');
  assert.equal(result.offered, result.planned, 'Load generator did not offer the planned request count');
  assert.equal(result.errors, 0, `${result.label}: client errors or changed response bytes`);
  assert.equal(result.completed, result.offered);
  assert(result.latency_ms.p99 < 1000, `${result.label}: p99 reached the 1000 ms stall-detection bound`);
  assert(result.scheduling_lateness_ms.p99 <= 100, `${result.label}: generator missed the offered rate`);
}

async function lockWriter(database) {
  const helper = path.join(repositoryRoot, 'target/release/examples/hold_db_lock');
  const child = spawn(helper, [database], { stdio: ['pipe', 'pipe', 'pipe'], env: cleanEnvironment });
  let stderr = '';
  child.stderr.on('data', data => { stderr += data; });
  try {
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('SQLite lock helper did not start within 7 seconds')), 7000);
      const finish = (error) => { clearTimeout(timer); error ? reject(error) : resolve(); };
      child.once('error', finish);
      child.once('exit', code => finish(new Error(`SQLite lock helper exited ${code}: ${stderr}`)));
      child.stdout.once('data', bytes => finish(bytes.toString().trim() === 'locked' ? null : new Error('Unexpected SQLite lock helper output')));
    });
    return child;
  } catch (error) { child.kill('SIGKILL'); throw error; }
}

async function releaseLock(child) {
  if (!child || child.exitCode !== null) return;
  const exited = waitForExit(child, 3000);
  child.stdin.end('\n');
  const code = await exited;
  if (code === 'timeout') { child.kill('SIGKILL'); await waitForExit(child, 2000); }
  assert.equal(code, 0, 'SQLite lock helper did not release cleanly');
}

async function runCase(spec, directory) {
  dashboardAuthorization = '';
  observerAccess = '';
  const context = { name: spec.name, port: await freePort(), callAgent: new http.Agent({ keepAlive: true, maxSockets: 2048, maxFreeSockets: 128 }), readAgent: new http.Agent({ keepAlive: true, maxSockets: 2 }) };
  const result = { name: spec.name, expected_capture_loss: spec.loss, upstream_delay_ms: spec.delay, checks: {}, passed: false };
  const database = path.join(directory, `${spec.name}.sqlite`);
  let proxy, lock, polling, dashboardPolling, stopPolling = false, proxyLog = '', upstreamAttempts = 0, healthErrors = 0, peakRssKiB = null;
  let peakQueue = 0, peakActive = 0, peakLag = 0, peakCpu = null, previousCpu = null;
  const mockErrors = [], dashboardLatencies = [], dashboardErrors = [], recentHealth = [];
  const mock = http.createServer((request, response) => {
    const chunks = [];
    request.on('data', chunk => chunks.push(chunk));
    request.on('end', () => {
      upstreamAttempts++;
      if (request.url !== '/v1/systemone' || request.headers.authorization !== `Bearer ${key}` || !Buffer.concat(chunks).equals(requestBytes) || Object.keys(request.headers).some(key => key.startsWith('x-observer-'))) mockErrors.push('Unexpected forwarded request');
      setTimeout(() => { response.writeHead(200, { 'content-type': 'application/json', 'content-length': responseBytes.length }); response.end(responseBytes); }, spec.delay);
    });
  });
  mock.keepAliveTimeout = 60000;
  try {
    await new Promise((resolve, reject) => { mock.once('error', reject); mock.listen(0, '127.0.0.1', resolve); });
    proxy = spawn(binary, ['--port', String(context.port), '--db', database, '--upstream', `http://127.0.0.1:${mock.address().port}/v1/systemone`, ...spec.flags], { env: cleanEnvironment, stdio: ['ignore', 'ignore', 'pipe'] });
    proxy.stderr.on('data', data => { proxyLog = (proxyLog + data).slice(-8000); });
    proxy.on('error', error => { proxyLog += error.message; });
    for (let attempt = 0; ; attempt++) {
      try { const token = await fs.readFile(database.replace(/\.sqlite$/, '.access-token'), 'utf8'); observerAccess = token; dashboardAuthorization = `Basic ${Buffer.from(`observer:${token}`).toString('base64')}`; } catch { /* Startup has not created the token yet. */ }
      const response = dashboardAuthorization ? await exchange(context.port, '/api/health', context.readAgent) : { status: 0 };
      if (response.status === 200) break;
      assert(attempt < 100 && proxy.exitCode === null, `Proxy startup failed: ${proxyLog}`);
      await sleep(50);
    }
    result.settings = await jsonGet(context, '/api/settings');
    polling = (async () => {
      while (!stopPolling) {
        try {
          const health = await jsonGet(context, '/api/health');
          peakQueue = Math.max(peakQueue, health.queue_depth); peakActive = Math.max(peakActive, health.active_captures); peakLag = Math.max(peakLag, health.lag_ms);
          recentHealth.push({ at: new Date().toISOString(), forwarded: health.forwarded, persisted: health.persisted, dropped: health.dropped, queue_depth: health.queue_depth, active_captures: health.active_captures, lag_ms: health.lag_ms });
          if (recentHealth.length > 30) recentHealth.shift();
        } catch { healthErrors++; }
        if (process.platform === 'linux') {
          try { const status = await fs.readFile(`/proc/${proxy.pid}/status`, 'utf8'); const match = /^VmRSS:\s+(\d+)/m.exec(status); if (match) peakRssKiB = Math.max(peakRssKiB ?? 0, Number(match[1])); } catch { /* Missing samples stay unknown, never zero. */ }
          try {
            const stat = await fs.readFile(`/proc/${proxy.pid}/stat`, 'utf8');
            const fields = stat.slice(stat.lastIndexOf(')') + 2).trim().split(/\s+/);
            const sample = { ticks: Number(fields[11]) + Number(fields[12]), at: performance.now() };
            if (previousCpu && pythonRuntime.clock_ticks_per_second) peakCpu = Math.max(peakCpu ?? 0, (sample.ticks - previousCpu.ticks) / pythonRuntime.clock_ticks_per_second / ((sample.at - previousCpu.at) / 1000) * 100);
            previousCpu = sample;
          } catch { /* CPU sampling is optional evidence. */ }
        }
        await sleep(100);
      }
    })();
    result.warmup = await load(context, { rate: 5, seconds: 1, label: 'warmup' });
    checkLoad(result.warmup);
    const before = await drain(context);
    if (spec.name === 'burst') {
      result.direct_baseline = await load({ ...context, port: mock.address().port, direct: true }, { rate: spec.rate, seconds: Math.min(10, spec.seconds), label: 'direct mock baseline' });
      checkLoad(result.direct_baseline);
    }
    dashboardPolling = (async () => {
      while (!stopPolling) {
        const snapshot = await exchange(context.port, '/api/dashboard?window=all', context.readAgent);
        dashboardLatencies.push(snapshot.ms);
        if (snapshot.status !== 200) dashboardErrors.push({ status: snapshot.status, error: snapshot.error ?? snapshot.body.toString() });
        await sleep(1000);
      }
    })();
    if (spec.lock) lock = await lockWriter(database);
    const started = performance.now();
    result.pressure = await load(context, { rate: spec.rate, seconds: spec.seconds, label: 'pressure' });
    // Preserve drained accounting and recovery evidence even when offered-rate
    // or client-latency bounds failed. The case must still finish as failed.
    try { checkLoad(result.pressure); } catch (error) { result.pressure_validation_error = String(error); }
    result.pressure_health_before_release = await jsonGet(context, '/api/health');
    if (spec.lock) {
      result.lock_held_ms = performance.now() - started;
      result.checks.clients_completed_while_locked = lock.exitCode === null && result.pressure.completed === result.pressure.offered;
      assert(result.checks.clients_completed_while_locked);
      assert(result.pressure.completed_before_two_seconds >= spec.rate * 1.5, 'Clients stalled before the SQLite busy timeout');
      assert(result.pressure_health_before_release.write_failures > before.write_failures, 'The injected lock did not cause a visible writer failure');
      await releaseLock(lock); lock = null;
    }
    const pressured = await drain(context);
    result.after_pressure = pressured;
    result.pressure_accounting = { forwarded: pressured.forwarded - before.forwarded, persisted: pressured.persisted - before.persisted, dropped: pressured.dropped - before.dropped,
      skipped_before_queue: (pressured.forwarded - before.forwarded) - (pressured.captured - before.captured), accepted_but_lost: (pressured.captured - before.captured) - (pressured.persisted - before.persisted) };
    assert.equal(result.pressure_accounting.forwarded, result.pressure.offered);
    assert.equal(result.pressure_accounting.persisted + result.pressure_accounting.dropped, result.pressure.offered, 'Every forwarded request must be either saved or visibly dropped after drain');
    if (spec.loss) assert(pressured.dropped > before.dropped && pressured.last_gap_at !== null, 'Expected capture loss was not visible');
    result.checks.loss_accounting_exact = true;
    result.recovery = await load(context, { rate: 10, seconds: 2, label: 'recovery' });
    checkLoad(result.recovery);
    const final = await drain(context);
    result.final_health = final;
    assert.equal(final.persisted - pressured.persisted, result.recovery.offered, 'Recovery did not persist every request');
    assert.equal(final.dropped, pressured.dropped, 'Capture loss continued during recovery');
    assert.equal(final.write_failures, pressured.write_failures, 'Writer still failed after recovery');
    result.checks.recovery_complete = true;
    const dashboard = await jsonGet(context, '/api/dashboard?window=all');
    result.summary = dashboard.summary;
    const total = result.warmup.offered + result.pressure.offered + result.recovery.offered;
    assert.equal(upstreamAttempts, total + (result.direct_baseline?.offered ?? 0)); assert.equal(final.forwarded, total); assert.equal(final.persisted + final.dropped, total);
    assert.equal(final.truncated, 0); assert.equal(dashboard.summary.incomplete_count, 0);
    assert.equal(dashboard.summary.request_count, final.persisted); assert.equal(dashboard.summary.answer_count, final.persisted * 3);
    assert.equal(dashboard.summary.input_tokens, final.persisted * 100); assert.equal(dashboard.summary.output_tokens, final.persisted * 10);
    assert.equal(dashboard.groups.length, 3); assert(dashboard.groups.every(group => group.valid_count === group.answer_count));
    assert.equal(mockErrors.length, 0); assert.equal(healthErrors, 0); assert.equal(dashboardErrors.length, 0);
    result.checks.persisted_accounting_exact = true;
    result.checks.responses_preserved = true;
    result.checks.health_independent_of_writer = true;
    const databaseFile = await fs.open(database, 'r');
    const databaseHeader = Buffer.alloc(16);
    try { await databaseFile.read(databaseHeader, 0, 16, 0); } finally { await databaseFile.close(); }
    assert(!databaseHeader.equals(Buffer.from('SQLite format 3\0')), 'Live stress history must remain encrypted');
    result.checks.live_database_not_plaintext = true;
    if (result.pressure_validation_error) throw new Error(result.pressure_validation_error);
    result.passed = true;
  } catch (error) {
    result.error = error.stack ?? String(error);
    if (proxy && proxy.exitCode === null) {
      try { result.failure_health = await jsonGet(context, '/api/health'); } catch { /* Existing error remains primary. */ }
    }
  }
  finally {
    if (lock) { try { await releaseLock(lock); } catch (error) { result.cleanup_error = String(error); result.passed = false; } }
    stopPolling = true;
    if (polling) await polling;
    if (dashboardPolling) await dashboardPolling;
    result.upstream_attempts = upstreamAttempts; result.health_poll_errors = healthErrors;
    result.direct_upstream_attempts = result.direct_baseline?.offered ?? 0;
    result.proxy_upstream_attempts = upstreamAttempts - result.direct_upstream_attempts;
    result.recent_health_samples = recentHealth;
    result.dashboard = { queries: dashboardLatencies.length, p95_ms: quantile(dashboardLatencies, 0.95), errors: dashboardErrors };
    result.peak_sampled_rss_mib = peakRssKiB === null ? null : peakRssKiB / 1024;
    result.peak_sampled_proxy_cpu_percent_one_core = peakCpu;
    result.peak_sampled_queue_depth = peakQueue; result.peak_sampled_active_captures = peakActive; result.peak_sampled_lag_ms = peakLag;
    context.callAgent.destroy(); context.readAgent.destroy();
    mock.closeAllConnections(); await new Promise(resolve => mock.close(resolve));
    if (proxy && proxy.exitCode === null) {
      const exited = waitForExit(proxy, 17000);
      proxy.kill('SIGTERM');
      const code = await exited;
      if (code === 'timeout') { proxy.kill('SIGKILL'); await waitForExit(proxy, 2000); }
      result.shutdown_exit = code;
      if (code !== 0) { result.passed = false; result.cleanup_error = `Proxy shutdown exit: ${code}`; }
    }
    if (healthErrors || dashboardErrors.length) result.passed = false;
    if (!result.passed) result.proxy_log = proxyLog;
  }
  return result;
}

const workRoot = path.resolve(String(args['work-dir'] ?? '.jev-observer/stress'));
await fs.mkdir(workRoot, { recursive: true });
const directory = await fs.mkdtemp(path.join(workRoot, 'run-'));
const report = { started_at: new Date().toISOString(), scope: 'Loopback failure injection; no provider inference', binary_sha256: hash(await fs.readFile(binary)), harness_sha256: hash(await fs.readFile(new URL(import.meta.url))),
  node: process.version, platform: `${os.platform()} ${os.release()} ${os.arch()}`, cpu: os.cpus()[0]?.model, logical_cpus: os.cpus().length,
  python: pythonRuntime,
  request_bytes: requestBytes.length, response_bytes: responseBytes.length, capture_state: false, rss_sampling_ms: 100, cases: [], passed: false,
  notes: ['Capture-budget and writer-lock losses are intentional and must be counted; burst losses are reported rather than assumed absent.', 'Client p99 < 1000 ms and generator p99 lateness <= 100 ms are stall-detection test bounds for this tiny local fixture, not production latency promises.', 'RSS is sampled for the proxy only and is null where Linux /proc sampling is unavailable.', 'This harness does not establish a throughput limit, high-cardinality query performance, or provider capacity.'] };
try {
  if (selected === 'all' || selected === 'locked-writer') execFileSync('cargo', ['build', '--release', '--locked', '--example', 'hold_db_lock'], { cwd: repositoryRoot, stdio: 'inherit' });
  for (const spec of [
    { name: 'capture-budget', delay: 100, rate: 500, seconds: 3, flags: ['--capture-slots', '4', '--queue-capacity', '4'], loss: true },
    { name: 'locked-writer', delay: 5, rate: 500, seconds: 3, flags: ['--capture-slots', '64', '--queue-capacity', '8'], loss: true, lock: true },
    { name: 'burst', delay: 5, rate: burstRate, seconds: burstSeconds, flags: [], loss: false },
  ]) if (selected === 'all' || selected === spec.name) {
    const result = await runCase(spec, directory);
    report.cases.push(result);
    console.log(JSON.stringify({ case: result.name, passed: result.passed, error: result.error, pressure_accounting: result.pressure_accounting }));
    if (!result.passed) break;
  }
  report.passed = report.cases.length > 0 && report.cases.every(result => result.passed);
} catch (error) { report.error = error.stack ?? String(error); }
finally {
  report.finished_at = new Date().toISOString();
  if (args['keep-db']) report.retained_database_directory = directory;
  else await fs.rm(directory, { recursive: true, force: true });
  await fs.mkdir(path.dirname(output), { recursive: true });
  await fs.writeFile(output, JSON.stringify(report, null, 2) + '\n');
  console.log(`Saved ${output}; passed=${report.passed}`);
  if (!report.passed) process.exitCode = 1;
}
