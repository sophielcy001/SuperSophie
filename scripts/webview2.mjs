// SPDX-License-Identifier: AGPL-3.0-or-later

import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { get } from 'node:http';
import { createServer } from 'node:net';
import { resolve } from 'node:path';
import { json } from 'node:stream/consumers';
import { setTimeout as delay } from 'node:timers/promises';
import { chromium } from 'playwright-core';

export const tauriOrigin = 'http://tauri.localhost';
export const tauriIpcOrigin = 'http://ipc.localhost';

async function discoverTargets(endpoint) {
  // WebView2 can use an ephemeral port that the browser Fetch policy forbids.
  const response = await new Promise((done, reject) => {
    get(`${endpoint}/json/list`, { signal: AbortSignal.timeout(500) }, done).on('error', reject);
  });
  const targets = await json(response);
  if (response.statusCode !== 200) throw new Error(`Discovery returned HTTP ${response.statusCode}`);
  if (!Array.isArray(targets)) throw new Error('Discovery did not return a target list');
  return targets;
}

export async function startWebView2(executable, data, profile, launchArgs = []) {
  assert.equal(process.platform, 'win32', 'WebView2 verification requires a Windows desktop');
  const reserve = createServer().listen(0, '127.0.0.1');
  await once(reserve, 'listening');
  const { port } = reserve.address();
  await new Promise((done, reject) => reserve.close((error) => error ? reject(error) : done()));
  const endpoint = `http://127.0.0.1:${port}`;
  const env = {
    ...process.env,
    WEBVIEW2_USER_DATA_FOLDER: resolve(profile),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1 --disable-background-networking`,
  };
  if (data === null) delete env.ZHIYE_DATA_DIR;
  else env.ZHIYE_DATA_DIR = resolve(data);
  delete env.WEBVIEW2_PIPE_FOR_SCRIPT_DEBUGGER;
  const child = spawn(resolve(executable), launchArgs, { stdio: ['ignore', 'ignore', 'inherit'], env });
  let launchError;
  const closed = new Promise((done) => child.once('close', done));
  child.on('error', (error) => { launchError = error; });

  let browser;
  let lastDiscovery = 'no response';
  async function close() {
    try {
      await browser?.close();
    } finally {
      if (child.pid && child.exitCode === null && child.signalCode === null) child.kill();
      await closed;
    }
  }

  try {
    const deadline = performance.now() + 30_000;
    while (performance.now() < deadline) {
      if (launchError) throw launchError;
      if (child.exitCode !== null || child.signalCode !== null) {
        throw new Error(`Desktop process exited before WebView2 attached: ${child.exitCode ?? child.signalCode}`);
      }
      if (!browser) {
        const targets = await discoverTargets(endpoint).catch((error) => {
          lastDiscovery = error.message;
          return null;
        });
        if (targets?.some((target) => target.type === 'page' && target.url.startsWith(`${tauriOrigin}/`))) {
          browser = await chromium.connectOverCDP(endpoint, { timeout: 5000 });
          lastDiscovery = 'connected to browser, waiting for the Tauri page';
        } else if (targets) {
          lastDiscovery = `no Tauri page in ${targets.length} discovered targets`;
        }
      }
      // The discovery endpoint can report the page before Playwright sees it after a restart.
      const page = browser?.contexts().flatMap((context) => context.pages())
        .find((tab) => tab.url().startsWith(`${tauriOrigin}/`));
      if (browser && page) return { browser, page, close };
      await delay(180);
    }
    throw new Error(`Timed out waiting for Tauri WebView2 at ${endpoint} (${lastDiscovery})`);
  } catch (error) {
    await close();
    throw error;
  }
}
