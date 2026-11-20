// SPDX-License-Identifier: AGPL-3.0-or-later

import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { test } from 'node:test';
import { ZhiyeApiError, ZhiyeClient } from '../dist/index.js';

const workspace = '10000000-0000-4000-8000-000000000000';
const page = '20000000-0000-4000-8000-000000000000';
const token = 'zhiye_' + '1'.repeat(64);
const prefix = `/api/v1/workspaces/${workspace}`;

async function listen(t, handler) {
  const server = createServer(handler).listen(0, '127.0.0.1');
  await new Promise((resolve) => server.once('listening', resolve));
  t.after(() => { server.closeAllConnections(); server.close(); });
  return `http://127.0.0.1:${server.address().port}`;
}

test('sends stable write preconditions and surfaces an API error without implicit retry', async (t) => {
  const captured = [];
  const origin = await listen(t, async (request, response) => {
    const body = [];
    for await (const chunk of request) body.push(chunk);
    captured.push([request.method, request.url, request.headers, Buffer.concat(body)]);
    response.setHeader('Content-Type', 'application/json');
    if (request.method === 'PATCH') {
      response.writeHead(429, { 'Retry-After': '4' });
      response.end(JSON.stringify({ code: 'RATE_LIMIT', message: '请稍后重试。', retryable: true, request_id: 'test-id' }));
    } else {
      response.setHeader('ETag', '"page-etag"');
      response.end(JSON.stringify({ id: page, page_revision: 7 }));
    }
  });
  const client = new ZhiyeClient({ baseUrl: origin + prefix, token });
  assert.equal(client.pageLink(page), `zhiye://open/workspaces/${workspace}/pages/${page}`);
  assert.equal(client.pageLink(page, workspace), `zhiye://open/workspaces/${workspace}/pages/${page}/blocks/${workspace}`);
  const { data, etag } = await client.getPage(page);
  await assert.rejects(client.patchPage(page, { title: '改名' }, {
    etag, baseRevision: data.page_revision, idempotencyKey: 'key',
  }), (error) => {
    assert.ok(error instanceof ZhiyeApiError);
    assert.deepEqual(error.problem, {
      code: 'RATE_LIMIT', message: '请稍后重试。', status: 429, request_id: 'test-id',
      retryable: true, details: null, retry_after_seconds: 4,
    });
    return true;
  });
  assert.equal(captured.length, 2);
  const [method, path, headers, body] = captured[1];
  assert.deepEqual([method, path], ['PATCH', `${prefix}/pages/${page}`]);
  assert.equal(headers.authorization, `Bearer ${token}`);
  assert.equal(headers['if-match'], '"page-etag"');
  assert.equal(headers['x-zhiye-base-revision'], '7');
  assert.equal(headers['idempotency-key'], 'key');
  assert.deepEqual(JSON.parse(body), { title: '改名' });
  assert.deepEqual(JSON.stringify(client), '{}');
});

test('keeps credentials on the configured loopback origin and encodes the opaque cursor', async (t) => {
  let redirected = false;
  const target = await listen(t, (_, response) => { redirected = true; response.end(); });
  const cursor = 'opaque&tag=?#';
  let requested;
  const origin = await listen(t, (request, response) => {
    requested = new URL(request.url, 'http://127.0.0.1');
    response.writeHead(307, { 'Location': target });
    response.end();
  });
  const client = new ZhiyeClient({ baseUrl: origin + prefix, token });
  await assert.rejects(client.changes(cursor, 3), (error) => error.problem?.code === 'UNEXPECTED_REDIRECT');
  assert.equal(redirected, false);
  assert.equal(requested.pathname, `${prefix}/changes`);
  assert.equal(requested.searchParams.get('cursor'), cursor);
  assert.equal(requested.searchParams.get('limit'), '3');
  for (const invalid of [
    `https://127.0.0.1:50000${prefix}`, `http://localhost:50000${prefix}`,
    `http://127.0.0.1:50000${prefix}?token=ignored`, `http://127.0.0.1:0${prefix}`,
  ]) {
    assert.throws(() => new ZhiyeClient({ baseUrl: invalid, token }), (error) => error.problem?.code === 'INVALID_API_BASE');
  }
  assert.throws(() => client.getPage('..'), (error) => error.problem?.code === 'INVALID_RESOURCE_ID');
});
