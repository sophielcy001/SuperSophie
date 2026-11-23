// SPDX-License-Identifier: AGPL-3.0-or-later

import { readFile, writeFile } from 'node:fs/promises';
import openapiTS, { astToString } from 'openapi-typescript';
import { parse } from 'yaml';

const root = new URL('../', import.meta.url);
const specification = new URL('docs/openapi.v1.yaml', root);
const api = parse(await readFile(specification, 'utf8'));
const operations = new Map(
  Object.values(api.paths).flatMap((path) => Object.values(path))
    .filter((operation) => operation.operationId)
    .map((operation) => [operation.operationId, operation]),
);

function dereference(node) {
  if (!node.$ref) return node;
  if (!node.$ref.startsWith('#/')) throw new Error('MCP 输入必须使用本地 OpenAPI 定义。');
  return node.$ref.slice(2).split('/').reduce((value, key) => value[key], api);
}

const body = (name) => operations.get(name).requestBody.content['application/json'].schema;
const parameter = (name) => api.components.parameters[name].schema;
const pageContentParameter = (name) => operations.get('getPageContent').parameters.find((item) => item.name === name).schema;
const blockOperations = body('applyBlockOperations').properties.operations;
const schemas = {
  search: body('search'),
  collectionQuery: body('queryCollection'),
  createPage: body('createPage'),
  patchPage: body('patchPage'),
  proposal: body('submitProposal'),
  operations: blockOperations,
  appendOnly: {
    ...blockOperations,
    items: api.components.schemas.BlockOperation.oneOf.find((operation) => operation.properties.op.const === 'append'),
  },
  contentFormat: pageContentParameter('format'),
  contentLimit: pageContentParameter('limit'),
  contentCursor: pageContentParameter('cursor'),
  uuid: parameter('Page'),
  idempotencyKey: parameter('IdempotencyKey'),
  baseRevision: parameter('BaseRevision'),
};

const definitions = {};
const definitionPrefix = '#/components/schemas/';
function toolSchema(node) {
  if (Array.isArray(node)) return node.map(toolSchema);
  if (node === null || typeof node !== 'object') return node;
  if (node.$ref) {
    if (!node.$ref.startsWith(definitionPrefix)) throw new Error('MCP 输入只能引用 OpenAPI Schema。');
    const name = node.$ref.slice(definitionPrefix.length);
    if (!Object.hasOwn(definitions, name)) {
      definitions[name] = {};
      definitions[name] = toolSchema(dereference(node));
    }
    return { $ref: `#/$defs/${name}` };
  }
  return Object.fromEntries(Object.entries(node).map(([key, value]) => [key, toolSchema(value)]));
}

const inputs = toolSchema(schemas);
const banner = '// 从 docs/openapi.v1.yaml 生成。使用 npm run generate:contracts 重新生成。\n';
const outputs = [
  ['packages/sdk-ts/src/openapi.generated.ts', banner + astToString(await openapiTS(specification, {
    defaultNonNullable: false,
  }))],
  ['packages/mcp/src/schemas.generated.ts',
    banner + `export const restInputs = ${JSON.stringify(inputs, null, 2)};\n`
      + `export const restDefinitions = ${JSON.stringify(definitions, null, 2)};\n`],
];

if (process.argv.includes('--check')) {
  for (const [path, expected] of outputs) {
    const existing = await readFile(new URL(path, root), 'utf8').catch(() => null);
    if (existing !== expected) {
      process.stderr.write(`${path} 与 OpenAPI 不一致。运行 npm run generate:contracts。\n`);
      process.exitCode = 1;
    }
  }
} else {
  await Promise.all(outputs.map(([path, content]) => writeFile(new URL(path, root), content)));
}
