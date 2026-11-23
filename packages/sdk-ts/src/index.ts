// SPDX-License-Identifier: AGPL-3.0-or-later

import type { components, operations } from './openapi.generated.js';

export type { components, operations, paths } from './openapi.generated.js';

type JsonBody<Operation extends keyof operations> =
  NonNullable<operations[Operation]['requestBody']> extends { content: { 'application/json': infer Body } } ? Body : never;
type JsonResult<Operation extends keyof operations, Status extends number = 200> =
  operations[Operation]['responses'] extends Record<Status, { content: { 'application/json': infer Body } }> ? Body : never;

export type IntegrationScope = JsonResult<'getIntegration'>['scopes'][number];
export type Page = components['schemas']['Page'];
export type Block = components['schemas']['Block'];
export type BlockOperation = components['schemas']['BlockOperation'];
export type AppendOperation = Extract<BlockOperation, { op: 'append' }>;
export type PageContent = JsonResult<'getPageContent'>;
export type Backlinks = JsonResult<'getPageBacklinks'>;
export type ReferenceSource = JsonResult<'getBlockReference'>;
export type ComputedPage = JsonResult<'getPageComputed'>;
export type ComputedValue = components['schemas']['ComputedValue'];
export type CatalogItem = components['schemas']['CatalogItem'];
export type ChangeEvent = components['schemas']['ChangeEvent'];
export type ChangePage = JsonResult<'pollChanges'>;
export type CatalogPage = JsonResult<'listCatalog'>;
export type SearchRequest = JsonBody<'search'>;
export type SearchResult = JsonResult<'search'>;
export type Collection = JsonResult<'getCollection'>;
export type CollectionQuery = JsonBody<'queryCollection'>;
export type CollectionRecords = JsonResult<'queryCollection'>;
export type CreatePage = JsonBody<'createPage'>;
export type PagePatch = JsonBody<'patchPage'>;
export type RelationDelta = components['schemas']['RelationDelta'];
export type OperationReceipt = components['schemas']['Operation'];
export type ProposalDraft = JsonBody<'submitProposal'>;
export type ProposalReceipt = JsonResult<'getProposal'>;
export type UploadReceipt = JsonResult<'uploadAsset', 201>;
export type BlockLocation = JsonResult<'getBlock'>;

const uuidPart = '[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}';
const workspaceBase = new RegExp(`^http://127\\.0\\.0\\.1:([1-9][0-9]{0,4})(/api/v1/workspaces/${uuidPart})/?$`);
const resourceId = new RegExp(`^${uuidPart}$`);
const tokenPattern = /^zhiye_[0-9a-f]{64}$/;

export interface ClientOptions {
  baseUrl: string;
  token: string;
  timeoutMs?: number;
}

export interface ApiProblem {
  code: string;
  message: string;
  status: number | null;
  request_id: string | null;
  retryable: boolean;
  details: unknown;
  retry_after_seconds: number | null;
}

export class ZhiyeApiError extends Error {
  readonly problem: ApiProblem;

  constructor(problem: ApiProblem) {
    super(problem.message);
    this.name = 'ZhiyeApiError';
    this.problem = problem;
  }
}

function clientError(code: string, message: string, retryable = false): ZhiyeApiError {
  return new ZhiyeApiError({
    code, message, status: null, request_id: null, retryable, details: null, retry_after_seconds: null,
  });
}

function idSegment(id: string): string {
  if (id.length !== 36 || !resourceId.test(id)) {
    throw clientError('INVALID_RESOURCE_ID', '资源 ID 必须是知页返回的 UUID。');
  }
  return id;
}

export interface PageOptions {
  limit?: number;
  cursor?: string;
}

export interface ContentOptions extends PageOptions {
  format?: PageContent['format'];
}

export interface Versioned<T> {
  data: T;
  etag: string;
}

export interface WriteCondition {
  etag: string;
  idempotencyKey: string;
  baseRevision?: number;
}

function queryString(fields: Record<string, number | string | undefined>): string {
  const entries = new URLSearchParams();
  for (const [key, value] of Object.entries(fields)) {
    if (value !== undefined) entries.set(key, String(value));
  }
  return entries.size ? `?${entries}` : '';
}

function writeHeaders(options: WriteCondition): Headers {
  const headers = new Headers({ 'If-Match': options.etag, 'Idempotency-Key': options.idempotencyKey });
  if (options.baseRevision !== undefined) {
    headers.set('X-Zhiye-Base-Revision', String(options.baseRevision));
  }
  return headers;
}

export class ZhiyeClient {
  readonly #baseUrl: string;
  readonly #workspacePath: string;
  readonly #token: string;
  readonly #timeoutMs: number;

  constructor(options: ClientOptions) {
    const base = workspaceBase.exec(options.baseUrl);
    if (!base || options.baseUrl.trim() !== options.baseUrl || Number(base[1]) > 65535) {
      throw clientError('INVALID_API_BASE', '请使用「集成与 API」显示的 127.0.0.1 工作空间请求前缀。');
    }
    if (options.token.length !== 70 || !tokenPattern.test(options.token)) {
      throw clientError('INVALID_TOKEN', '请提供知页工作空间的集成令牌。');
    }
    const timeout = options.timeoutMs ?? 30_000;
    if (!Number.isSafeInteger(timeout) || timeout < 1 || timeout > 4_294_967_295) {
      throw clientError('INVALID_TIMEOUT', '请求超时应为正整数毫秒值。');
    }
    this.#baseUrl = options.baseUrl.replace(/\/$/, '');
    this.#workspacePath = base[2];
    this.#token = options.token;
    this.#timeoutMs = timeout;
  }

  pagePath(id: string): string {
    return `${this.#workspacePath}/pages/${idSegment(id)}`;
  }

  pageLink(id: string, blockId?: string): string {
    const suffix = blockId === undefined ? '' : `/blocks/${idSegment(blockId)}`;
    return `zhiye://open${this.pagePath(id).slice('/api/v1'.length)}${suffix}`;
  }

  getIntegration(): Promise<JsonResult<'getIntegration'>> {
    return this.#json('/integration');
  }

  catalog(options: PageOptions = {}): Promise<CatalogPage> {
    return this.#json(`/catalog${queryString({ limit: options.limit, cursor: options.cursor })}`);
  }

  changes(cursor: string, limit?: number): Promise<ChangePage> {
    return this.#json(`/changes${queryString({ cursor, limit })}`);
  }

  search(request: SearchRequest): Promise<SearchResult> {
    return this.#sendJson('/search', 'POST', request);
  }

  getCollection(id: string): Promise<Collection> {
    return this.#json(`/collections/${idSegment(id)}`);
  }

  queryCollection(id: string, query: CollectionQuery = {}): Promise<CollectionRecords> {
    return this.#sendJson(`/collections/${idSegment(id)}/query`, 'POST', query);
  }

  getPage(id: string): Promise<Versioned<Page>> {
    return this.#versioned(`/pages/${idSegment(id)}`);
  }

  getPageContent(id: string, options: ContentOptions = {}): Promise<Versioned<PageContent>> {
    const query = queryString({ limit: options.limit, cursor: options.cursor, format: options.format });
    return this.#versioned(`/pages/${idSegment(id)}/content${query}`);
  }

  getPageComputed(id: string): Promise<ComputedPage> {
    return this.#json(`/pages/${idSegment(id)}/computed`);
  }

  getPageBacklinks(id: string, options: PageOptions = {}): Promise<Backlinks> {
    const query = queryString({ limit: options.limit, cursor: options.cursor });
    return this.#json(`/pages/${idSegment(id)}/backlinks${query}`);
  }

  getBlockReference(pageId: string, referenceId: string): Promise<ReferenceSource> {
    return this.#json(`/pages/${idSegment(pageId)}/references/${idSegment(referenceId)}`);
  }

  getBlock(id: string): Promise<BlockLocation> {
    return this.#json(`/blocks/${idSegment(id)}`);
  }

  createPage(draft: CreatePage, idempotencyKey: string): Promise<OperationReceipt> {
    return this.#sendJson('/pages', 'POST', draft, { 'Idempotency-Key': idempotencyKey });
  }

  patchPage(id: string, patch: PagePatch, condition: WriteCondition): Promise<OperationReceipt> {
    return this.#sendJson(`/pages/${idSegment(id)}`, 'PATCH', patch, writeHeaders(condition));
  }

  applyBlockOperations(id: string, operations: BlockOperation[], condition: WriteCondition): Promise<OperationReceipt> {
    return this.#sendJson(`/pages/${idSegment(id)}/block-operations`, 'POST', { operations }, writeHeaders(condition));
  }

  archivePage(id: string, condition: WriteCondition): Promise<OperationReceipt> {
    return this.#json(`/pages/${idSegment(id)}/archive`, { method: 'POST', headers: writeHeaders(condition) });
  }

  uploadAsset(
    pageId: string, fileName: string, asset: Blob | Uint8Array<ArrayBuffer>, condition: Omit<WriteCondition, 'baseRevision'>,
  ): Promise<UploadReceipt> {
    const headers = writeHeaders(condition);
    headers.set('Content-Type', 'application/octet-stream');
    headers.set('X-Zhiye-Page-Id', idSegment(pageId));
    headers.set('X-Zhiye-File-Name', encodeURIComponent(fileName));
    return this.#json('/assets', { method: 'POST', headers, body: asset });
  }

  // The response body can be streamed directly to a file by a Node caller.
  downloadAsset(id: string): Promise<Response> {
    return this.#request(`/assets/${idSegment(id)}/content`);
  }

  submitProposal(draft: ProposalDraft, idempotencyKey: string): Promise<ProposalReceipt> {
    return this.#sendJson('/proposals', 'POST', draft, { 'Idempotency-Key': idempotencyKey });
  }

  getProposal(id: string): Promise<ProposalReceipt> {
    return this.#json(`/proposals/${idSegment(id)}`);
  }

  async #versioned<T>(path: string): Promise<Versioned<T>> {
    const response = await this.#request(path);
    const etag = response.headers.get('etag');
    if (!etag) {
      await response.body?.cancel();
      throw clientError('INVALID_RESPONSE', '本机 API 响应缺少页面 ETag。');
    }
    return { data: await this.#readJson<T>(response), etag };
  }

  #sendJson<T>(path: string, method: string, data: unknown, values?: HeadersInit): Promise<T> {
    const headers = new Headers(values);
    headers.set('Content-Type', 'application/json');
    return this.#json(path, { method, headers, body: JSON.stringify(data) });
  }

  async #json<T>(path: string, init?: RequestInit): Promise<T> {
    return this.#readJson<T>(await this.#request(path, init));
  }

  async #readJson<T>(response: Response): Promise<T> {
    try {
      return await response.json() as T;
    } catch {
      throw clientError('INVALID_RESPONSE', '本机 API 的 JSON 响应不完整或无效。', true);
    }
  }

  async #request(path: string, init: RequestInit = {}): Promise<Response> {
    const headers = new Headers(init.headers);
    headers.set('Authorization', `Bearer ${this.#token}`);
    const signal = AbortSignal.timeout(this.#timeoutMs);
    let response: Response;
    try {
      response = await fetch(this.#baseUrl + path, { ...init, headers, signal, redirect: 'manual', cache: 'no-store' });
    } catch {
      throw signal.aborted
        ? clientError('REQUEST_TIMEOUT', '本机 API 响应超时。', true)
        : clientError('API_UNREACHABLE', '无法连接本机 API。请确认知页当前显示的地址与运行状态。', true);
    }
    if (response.ok) return response;
    if (response.status >= 300 && response.status < 400) {
      await response.body?.cancel();
      throw clientError('UNEXPECTED_REDIRECT', '本机 API 返回了意外的重定向。');
    }
    const value = await response.json().catch(() => null) as Partial<ApiProblem> | null;
    const retryAfter = response.headers.get('retry-after');
    throw new ZhiyeApiError({
      status: response.status,
      code: typeof value?.code === 'string' ? value.code : 'HTTP_ERROR',
      message: typeof value?.message === 'string' ? value.message : '本机 API 请求失败。',
      request_id: typeof value?.request_id === 'string' ? value.request_id : null,
      retryable: value?.retryable === true,
      details: value?.details ?? null,
      retry_after_seconds: retryAfter && /^[0-9]+$/.test(retryAfter) ? Number(retryAfter) : null,
    });
  }
}
