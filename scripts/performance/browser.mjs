// SPDX-License-Identifier: AGPL-3.0-or-later

import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { extname, join, resolve, sep } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { chromium } from 'playwright-core';
import { startWebView2, tauriIpcOrigin, tauriOrigin } from '../webview2.mjs';
import { coreAdapter, elapsed, environment, fileSha256, metrics } from './measure.mjs';

const { positionals, values: { webview2 } } = parseArgs({ allowPositionals: true, options: { webview2: { type: 'string' } } });
const [data, spec, resultPath, mode = 'baseline'] = positionals;
assert.ok(positionals.length <= 4 && data && spec && resultPath && ['baseline', 'pressure'].includes(mode),
  'usage: node scripts/performance/browser.mjs <data-dir> <fixture.json> <report.json> [baseline|pressure] [--webview2 <executable>]');
const fixture = JSON.parse(await readFile(spec, 'utf8'));
const root = resolve('apps/desktop/dist');
const media = { '.html': 'text/html', '.js': 'application/javascript', '.mjs': 'application/javascript',
  '.css': 'text/css', '.woff2': 'font/woff2' };
const report = { fixtureSha256: fixture.base?.logicalSha256 ?? fixture.logicalSha256,
  fixtureVersion: fixture.version, fixtureWorkspace: fixture.workspace, measuredWorkspace: fixture.measuredWorkspace,
  platform: `${process.platform} ${process.arch}`,
  environment: { ...environment(), profile: process.env.ZHIYE_PROBE_PROFILE ?? 'debug' }, mode,
  renderer: webview2 ? 'webview2' : 'chromium-adapter', pageErrors: [], blockedExternalRequests: 0 };
let browser;
let backend;
let server;
let desktop;
let profile;

async function chromiumWithAdapter() {
  server = createServer(async (request, response) => {
    const path = new URL(request.url, 'http://127.0.0.1').pathname;
    const file = resolve(root, `.${path === '/' ? '/index.html' : path}`);
    if (!file.startsWith(root + sep)) return response.writeHead(404).end();
    if (path === '/favicon.ico') return response.writeHead(204).end();
    try {
      const bytes = await readFile(file);
      response.writeHead(200, { 'content-type': media[extname(file)] ?? 'application/octet-stream' }).end(bytes);
    } catch {
      response.writeHead(404).end();
    }
  }).listen(0, '127.0.0.1');
  await once(server, 'listening');
  const origin = `http://127.0.0.1:${server.address().port}`;
  const executablePath = process.env.ZHIYE_TEST_CHROMIUM
    ?? (process.platform === 'darwin' ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' : undefined);
  browser = await chromium.launch({ headless: true, executablePath });
  report.engine = `Chromium ${browser.version()}`;
  const page = await browser.newPage({ viewport: { width: 1280, height: 860 } });
  backend = coreAdapter(resolve(data));
  const overview = await backend.call('list_workspaces');
  assert.deepEqual(overview.workspaces.map(({ id }) => id), [fixture.workspace]);
  await page.exposeFunction('invokePerformanceCore', backend.call);
  await page.addInitScript((appOrigin) => {
    if (location.origin !== appOrigin) return;
    const requests = [];
    window.performanceProbe = { requests };
    window.isTauri = true;
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' } },
      transformCallback: () => 1,
      invoke: async (command, args) => {
        if (command.startsWith('plugin:event|')) return 1;
        if (command === 'take_location') return null;
        const tracing = command === 'search_text' ? window.performanceProbe.searchActive : undefined;
        const callAt = performance.now();
        const result = await window.invokePerformanceCore(command, args);
        if (tracing && tracing.query === args?.request?.query) tracing.coreRoundTripMs = performance.now() - callAt;
        requests.push(command);
        if (command === 'advance_search_index') window.performanceProbe.pending = result.pending;
        return result;
      },
    };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
  }, origin);
  return { page, origins: [origin], load: () => page.goto(origin, { waitUntil: 'domcontentloaded' }) };
}

async function nativeWebView() {
  assert.equal(process.platform, 'win32', '--webview2 requires a Windows desktop');
  report.executableSha256 = await fileSha256(resolve(webview2));
  profile = await mkdtemp(join(tmpdir(), 'zhiye-perf-webview-'));
  desktop = await startWebView2(webview2, data, profile);
  const { browser, page } = desktop;
  report.engine = `WebView2 ${browser.version()}`;
  await page.addInitScript({ path: fileURLToPath(new URL('tauri-ipc.js', import.meta.url)) });
  return { page, origins: [tauriOrigin, tauriIpcOrigin], load: () => page.reload({ waitUntil: 'domcontentloaded' }) };
}

try {
  report.phase = 'launch';
  const launch = performance.now();
  const { page, origins, load } = await (webview2 ? nativeWebView() : chromiumWithAdapter());
  page.setDefaultTimeout(12_000);
  const pressureCdp = mode === 'pressure' ? await page.context().newCDPSession(page) : null;
  await pressureCdp?.send('Performance.enable');
  page.on('pageerror', (error) => report.pageErrors.push(error.message));
  await page.route(/^https?:/, (route) => {
    if (origins.includes(new URL(route.request().url()).origin)) return route.continue();
    report.blockedExternalRequests += 1;
    return route.abort();
  });
  const button = (name) => page.getByRole('button', { name, exact: true });
  const tree = page.getByRole('navigation', { name: '页面树' });
  const treePage = (title) => tree.getByRole('button', { name: title, exact: true });
  const query = page.getByRole('textbox', { name: '搜索关键词' });
  const searchResult = page.locator('.picker-results');
  const waitEditor = (title) => page.waitForFunction((expected) =>
    document.querySelector('[aria-label="页面标题"]')?.value === expected
    && document.querySelector('[aria-label="页面正文"][contenteditable="true"]')?.getBoundingClientRect().height > 0, title);
  const firstTitle = fixture.ordinaryPages[0].title;
  try {
    if (webview2 && mode === 'pressure') {
      // Attaching over CDP can finish before the first document reads its page list.
      // Measure that startup before reloading to attach the IPC observer.
      report.phase = 'native-first-editor';
      await waitEditor(firstTitle);
      report.nativeFirstEditorObservedMs = elapsed(launch);
      report.phase = 'traced-reload';
    }
    const startup = performance.now();
    await load();
    report.phase = 'initial-editor';
    await waitEditor(firstTitle);
    report.browserPageLoadToEditorMs = elapsed(startup);
  } catch (error) {
    if (webview2 && mode === 'pressure') {
      report.startupState = await page.evaluate((expected) => {
        const editor = document.querySelector('[aria-label="页面正文"][contenteditable="true"]');
        return { readyState: document.readyState, visibility: document.visibilityState,
          targetTitleVisible: document.querySelector('[aria-label="页面标题"]')?.value === expected,
          editorVisible: (editor?.getBoundingClientRect().height ?? 0) > 0,
          loadingScreen: document.querySelector('.startup') !== null,
          alertVisible: document.querySelector('[role="alert"]') !== null };
      }, firstTitle).catch(() => null);
    }
    throw error;
  }
  report.renderedInitialTreeRows = await tree.locator('.tree-row').count();
  const calls = await page.evaluate(() => window.performanceProbe.requests.slice());
  report.startupBodyReads = calls.filter((command) => command === 'read_page').length;
  assert.equal(report.startupBodyReads, 1);
  if (webview2) {
    const workspaces = await page.evaluate(async () =>
      (await window.__TAURI_INTERNALS__.invoke('list_workspaces')).workspaces.map(({ id }) => id));
    assert.deepEqual(workspaces, [fixture.workspace]);
  }

  if (mode === 'pressure') {
    report.phase = 'pressure-navigation';
    const usage = ({ metrics }) => Object.fromEntries(metrics
      .filter(({ name }) => name === 'Nodes' || name === 'JSHeapUsedSize')
      .map(({ name, value }) => [name, value]));
    report.startupRenderer = usage(await pressureCdp.send('Performance.getMetrics'));
    assert.ok(report.renderedInitialTreeRows <= 130,
      `Large page tree initially rendered ${report.renderedInitialTreeRows} rows`);
    const loadMore = tree.getByRole('button', { name: /^显示更多页面/ });
    if (await loadMore.count() === 0) {
      await tree.getByRole('button', { name: `展开 ${fixture.ordinaryPages[0].title}`, exact: true }).click();
    }
    const beforeMore = await tree.locator('.tree-row').count();
    await loadMore.first().focus();
    await page.keyboard.press('Enter');
    assert.ok(await tree.locator('.tree-row').count() > beforeMore);
    await button('搜索').click();
    await query.fill(fixture.uniqueQuery);
    await searchResult.locator('.picker-row').waitFor();
    assert.equal(await searchResult.locator('.picker-row').count(), 1);
    const targetOpen = performance.now();
    await searchResult.locator('.picker-row').click();
    await waitEditor(fixture.lastPage.title);
    report.lastPageOpenMs = elapsed(targetOpen);
    assert.equal(await treePage(fixture.lastPage.title).getAttribute('aria-current'), 'page');
    report.lastPageReachableBySearch = true;
    report.renderedAfterRevealingSelection = await tree.locator('.tree-row').count();
    report.selectionRenderer = usage(await pressureCdp.send('Performance.getMetrics'));
    // The recent-page shortcut updates after Core commits the separate navigation write.
    const recent = page.getByRole('navigation', { name: '资料入口' });
    await recent.getByText('最近访问', { exact: true }).click();
    await recent.getByRole('button', { name: `最近访问 ${fixture.lastPage.title}`, exact: true }).waitFor();
  } else {
    report.phase = 'ordinary-pages';
    const pageTimes = [];
    for (const sample of fixture.ordinaryPages.slice(1)) {
      const target = treePage(sample.title);
      await target.scrollIntoViewIfNeeded();
      const start = performance.now();
      await target.click();
      await waitEditor(sample.title);
      pageTimes.push(elapsed(start));
    }
    report.ordinaryPageOpen = metrics(pageTimes);
    report.phase = 'search';
    const search = Object.fromEntries(fixture.searchCases.map(([query]) => [query, { paint: [], core: [] }]));
    await button('搜索').click();
    await searchResult.evaluate((results) => {
      const probe = window.performanceProbe;
      probe.searchPaints = [];
      const observer = new MutationObserver(() => {
        const trace = probe.searchActive;
        if (!trace || trace.painted) return;
        if (results.getAttribute('aria-busy') === 'true') {
          trace.sawBusy = true;
          return;
        }
        if (!trace.sawBusy || trace.coreRoundTripMs === undefined) return;
        trace.painted = true;
        requestAnimationFrame(() => {
          if (probe.searchActive !== trace) return;
          probe.searchPaints.push({
            query: trace.query, coreRoundTripMs: trace.coreRoundTripMs, paintMs: performance.now() - trace.inputAt,
            count: results.querySelectorAll('.picker-row').length,
          });
        });
      });
      observer.observe(results, { attributes: true, attributeFilter: ['aria-busy'], childList: true, subtree: true });
      document.querySelector('[aria-label="搜索关键词"]').addEventListener('input', (event) => {
        probe.searchActive = { query: event.currentTarget.value, inputAt: event.timeStamp, sawBusy: false };
      }, { capture: true });
    });
    let rendered = 0;
    for (let sample = 0; sample < 25; sample++) {
      for (const [keyword, count] of fixture.searchCases) {
        await query.fill(keyword);
        rendered++;
        await page.waitForFunction((n) => window.performanceProbe.searchPaints.length === n, rendered);
        const measurement = await page.evaluate(() => window.performanceProbe.searchPaints.at(-1));
        assert.equal(measurement.query, keyword);
        assert.equal(measurement.count, count);
        if (count === 0) await searchResult.getByText('没有找到匹配的页面。', { exact: true }).waitFor();
        search[keyword].paint.push(measurement.paintMs);
        search[keyword].core.push(measurement.coreRoundTripMs);
      }
    }
    report.search = Object.fromEntries(Object.entries(search).map(([word, times]) => [
      word, { ...metrics(times.paint), [webview2 ? 'ipcRoundTrip' : 'adapterRoundTrip']: metrics(times.core) },
    ]));
    await page.keyboard.press('Escape');
    report.phase = 'filtered-records';
    const records = [];
    for (let sample = 0; sample < 25; sample++) {
      const target = treePage('知识库基线');
      await target.scrollIntoViewIfNeeded();
      const start = performance.now();
      await target.click();
      const table = page.getByRole('table');
      await table.getByText('资料 09998', { exact: true }).waitFor();
      assert.equal(await table.locator('tbody tr').count(), 50);
      records.push(elapsed(start));
      await treePage(fixture.ordinaryPages[0].title).click();
      await waitEditor(fixture.ordinaryPages[0].title);
    }
    report.filteredRecordsFirstScreen = metrics(records);
    report.phase = 'input-while-indexing';
    await button('搜索').click();
    await button('重建索引').click();
    await page.getByText(/索引待处理 \d+ 页/).waitFor();
    await page.keyboard.press('Escape');
    await page.waitForFunction(() => window.performanceProbe.pending > 0);
    const body = page.getByRole('textbox', { name: '页面正文', exact: true });
    await body.focus();
    await page.keyboard.press('Home');
    await body.evaluate((editor) => {
      const state = window.performanceProbe;
      state.typing = { samples: [], frameGaps: [], longTasks: [], active: true };
      const trace = state.typing;
      let frame = performance.now();
      const tick = (time) => {
        if (!trace.active) return;
        trace.frameGaps.push(time - frame);
        frame = time;
        requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
      if (PerformanceObserver.supportedEntryTypes.includes('longtask')) {
        trace.observer = new PerformanceObserver((list) => {
          for (const task of list.getEntries()) trace.longTasks.push(task.duration);
        });
        trace.observer.observe({ type: 'longtask' });
      }
      editor.addEventListener('keydown', (event) => {
        if (!trace.active || !event.isTrusted || event.key !== 'x') return;
        const before = editor.textContent;
        const { pending, indexActive } = state;
        const record = () => {
          if (editor.textContent === before) return requestAnimationFrame(record);
          trace.samples.push({ ms: performance.now() - event.timeStamp, pending, indexActive });
        };
        requestAnimationFrame(record);
      }, { capture: true });
    });
    for (let sample = 1; sample <= 25; sample++) {
      await delay(60);
      await page.keyboard.press('x');
      await page.waitForFunction((count) => window.performanceProbe.typing.samples.length === count, sample);
    }
    const typing = await page.evaluate(() => {
      const trace = window.performanceProbe.typing;
      trace.active = false;
      if (trace.observer) trace.longTasks.push(...trace.observer.takeRecords().map((task) => task.duration));
      trace.observer?.disconnect();
      return { samples: trace.samples, frameGaps: trace.frameGaps, longTasks: trace.longTasks };
    });
    assert.ok(typing.samples.every(({ pending }) => pending > 0), 'An index batch was pending on each input');
    assert.ok(new Set(typing.samples.map(({ pending }) => pending)).size > 1, 'Indexing progressed during the sample window');
    report.inputWhileIndexing = { ...metrics(typing.samples.map(({ ms }) => ms)),
      pendingAtEachEvent: typing.samples.map(({ pending }) => pending),
      slowFrameCount: typing.frameGaps.filter((gap) => gap > 50).length,
      maxFrameGapMs: Math.max(...typing.frameGaps), longTasksMs: typing.longTasks,
    };
    if (webview2) report.inputWhileIndexing.eventsDuringActiveBatch = typing.samples.filter(({ indexActive }) => indexActive).length;
    const save = process.platform === 'darwin' ? 'Meta+s' : 'Control+s';
    await page.keyboard.press(save);
    await page.getByText('已保存', { exact: true }).waitFor();
    await page.waitForFunction(() => window.performanceProbe.pending === 0, null, { timeout: 120_000 });
  }
  assert.deepEqual(report.pageErrors, []);
  assert.equal(report.blockedExternalRequests, 0);
  delete report.phase;
  report.complete = true;
} catch (error) {
  report.complete = false;
  report.error = error instanceof Error ? error.message : error?.message ?? 'Benchmark scenario failed';
  process.exitCode = 1;
} finally {
  try {
    await desktop?.close();
    await browser?.close();
    if (server) await new Promise((done) => server.close(done));
    await backend?.close();
    if (profile) await rm(profile, { recursive: true, force: true, maxRetries: 10, retryDelay: 250 });
  } catch (error) {
    report.complete = false;
    report.cleanupError = error.message;
    process.exitCode = 1;
  }
  await writeFile(resultPath, JSON.stringify(report, null, 2) + '\n');
}
