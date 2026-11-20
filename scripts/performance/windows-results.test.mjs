// SPDX-License-Identifier: AGPL-3.0-or-later

import assert from 'node:assert/strict';
import test from 'node:test';
import { metrics } from './measure.mjs';
import { baselineResults } from './windows-results.mjs';

const fixture = {
  version: '0.27.0',
  profile: 'debug', workspace: 'fixture-workspace',
  base: { logicalSha256: '107fb70e9bcd39ca6db88459050a780d703540e4ad36b9e5de1098297a03e875' },
  searchCases: Array.from({ length: 8 }, (_, index) => [`keyword-${index}`, 20]),
};

function reports() {
  const evidence = {
    complete: true, fixtureSha256: fixture.base.logicalSha256, fixtureVersion: fixture.version, fixtureWorkspace: fixture.workspace,
    environment: { platform: 'win32', architecture: 'x64', profile: 'debug' },
    executableSha256: 'a'.repeat(64),
  };
  const samples = (ms) => metrics(Array(25).fill(ms));
  return {
    app: { ...evidence, appVersion: fixture.version, startup: samples(3000), http: { requests: 226, errorRate: 0 } },
    ui: { ...evidence, mode: 'baseline', renderer: 'webview2', engine: 'WebView2 155.0.0.0',
      startupBodyReads: 1, ordinaryPageOpen: samples(500), filteredRecordsFirstScreen: samples(1000),
      inputWhileIndexing: { ...samples(50), pendingAtEachEvent: Array.from({ length: 25 }, (_, i) => 100 - i) },
      search: Object.fromEntries(fixture.searchCases.map(([query]) => [query, samples(1000)])) },
  };
}

test('accepts inclusive p95 limits and rejects a single slow search group', () => {
  const { app, ui } = reports();
  assert.equal(baselineResults(fixture.version, fixture, app, ui).passed, true);
  ui.search['keyword-7'] = metrics(Array(25).fill(1001));
  const result = baselineResults(fixture.version, fixture, app, ui);
  assert.equal(result.passed, false);
  assert.equal(result.checks.find((check) => check.metric === 'search').p95Ms, 1001);
});

test('rejects a foreign runtime, wrong build, incomplete samples, or stalled indexing', () => {
  const changes = [
    ({ ui }) => { ui.renderer = 'chromium-adapter'; },
    ({ app }) => { app.environment = { ...app.environment, platform: 'darwin' }; },
    ({ app }) => { app.appVersion = '0.26.0'; },
    ({ ui }) => { ui.executableSha256 = 'b'.repeat(64); },
    ({ ui }) => { ui.fixtureSha256 = 'b'.repeat(64); },
    ({ ui }) => { ui.ordinaryPageOpen.samplesMs.pop(); },
    ({ ui }) => { ui.filteredRecordsFirstScreen.samplesMs[0] = null; },
    ({ ui }) => { ui.inputWhileIndexing.pendingAtEachEvent.fill(100); },
    ({ ui }) => { delete ui.search['keyword-0']; },
  ];
  for (const change of changes) {
    const pair = reports();
    change(pair);
    assert.throws(() => baselineResults(fixture.version, fixture, pair.app, pair.ui), assert.AssertionError);
  }
});
