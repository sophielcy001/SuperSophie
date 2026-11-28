// SPDX-License-Identifier: AGPL-3.0-or-later

// Local UI preview: runs the desktop front end in an ordinary browser against
// the real Rust Core, with hot reload for style and component changes.
//
//   cargo build --manifest-path .verify/m2-browser/driver/Cargo.toml
//   node scripts/ui-preview.mjs [port]
//
// Each start uses a fresh data directory seeded with the first-run content.
// Sync commands are simulated in memory so the sync dialog can be laid out
// without a service; real sync is covered by the Rust tests.

import { spawn } from 'node:child_process';
import { mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';
import { createServer } from 'vite';

const root = fileURLToPath(new URL('../', import.meta.url));
const port = Number(process.argv[2] ?? 1430);
const sandbox = await mkdtemp(join(tmpdir(), 'zhiye-ui-preview-'));
const driverName = process.platform === 'win32' ? 'zhiye-ui-verification.exe' : 'zhiye-ui-verification';
const driver = resolve(root, process.env.CARGO_TARGET_DIR ?? 'target', 'debug', driverName);

const child = spawn(driver, [join(sandbox, 'workspaces'), join(sandbox, 'workspace.zhiye.zip')], {
  env: { ...process.env, ZHIYE_PREVIEW_SEED: '1' },
});
child.stderr.pipe(process.stderr);
child.on('exit', (code) => { console.error(`Core driver exited (${code})`); process.exit(1); });
const pending = new Map();
let requestId = 0;
createInterface({ input: child.stdout }).on('line', (line) => {
  const { id, result, error } = JSON.parse(line);
  const action = pending.get(id);
  pending.delete(id);
  if (error) action.reject(error); else action.resolve(result);
});
function core(cmd, args) {
  return new Promise((resolveCall, reject) => {
    const id = ++requestId;
    pending.set(id, { resolve: resolveCall, reject });
    child.stdin.write(JSON.stringify({ id, cmd, args }) + '\n');
  });
}

// In-memory stand-in for the sync commands.
const sync = { device: null, workspaces: new Map() };
function syncStatus(workspace) {
  const state = sync.workspaces.get(workspace) ?? { enabled: false, lastSuccessAt: null };
  return {
    enabled: state.enabled, serverUrl: state.enabled ? sync.device?.server ?? null : null,
    lastAttemptAt: state.lastSuccessAt, lastSuccessAt: state.lastSuccessAt, lastError: null,
    cursor: 0, tracked: state.enabled ? 4 : 0, unsent: state.enabled ? 0 : 4, pending: [],
    skipped: [],
  };
}
const report = () => ({ accepted: 4, applied: 0, conflicts: [], pending: [], cursor: 4 });
const simulated = {
  sync_overview: ({ workspace }) => ({ device: sync.device, status: syncStatus(workspace) }),
  register_sync_device: ({ server, name }) => {
    sync.device = { server: server.replace(/\/+$/, ''), deviceId: 'preview-device', deviceName: name, registeredAt: Date.now() };
    return sync.device;
  },
  forget_sync_device: () => { sync.device = null; return null; },
  enable_workspace_sync: ({ workspace }) => {
    sync.workspaces.set(workspace, { enabled: true, lastSuccessAt: Date.now() });
    return report();
  },
  disable_workspace_sync: ({ workspace }) => {
    sync.workspaces.set(workspace, { ...sync.workspaces.get(workspace), enabled: false });
    return simulated.sync_overview({ workspace });
  },
  sync_workspace_now: ({ workspace }) => {
    sync.workspaces.get(workspace).lastSuccessAt = Date.now();
    return report();
  },
  list_remote_workspaces: () => [{ id: 'preview-remote', name: '家里的资料', local: false }],
  local_api_status: () => ({ address: null }),
};

const bridge = `
const callbacks = new Map();
const subscriptions = new Map();
let next = 0;
window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (_event, id) => subscriptions.delete(id) };
window.isTauri = true;
window.__TAURI_INTERNALS__ = {
  metadata: { currentWindow: { label: 'main' } },
  transformCallback: (callback) => { callbacks.set(++next, callback); return next; },
  invoke: async (command, args) => {
    if (command === 'plugin:event|listen') { subscriptions.set(++next, args); return next; }
    if (command.startsWith('plugin:')) return null;
    if (command === 'take_location') return null;
    let response;
    try {
      response = await fetch('/__core', { method: 'POST', body: JSON.stringify({ cmd: command, args }) });
    } catch {
      throw new Error('界面预览服务已停止。请重新运行 node scripts/ui-preview.mjs 并刷新页面（预览数据会重新生成）。');
    }
    const body = await response.json();
    if ('error' in body) throw body.error;
    return command === 'preview_attachment' ? new Uint8Array(body.result).buffer : body.result;
  },
};`;

const server = await createServer({
  root: resolve(root, 'apps/desktop'),
  configFile: resolve(root, 'apps/desktop/vite.config.ts'),
  server: { host: '127.0.0.1', port, strictPort: true },
  plugins: [{
    name: 'zhiye-core-bridge',
    transformIndexHtml: () => [{ tag: 'script', children: bridge, injectTo: 'head-prepend' }],
    configureServer(dev) {
      dev.middlewares.use('/__core', (request, response) => {
        let body = '';
        request.on('data', (chunk) => { body += chunk; });
        request.on('end', async () => {
          const { cmd, args = {} } = JSON.parse(body);
          let payload;
          try {
            payload = { result: cmd in simulated ? simulated[cmd](args) : await core(cmd, args) };
          } catch (error) {
            payload = { error };
          }
          response.setHeader('Content-Type', 'application/json');
          response.end(JSON.stringify(payload ?? { result: null }));
        });
      });
    },
  }],
});
await server.listen();
console.log(`知页界面预览：http://127.0.0.1:${port}/  数据目录：${sandbox}`);
