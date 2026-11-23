// SPDX-License-Identifier: AGPL-3.0-or-later

import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { setTimeout as delay } from 'node:timers/promises';
import { coreAdapter, elapsed, environment, fileSha256, metrics } from './measure.mjs';

const [executable, data, spec, output, sampleArg = '25'] = process.argv.slice(2);
const samples = Number(sampleArg);
assert.ok(executable && data && spec && output && Number.isInteger(samples) && samples > 0,
  'usage: node scripts/performance/native.mjs <probe-executable> <data-dir> <fixture.json> <report.json> [startup-samples]');
const fixture = JSON.parse(await readFile(spec, 'utf8'));
const report = {
  fixtureVersion: fixture.version, fixtureWorkspace: fixture.workspace,
  fixtureSha256: fixture.base.logicalSha256, measuredWorkspace: fixture.measuredWorkspace,
  environment: { ...environment(), profile: process.env.ZHIYE_PROBE_PROFILE ?? 'debug' },
  startup: { cache: 'fresh app process for each sample; OS filesystem cache untouched' },
  http: { requests: 0, errors: {}, samples: {} },
};
let profile;

function launch(http = false) {
  const start = performance.now();
  const env = { ...process.env, ZHIYE_DATA_DIR: resolve(data), ZHIYE_PERFORMANCE_PROBE: '1' };
  delete env.ZHIYE_PERFORMANCE_PROBE_HTTP;
  if (http) env.ZHIYE_PERFORMANCE_PROBE_HTTP = '1';
  if (profile) {
    delete env.WEBVIEW2_PIPE_FOR_SCRIPT_DEBUGGER;
    env.WEBVIEW2_USER_DATA_FOLDER = profile;
    env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--disable-background-networking';
  }
  const child = spawn(resolve(executable), [], {
    env,
    stdio: ['ignore', 'pipe', 'inherit'], windowsHide: false,
  });
  const lines = createInterface({ input: child.stdout });
  const pending = new Map();
  const arrived = new Map();
  function end(error) {
    for (const { reject } of pending.values()) reject(error);
    pending.clear();
  }
  const exited = once(child, 'exit');
  child.on('exit', () => end(new Error('Application exited before marking itself ready')));
  child.on('error', end);
  lines.on('line', (line) => {
    const [signal, payload] = line.split(' ', 2);
    if (!signal.startsWith('M5_PERFORMANCE_')) return;
    const result = { elapsedMs: elapsed(start), payload };
    arrived.set(signal, result);
    pending.get(signal)?.resolve(result);
    pending.delete(signal);
  });
  return {
    async wait(signal) {
      if (arrived.has(signal)) return arrived.get(signal);
      if (child.exitCode !== null) throw new Error('Application already exited');
      let timeout;
      try {
        return await Promise.race([
          new Promise((resolve, reject) => pending.set(signal, { resolve, reject })),
          new Promise((_, reject) => { timeout = setTimeout(() => reject(new Error(`Timed out waiting for ${signal}`)), 30_000); }),
        ]);
      } finally { clearTimeout(timeout); pending.delete(signal); }
    },
    async close() {
      if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
      await exited;
      lines.close();
    },
  };
}

async function useAdapter(action) {
  const core = coreAdapter(resolve(data));
  try { return await action(core.call); }
  finally { await core.close(); }
}

try {
  report.executableSha256 = await fileSha256(resolve(executable));
  if (process.platform === 'win32') {
    profile = await mkdtemp(join(tmpdir(), 'zhiye-perf-startup-'));
    report.startup.webviewCache = 'separate WebView2 profile, initially empty and retained across samples';
  }
  const times = [];
  for (let index = 0; index < samples; index++) {
    const app = launch();
    try {
      const ready = await app.wait('M5_PERFORMANCE_READY');
      if (index === 0) report.appVersion = ready.payload ?? null;
      else assert.equal(ready.payload ?? null, report.appVersion);
      times.push(ready.elapsedMs);
    }
    finally { await app.close(); }
  }
  Object.assign(report.startup, metrics(times), { firstObservedMs: times[0] });

  const workspace = fixture.workspace;
  const prepared = await useAdapter(async (call) => {
    const collection = await call('read_collection', { workspace, collection: fixture.collection });
    const settings = collection.views.find((view) => view.id === fixture.view).settings;
    const issued = await call('create_integration', {
      workspace,
      draft: { name: 'M5 合成性能验收', scopes: ['content:read'],
        grants: [{ kind: 'collection', id: fixture.collection }] },
    });
    return { settings, issued };
  });
  try {
    const app = launch(true);
    try {
      const address = new URL((await app.wait('M5_PERFORMANCE_HTTP')).payload);
      assert.equal(address.hostname, '127.0.0.1');
      const base = `${address.origin}/api/v1/workspaces/${workspace}`;
      const authorization = `Bearer ${prepared.issued.token}`;
      const health = await fetch(base + '/integration', {
        headers: { authorization }, signal: AbortSignal.timeout(8_000),
      });
      assert.equal(health.status, 200, 'Preflight from the authenticated native service');
      assert.deepEqual((await health.json()).scopes, ['content:read']);
      const { filter, sort } = prepared.settings;
      let previousStart = 0;
      async function call(name, endpoint, payload, check) {
        // One documented per-integration rate window, with room for runtime jitter.
        await delay(Math.max(0, 205 - (performance.now() - previousStart)));
        previousStart = performance.now();
        report.http.requests++;
        try {
          const response = await fetch(base + endpoint, {
            method: 'POST', headers: { authorization, 'content-type': 'application/json' },
            body: JSON.stringify(payload), signal: AbortSignal.timeout(8_000),
          });
          const body = await response.json();
          const duration = elapsed(previousStart);
          if (response.status !== 200) throw new Error(`HTTP_${response.status}`);
          check(body);
          (report.http.samples[name] ??= []).push(duration);
        } catch (error) {
          const code = ['AssertionError', 'TimeoutError'].includes(error.name)
            ? error.name : error.message.startsWith('HTTP_') ? error.message : 'REQUEST_FAILED';
          report.http.errors[code] = (report.http.errors[code] ?? 0) + 1;
        }
      }
      for (let index = 0; index < 25; index++) {
        for (const [query, expected] of fixture.searchCases) {
          await call(query, '/search', { query, limit: 20 }, (body) => {
            assert.equal(body.items.length, expected);
            assert.ok(body.items.every((item) => item.title.startsWith('资料 ')));
          });
        }
        await call('filtered-records', `/collections/${fixture.collection}/query`, {
          limit: 50,
          filters: [{ field_id: filter.fieldId, operator: filter.operator, value: filter.value }],
          sort: { field_id: sort.fieldId, direction: sort.direction },
        }, (body) => {
          assert.equal(body.items.length, 50);
          assert.equal(body.items[0].title, '资料 09998');
        });
      }
      await call('outside-grant', '/search', { query: 'Editor sample', limit: 20 }, (body) => assert.equal(body.items.length, 0));
    } finally { await app.close(); }
  } finally {
    await useAdapter(async (call) => {
      await call('revoke_integration', { workspace, id: prepared.issued.integration.id });
      const integrations = await call('list_integrations', { workspace });
      assert.ok(integrations.find((item) => item.id === prepared.issued.integration.id)?.revokedAt);
    });
  }
  report.http.errorRate = Object.values(report.http.errors).reduce((sum, count) => sum + count, 0) / report.http.requests;
  report.http.samples = Object.fromEntries(Object.entries(report.http.samples).map(([key, samples]) => [key, metrics(samples)]));
  assert.equal(report.http.errorRate, 0, 'Every sampled request returned its expected results');
  report.complete = true;
} catch (error) {
  report.complete = false;
  report.error = error instanceof Error ? error.message : error?.message ?? 'Native benchmark failed';
  process.exitCode = 1;
} finally {
  try {
    if (profile) await rm(profile, { recursive: true, force: true, maxRetries: 10, retryDelay: 250 });
  } catch (error) {
    report.complete = false;
    report.cleanupError = error.message;
    process.exitCode = 1;
  }
  await writeFile(output, JSON.stringify(report, null, 2) + '\n');
}
