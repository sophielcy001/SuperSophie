// SPDX-License-Identifier: AGPL-3.0-or-later

import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdir, open, readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { environment, fileSha256 } from './measure.mjs';
import { baselineResults, pressureSha256 } from './windows-results.mjs';

const [destination] = process.argv.slice(2);
assert.ok(destination && process.argv.length === 3,
  'usage: node scripts/performance/windows.mjs <new results directory on the local SSD>');
assert.ok(process.platform === 'win32' && process.arch === 'x64', 'M5 reference run requires Windows x64');

const root = fileURLToPath(new URL('../../', import.meta.url));
const target = resolve(root, process.env.CARGO_TARGET_DIR || 'target', 'debug');
const executable = join(target, 'zhiye-desktop.exe');
const adapter = join(target, 'examples', 'm5_adapter.exe');
const env = { ...process.env, ZHIYE_PERF_ADAPTER: adapter, ZHIYE_PROBE_PROFILE: 'debug' };
const directory = resolve(destination);
await mkdir(directory);
const output = (name) => join(directory, `${name}.json`);
const json = async (file) => JSON.parse(await readFile(file, 'utf8'));
const report = { environment: { ...environment(), profile: 'debug' }, startedAt: new Date().toISOString(), steps: [] };

async function run(command, args, stdout) {
  const step = { command, args, startedAt: new Date().toISOString(), ...(stdout ? { stdout } : {}) };
  report.steps.push(step);
  const file = stdout ? await open(stdout, 'wx') : null;
  try {
    const child = spawn(command, args, { cwd: root, env, stdio: ['ignore', file?.fd ?? 'inherit', 'inherit'] });
    const [code, signal] = await once(child, 'close');
    Object.assign(step, { finishedAt: new Date().toISOString(), code, signal });
    assert.equal(code, 0, `${command} ${args.join(' ')} failed${signal ? ` with ${signal}` : ''}`);
  } finally {
    await file?.close();
  }
}

const scenario = (name, ...args) => run(process.execPath, [join(root, 'scripts', 'performance', `${name}.mjs`), ...args]);

try {
  const { version } = await json(join(root, 'package.json'));
  report.appVersion = version;
  report.locks = {
    cargo: await fileSha256(join(root, 'Cargo.lock')),
    npm: await fileSha256(join(root, 'package-lock.json')),
  };
  report.executableSha256 = await fileSha256(executable);
  const baseline = join(directory, 'baseline-data');
  await run(join(target, 'examples', 'm5_baseline.exe'), [baseline], output('baseline-core'));
  await scenario('native', executable, baseline, output('baseline-core'), output('baseline-native'));
  await scenario('browser', baseline, output('baseline-core'), output('baseline-webview2'), '--webview2', executable);
  const fixture = await json(output('baseline-core'));
  const startup = await json(output('baseline-native'));
  const interaction = await json(output('baseline-webview2'));
  assert.equal(startup.executableSha256, report.executableSha256);
  report.baseline = baselineResults(version, fixture, startup, interaction);

  const pressure = join(directory, 'pressure-data');
  await run(join(target, 'examples', 'm5_pressure.exe'), [pressure], output('pressure-core'));
  await scenario('browser', pressure, output('pressure-core'), output('pressure-webview2'), 'pressure', '--webview2', executable);
  const scale = await json(output('pressure-core'));
  const pressureView = await json(output('pressure-webview2'));
  assert.equal(scale.version, version);
  assert.equal(scale.logicalSha256, pressureSha256);
  assert.equal(scale.pages, 100_000);
  assert.equal(scale.blocks, 1_000_000);
  assert.equal(pressureView.fixtureWorkspace, scale.workspace);
  assert.equal(pressureView.executableSha256, report.executableSha256);
  assert.equal(pressureView.renderer, 'webview2');
  assert.equal(pressureView.lastPageReachableBySearch, true);
  report.pressure = { fixtureSha256: pressureSha256, engine: pressureView.engine, pages: scale.pages, blocks: scale.blocks };
  report.complete = true;
  report.passed = report.baseline.passed;
  if (!report.passed) process.exitCode = 1;
} catch (error) {
  report.complete = false;
  report.passed = false;
  report.error = error.message;
  process.exitCode = 1;
} finally {
  report.finishedAt = new Date().toISOString();
  await writeFile(output('windows-performance'), JSON.stringify(report, null, 2) + '\n', { flag: 'wx' });
  console.log(output('windows-performance'));
}
