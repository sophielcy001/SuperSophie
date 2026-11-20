// SPDX-License-Identifier: AGPL-3.0-or-later

import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { once } from 'node:events';
import { readFile, writeFile } from 'node:fs/promises';
import { createServer, request as httpRequest } from 'node:http';
import { resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { setTimeout as wait } from 'node:timers/promises';
import { fileURLToPath, pathToFileURL } from 'node:url';
import Ajv2020 from 'ajv/dist/2020.js';
import { parse } from 'yaml';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { ZhiyeClient } from '@zhiye/sdk-ts';

const [executable, output, previous] = process.argv.slice(2);
assert.ok(executable && output, 'Usage: live-contract.mjs <api_contract executable> <report.json> [previous SDK module]');
const specification = parse(await readFile(new URL('../../../docs/openapi.v1.yaml', import.meta.url), 'utf8'));
const operations = new Map(Object.entries(specification.paths).flatMap(([path, entry]) =>
  Object.entries(entry).filter(([, operation]) => operation.operationId)
    .map(([method, operation]) => [operation.operationId, { path, method: method.toUpperCase(), operation }])));
const report = { version: specification.info.version, environment: `${process.platform} ${process.arch} ${process.version}`,
  checks: [], coverage: {}, interruptedReplies: [], schemaIssues: [] };
const reportChecks = (text) => report.checks.push(text);
const validate = new Ajv2020({ strict: false, allErrors: true });
validate.addFormat('uuid', /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i);
validate.addFormat('uri', (value) => { try { return Boolean(new URL(value).protocol); } catch { return false; } });
const localSchemaRefs = (node) => JSON.parse(JSON.stringify(node).replaceAll('#/components/schemas/', '#/$defs/'));
const definitions = localSchemaRefs(specification.components.schemas);
const validators = new Map();
const resolveRef = (node) => node.$ref
  ? node.$ref.slice(2).split('/').reduce((value, part) => value[part], specification) : node;

function checkSchema(key, schema, value) {
  if (!validators.has(key)) validators.set(key, validate.compile({ ...localSchemaRefs(schema), $defs: definitions }));
  const check = validators.get(key);
  if (!check(value)) {
    const errors = (check.errors ?? []).slice(0, 6).map(({ instancePath, keyword, message }) =>
      ({ path: instancePath, keyword, message }));
    if (!report.schemaIssues.some((issue) => issue.key === key)) report.schemaIssues.push({ key, errors });
  }
}

function withFutureField(entry) {
  return { ...entry, future_optional_field: 1 };
}

const child = spawn(resolve(executable), [], { stdio: ['pipe', 'pipe', 'inherit'] });
const exit = once(child, 'exit');
const lines = createInterface({ input: child.stdout });
const replies = lines[Symbol.asyncIterator]();
let controlId = 0;
async function readReply() {
  const reply = await replies.next();
  assert.equal(reply.done, false, 'The fixture remains available');
  return JSON.parse(reply.value);
}

async function owner(op, fields = {}) {
  const id = ++controlId;
  child.stdin.write(JSON.stringify({ id, op, ...fields }) + '\n');
  const reply = await readReply();
  assert.equal(reply.id, id);
  assert.equal(reply.error, undefined, reply.error?.code);
  return reply.result;
}

function loopback(value) {
  const url = new URL(value);
  assert.equal(url.protocol, 'http:');
  assert.equal(url.hostname, '127.0.0.1');
  return url.origin;
}

function requestWithHost(url, options) {
  // A browser-compatible fetch may choose its own Host value.
  return new Promise((resolve, reject) => {
    const request = httpRequest(url, {
      method: options.method, headers: Object.fromEntries(options.headers), signal: options.signal,
    }, (incoming) => {
      const chunks = [];
      incoming.on('data', (chunk) => chunks.push(chunk));
      incoming.on('error', reject);
      incoming.on('end', () => resolve(new Response(Buffer.concat(chunks),
        { status: incoming.statusCode, headers: incoming.headers })));
    });
    request.on('error', reject);
    request.end(options.body);
  });
}

async function discardReply(url, options) {
  let upstream;
  let forwardingError;
  const proxy = createServer(async (incoming, downstream) => {
    try {
      const sent = await fetch(url, { ...options, body: incoming, duplex: 'half' });
      upstream = new Response(await sent.arrayBuffer(), { status: sent.status, headers: sent.headers });
    } catch (error) {
      forwardingError = error;
    } finally {
      downstream.destroy();
    }
  });
  proxy.listen(0, '127.0.0.1');
  await once(proxy, 'listening');
  try {
    const local = new URL(url);
    local.port = String(proxy.address().port);
    const error = await fetch(local, options).then(() => null, (error) => error);
    if (forwardingError) throw forwardingError;
    assert.ok(upstream, 'The real API completed before dropping the reply');
    assert.equal(error?.cause?.code, 'UND_ERR_SOCKET', 'Caller observed a closed connection');
    return upstream;
  } finally {
    await new Promise((resolve) => {
      proxy.close(resolve);
      proxy.closeAllConnections();
    });
  }
}

try {
  const boot = await readReply();
  let origin = loopback(boot.origin);
  const f = boot.fixture;
  const { writer, reader, proposer, outside, single, rate, relation } = f.tokens;
  const base = (workspace = f.workspace) => `${origin}/api/v1/workspaces/${workspace}`;
  const note = { page_id: f.note };
  const db = { collection_id: f.collection };
  const append = (text) => ({ op: 'append', block: { type: 'paragraph', payload: { text } } });
  const request = async (id, {
    params = {}, query, json, raw, contentType, headers, role = writer, workspace = f.workspace,
    status = 200, code, dropReply = false,
  } = {}) => {
    const endpoint = operations.get(id);
    assert.ok(endpoint, id);
    const path = endpoint.path.replace(/\{(\w+)\}/g, (_, identifier) => {
      const value = identifier === 'workspace_id' ? workspace : params[identifier];
      assert.ok(value, `${id} parameter ${identifier}`);
      return encodeURIComponent(value);
    });
    const url = new URL(path, origin);
    for (const [key, value] of Object.entries(query ?? {})) url.searchParams.set(key, String(value));
    const outgoing = new Headers(headers);
    if (role) outgoing.set('Authorization', `Bearer ${role.token}`);
    if (json !== undefined) outgoing.set('Content-Type', 'application/json');
    if (contentType) outgoing.set('Content-Type', contentType);
    const send = dropReply ? discardReply : outgoing.has('host') ? requestWithHost : fetch;
    const response = await send(url, {
      method: endpoint.method, headers: outgoing, body: json === undefined ? raw : JSON.stringify(json),
      signal: AbortSignal.timeout(10_000), redirect: 'error',
    });
    const media = response.headers.get('content-type')?.split(';')[0];
    const result = media === 'application/json' ? await response.json() : Buffer.from(await response.arrayBuffer());
    assert.equal(response.status, status, `${id} ${result?.code ?? ''}`);
    if (code) assert.equal(result.code, code, id);
    assert.equal(response.headers.get('cache-control'), 'no-store');
    assert.equal(response.headers.get('x-content-type-options'), 'nosniff');
    report.coverage[id] ??= {};
    report.coverage[id][status] = (report.coverage[id][status] ?? 0) + 1;
    const declared = endpoint.operation.responses[status] ?? endpoint.operation.responses.default;
    assert.ok(declared, `${id} ${status} requires a response definition`);
    const responseSpec = resolveRef(declared);
    assert.ok(Object.hasOwn(responseSpec.content, media), `${id} ${status} media type ${media}`);
    if (status < 400 && json !== undefined) {
      checkSchema(`${id} input`, resolveRef(endpoint.operation.requestBody).content['application/json'].schema, json);
    }
    const responseSchema = responseSpec.content[media]?.schema;
    if (responseSchema && media === 'application/json') {
      const key = `${id} ${status}`;
      checkSchema(key, responseSchema, result);
      const future = withFutureField(result);
      if (Array.isArray(result.items)) future.items = result.items.map(withFutureField);
      if (id === 'getPageComputed' && status === 200) {
        future.properties = Object.fromEntries(Object.entries(result.properties)
          .map(([field, value]) => [field, withFutureField(value)]));
      }
      checkSchema(`${key} additive field`, responseSchema, future);
    }
    for (const [name, header] of Object.entries(responseSpec.headers ?? {})) {
      const definition = resolveRef(header);
      const value = response.headers.get(name);
      if (value === null) {
        assert.ok(!definition.required, `${id} ${status} missing header ${name}`);
        continue;
      }
      const schema = definition.schema;
      checkSchema(`${id} ${status} header ${name}`, schema,
        schema.type === 'integer' ? Number(value) : value);
    }
    return { body: result, headers: response.headers, etag: response.headers.get('etag') };
  };
  const retryAfterLostReply = async (id, options) => {
    const before = await request(id, { ...options, dropReply: true });
    const after = await request(id, options);
    assert.deepEqual(after.body, before.body, id);
    report.interruptedReplies.push(id);
    return after.body;
  };
  const patch = (body, headers, expect = {}) => request('patchPage', { params: note, json: body, headers, ...expect });
  const revisionHeaders = (current, key) => ({
    'If-Match': current.etag, 'Idempotency-Key': key, 'X-Zhiye-Base-Revision': String(current.body.page_revision),
  });
  const assets = (id, options = {}) => request('downloadAsset', { params: { asset_id: id }, ...options });

  assert.equal(f.defaultAttachmentMiB, 100);
  const unauthorized = await request('getIntegration', { role: null, status: 401, code: 'UNAUTHORIZED' });
  assert.equal(unauthorized.headers.get('www-authenticate'), 'Bearer');
  await request('getIntegration', { workspace: f.other, status: 401, code: 'UNAUTHORIZED' });
  await request('getIntegration', { headers: { Origin: 'https://example.invalid' }, status: 403, code: 'REQUEST_ORIGIN' });
  await request('getIntegration', { headers: { Host: 'example.invalid' }, status: 403, code: 'REQUEST_ORIGIN' });
  assert.deepEqual((await request('getIntegration', { role: reader })).body.scopes, ['content:read']);
  await request('getPage', { params: { page_id: f.hidden }, status: 404, code: 'RESOURCE_NOT_FOUND' });
  await assets(f.hiddenAsset, { status: 404, code: 'RESOURCE_NOT_FOUND' });
  await assets(f.asset.id, { role: reader, status: 403, code: 'FORBIDDEN' });
  const downloaded = await assets(f.asset.id);
  assert.equal(Number(downloaded.headers.get('content-length')), f.asset.bytes);
  assert.equal(downloaded.headers.get('content-disposition'), 'attachment');
  assert.equal(createHash('sha256').update(downloaded.body).digest('hex'), f.asset.sha256);
  reportChecks('回环传输校验 Host、Origin、工作空间和资源范围，下载校验长度及 SHA-256');

  const first = (await request('listCatalog', { query: { limit: 1 } })).body;
  assert.equal(first.items.length, 1);
  assert.ok(first.change_cursor);
  assert.ok(first.catalog_cursor);
  const captured = await request('getPage', { params: note, role: reader });
  assert.equal(captured.body.id, f.note);
  assert.match(captured.etag, /^".+"$/);
  const pageBody = await request('getPageContent', { params: note, role: reader });
  assert.equal(pageBody.body.blocks.length, 50);
  assert.equal(pageBody.etag, captured.etag);
  assert.ok(!JSON.stringify(pageBody.body).includes(f.hidden));
  assert.ok(!JSON.stringify(pageBody.body).includes('夹具保密链接文字'));
  const nextBody = await request('getPageContent', { params: note, role: reader, query: { cursor: pageBody.body.next_cursor } });
  assert.equal(nextBody.body.blocks.length, 5);
  assert.equal(nextBody.body.next_cursor, null);
  const markdown = await request('getPageContent', { params: note, role: reader, query: { format: 'markdown' } });
  assert.equal(markdown.body.format, 'markdown');
  assert.ok(markdown.body.markdown.includes('接口固定关键词'));
  await request('getPageContent', {
    params: note, role: reader, query: { format: 'markdown', cursor: pageBody.body.next_cursor },
    status: 409, code: 'CURSOR_INVALID',
  });
  const located = (await request('getBlock', { params: { block_id: f.visibleBlock }, role: reader })).body;
  assert.equal(located.page_id, f.note);
  assert.ok(located.block_link.endsWith(f.visibleBlock));
  const reference = await request('getBlockReference', { params: { ...note, reference_id: f.reference }, role: reader });
  assert.equal(reference.body.block.id, f.anchor);
  const backlinks = await request('getPageBacklinks', { params: { page_id: f.source }, role: reader });
  assert.deepEqual(backlinks.body.items.map((item) => item.page_id), [f.note]);
  const found = await request('search', { json: { query: '接口固定关键词' }, role: reader });
  assert.deepEqual(found.body.items.map((item) => item.page_id), [f.note]);
  assert.equal((await request('search', { json: { query: '夹具保密链接文字' }, role: reader })).body.items.length, 0);
  await request('search', { json: { query: '固定', limit: 101 }, role: reader, status: 400, code: 'QUERY_LIMIT' });
  const schema = (await request('getCollection', { params: db, role: reader })).body;
  assert.ok(schema.properties.some((field) => field.id === f.formula));
  const records = (await request('queryCollection', { params: db, role: reader, json: {
    filters: [{ field_id: f.score, operator: 'notEmpty' }], sort: { field_id: f.score, direction: 'desc' }, limit: 1,
  } })).body;
  assert.equal(records.items[0].page_id, f.high);
  assert.ok(records.next_cursor);
  const computed = await request('getPageComputed', { params: { page_id: f.high }, role: reader });
  assert.equal(computed.body.stale, false);
  assert.deepEqual(computed.body.properties[f.formula], { status: 'value', value: 16 });
  reportChecks('按授权读取编目、正文两种表示和游标、块与引用、全文、数据库以及真实计算属性');

  const pdfSearch = { query: 'PDF原件契约词' };
  const pdfHit = (await request('search', { json: pdfSearch })).body;
  assert.equal(pdfHit.items[0].source, 'pdf');
  assert.equal(pdfHit.items[0].pdf.attachment_id, f.pdf.id);
  assert.equal(pdfHit.items[0].pdf.page_number, 1);
  assert.equal(pdfHit.pdf_index.indexed, 1);
  const withoutPdf = (await request('search', { json: pdfSearch, role: reader })).body;
  assert.deepEqual(withoutPdf.items, []);
  assert.equal(withoutPdf.pdf_index, undefined);
  const rowParams = { page_id: f.high };
  const visibleTargets = await request('getPage', { params: rowParams, role: relation });
  assert.deepEqual(visibleTargets.body.properties?.[f.relation], [f.visibleRelated]);
  assert.ok(visibleTargets.body.restricted_property_ids.includes(f.relation));
  assert.ok(!JSON.stringify(visibleTargets.body).includes(f.hiddenRelated));
  await request('patchPage', { params: rowParams, role: relation,
    json: { properties: { [f.relation]: [] } }, headers: revisionHeaders(visibleTargets, 'relation-deny'),
    status: 403, code: 'PARTIAL_CONTENT' });
  await request('patchPage', { params: rowParams, role: relation,
    json: { properties: { [f.relation]: { remove: [f.visibleRelated] } } }, headers: revisionHeaders(visibleTargets, 'relation-remove') });
  assert.deepEqual((await request('getPage', { params: rowParams })).body.properties[f.relation], [f.hiddenRelated]);
  reportChecks('PDF 搜索按原件授权返回命中和页码，部分可读关联拒绝整体覆盖并支持局部增删');

  const padded = JSON.stringify({ query: '接口固定关键词' });
  const atLimit = padded + ' '.repeat(1024 * 1024 - Buffer.byteLength(padded));
  await request('search', { raw: atLimit, contentType: 'application/json', role: reader });
  await request('search', { raw: atLimit + ' ', contentType: 'application/json', role: reader, status: 413, code: 'REQUEST_TOO_LARGE' });
  await request('search', { raw: padded, contentType: 'text/plain', role: reader, status: 415, code: 'CONTENT_TYPE' });
  const writeTarget = await request('getPage', { params: note });
  const renameCondition = revisionHeaders(writeTarget, 'v1-edit');
  await patch({ title: '禁止的只读修改' }, renameCondition, { role: reader, status: 403, code: 'FORBIDDEN' });
  await patch({ title: '缺少条件' }, { 'Idempotency-Key': 'v1-missing' }, { status: 428, code: 'PRECONDITION_REQUIRED' });
  const renamed = await patch({ title: '冻结合成笔记已更新' }, renameCondition);
  assert.deepEqual((await patch({ title: '冻结合成笔记已更新' }, renameCondition)).body, renamed.body);
  await patch({ title: '同键不同内容' }, renameCondition, { status: 409, code: 'IDEMPOTENCY_CONFLICT' });
  await patch({ title: '陈旧基准修改' }, { ...renameCondition, 'Idempotency-Key': 'v1-stale' }, { status: 412, code: 'REVISION_CONFLICT' });
  assert.equal((await request('getPage', { params: note })).body.page_revision, renamed.body.revision);
  const sibling = { page_id: f.sibling };
  const oldSibling = await request('getPage', { params: sibling });
  const edits = Array.from({ length: 100 }, (_, number) => append(`批次段落 ${number}`));
  await request('applyBlockOperations', { params: sibling, json: { operations: [...edits, append('超限')] },
    headers: revisionHeaders(oldSibling, 'v1-too-many-blocks'), status: 413, code: 'TOO_MANY_OPERATIONS' });
  const applied = await retryAfterLostReply('applyBlockOperations', { params: sibling, json: { operations: edits },
    headers: revisionHeaders(oldSibling, 'v1-100-blocks') });
  assert.equal(applied.created_block_ids.length, 100);
  const created = await retryAfterLostReply('createPage', { json: { parent_id: f.root, title: '编目期间新增页' },
    headers: { 'Idempotency-Key': 'v1-catalog-add' }, status: 201 });
  const createdRecord = (await request('createPage', { json: { collection_id: f.collection, title: '新增记录', properties: { [f.score]: 7 } },
    headers: { 'Idempotency-Key': 'v1-record-add' }, status: 201 })).body;
  assert.notEqual(createdRecord.page_id, created.page_id);
  reportChecks('验证 1 MiB JSON、100 块边界、前置条件和幂等重放，页面和记录都可创建');

  await owner('limitOneMiB');
  const uploadGate = await request('getPage', { params: note });
  const bytes = Buffer.from('v1 fixture bytes');
  const uploadHeaders = {
    'If-Match': uploadGate.etag, 'Idempotency-Key': 'v1-upload',
    'X-Zhiye-Page-Id': f.note, 'X-Zhiye-File-Name': encodeURIComponent('复验原件.txt'),
  };
  await request('uploadAsset', { raw: Buffer.alloc(1024 * 1024 + 1), contentType: 'application/octet-stream',
    headers: { ...uploadHeaders, 'Idempotency-Key': 'v1-too-large' }, status: 413, code: 'FILE_TOO_LARGE' });
  const receipt = await retryAfterLostReply('uploadAsset', {
    raw: bytes, contentType: 'application/octet-stream', headers: uploadHeaders, status: 201,
  });
  assert.deepEqual((await assets(receipt.asset_id)).body, bytes);
  const archiveTarget = { page_id: f.archive };
  const archiveOnly = await request('getPage', { params: archiveTarget, role: single });
  await request('archivePage', { params: archiveTarget, role: single, headers: revisionHeaders(archiveOnly, 'v1-hidden-archive'),
    status: 403, code: 'PARTIAL_CONTENT' });
  const archiveAll = await request('getPage', { params: archiveTarget });
  await request('archivePage', { params: archiveTarget, headers: revisionHeaders(archiveAll, 'v1-archive') });
  await request('getPage', { params: { page_id: f.child }, status: 404, code: 'RESOURCE_NOT_FOUND' });
  reportChecks('附件限额、原件重取和上传重放有效，归档不能借单页授权影响隐藏子页');

  const head = await request('getPage', { params: note, role: proposer });
  const suggestion = {
    role: proposer, json: { page_id: f.note, base_revision: head.body.page_revision, changes: { title: '候选标题' } },
    headers: { 'Idempotency-Key': 'v1-proposal' }, status: 201,
  };
  const proposed = await retryAfterLostReply('submitProposal', suggestion);
  const proposal = { proposal_id: proposed.id };
  assert.equal((await request('getProposal', { params: proposal, role: proposer })).body.status, 'pending');
  await request('getProposal', { params: proposal, role: outside, status: 404, code: 'RESOURCE_NOT_FOUND' });
  assert.equal((await request('getPage', { params: note })).body.page_revision, head.body.page_revision);
  await owner('edit');
  assert.equal((await owner('accept', { proposal: proposed.id })).status, 'conflict');
  assert.equal((await request('getProposal', { params: proposal, role: proposer })).body.status, 'conflict');
  assert.equal((await request('getPage', { params: note })).body.title, '桌面同步的新修订');
  reportChecks('建议由提交者读取并可重放，底稿变更使审阅进入冲突而未覆盖正文');
  reportChecks('回环代理在服务提交后关闭调用方连接，四种写入用原幂等键重试得到相同结果');

  const tail = await request('listCatalog', { query: { cursor: first.catalog_cursor } });
  assert.equal(tail.body.change_cursor, null);
  const delta = await request('pollChanges', { query: { cursor: first.change_cursor, limit: 1 } });
  assert.ok(delta.body.has_more);
  assert.equal((await request('pollChanges', { query: { cursor: first.change_cursor, limit: 1 } })).body.items[0].event_id,
    delta.body.items[0].event_id);
  const previousOrigin = origin;
  await owner('stop');
  await assert.rejects(fetch(previousOrigin, { signal: AbortSignal.timeout(2_000) }), TypeError);
  origin = loopback(await owner('start'));
  let cursor = delta.body.next_cursor;
  const replayed = [...delta.body.items];
  for (let turn = 0; turn < 20; turn++) {
    const changes = (await request('pollChanges', { query: { cursor, limit: 10 } })).body;
    replayed.push(...changes.items);
    cursor = changes.next_cursor;
    if (!changes.has_more) break;
    assert.ok(turn < 19);
  }
  assert.ok(replayed.some((event) => event.id === created.page_id && event.action === 'upsert'));
  assert.ok(replayed.some((event) => event.id === f.archive && event.action === 'delete'));
  assert.ok(!replayed.some((event) => event.id === f.hidden));
  await owner('moveOutside');
  await request('getPage', { params: { page_id: f.movable }, status: 404, code: 'RESOURCE_NOT_FOUND' });
  await request('pollChanges', { query: { cursor }, status: 409, code: 'CURSOR_INVALID' });
  reportChecks('从编目起点回放新增和归档，服务关闭停服并可跨重启续读，授权收缩后旧游标失效');

  const sdk = new ZhiyeClient({ baseUrl: base(), token: writer.token });
  assert.deepEqual((await sdk.getPage(f.note)).data.id, f.note);
  assert.equal((await sdk.search({ query: '接口固定关键词' })).items[0].page_id, f.note);
  assert.deepEqual((await sdk.getPageComputed(f.high)).properties[f.formula], { status: 'value', value: 16 });
  if (previous) {
    const { ZhiyeClient: Previous } = await import(pathToFileURL(resolve(previous)).href);
    const oldSdk = new Previous({ baseUrl: base(), token: writer.token });
    assert.equal((await oldSdk.getPage(f.note)).data.id, f.note);
    assert.equal((await oldSdk.search({ query: '接口固定关键词' })).items[0].page_id, f.note);
    reportChecks('0.25.0 的 SDK 与当前服务完成页面读取和全文搜索');
  }
  let protocolVersion;
  class ObservedStdioTransport extends StdioClientTransport {
    async start() {
      const receive = this.onmessage;
      this.onmessage = (message, extra) => {
        protocolVersion ??= message.result?.protocolVersion;
        receive?.(message, extra);
      };
      await super.start();
    }
  }
  const transport = new ObservedStdioTransport({ command: process.execPath,
    args: [fileURLToPath(new URL('../dist/cli.js', import.meta.url))],
    env: { ZHIYE_BASE: base(), ZHIYE_TOKEN: writer.token }, stderr: 'pipe' });
  const client = new Client({ name: 'zhiye-contract-tests', version: '1.0.0' });
  try {
    await client.connect(transport);
    assert.equal(protocolVersion, '2025-11-25');
    const tools = (await client.listTools()).tools;
    assert.deepEqual(tools.map(({ name }) => name), ['search_knowledge', 'get_page', 'get_block', 'query_database',
      'create_page', 'append_blocks', 'update_page', 'propose_page_changes']);
    const call = async (name, args) => {
      const result = await client.callTool({ name, arguments: args });
      assert.equal(result.isError ?? false, false, result.structuredContent?.error?.code);
      return result.structuredContent;
    };
    assert.equal((await call('search_knowledge', { query: '接口固定关键词' })).items[0].page_id, f.note);
    const extra = await call('create_page', { page: { parent_id: f.root, title: 'stdio 验收页' }, idempotency_key: 'mcp-v1-create' });
    const metadata = await call('get_page', { page_id: extra.page_id, format: 'metadata' });
    const condition = { page_id: extra.page_id, base_revision: metadata.page.page_revision, if_match: metadata.etag };
    const write = await call('append_blocks', { ...condition, operations: [append('stdio 真实正文')], idempotency_key: 'mcp-v1-write' });
    assert.equal((await call('get_block', { block_id: write.created_block_ids[0] })).page_id, extra.page_id);
    assert.ok((await call('query_database', { collection_id: f.collection })).items.length >= 3);
    const current = await call('get_page', { page_id: extra.page_id, format: 'metadata' });
    await call('update_page', { page_id: extra.page_id, base_revision: current.page.page_revision, if_match: current.etag,
      idempotency_key: 'mcp-v1-update', patch: { title: 'stdio 更新页' } });
    const changed = await call('get_page', { page_id: extra.page_id, format: 'metadata' });
    const suggested = await call('propose_page_changes', { idempotency_key: 'mcp-v1-suggest',
      proposal: { page_id: extra.page_id, base_revision: changed.page.page_revision, changes: { title: 'stdio 候选' } } });
    assert.equal(suggested.status, 'pending');
    await owner('revoke', { name: 'writer' });
    const revokedTools = await client.callTool({ name: 'get_page', arguments: { page_id: extra.page_id } });
    assert.equal(revokedTools.structuredContent.error.code, 'UNAUTHORIZED');
    await request('getIntegration', { status: 401, code: 'UNAUTHORIZED' });
    report.mcp = { ...client.getServerVersion(), protocolVersion, clientSdk: '1.32.2', tools: tools.map(({ name }) => name) };
  } finally { await client.close(); }
  reportChecks('生产 SDK 和独立 stdio 客户端连真实服务执行八个工具，撤销后同连接立即拒绝');

  for (let count = 0; count < 60; count++) await request('getIntegration', { role: rate });
  const exhausted = await request('getIntegration', { role: rate, status: 429, code: 'RATE_LIMIT' });
  assert.equal(Number(exhausted.headers.get('retry-after')), 10);
  assert.equal(exhausted.body.retryable, true);
  assert.equal((await request('getIntegration', { role: reader })).body.scopes[0], 'content:read');
  reportChecks('第 61 次请求按集成独立限流并返回 Retry-After');

  const expiresAt = Date.now() + 4000;
  const expiring = await owner('issueExpiring', { expiresAt });
  assert.equal(expiring.expiresAt, expiresAt);
  await request('getPage', { role: expiring, params: note });
  await wait(Math.max(0, expiresAt - Date.now()) + 5);
  const expired = await request('getPage', { role: expiring, params: note, status: 401, code: 'UNAUTHORIZED' });
  assert.equal(expired.headers.get('www-authenticate'), 'Bearer');
  await request('getPage', { role: reader, params: note });
  reportChecks('真实回环请求在有限期令牌到期前可读取，到期后收到 401。其他授权仍可读取');

  assert.deepEqual(Object.keys(report.coverage).sort(), [...operations.keys()].sort());
  assert.equal(report.schemaIssues.length, 0, 'Request and reply shapes must conform to the open v1 schema');
  report.passed = true;
} catch (error) {
  report.passed = false;
  report.error = String(error instanceof Error ? error.stack : error?.code ?? error)
    .replace(/zhiye_[a-f0-9]{64}/g, '[synthetic token]');
  process.exitCode = 1;
} finally {
  child.stdin.end();
  lines.close();
  const [status] = await exit;
  if (status !== 0) { report.passed = false; report.fixtureExit = status; process.exitCode = 1; }
  await writeFile(output, JSON.stringify(report, null, 2) + '\n');
}
