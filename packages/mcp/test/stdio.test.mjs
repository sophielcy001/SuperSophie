// SPDX-License-Identifier: AGPL-3.0-or-later

import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';

const workspace = '10000000-0000-4000-8000-000000000000';
const page = '20000000-0000-4000-8000-000000000000';
const token = 'zhiye_' + '2'.repeat(64);
const cli = fileURLToPath(new URL('../dist/cli.js', import.meta.url));

test('stdio exposes live scopes, validates tool input, and refuses revoked credentials on the same connection', async (t) => {
  let scopes = ['content:read'];
  let revoked = false;
  let pageReads = 0;
  const service = createServer((request, response) => {
    assert.equal(request.headers.authorization, `Bearer ${token}`);
    response.setHeader('Content-Type', 'application/json');
    if (revoked) {
      response.writeHead(401);
      response.end(JSON.stringify({ code: 'UNAUTHORIZED', message: '令牌已撤销。' }));
    } else if (request.url.endsWith('/integration')) {
      response.end(JSON.stringify({ scopes }));
    } else {
      assert.ok(request.url.endsWith(`/pages/${page}`));
      pageReads++;
      response.setHeader('ETag', '"read-tag"');
      response.end(JSON.stringify({ id: page, page_revision: 9 }));
    }
  }).listen(0, '127.0.0.1');
  await new Promise((resolve) => service.once('listening', resolve));
  t.after(() => { service.closeAllConnections(); service.close(); });
  const base = `http://127.0.0.1:${service.address().port}/api/v1/workspaces/${workspace}`;
  const transport = new StdioClientTransport({
    command: process.execPath, args: [cli], env: { ZHIYE_BASE: base, ZHIYE_TOKEN: token }, stderr: 'pipe',
  });
  const stderr = [];
  transport.stderr.on('data', (chunk) => stderr.push(String(chunk)));
  const client = new Client({ name: 'zhiye-stdio-test', version: '1' });
  t.after(() => client.close());
  await client.connect(transport);
  assert.equal(client.getServerVersion().name, 'zhiye');
  const list = async () => (await client.listTools()).tools.map(({ name }) => name);
  assert.deepEqual(await list(), ['search_knowledge', 'get_page', 'get_block', 'query_database']);
  const call = (name, args) => client.callTool({ name, arguments: args });
  const hidden = await call('propose_page_changes', {});
  assert.equal(hidden.isError, true);
  assert.equal(hidden.structuredContent.error.code, 'FORBIDDEN');

  scopes = ['content:read', 'content:update', 'proposals:create'];
  assert.deepEqual(await list(), ['search_knowledge', 'get_page', 'get_block', 'query_database',
    'append_blocks', 'update_page', 'propose_page_changes']);
  const bad = await call('append_blocks', {
    page_id: page, base_revision: 9, if_match: '"read-tag"', idempotency_key: 'scope-test',
    operations: [{ op: 'delete', block_id: page }],
  });
  assert.equal(bad.structuredContent.error.code, 'INVALID_ARGUMENTS');
  const read = await call('get_page', { page_id: page, format: 'metadata' });
  assert.deepEqual(read.structuredContent, { page: { id: page, page_revision: 9 }, etag: '"read-tag"' });
  assert.deepEqual(JSON.parse(read.content[0].text), read.structuredContent);
  assert.equal(pageReads, 1);

  revoked = true;
  const revokedCall = await call('get_page', { page_id: page });
  assert.equal(revokedCall.structuredContent.error.code, 'UNAUTHORIZED');
  await assert.rejects(client.listTools(), /令牌已撤销/);
  assert.equal(pageReads, 1);
  assert.deepEqual(stderr, []);
});
