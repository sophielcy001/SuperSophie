// SPDX-License-Identifier: AGPL-3.0-or-later

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { runInNewContext } from 'node:vm';

const observer = readFileSync(new URL('tauri-ipc.js', import.meta.url), 'utf8');

test('observes native Tauri replies without consuming or replacing their bodies', async () => {
  const request = [];
  const location = new URL('http://tauri.localhost/');
  const original = (url, options) => {
    request.push({ url, options });
    const headers = { 'Tauri-Response': new URL(url).pathname === '/rejected' ? 'error' : 'ok' };
    return Promise.resolve(Response.json({ pending: 12 }, { headers }));
  };
  const window = { fetch: original };
  runInNewContext(observer, { window, location, URL, performance });

  const invoke = (cmd, payload = {}) => window.fetch(`http://ipc.localhost/${cmd}`, {
    method: 'POST', body: JSON.stringify(payload),
  });
  const trace = { query: '中文' };
  window.performanceProbe.searchActive = trace;
  const page = await invoke('read_page');
  assert.equal(page.bodyUsed, false);
  assert.deepEqual(await page.json(), { pending: 12 });
  assert.equal(trace.coreRoundTripMs, undefined);
  assert.deepEqual(await (await invoke('search_text', { request: { query: 'other' } })).json(), { pending: 12 });
  assert.equal(trace.coreRoundTripMs, undefined);
  assert.deepEqual(await (await invoke('search_text', { request: { query: '中文' } })).json(), { pending: 12 });
  assert.ok(Number.isFinite(trace.coreRoundTripMs));
  assert.deepEqual(await (await invoke('advance_search_index')).json(), { pending: 12 });
  assert.equal(window.performanceProbe.pending, 12);
  assert.equal(window.performanceProbe.indexActive, false);
  await (await invoke('rejected')).json();
  await (await window.fetch('https://example.com/advance_search_index', { method: 'POST' })).json();
  assert.deepEqual(Array.from(window.performanceProbe.requests), ['read_page', 'search_text', 'search_text', 'advance_search_index']);
  assert.equal(request[0].options.body, '{}');
});

test('tracks the lifetime of indexing IPC including a rejected request', async () => {
  let reject;
  const window = { fetch: () => new Promise((_, fail) => { reject = fail; }) };
  runInNewContext(observer, { window, location: new URL('http://tauri.localhost/'), URL, performance });
  const pending = window.fetch('http://ipc.localhost/advance_search_index', { method: 'POST', body: '{}' });
  assert.equal(window.performanceProbe.indexActive, true);
  const failed = assert.rejects(pending, /cancelled/);
  reject(new Error('cancelled'));
  await failed;
  assert.equal(window.performanceProbe.indexActive, false);
  assert.equal(window.performanceProbe.requests.length, 0);
});

test('leaves documents outside the app origin untouched', () => {
  const fetch = () => {};
  const window = { fetch };
  runInNewContext(observer, { window, location: new URL('https://example.com/'), URL, performance });
  assert.equal(window.fetch, fetch);
  assert.equal(window.performanceProbe, undefined);
});
