// SPDX-License-Identifier: AGPL-3.0-or-later

import assert from 'node:assert/strict';
import { metrics } from './measure.mjs';

export const baselineSha256 = '107fb70e9bcd39ca6db88459050a780d703540e4ad36b9e5de1098297a03e875';
export const pressureSha256 = '4d9892bd3742a6bcddb851d62356a7a2877f06e868fd171523e78339a1f49220';
const sampleCount = 25;

export function baselineResults(version, fixture, app, ui) {
  assert.equal(fixture.version, version, 'Fixture generator must match the source version');
  assert.equal(fixture.base.logicalSha256, baselineSha256, 'Fixed M5 baseline identity');
  assert.equal(app.appVersion, version, 'Native probe must match the source version');
  assert.match(app.executableSha256, /^[0-9a-f]{64}$/);
  assert.equal(ui.executableSha256, app.executableSha256, 'Startup and interaction must use the same build');
  for (const report of [app, ui]) {
    assert.equal(report.complete, true);
    assert.equal(report.environment.platform, 'win32');
    assert.equal(report.environment.architecture, 'x64');
    assert.equal(report.environment.profile, fixture.profile);
    assert.equal(report.fixtureSha256, baselineSha256);
    assert.equal(report.fixtureVersion, version);
    assert.equal(report.fixtureWorkspace, fixture.workspace);
  }
  assert.equal(ui.mode, 'baseline');
  assert.equal(ui.renderer, 'webview2');
  assert.match(ui.engine, /^WebView2 .+/);
  assert.equal(ui.startupBodyReads, 1);
  assert.equal(app.http.requests, 226);
  assert.equal(app.http.errorRate, 0);
  const pending = ui.inputWhileIndexing.pendingAtEachEvent;
  assert.equal(pending.length, sampleCount);
  assert.ok(pending.every((count) => Number.isInteger(count) && count > 0));
  assert.ok(new Set(pending).size > 1, 'Indexing must progress during the input samples');

  const p95 = (measurement, name) => {
    assert.ok(Array.isArray(measurement?.samplesMs), `Missing raw samples for ${name}`);
    const values = measurement.samplesMs;
    assert.equal(values.length, sampleCount, `Sample count for ${name}`);
    assert.ok(values.every((ms) => Number.isFinite(ms) && ms >= 0), `Invalid sample for ${name}`);
    return metrics(values).p95Ms;
  };
  assert.equal(fixture.searchCases.length, 8);
  const searchP95Ms = Object.fromEntries(fixture.searchCases.map(([query]) => [query, p95(ui.search[query], query)]));
  const checks = [
    ['startup', p95(app.startup, 'startup'), 3000],
    ['ordinaryPageOpen', p95(ui.ordinaryPageOpen, 'ordinaryPageOpen'), 500],
    ['search', Math.max(...Object.values(searchP95Ms)), 1000],
    ['filteredRecordsFirstScreen', p95(ui.filteredRecordsFirstScreen, 'filteredRecordsFirstScreen'), 1000],
    ['inputWhileIndexing', p95(ui.inputWhileIndexing, 'inputWhileIndexing'), 50],
  ].map(([metric, p95Ms, limitMs]) => ({ metric, p95Ms, limitMs, passed: p95Ms <= limitMs }));
  return { passed: checks.every((check) => check.passed), checks, searchP95Ms };
}
