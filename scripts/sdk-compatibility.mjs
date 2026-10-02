#!/usr/bin/env node
// Executed by sdk-compatibility.py after it starts the loopback-only fixture.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const packagePath = process.env.OBSERVER_SDK_PACKAGE;
assert(packagePath, 'Set OBSERVER_SDK_PACKAGE to the installed @typesafe-ai/sdk directory');
const { TypeSafeClient, APIError } = require(packagePath);
const version = JSON.parse(fs.readFileSync(path.join(packagePath, 'package.json'), 'utf8')).version;
assert.equal(version, '0.6.0', 'Install the pinned SDK fixture version');
const baseURL = process.env.OBSERVER_SDK_PROXY;
const origin = new URL(baseURL);
assert.equal(origin.hostname, '127.0.0.1');
assert.equal(origin.protocol, 'http:');
assert.equal(origin.pathname, '/');
const fixture = path.resolve(process.env.OBSERVER_SDK_FIXTURES);
const request = JSON.parse(fs.readFileSync(path.join(fixture, 'request.json'), 'utf8'));
const successBytes = fs.readFileSync(path.join(fixture, 'response.json'));
const errorBytes = fs.readFileSync(path.join(fixture, 'error.json'));
const errorBody = JSON.parse(errorBytes);
let attempts = 0;
const client = new TypeSafeClient({
  apiKey: process.env.OBSERVER_SDK_KEY || 'sdk-compat-placeholder', baseURL, defaultModel: request.model,
  defaultHeaders: { 'x-observer-source': 'sdk-javascript', 'x-observer-access': process.env.OBSERVER_SDK_ACCESS, 'Accept-Encoding': 'identity' },
  timeout: 5000, retry: { maxRetries: 0 }, logLevel: 'off',
  fetch: async (input, init) => {
    const destination = new URL(input);
    assert.equal(destination.origin, origin.origin, 'External SDK request blocked');
    assert.equal(destination.pathname, '/v1/systemone');
    assert.equal(destination.search, '');
    attempts++;
    const response = await fetch(input, init);
    const body = Buffer.from(await response.clone().arrayBuffer());
    assert.deepEqual(body, response.status === 200 ? successBytes : errorBytes, 'Proxy altered upstream bytes');
    assert.equal(response.headers.get('x-sdk-fixture'), 'preserved');
    return response;
  },
});

for (let index = 0; index < 3; index++) {
  const payload = structuredClone(request);
  payload.state.sequence = index;
  if (index === 2) payload.questions.department.instructions += ' Treat outages as technical.';
  const response = await client.systemOne(payload);
  assert.equal(response.answers.department.choice, 'billing');
  assert.equal(response.answers.urgency.noul, 0.9);
  assert.equal(response.answers.frustration.score, 1.6);
  assert.equal(response.usage.input_tokens, 100);
  assert.deepEqual(response.provider_extension, { wire: true, unknown: [2, null, 'future'] });
  assert.equal(response.answers.department.answer_extension.preserved, true);
}
try {
  await client.systemOne({ ...request, state: { fixture_error: true } });
  assert.fail('Expected the SDK to surface upstream 422');
} catch (error) {
  assert(error instanceof APIError);
  assert.equal(error.status, 422);
  assert.deepEqual(error.body, errorBody);
}
assert.equal(attempts, 4, 'Unexpected retries');
process.stdout.write(JSON.stringify({ sdk: '@typesafe-ai/sdk', version, node: process.version, attempts, successes: 3, errors: 1, raw_bytes_preserved: true, unknown_fields_preserved: true }) + '\n');
