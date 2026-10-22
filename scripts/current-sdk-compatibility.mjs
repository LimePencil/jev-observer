#!/usr/bin/env node
// Called only by current-sdk-compatibility.py against its owned loopback mock.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { createHash } from 'node:crypto';
const require = createRequire(import.meta.url);
const packagePath = process.env.OBSERVER_SDK_PACKAGE;
const { TypeSafeClient, Image } = require(packagePath);
assert.equal(JSON.parse(fs.readFileSync(path.join(packagePath, 'package.json'), 'utf8')).version, '0.7.0');
const origin = new URL(process.env.OBSERVER_SDK_PROXY);
assert.equal(origin.hostname, '127.0.0.1');
assert.equal(origin.protocol, 'http:');
assert.equal(origin.pathname, '/');
const responseBytes = Buffer.from(process.env.OBSERVER_SDK_RESPONSE, 'base64');
const png = Buffer.from(process.env.OBSERVER_SDK_IMAGE, 'base64');
const calls = [];
for (const kind of ['small', 'large', 'multipart', 'disabled']) {
  const client = new TypeSafeClient({
    apiKey: 'current-sdk-placeholder', baseURL: origin.origin,
    timeout: 5000, retry: { maxRetries: 0 }, logLevel: 'off',
    ...(kind === 'disabled' ? { compression: false } : {}),
    defaultHeaders: { 'x-observer-source': `current-javascript-${kind}`, 'x-observer-access': process.env.OBSERVER_SDK_ACCESS, 'Accept-Encoding': 'identity', ...(kind === 'multipart' ? { 'TypeSafe-Danger-Zone-Images': 'true' } : {}) },
    fetch: async (input, init) => {
      const destination = new URL(input);
      assert.equal(destination.href, origin.origin + '/v1/systemone');
      const body = Buffer.from(init.body);
      assert(body.length <= 65536);
      const headers = new Headers(init.headers);
      const coding = headers.get('Content-Encoding') || 'identity';
      assert.equal(coding, ['large', 'multipart'].includes(kind) ? 'zstd' : 'identity');
      const media = headers.get('Content-Type');
      assert.equal(media.startsWith('multipart/form-data;'), kind === 'multipart');
      const images = headers.get('TypeSafe-Danger-Zone-Images');
      assert.equal(images, kind === 'multipart' ? 'true' : null);
      calls.push({ sha256: createHash('sha256').update(body).digest('hex'), coding, media, images });
      const response = await fetch(input, init);
      assert.equal(response.status, 200);
      assert.deepEqual(Buffer.from(await response.clone().arrayBuffer()), responseBytes);
      assert.equal(response.headers.get('x-current-sdk-fixture'), 'preserved');
      return response;
    },
  });
  const state = { password: 'current-state-placeholder', padding: kind === 'small' ? '' : 'x'.repeat(2048) };
  if (kind === 'multipart') state.photo = new Image(png);
  const result = await client.systemOne({ model: 'synthetic', state,
    questions: { q: { type: 'noul', instructions: 'Evaluate the synthetic example' } },
    echo: 'current-state-placeholder', ordinary: 'retained' });
  assert.equal(result.answers.q.noul, 0.75);
}
assert.equal(calls.length, 4);
process.stdout.write(JSON.stringify({ version: '0.7.0', calls }) + '\n');
