// SPDX-License-Identifier: AGPL-3.0-or-later

import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { parse } from 'yaml';
import { compatibilityIssues } from './check-api-v1.mjs';

const frozen = parse(await readFile(new URL('../docs/contracts/rest-v1.0.0.yaml', import.meta.url), 'utf8'));
const page = '/api/v1/workspaces/{workspace_id}/pages/{page_id}';
const search = '/api/v1/workspaces/{workspace_id}/search';

test('an optional response field and updated prose keep the v1 contract compatible', () => {
  const next = structuredClone(frozen);
  next.info.version = '1.1.0';
  next.components.schemas.Page.properties.future_note = { type: 'string' };
  next.components.schemas.Page.properties.title.description = 'Author-maintained title.';
  assert.deepEqual(compatibilityIssues(frozen, next), []);
});

test('protects v1 operations, scopes, and required preconditions', () => {
  const next = structuredClone(frozen);
  delete next.paths[page].get;
  next.paths[page].patch['x-scope'] = 'content:read';
  next.components.parameters.IfMatch.required = false;
  const issues = compatibilityIssues(frozen, next).join('\n');
  assert.match(issues, /GET .*operation removed/);
  assert.match(issues, /PATCH .*scope/);
  assert.match(issues, /PATCH .*if-match required/);
});

test('requires an explicit major-version change to remove required output or narrow input', () => {
  const next = structuredClone(frozen);
  next.components.schemas.Page.required = next.components.schemas.Page.required.filter((field) => field !== 'title');
  next.components.schemas.SearchRequest.properties.limit.maximum = 50;
  delete next.paths[search].post.responses['200'];
  const issues = compatibilityIssues(frozen, next).join('\n');
  assert.match(issues, /Page.required/);
  assert.match(issues, /SearchRequest.properties.limit.maximum/);
  assert.match(issues, /200: response removed/);
});
