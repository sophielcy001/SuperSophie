// SPDX-License-Identifier: AGPL-3.0-or-later

import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { isDeepStrictEqual } from 'node:util';
import { parse } from 'yaml';

const stable = new URL('../docs/contracts/rest-v1.0.0.yaml', import.meta.url);
const current = new URL('../docs/openapi.v1.yaml', import.meta.url);
const methods = ['get', 'put', 'post', 'delete', 'options', 'head', 'patch', 'trace'];
const annotations = new Set(['title', 'description', '$comment', 'example', 'examples', 'deprecated']);
const subschemas = new Set([
  'items', 'prefixItems', 'additionalProperties', 'unevaluatedProperties', 'not',
  'if', 'then', 'else', 'oneOf', 'anyOf', 'allOf',
]);
const plainObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const sorted = (values = []) => [...values].sort();
const deref = (api, definition) => definition?.$ref
  ? definition.$ref.slice(2).split('/').reduce((node, key) => node?.[key], api) : definition;
const paramKey = (param) => `${param.in}:${param.in === 'header' ? param.name.toLowerCase() : param.name}`;

export function compatibilityIssues(frozen, candidate) {
  const issues = [];
  const problem = (at, message) => issues.push(`${at}: ${message}`);
  const equal = (previous, next, at) => {
    if (!isDeepStrictEqual(previous, next)) problem(at, 'existing contract changed');
  };

  function schema(previous, next, at) {
    if (Array.isArray(previous) && Array.isArray(next)) {
      if (previous.length !== next.length) problem(at, 'schema variants changed');
      for (let index = 0; index < Math.min(previous.length, next.length); index++) {
        schema(previous[index], next[index], `${at}[${index}]`);
      }
      return;
    }
    if (!plainObject(previous) || !plainObject(next)) {
      equal(previous, next, at);
      return;
    }
    equal(sorted(previous.required), sorted(next.required), `${at}.required`);
    for (const [key, entry] of Object.entries(previous.properties ?? {})) {
      schema(entry, next.properties?.[key], `${at}.properties.${key}`);
    }
    const keywords = new Set([...Object.keys(previous), ...Object.keys(next)]);
    for (const key of keywords) {
      if (annotations.has(key) || key === 'required' || key === 'properties') continue;
      const location = `${at}.${key}`;
      if (subschemas.has(key)) schema(previous[key], next[key], location);
      else if (key === 'enum') equal(sorted(previous.enum), sorted(next.enum), location);
      else equal(previous[key], next[key], location);
    }
  }

  function parameters(api, item, operation) {
    const entries = [...(item.parameters ?? []), ...(operation.parameters ?? [])].map((entry) => deref(api, entry));
    return new Map(entries.map((entry) => [paramKey(entry), entry]));
  }

  function content(previous, next, at) {
    for (const [media, entry] of Object.entries(previous ?? {})) {
      const current = next?.[media];
      if (!current) problem(`${at} ${media}`, 'media type removed');
      else schema(entry.schema, current.schema, `${at} ${media}`);
    }
  }

  function response(previous, next, at) {
    if (!next) {
      problem(at, 'response removed');
      return;
    }
    content(previous.content, next.content, at);
    const headers = new Map(Object.entries(next.headers ?? {}).map(([name, header]) => [name.toLowerCase(), header]));
    for (const [name, definition] of Object.entries(previous.headers ?? {})) {
      const oldHeader = deref(frozen, definition);
      const newHeader = deref(candidate, headers.get(name.toLowerCase()));
      if (!newHeader) problem(`${at} ${name}`, 'response header removed');
      else {
        if (oldHeader.required && !newHeader.required) problem(`${at} ${name}`, 'response header guarantee removed');
        schema(oldHeader.schema, newHeader.schema, `${at} ${name}`);
      }
    }
  }

  if (!/^1\.\d+\.\d+$/.test(candidate.info?.version ?? '')) problem('info.version', 'expected a stable v1 version');
  equal(frozen.openapi, candidate.openapi, 'openapi');
  equal(frozen.security, candidate.security, 'security');
  equal(frozen.servers?.map((server) => server.url), candidate.servers?.map((server) => server.url), 'servers');
  equal(frozen['x-zhiye-limits'], candidate['x-zhiye-limits'], 'x-zhiye-limits');
  for (const [name, definition] of Object.entries(frozen.components.securitySchemes ?? {})) {
    const signature = (value) => value && Object.fromEntries(Object.entries(value).filter(([key]) => !annotations.has(key)));
    equal(signature(definition), signature(candidate.components?.securitySchemes?.[name]), `securitySchemes.${name}`);
  }
  for (const [name, definition] of Object.entries(frozen.components.schemas ?? {})) {
    schema(definition, candidate.components?.schemas?.[name], `schemas.${name}`);
  }

  for (const [path, oldItem] of Object.entries(frozen.paths)) {
    const newItem = candidate.paths?.[path];
    for (const method of methods.filter((name) => oldItem[name])) {
      const label = `${method.toUpperCase()} ${path}`;
      const before = oldItem[method];
      const after = newItem?.[method];
      if (!after) {
        problem(label, 'operation removed');
        continue;
      }
      equal(before.operationId, after.operationId, `${label} operationId`);
      equal(before['x-scope'], after['x-scope'], `${label} scope`);
      equal(before.security, after.security, `${label} security`);
      const oldParams = parameters(frozen, oldItem, before);
      const newParams = parameters(candidate, newItem, after);
      for (const [name, param] of oldParams) {
        const updated = newParams.get(name);
        if (!updated) problem(`${label} ${name}`, 'parameter removed');
        else {
          equal(Boolean(param.required), Boolean(updated.required), `${label} ${name} required`);
          schema(param.schema, updated.schema, `${label} ${name}`);
        }
      }
      for (const [name, param] of newParams) {
        if (param.required && !oldParams.has(name)) problem(`${label} ${name}`, 'new required parameter');
      }
      const oldBody = deref(frozen, before.requestBody);
      const newBody = deref(candidate, after.requestBody);
      if (Boolean(oldBody?.required) !== Boolean(newBody?.required)) problem(label, 'request body requirement changed');
      content(oldBody?.content, newBody?.content, `${label} request`);
      for (const [status, entry] of Object.entries(before.responses)) {
        response(deref(frozen, entry), deref(candidate, after.responses?.[status]), `${label} ${status}`);
      }
    }
  }
  return issues;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [before, after] = await Promise.all([stable, current].map(async (url) => parse(await readFile(url, 'utf8'))));
  const issues = compatibilityIssues(before, after);
  if (issues.length) {
    process.stderr.write(issues.join('\n') + '\n');
    process.exitCode = 1;
  }
}
