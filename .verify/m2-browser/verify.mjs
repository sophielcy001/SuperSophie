import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { createServer } from 'node:http';
import { copyFile, cp, mkdir, mkdtemp, readdir, readFile, rename, rm, rmdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { extname, join, resolve, sep } from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { chromium } from 'playwright-core';
import { verifyKeyboard } from '../../scripts/verify-keyboard.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const reportDir = resolve(root, process.argv[2] ?? '.verify/m2-browser');
const webRoot = resolve(root, 'apps/desktop/dist');
const sandbox = await mkdtemp(join(tmpdir(), 'zhiye-m2-browser-'));
const backup = join(sandbox, 'workspace.zhiye.zip');
const imageFixture = join(root, '.verify/rich-blocks/fixtures/validation-image.png');
const driverName = process.platform === 'win32' ? 'zhiye-ui-verification.exe' : 'zhiye-ui-verification';
const driver = resolve(root, process.env.CARGO_TARGET_DIR ?? 'target', 'debug', driverName);
const pending = new Map();
const checks = [];
let requestId = 0;

function startBackend() {
  const child = spawn(driver, [join(sandbox, 'workspaces'), backup, imageFixture]);
  createInterface({ input: child.stdout }).on('line', (line) => {
    const { id, result, error } = JSON.parse(line);
    const action = pending.get(id);
    pending.delete(id);
    if (error) action.reject(error);
    else action.resolve(result);
  });
  child.on('exit', () => {
    for (const action of pending.values()) action.reject(new Error('Core driver stopped'));
    pending.clear();
  });
  child.stderr.pipe(process.stderr);
  return child;
}

let backend = startBackend();
let previewAddress = null;
async function stopBackend() {
  if (backend.exitCode !== null || backend.signalCode !== null) return;
  const exited = new Promise((resolve) => backend.once('exit', resolve));
  backend.stdin.end();
  await exited;
}

function invoke(cmd, args = {}) {
  if (cmd === 'set_local_api') previewAddress = args.enabled ? 'http://127.0.0.1:46832' : null;
  if (cmd === 'local_api_status' || cmd === 'set_local_api') return Promise.resolve({ address: previewAddress });
  return new Promise((resolve, reject) => {
    const id = ++requestId;
    pending.set(id, { resolve, reject });
    backend.stdin.write(JSON.stringify({ id, cmd, args }) + '\n');
  });
}

const types = { '.html': 'text/html', '.js': 'application/javascript', '.mjs': 'application/javascript',
  '.css': 'text/css', '.woff2': 'font/woff2' };
const server = createServer(async (request, response) => {
  const path = new URL(request.url, 'http://127.0.0.1').pathname;
  const file = resolve(webRoot, `.${path === '/' ? '/index.html' : path}`);
  if (!file.startsWith(webRoot + sep)) return response.writeHead(404).end();
  if (path === '/favicon.ico') return response.writeHead(204).end();
  try {
    const body = await readFile(file);
    response.writeHead(200, { 'content-type': types[extname(file)] ?? 'application/octet-stream' }).end(body);
  } catch {
    response.writeHead(404).end();
  }
}).listen(0, '127.0.0.1');
await new Promise((resolve, reject) => { server.once('listening', resolve); server.once('error', reject); });
const origin = `http://127.0.0.1:${server.address().port}`;

let browser;
const errors = [];
const external = [];
try {
  browser = await chromium.launch({
    headless: true,
    executablePath: process.env.ZHIYE_TEST_CHROMIUM
      ?? (process.platform === 'darwin' ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' : undefined),
  });
  const browserVersion = browser.version();
  const page = await browser.newPage({ viewport: { width: 1280, height: 860 } });
  page.setDefaultTimeout(8_000);
  page.on('pageerror', (error) => errors.push(error.message));
  await page.route('**/*', (route) => {
    if (new URL(route.request().url()).origin === origin) return route.continue();
    external.push(route.request().url());
    return route.abort();
  });
  await page.exposeFunction('invokeTestCore', invoke);
  await page.addInitScript((appOrigin) => {
    if (window !== window.top || location.origin !== appOrigin) return;
    const callbacks = new Map();
    const subscriptions = new Map();
    let next = 0;
    let pendingLocation = sessionStorage.getItem('test-start-location');
    sessionStorage.removeItem('test-start-location');
    window.testNativeMenuCommand = (command) => {
      for (const [id, subscription] of subscriptions) {
        if (subscription.event === 'native-command') {
          callbacks.get(subscription.handler)?.({ event: subscription.event, id, payload: command });
        }
      }
    };
    window.testQuitReplies = [];
    window.testRequestSystemQuit = async (request) => {
      for (const [id, subscription] of subscriptions) {
        if (subscription.event === 'system-quit-requested') {
          await callbacks.get(subscription.handler)?.({ event: subscription.event, id, payload: request });
        }
      }
    };
    window.testViewRecordReads = 0;
    window.testWorkspaceContentChanged = (workspace) => {
      for (const [id, subscription] of subscriptions) {
        if (subscription.event === 'workspace-content-changed') {
          callbacks.get(subscription.handler)?.({ event: subscription.event, id, payload: workspace });
        }
      }
    };
    window.testDeliverDesktopLocation = (url) => {
      pendingLocation = url;
      for (const [id, subscription] of subscriptions) {
        if (subscription.event === 'location-ready') callbacks.get(subscription.handler)?.({ event: subscription.event, id, payload: null });
      }
    };
    window.testRequestDesktopClose = async () => {
      for (const [id, subscription] of subscriptions) {
        if (subscription.event === 'tauri://close-requested') {
          await callbacks.get(subscription.handler)?.({ event: subscription.event, id, payload: null });
        }
      }
    };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (_event, id) => subscriptions.delete(id) };
    window.isTauri = true;
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' } },
      transformCallback: (callback) => { callbacks.set(++next, callback); return next; },
      invoke: async (command, args, options) => {
        if (command === 'plugin:event|listen') { subscriptions.set(++next, args); return next; }
        if (command.startsWith('plugin:event|')) return null;
        if (command === 'finish_macos_quit') {
          window.testQuitReplies.push(args);
          return null;
        }
        if (command === 'plugin:window|destroy') {
          window.testWindowDestroyed = true;
          return null;
        }
        if (command === 'take_location') {
          const url = pendingLocation;
          pendingLocation = null;
          return url;
        }
        if (command === 'attach_image_bytes') {
          const headers = options.headers;
          return window.invokeTestCore(command, {
            workspace: headers['x-zhiye-workspace'],
            page: headers['x-zhiye-page'],
            name: decodeURIComponent(headers['x-zhiye-name']),
            bytes: Array.from(new Uint8Array(args)),
          });
        }
        if (command === 'list_records') window.testViewRecordReads++;
        const result = await window.invokeTestCore(command, args);
        if (command === 'run_workspace_backup' && result) {
          for (const [id, subscription] of subscriptions) {
            if (subscription.event === 'backup-finished') callbacks.get(subscription.handler)?.({ event: subscription.event, id, payload: result });
          }
        }
        if (command === 'open_web_link') window.testOpenedWebLink = args.href;
        return command === 'preview_attachment' ? new Uint8Array(result).buffer : result;
      },
    };
  }, origin);

  const button = (name, scope = page) => scope.getByRole('button', { name, exact: true });
  const field = (name, scope = page) => scope.getByLabel(name, { exact: true });
  const dialog = () => page.getByRole('dialog');
  const body = () => page.getByRole('textbox', { name: '页面正文', exact: true });
  const count = (value) => page.getByText(`共 ${value} 条记录`, { exact: true }).waitFor();
  const table = () => page.getByRole('table');
  const save = () => page.keyboard.press('Control+s');
  const title = () => field('页面标题');
  const breadcrumb = () => page.getByRole('navigation', { name: '页面位置' });

  const initialSpace = (await invoke('list_workspaces')).workspaces[0];
  const initialDirectory = join(sandbox, 'workspaces', initialSpace.id);
  const parkedDirectory = join(sandbox, 'workspaces', 'unavailable-workspace');
  await rename(initialDirectory, parkedDirectory);
  try {
    await page.goto(origin);
    await button('切换工作空间').waitFor();
    assert.equal(await button('新建页面').isDisabled(), true);
    await page.evaluate(() => window.testNativeMenuCommand('backup'));
    await page.getByRole('dialog', { name: '备份与恢复' }).waitFor();
    assert.equal(await dialog().getByRole('button', { name: /^从备份恢复/ }).isEnabled(), true);
    assert.equal(await dialog().getByRole('button', { name: /^创建整库恢复包/ }).isDisabled(), true);
    await button('关闭', dialog()).click();
    checks.push('没有可打开的工作空间时，原生菜单仍可进入备份恢复');
  } finally {
    await rename(parkedDirectory, initialDirectory);
  }
  await page.goto(origin);
  await button('新建数据库').waitFor();
  await page.evaluate(() => window.testNativeMenuCommand('commands'));
  await page.getByRole('dialog', { name: '命令面板' }).waitFor();
  await page.evaluate(() => window.testNativeMenuCommand('new-page'));
  await page.getByRole('dialog', { name: '命令面板' }).waitFor();
  await page.keyboard.press('Escape');
  await page.evaluate(() => window.testNativeMenuCommand('search'));
  await page.getByRole('dialog', { name: '搜索页面' }).waitFor();
  await page.keyboard.press('Escape');
  checks.push('原生菜单打开搜索和命令面板，模态窗口保持当前操作');

  await button('新建数据库').click();
  await title().fill('M2 阅读清单');
  await save();
  await button('管理属性').click();
  await field('属性名称', dialog()).fill('评分');
  await field('属性类型', dialog()).selectOption('number');
  await button('创建属性', dialog()).click();
  await count(0);
  checks.push('新建数据库与数字属性');

  await button('新建记录').click();
  await title().fill('SQLite 事务');
  await body().fill('记录标题、正文和属性共用同一页。');
  await button('编辑属性').click();
  await field('状态', dialog()).selectOption({ label: '进行中' });
  await field('评分', dialog()).fill('0');
  await button('保存', dialog()).click();
  await button('M2 阅读清单', breadcrumb()).click();
  await button('SQLite 事务', table()).waitFor();
  await table().getByText('进行中', { exact: true }).waitFor();
  await table().getByText('0', { exact: true }).waitFor();
  checks.push('记录页编辑正文与属性，表格显示零值');

  await button('看板').click();
  await button('SQLite 事务', page.getByRole('region', { name: '进行中', exact: true })).waitFor();
  await button('编辑「SQLite 事务」的属性').click();
  await field('状态', dialog()).selectOption({ label: '已完成' });
  await field('评分', dialog()).fill('9');
  await button('保存', dialog()).click();
  await button('SQLite 事务', page.getByRole('region', { name: '已完成', exact: true })).waitFor();
  await page.screenshot({ path: join(reportDir, 'board.png') });
  await button('列表').click();
  const row = page.getByRole('listitem').filter({ has: button('SQLite 事务') });
  await row.getByText('9', { exact: true }).waitFor();
  await button('SQLite 事务', row).click();
  await body().getByText('记录标题、正文和属性共用同一页。', { exact: true }).waitFor();
  await page.screenshot({ path: join(reportDir, 'record.png') });
  checks.push('同一记录在表格、看板、列表与记录页一致');

  await button('M2 阅读清单', breadcrumb()).click();
  await button('新建记录').click();
  await title().fill('另一篇笔记');
  await button('M2 阅读清单', breadcrumb()).click();
  await button('表格').click();
  await count(2);
  await button('视图设置').click();
  await field('筛选字段', dialog()).selectOption('title');
  await field('筛选值', dialog()).fill('SQLite');
  await field('排序字段', dialog()).selectOption({ label: '评分' });
  await field('排序顺序', dialog()).selectOption('desc');
  await field('分组字段', dialog()).selectOption({ label: '状态' });
  await button('保存', dialog()).click();
  await count(1);
  await page.getByRole('heading', { name: '已完成', exact: true }).waitFor();
  await page.reload();
  await count(1);
  await button('SQLite 事务', table()).waitFor();
  await page.getByText('评分 降序', { exact: true }).waitFor();
  await button('列表').click();
  await count(2);
  checks.push('视图筛选、排序和分组各自保存，重开后设置有效');

  const source = (await invoke('list_workspaces')).workspaces[0];
  const [collection] = await invoke('list_collections', { workspace: source.id });
  const schema = await invoke('read_collection', { workspace: source.id, collection: collection.id });
  const parent = await invoke('create_page', { workspace: source.id, parentId: null });
  await invoke('save_page', { workspace: source.id, id: parent.id, revision: parent.revision,
    draft: { title: '检索验收目录', blocks: [] } });
  const target = await invoke('create_page', { workspace: source.id, parentId: parent.id });
  const block = (type, payload, children = []) => ({ id: randomUUID(), type, schemaVersion: 1, payload, children });
  const paragraph = (text) => block('paragraph', { inline: [{ type: 'text', text }] });
  const anchor = paragraph('蓝鲸游过海湾，混合 SQLite 事务和全文正文。');
  const list = block('bulletList', {}, [block('listItem', {}, [anchor])]);
  const code = block('codeBlock', { text: 'const sample = api::run("宽频");' });
  const longPage = [...Array.from({ length: 46 }, (_, i) => paragraph(`前言 ${i + 1}`)), list, code];
  const savedTarget = await invoke('save_page', { workspace: source.id, id: target.id, revision: target.revision,
    draft: { title: '全文索引样例', blocks: longPage } });
  await page.reload();

  const searchDialog = () => page.getByRole('dialog', { name: '搜索页面', exact: true });
  const searchInput = () => field('搜索关键词', searchDialog());
  async function typeQuery(term) {
    await button('搜索').click();
    await page.waitForFunction(() => document.activeElement?.getAttribute('aria-label') === '搜索关键词');
    await searchInput().fill(term);
  }
  const result = () => searchDialog().getByRole('button', { name: /全文索引样例/ });
  for (const term of ['鲸', '蓝鲸', '混合 sqlite']) {
    await typeQuery(term);
    await result().locator('mark').filter({ hasText: term === '混合 sqlite' ? '混合 SQLite' : term }).waitFor();
    await result().getByText('检索验收目录', { exact: true }).waitFor();
    if (term !== '混合 sqlite') await searchInput().press('Escape');
  }
  await page.screenshot({ path: join(reportDir, 'search-results.png') });
  await searchInput().press('Enter');
  const location = (block) => page.locator(`[data-id="${block.id}"][data-search-target="true"]`);
  await location(anchor).waitFor();
  await page.waitForFunction(() => document.activeElement?.matches('.document-body'));
  assert.ok(await page.locator('.document-scroll').evaluate((element) => element.scrollTop) > 300);
  const anchorRect = await location(anchor).boundingBox();
  assert.ok(anchorRect && anchorRect.y > 55 && anchorRect.y < 860, 'matching nested block is actually in the viewport');
  await page.screenshot({ path: join(reportDir, 'search-location.png') });
  await typeQuery('"宽频"');
  await result().locator('mark').filter({ hasText: '"宽频"' }).waitFor();
  await searchInput().press('Enter');
  await location(code).waitFor();
  assert.equal(await location(anchor).count(), 0);
  await typeQuery('全文索引样例');
  await result().locator('mark').waitFor();
  await searchInput().press('Enter');
  await searchDialog().waitFor({ state: 'hidden' });
  assert.equal(await location(code).count(), 0);
  assert.equal(await page.locator('.document-scroll').evaluate((element) => element.scrollTop), 0);
  assert.equal((await invoke('read_page', { workspace: source.id, id: target.id })).revision, savedTarget.revision);
  checks.push('中文单字、双字、中英混合和代码符号命中，路径和原文片段准确，键盘定位到嵌套块与代码块');

  await typeQuery('SQLite');
  await result().waitFor();
  await searchDialog().getByText('按数据库属性筛选', { exact: true }).click();
  await field('搜索范围', searchDialog()).selectOption({ label: 'M2 阅读清单' });
  await field('筛选字段', searchDialog()).selectOption({ label: '评分' });
  await field('筛选值', searchDialog()).fill('9');
  await button('应用筛选', searchDialog()).click();
  const matchingRecord = () => searchDialog().getByRole('button', { name: /SQLite 事务/ });
  await matchingRecord().waitFor();
  assert.equal(await result().count(), 0);
  await button('重建索引', searchDialog()).click();
  await matchingRecord().waitFor({ state: 'hidden' });
  await matchingRecord().waitFor();
  const reindexed = await invoke('search_text', { workspace: source.id, request: { query: 'SQLite' } });
  assert.equal(reindexed.index.pending, 0);
  assert.equal(reindexed.hits.length, 2);
  await page.screenshot({ path: join(reportDir, 'search-filter.png') });
  await searchInput().press('Escape');
  checks.push('工作空间属性筛选和重建入口，重建后自动恢复匹配结果');

  await button('备份与恢复').click();
  assert.equal(await dialog().getByRole('checkbox', { name: '内容包包含修订历史' }).isChecked(), true);
  await dialog().getByRole('button', { name: /^导出工作空间/ }).click();
  await page.getByText('工作空间包已导出。', { exact: true }).waitFor();
  await button('备份与恢复').click();
  await dialog().getByRole('button', { name: /^从备份恢复/ }).click();
  await page.getByText('已恢复到新的工作空间。', { exact: true }).waitFor();
  assert.equal(await title().inputValue(), '全文索引样例');
  await button('M2 阅读清单', page.getByRole('navigation', { name: '页面树' })).click();
  await count(1);
  const restored = (await invoke('list_workspaces')).workspaces.find((space) => space.id !== source.id);
  assert.ok(restored);
  assert.deepEqual(await invoke('read_collection', { workspace: restored.id, collection: collection.id }), schema);
  const originalPages = await invoke('list_pages', { workspace: source.id });
  assert.deepEqual(await invoke('list_pages', { workspace: restored.id }), originalPages);
  const restoredRecord = originalPages.find((item) => item.title === 'SQLite 事务');
  assert.ok(restoredRecord);
  const restoredPage = await invoke('read_page', { workspace: restored.id, id: restoredRecord.id });
  assert.match(JSON.stringify(restoredPage.blocks), /记录标题、正文和属性共用同一页/);
  const score = schema.properties.find((field) => field.name === '评分');
  assert.equal(restoredPage.properties[score.id], 9);
  assert.ok((await invoke('page_history', { workspace: restored.id, id: restoredRecord.id })).length > 1);
  checks.push('备份与恢复入口连通真实 Core，恢复后页面和视图保留身份');

  await button('快速收集').click();
  await title().fill('版本收集样例');
  await body().fill('收集正文');
  await save();
  await button('收藏页面').click();
  await button('收藏 版本收集样例').waitFor();
  await button('打开收集箱').click();
  await page.waitForFunction(() => document.querySelector('[aria-label="页面标题"]')?.value === '收集箱');
  await button('收藏 版本收集样例').click();
  await body().getByText('收集正文', { exact: true }).waitFor();
  await page.locator('summary').filter({ hasText: '最近访问' }).click();
  await button('最近访问 版本收集样例').waitFor();
  const captured = (await invoke('list_pages', { workspace: restored.id })).find((item) => item.title === '版本收集样例');
  const shortcuts = await invoke('read_navigation', { workspace: restored.id });
  assert.equal(captured.parentId, shortcuts.inboxPageId);
  assert.ok(shortcuts.favorites.includes(captured.id) && shortcuts.recentPages.includes(captured.id));
  checks.push('快速收集使用普通页面，收藏和最近访问从 SQLite 读取');

  const pageAction = async (name) => {
    await button('页面操作').click();
    await button(name).click();
  };
  const reportDialog = () => page.getByRole('dialog', { name: '导入报告' });
  await button('导入资料').click();
  await button('选择 Markdown 文件', dialog()).click();
  await reportDialog().getByText('2 个文件已导入，0 个重复，1 个失败。').waitFor();
  await reportDialog().getByText(/图片引用已保留/).waitFor();
  await button('保存报告', reportDialog()).click();
  await page.locator('.app-shell[aria-busy="false"]').waitFor();
  assert.match(await readFile(join(sandbox, 'import-report.txt'), 'utf8'), /非UTF8\.md · 失败/);
  await page.screenshot({ path: join(reportDir, 'import-report.png') });
  await button('打开 导入首页.md', reportDialog()).click();
  await body().getByRole('link', { name: '↗ 下一页' }).click();
  await body().getByText('中文导入正文', { exact: true }).waitFor();
  await button('附件').click();
  await dialog().getByText('下一页.md', { exact: true }).waitFor();
  await button('关闭', dialog()).click();
  await button('导入资料').click();
  await button('选择 Markdown 文件', dialog()).click();
  await reportDialog().getByText('0 个文件已导入，2 个重复，1 个失败。').waitFor();
  await button('打开 下一页.md', reportDialog()).click();
  checks.push('Markdown 批量导入、原件、损失报告保存、重复识别与页间链接');

  await pageAction('保存为页面模板');
  await field('模板名称', dialog()).fill('可复用的笔记');
  await button('保存模板', dialog()).click();
  await page.getByText('模板已保存。', { exact: true }).waitFor();
  await button('模板').click();
  await field('新页面标题', dialog()).fill('新笔记样例');
  await page.screenshot({ path: join(reportDir, 'template.png') });
  await button('创建页面', dialog()).click();
  await body().getByText('中文导入正文', { exact: true }).waitFor();
  const withTemplate = await invoke('list_pages', { workspace: restored.id });
  const newPage = withTemplate.find((item) => item.title === '新笔记样例');
  const templateSource = withTemplate.find((item) => item.title === '下一页');
  assert.ok(newPage && newPage.id !== templateSource.id);
  assert.notEqual((await invoke('read_page', { workspace: restored.id, id: newPage.id })).blocks[0].id,
    (await invoke('read_page', { workspace: restored.id, id: templateSource.id })).blocks[0].id);
  checks.push('从页面保存模板并创建独立副本');

  await button('导入资料').click();
  await field('文件格式', dialog()).selectOption('csv');
  await button('选择 CSV 文件', dialog()).click();
  await reportDialog().getByText('已导入 · 2 条记录').waitFor();
  await reportDialog().getByText(/预期 3 列/).waitFor();
  await button('打开 导入清单.csv', reportDialog()).click();
  await count(2);
  await table().getByText('001', { exact: true }).waitFor();
  await pageAction('设为收集箱');
  await page.getByText('已设为收集箱。', { exact: true }).waitFor();
  assert.equal((await invoke('read_navigation', { workspace: restored.id })).inboxPageId,
    (await invoke('list_pages', { workspace: restored.id })).find((item) => item.title === '导入清单').id);
  await button('M2 阅读清单').click();
  await pageAction('导入 CSV');
  await field('写入方式', dialog()).selectOption('existing');
  await button('选择 CSV 文件', dialog()).click();
  await reportDialog().getByText(/外部备注.*没有可写入/).waitFor();
  await reportDialog().getByText(/评分.*无法转换/).waitFor();
  await button('打开 追加数据.csv', reportDialog()).click();
  await button('列表').click();
  await count(4);
  await page.getByRole('listitem').filter({ has: button('架构评审') }).getByText('2.5', { exact: true }).waitFor();
  checks.push('CSV 新建与追加模式、类型转换、无效行和失配报告');

  await button('架构评审', page.getByRole('listitem')).click();
  await pageAction('保存为记录模板');
  await field('模板名称', dialog()).fill('架构评审模板');
  await button('保存模板', dialog()).click();
  await page.getByText('模板已保存。', { exact: true }).waitFor();
  await button('M2 阅读清单', breadcrumb()).click();
  await pageAction('从模板新建记录');
  await field('新页面标题', dialog()).fill('下一次评审');
  await button('创建记录', dialog()).click();
  await button('编辑属性').click();
  assert.equal(await field('评分', dialog()).inputValue(), '2.5');
  await button('取消', dialog()).click();
  checks.push('数据库记录模板复制属性，应用时自动归属目标数据库');

  await button('新建页面').click();
  await title().fill('图文验收页');
  await save();
  const insert = async (name) => {
    await button('插入').click();
    await button(name, page.locator('.insert-menu')).click();
  };
  const docTables = () => body().locator('table');
  const trailingParagraph = () => body().locator(':scope > p').last();
  const placeAfterBlocks = async () => {
    await trailingParagraph().click({ position: { x: 6, y: 8 } });
    await page.waitForFunction(() => {
      const editor = document.querySelector('.document-body')?.editor;
      const anchor = document.getSelection()?.anchorNode;
      const element = anchor instanceof Element ? anchor : anchor?.parentElement;
      return !!element?.closest('.document-body > p') && editor?.state.selection.$from.depth === 1;
    });
  };
  await insert('表格');
  const editedTable = docTables().first();
  await editedTable.getByRole('columnheader').first().click();
  await page.keyboard.insertText('海湾观测');
  await button('下方加行').click();
  await button('右侧加列').click();
  assert.equal(await editedTable.locator('tr').count(), 4);
  assert.equal(await editedTable.locator('tr').first().locator('td,th').count(), 4);
  await editedTable.locator('td').last().click();
  await page.keyboard.insertText('北岸观测点');
  await save();
  await button('撤销').click();
  await editedTable.getByText('北岸观测点').waitFor({ state: 'hidden' });
  await button('重做').click();
  await editedTable.getByText('北岸观测点').waitFor();
  await placeAfterBlocks();
  async function pasteData(mime, text) {
    await body().evaluate((element, { mime, text }) => {
      const clipboardData = new DataTransfer();
      clipboardData.setData(mime, text);
      element.dispatchEvent(new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData }));
    }, { mime, text });
  }
  await pasteData('text/html', '<table><tbody><tr><th colspan="2">跨列标题</th></tr><tr><td>东</td><td>西</td></tr></tbody></table>');
  await docTables().nth(1).getByRole('columnheader', { name: '跨列标题' }).waitFor();
  assert.equal(await docTables().nth(1).locator('th').getAttribute('colspan'), '2');
  await placeAfterBlocks();
  await pasteData('text/plain', '日期\t温度\n周一\t22');
  await docTables().nth(2).getByText('周一', { exact: true }).waitFor();
  await docTables().nth(2).getByText('22', { exact: true }).click();
  await pasteData('text/plain', '24\t25');
  await docTables().nth(2).getByText('25', { exact: true }).waitFor();
  assert.equal(await docTables().count(), 3);
  checks.push('正文表格可改行列，支持中文内容、撤销重做、HTML 合并单元格和制表符数据粘贴');

  await placeAfterBlocks();
  await button('网页链接').click();
  await field('网址', dialog()).fill('javascript:1');
  await button('保存链接', dialog()).click();
  await dialog().getByRole('alert').waitFor();
  await field('网址', dialog()).fill('https://example.test/海湾');
  await field('显示文字', dialog()).fill('观测来源');
  await button('保存链接', dialog()).click();
  const webLink = body().getByRole('link', { name: '观测来源', exact: true });
  assert.equal(await webLink.getAttribute('href'), 'https://example.test/%E6%B5%B7%E6%B9%BE');
  await webLink.click({ modifiers: ['Meta'] });
  await page.waitForFunction(() => window.testOpenedWebLink?.startsWith('https://example.test/'));
  assert.equal(await readFile(join(sandbox, 'link-opened.txt'), 'utf8'), 'https://example.test/%E6%B5%B7%E6%B9%BE');
  checks.push('网页链接可编辑，危险协议被拒绝，修饰键点击交给桌面命令');

  await placeAfterBlocks();
  await insert('图片');
  const image = (alt) => body().getByRole('img', { name: alt, exact: true });
  const readyImage = async (alt) => {
    await image(alt).waitFor();
    await page.waitForFunction((label) => Array.from(document.images).some((item) =>
      item.alt === label && item.complete && item.naturalWidth === 480), alt);
  };
  await readyImage('海湾');
  await image('海湾').click();
  await button('图片说明').click();
  await field('说明文字', dialog()).fill('蓝鲸图像检索');
  await button('保存说明', dialog()).click();
  await readyImage('蓝鲸图像检索');
  const bytes = [...await readFile(imageFixture)];
  async function inputImage(name, drop) {
    await placeAfterBlocks();
    await body().evaluate((element, { name, bytes, drop }) => {
      const file = new File([new Uint8Array(bytes)], name, { type: 'image/png' });
      const transfer = new DataTransfer();
      transfer.items.add(file);
      const rect = element.lastElementChild.getBoundingClientRect();
      element.dispatchEvent(drop
        ? new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: transfer, clientX: rect.left + 3, clientY: rect.top + 4 })
        : new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData: transfer }));
    }, { name, bytes, drop });
  }
  await inputImage('剪贴板图.png', false);
  await readyImage('剪贴板图');
  await inputImage('拖放图.png', true);
  await readyImage('拖放图');
  await save();
  const richPage = (await invoke('list_pages', { workspace: restored.id })).find((item) => item.title === '图文验收页');
  assert.ok(richPage);
  const richContent = await invoke('read_page', { workspace: restored.id, id: richPage.id });
  const savedImages = richContent.blocks.filter((block) => block.type === 'attachmentImage');
  assert.equal(savedImages.length, 3);
  assert.equal(new Set(savedImages.map((block) => block.id)).size, 3);
  const assets = await invoke('list_attachments', { workspace: restored.id, page: richPage.id });
  assert.equal(assets.length, 3);
  assert.equal(new Set(assets.map((asset) => asset.sha256)).size, 1);
  await page.screenshot({ path: join(reportDir, 'image-editor.png') });
  checks.push('本地选择、剪贴板和拖放插图均使用附件，图片可显示、编辑说明并复用原件');

  await pageAction('导出 Markdown');
  await page.getByRole('status').filter({ hasText: '导出报告位于同目录的「images」文件夹。' }).waitFor();
  const markdown = await readFile(join(sandbox, 'page.md'), 'utf8');
  const imageReport = await readFile(join(sandbox, 'images', '导出报告.txt'), 'utf8');
  assert.match(markdown, /海湾观测.*\|/);
  assert.match(markdown, /\[观测来源\]\(<https:\/\/example\.test\//);
  for (const asset of assets) {
    assert.ok(markdown.includes(`images/${asset.id}.png`));
    assert.ok(imageReport.split('\n').some((line) => line.includes(`images/${asset.id}.png`) && line.includes(asset.sha256)));
    assert.deepEqual(await readFile(join(sandbox, 'images', `${asset.id}.png`)), Buffer.from(bytes));
  }
  await pageAction('导出 HTML');
  await page.getByRole('status').filter({ hasText: 'HTML 已导出。' }).waitFor();
  const relocated = await mkdtemp(join(sandbox, 'html-relocated-'));
  await copyFile(join(sandbox, 'page.html'), join(relocated, '图文文件.html'));
  await cp(join(sandbox, 'images'), join(relocated, 'images'), { recursive: true });
  const htmlReport = await readFile(join(relocated, 'images', '导出报告.txt'), 'utf8');
  assert.match(htmlReport, /^知页 HTML 导出报告/);
  assert.match(htmlReport, /复杂表格转为普通行列/);
  for (const asset of assets) {
    assert.ok(htmlReport.split('\n').some((line) => line.includes(asset.id) && line.includes(asset.sha256)));
  }
  const htmlViewer = await browser.newPage({ offline: true, viewport: { width: 960, height: 860 } });
  try {
    await htmlViewer.route(/^https?:/, (route) => {
      external.push(route.request().url());
      return route.abort();
    });
    await htmlViewer.goto(pathToFileURL(join(relocated, '图文文件.html')).href);
    await htmlViewer.getByRole('heading', { name: '图文验收页', level: 1 }).waitFor();
    await htmlViewer.locator('table').getByText('海湾观测', { exact: true }).waitFor();
    assert.equal(await htmlViewer.getByText(/表格内容已转为普通 Markdown 行列/).count(), 0);
    const image = htmlViewer.getByRole('img', { name: '蓝鲸图像检索', exact: true });
    assert.ok(await image.evaluate((result) => result.decode().then(() => result.naturalWidth)));
    assert.match(await htmlViewer.getByRole('link', { name: '观测来源', exact: true }).getAttribute('href'), /^https:\/\/example\.test\//);
    await htmlViewer.screenshot({ path: join(reportDir, 'html-export.png') });
  } finally {
    await htmlViewer.close();
  }
  checks.push('从桌面导出 HTML 和报告，移动配套目录后离线打开页面、表格、链接和本地原图');

  await page.reload();
  await button('切换工作空间').click();
  await button(restored.name).click();
  await typeQuery('蓝鲸图像检索');
  await searchDialog().getByRole('button', { name: /图文验收页/ }).click();
  const searchedImage = savedImages.find((block) => block.payload.alt === '蓝鲸图像检索');
  await location(searchedImage).waitFor();
  await readyImage('蓝鲸图像检索');
  await typeQuery('海湾观测');
  await searchDialog().getByRole('button', { name: /图文验收页/ }).click();
  const savedTable = richContent.blocks.find((block) => block.type === 'table');
  await location(savedTable.children[0].children[0].children[0]).waitFor();
  await page.screenshot({ path: join(reportDir, 'table-editor.png') });
  checks.push('保存重开后按说明定位图片，按单元格内容定位表格，Markdown 与图片原件同步导出');

  const createNamed = async (name) => {
    await button('新建页面').click();
    await title().fill(name);
    await save();
    await page.getByRole('navigation', { name: '页面树' }).getByRole('button', { name, exact: true }).waitFor();
  };
  await createNamed('待整理第一支');
  await pageAction('新建子页面');
  await title().fill('保留层级子页');
  await save();
  await createNamed('待整理第二支');
  await createNamed('批量目标目录');
  const batchBar = () => page.getByRole('group', { name: '批量整理操作' });
  const check = (name, scope = page) => scope.getByRole('checkbox', { name: `选择「${name}」`, exact: true });
  const chosenTitles = ['待整理第一支', '待整理第二支', '保留层级子页'];
  const beforeOrganization = await invoke('list_pages', { workspace: restored.id });
  const initialBranchRevisions = new Map(beforeOrganization.map((item) => [item.id, item.revision]));
  async function chooseBranches() {
    await button('批量整理页面').click();
    for (const name of chosenTitles) await check(name).check();
    await batchBar().getByText('已选 3 项').waitFor();
  }
  await chooseBranches();
  await button('批量移动', batchBar()).click();
  const destinations = await field('目标位置', dialog()).locator('option').allTextContents();
  assert.ok(!destinations.some((label) => chosenTitles.some((title) => label.includes(title))));
  await field('目标位置', dialog()).selectOption({ label: '批量目标目录' });
  await button('移动', dialog()).click();
  await page.getByText('已移动 2 个页面分支。', { exact: true }).waitFor();
  const reorganized = await invoke('list_pages', { workspace: restored.id });
  const found = (title) => reorganized.find((item) => item.title === title);
  const movedParent = found('待整理第一支');
  const movedChild = found('保留层级子页');
  const movedSibling = found('待整理第二支');
  assert.equal(movedChild.parentId, movedParent.id);
  assert.equal(movedParent.parentId, found('批量目标目录').id);
  assert.equal(movedSibling.parentId, movedParent.parentId);
  assert.equal(movedChild.revision, initialBranchRevisions.get(movedChild.id));
  assert.equal(movedParent.revision, initialBranchRevisions.get(movedParent.id) + 1);
  assert.equal(movedSibling.revision, initialBranchRevisions.get(movedSibling.id) + 1);
  const untouched = await invoke('read_page', { workspace: restored.id, id: found('批量目标目录').id });
  assert.equal(untouched.revision, initialBranchRevisions.get(untouched.id));
  assert.deepEqual(untouched.blocks, []);

  await button('保留层级子页').click();
  await chooseBranches();
  await body().fill('批量操作前的正文仍会提交');
  await button('批量归档', batchBar()).click();
  await button('移入回收站', dialog()).click();
  await page.getByText('已将 3 个页面移入回收站。', { exact: true }).waitFor();
  assert.equal(await button('待整理第一支').count(), 0);
  await button('回收站').click();
  await check('待整理第一支', dialog()).check();
  await check('待整理第二支', dialog()).check();
  assert.equal(await check('保留层级子页', dialog()).count(), 0);
  await button('恢复所选', dialog()).click();
  await page.getByText('已恢复 3 个页面。', { exact: true }).waitFor();
  const restoredChild = await invoke('read_page', { workspace: restored.id, id: movedChild.id });
  assert.equal(restoredChild.parentId, movedParent.id);
  assert.match(JSON.stringify(restoredChild.blocks), /批量操作前的正文仍会提交/);
  assert.equal(restoredChild.trashRootId, null);
  checks.push('页面树多选移动折叠重叠分支，归档前提交草稿，回收恢复保留层级与 ID');

  const menuRecord = await invoke('read_record', { workspace: restored.id, collection: collection.id, id: restoredRecord.id });
  const assetResult = await invoke('pick_images', { workspace: restored.id, page: menuRecord.id });
  const csvAsset = assetResult.added[0].attachment;
  const savedSchema = await invoke('save_property', { workspace: restored.id, collection: collection.id,
    revision: schema.schemaRevision, draft: { id: null, name: '配图', type: 'attachment', options: [] } });
  const assetField = savedSchema.properties.find((item) => item.name === '配图');
  await invoke('save_record', { workspace: restored.id, collection: collection.id, id: menuRecord.id, revision: menuRecord.revision,
    draft: { title: menuRecord.title, schemaRevision: savedSchema.schemaRevision, properties: {
      ...menuRecord.properties, [assetField.id]: [csvAsset.id],
    } } });
  await body().waitFor();
  await save();
  await button('M2 阅读清单').click();
  for (const moved of [movedParent, movedSibling]) {
    const after = await invoke('read_page', { workspace: restored.id, id: moved.id });
    assert.equal(after.revision, moved.revision + 2);
    assert.deepEqual(after.blocks, []);
  }
  await button('表格').click();
  await count(1);
  await button('导出 CSV').click();
  await field('导出范围', dialog()).selectOption('view');
  await button('选择保存位置', dialog()).click();
  await dialog().getByText('已导出 1 条记录和 1 份附件原件。').waitFor();
  const filteredCsv = await readFile(join(sandbox, 'records.csv'), 'utf8');
  assert.match(filteredCsv, /^\uFEFF标题,状态,评分,配图\r\n/);
  assert.match(filteredCsv, /SQLite 事务,已完成,9/);
  assert.doesNotMatch(filteredCsv, /架构评审|下一次评审/);
  assert.deepEqual(await readFile(join(sandbox, 'csv-files', `${csvAsset.id}.png`)), Buffer.from(bytes));
  await page.screenshot({ path: join(reportDir, 'csv-export.png') });
  await button('完成', dialog()).click();
  await button('导出 CSV').click();
  await button('选择保存位置', dialog()).click();
  await dialog().getByText('已导出 5 条记录和 1 份附件原件。').waitFor();
  assert.match(await readFile(join(sandbox, 'records.csv'), 'utf8'), /下一次评审/);
  await button('完成', dialog()).click();
  checks.push('CSV 导出可选择视图或完整数据库，保持筛选、数值和附件原件');

  await button('列表').click();
  await count(5);
  await button('批量选择').click();
  await check('架构评审').check();
  await button('看板').click();
  await check('下一次评审').check();
  assert.equal(await check('架构评审').isChecked(), true);
  assert.equal(await button('批量移动', batchBar()).isDisabled(), true);
  await page.screenshot({ path: join(reportDir, 'batch-records.png') });
  await button('批量归档', batchBar()).click();
  await button('移入回收站', dialog()).click();
  await count(3);
  await button('回收站').click();
  await check('架构评审', dialog()).check();
  await check('下一次评审', dialog()).check();
  await button('恢复所选', dialog()).click();
  await page.getByText('已恢复 2 个页面。', { exact: true }).waitFor();
  const afterRestore = await invoke('list_pages', { workspace: restored.id });
  assert.equal(afterRestore.filter((item) => item.parentId === collection.containerPageId && !item.trashRootId).length, 5);
  checks.push('跨列表与看板选择记录，批量归档和恢复保留数据库归属');

  await page.setViewportSize({ width: 1280, height: 980 });
  const newDocument = async (name, makeBlocks) => {
    const created = await invoke('create_page', { workspace: restored.id });
    const blocks = await makeBlocks(created.id);
    await invoke('save_page', { workspace: restored.id, id: created.id, revision: created.revision,
      draft: { title: name, blocks } });
    return invoke('read_page', { workspace: restored.id, id: created.id });
  };
  const leadBlock = block('paragraph', { inline: [
    { type: 'text', text: '蓝潮段落先保留', marks: [{ type: 'bold' }] },
    { type: 'pageReference', pageId: menuRecord.id, label: 'SQLite 事务' },
  ] });
  const tableBlock = block('table', {}, [block('tableRow', {}, [
    block('tableHeader', {}, [paragraph('仍是原表格')]),
  ])]);
  const taskBlock = block('taskList', {}, [
    block('taskItem', { checked: false }, [paragraph('待办甲')]),
    block('taskItem', { checked: true }, [paragraph('待办乙')]),
    block('taskItem', { checked: false }, [paragraph('待办丙')]),
  ]);
  const divider = block('horizontalRule', {});
  const future = { ...block('futureCanvas', { shape: 'hexagon' }), schemaVersion: 3, plugin: { version: 8 } };
  const moveDocument = await newDocument('正文块移动验收', async (id) => {
    const file = (await invoke('pick_images', { workspace: restored.id, page: id })).added[0].attachment;
    const image = block('attachmentImage', { attachmentId: file.id, mediaType: file.mediaType, alt: '保持原件' });
    return [leadBlock, tableBlock, taskBlock, image, divider, future];
  });
  const longDocument = await newDocument('长文拖动验收', async () =>
    Array.from({ length: 44 }, (_, index) => paragraph(`远距离段落 ${index + 1}`)));
  await page.reload();
  await button('切换工作空间').click();
  await button(restored.name).click();
  const treeButton = (name) => button(name, page.getByRole('navigation', { name: '页面树' }));
  await treeButton(moveDocument.title).click();
  await body().getByText('蓝潮段落先保留').waitFor();

  const locateBlock = ({ id }) => body().locator(`[data-id="${id}"]`);
  const dropLine = () => page.locator('.block-drop-line');
  const boundsFor = async (locator) => {
    const bounds = await locator.boundingBox();
    assert.ok(bounds, `${locator} must have a rendered box`);
    return bounds;
  };
  async function beginDrag(source, side = 'outer') {
    await locateBlock(source).hover();
    const grip = page.locator(`.block-drag-${side}`);
    await grip.waitFor({ state: 'visible' });
    const box = await boundsFor(grip);
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
  }
  async function pointAt(target, fraction) {
    const box = await boundsFor(locateBlock(target));
    await page.mouse.move(box.x + box.width / 2, box.y + box.height * fraction, { steps: 12 });
  }
  async function placeCaret(block) {
    await locateBlock(block).click({ position: { x: 9, y: 9 } });
    await page.waitForFunction((id) => document.querySelector('.document-body')
      ?.editor?.state.selection.$head.parent.attrs.id === id, block.id);
  }
  async function savedDocument(doc) {
    await save();
    await page.locator('.save-indicator.saved').waitFor();
    return invoke('read_page', { workspace: restored.id, id: doc.id });
  }

  const [initialLead, initialTable, initialTasks, initialImage, initialDivider, initialFuture] = moveDocument.blocks;
  await beginDrag(initialLead);
  await pointAt(initialImage, 0.8);
  await dropLine().waitFor();
  await page.screenshot({ path: join(reportDir, 'block-drop-line.png') });
  await page.mouse.up();
  const rootMoved = [initialTable, initialTasks, initialImage, initialLead, initialDivider, initialFuture];
  assert.deepEqual((await savedDocument(moveDocument)).blocks, rootMoved);

  await locateBlock(initialLead).click({ position: { x: 15, y: 9 } });
  await page.keyboard.insertText('新字');
  assert.match(JSON.stringify((await savedDocument(moveDocument)).blocks[3]), /新字/);
  await button('撤销').click();
  assert.deepEqual((await savedDocument(moveDocument)).blocks, rootMoved);
  await button('撤销').click();
  assert.deepEqual((await savedDocument(moveDocument)).blocks, moveDocument.blocks);
  await button('重做').click();
  assert.deepEqual((await savedDocument(moveDocument)).blocks, rootMoved);
  checks.push('指针拖动整块，保存保留图片附件、表格、未知块与中文标记，移动和后续输入可分别撤销');

  const [itemA, itemB, itemC] = initialTasks.children;
  await placeCaret(itemB.children[0]);
  await page.keyboard.press('Alt+Shift+ArrowUp');
  const keyboardTasks = { ...initialTasks, children: [itemB, itemA, itemC] };
  assert.deepEqual((await savedDocument(moveDocument)).blocks, [initialTable, keyboardTasks, ...rootMoved.slice(2)]);
  await beginDrag(itemC.children[0], 'inner');
  await pointAt(itemB, 0.2);
  await dropLine().waitFor();
  await page.mouse.up();
  const draggedTasks = { ...initialTasks, children: [itemC, itemB, itemA] };
  assert.deepEqual((await savedDocument(moveDocument)).blocks, [initialTable, draggedTasks, ...rootMoved.slice(2)]);
  await placeCaret(initialTable.children[0].children[0].children[0]);
  await button('下移块').click();
  const sortedMixed = [draggedTasks, initialTable, ...rootMoved.slice(2)];
  const beforeCancel = await savedDocument(moveDocument);
  assert.deepEqual(beforeCancel.blocks, sortedMixed);
  await beginDrag(initialFuture);
  await pointAt(initialDivider, 0.2);
  await dropLine().waitFor();
  await page.keyboard.press('Escape');
  await page.mouse.up();
  await dropLine().waitFor({ state: 'hidden' });
  assert.deepEqual(await savedDocument(moveDocument), beforeCancel);
  checks.push('列表项可用键盘和右侧手柄同级移动，表格整体保留单元格，取消拖动不会保存新修订');

  await page.evaluate(() => {
    const adapter = window.__TAURI_INTERNALS__;
    const invoke = adapter.invoke;
    adapter.invoke = async (...args) => {
      if (args[0] !== 'save_page') return invoke(...args);
      adapter.invoke = invoke;
      throw { code: 'IO', message: '合成写入失败' };
    };
  });
  await beginDrag(initialDivider);
  await pointAt(initialFuture, 0.81);
  await dropLine().waitFor();
  await page.mouse.up();
  await save();
  await body().locator('hr').waitFor();
  await page.getByText('合成写入失败', { exact: true }).waitFor();
  assert.deepEqual((await invoke('read_page', { workspace: restored.id, id: moveDocument.id })).blocks, sortedMixed);
  await button('导出草稿').click();
  await page.getByText('草稿已导出。', { exact: true }).waitFor();
  const draftFile = JSON.parse(await readFile(join(sandbox, 'editor-draft.json'), 'utf8'));
  assert.equal(draftFile.baseRevision, beforeCancel.revision);
  assert.equal(draftFile.pageId, moveDocument.id);
  assert.deepEqual(draftFile.draft.blocks.map(({ id }) => id), [
    ...sortedMixed.slice(0, -2), initialFuture, initialDivider,
  ].map(({ id }) => id));
  await button('重试').click();
  const finalMixed = [...sortedMixed.slice(0, -2), initialFuture, initialDivider];
  assert.deepEqual((await savedDocument(moveDocument)).blocks, finalMixed);
  await treeButton(longDocument.title).click();
  await locateBlock(longDocument.blocks[0]).waitFor();
  await treeButton(moveDocument.title).click();
  await locateBlock(initialLead).waitFor();
  const domIds = await body().evaluate((node) => Array.from(node.children).flatMap((child) => {
    const id = child.getAttribute('data-id') ?? child.querySelector('table[data-id]')?.getAttribute('data-id');
    return id ? [id] : [];
  }));
  assert.deepEqual(domIds.slice(0, finalMixed.length), finalMixed.map(({ id }) => id));
  await typeQuery('蓝潮段落');
  await searchDialog().getByRole('button', { name: /正文块移动验收/ }).click();
  await location(initialLead).waitFor();
  checks.push('保存失败时导出结构草稿与基准修订，重试后重开保留顺序及全文定位');

  await placeCaret(initialLead);
  const ime = await page.context().newCDPSession(page);
  try {
    await ime.send('Input.imeSetComposition', { text: '组合输入', selectionStart: 4, selectionEnd: 4 });
    await page.waitForFunction(() => document.querySelector('.document-body')?.editor.view.composing);
    await page.keyboard.press('Alt+Shift+ArrowUp');
    await ime.send('Input.insertText', { text: '中文提交' });
  } finally {
    await ime.detach();
  }
  const afterComposition = await savedDocument(moveDocument);
  assert.deepEqual(afterComposition.blocks.map(({ id }) => id), finalMixed.map(({ id }) => id));
  const composedText = afterComposition.blocks.find(({ id }) => id === initialLead.id).payload.inline
    .filter((part) => part.type === 'text').map((part) => part.text).join('');
  assert.equal(composedText.match(/中文提交/g)?.length, 1);
  assert.doesNotMatch(composedText, /组合输入/);
  checks.push('Chromium 合成输入期间传入移动快捷键，提交的中文只写入一次，块顺序保持');

  await treeButton(longDocument.title).click();
  await beginDrag(longDocument.blocks[0]);
  const viewport = await boundsFor(page.locator('.document-scroll'));
  await page.mouse.move(viewport.x + viewport.width / 2, viewport.y + viewport.height - 8, { steps: 14 });
  const distant = longDocument.blocks[26];
  await page.waitForFunction((id) => {
    const scroll = document.querySelector('.document-scroll');
    const target = scroll?.querySelector(`[data-id="${CSS.escape(id)}"]`);
    if (!target || scroll.scrollTop <= 480) return false;
    const bounds = scroll.getBoundingClientRect();
    const row = target.getBoundingClientRect();
    const margin = bounds.height / 4;
    return row.top > bounds.top + margin && row.bottom < bounds.bottom - margin;
  }, distant.id);
  await page.mouse.move(viewport.x + viewport.width / 2, viewport.y + viewport.height / 2);
  await pointAt(distant, 0.85);
  await dropLine().waitFor();
  await page.mouse.up();
  assert.deepEqual((await savedDocument(longDocument)).blocks, [
    ...longDocument.blocks.slice(1, 27), longDocument.blocks[0], ...longDocument.blocks.slice(27),
  ]);
  checks.push('长文拖动到窗口边缘会继续滚动，跨越多屏后保存仍保留原块 ID');

  const scaleFixture = await invoke('verify_seed_thousand_pages');
  assert.equal(scaleFixture.pageCount, 1000);
  const scaleSpace = scaleFixture.workspace;
  const scalePage = scaleFixture.target;
  const scaleTree = page.getByRole('navigation', { name: '页面树' });
  const switchToScale = async () => {
    await button('切换工作空间').click();
    await button(scaleSpace.name).click();
    await button('目录 01', scaleTree).waitFor();
  };
  await page.reload();
  await switchToScale();
  assert.equal(await scaleTree.locator('.tree-link').count(), 1000);
  await button('折叠 目录 02', scaleTree).click();
  assert.equal(await button('资料 02·01', scaleTree).count(), 0);
  await button('展开 目录 02', scaleTree).click();
  await button('资料 02·01', scaleTree).waitFor();
  await typeQuery('千页验收青岚');
  const scaleHit = searchDialog().getByRole('button', { name: /资料 20·49/ });
  await scaleHit.click();
  await location(scalePage.blocks[0]).waitFor();
  await placeCaret(scalePage.blocks[0]);
  await page.keyboard.insertText('离线修订千页样例');
  await title().fill('千页改名与移动验收');
  await save();
  await page.locator('.save-indicator.saved').waitFor();
  await pageAction('移动页面');
  await dialog().getByRole('combobox').selectOption({ label: '目录 01' });
  await button('移动', dialog()).click();
  await button('目录 01', breadcrumb()).waitFor();
  await page.screenshot({ path: join(reportDir, 'm1-page-tree.png') });
  const beforeRestart = await invoke('read_page', { workspace: scaleSpace.id, id: scalePage.id });
  assert.equal(beforeRestart.title, '千页改名与移动验收');
  assert.equal(beforeRestart.parentId, scaleFixture.roots[0].id);
  assert.equal(beforeRestart.blocks[0].id, scalePage.blocks[0].id);
  assert.ok(beforeRestart.blocks[0].payload.inline.some((part) => part.text?.includes('离线修订千页样例')));
  assert.equal(beforeRestart.blocks[0].payload.inline.find((part) => part.type === 'pageReference').pageId,
    scaleFixture.roots[19].id);

  await page.goto('about:blank');
  const restartStarted = performance.now();
  await stopBackend();
  assert.equal(backend.exitCode, 0);
  backend = startBackend();
  await page.goto(origin);
  await switchToScale();
  await button(beforeRestart.title, scaleTree).click();
  await locateBlock(scalePage.blocks[0]).waitFor();
  const reopenedMs = Math.round(performance.now() - restartStarted);
  assert.deepEqual(await invoke('read_page', { workspace: scaleSpace.id, id: scalePage.id }), beforeRestart);
  await body().getByRole('link', { name: /目录 20/ }).click();
  await page.waitForFunction(() => document.querySelector('[aria-label="页面标题"]')?.value === '目录 20');
  checks.push('千页树可折叠、检索到末级正文、编辑改名和移动，重启 Core 后仍可沿原引用打开源页');

  const scaleRestore = await invoke('verify_restore_thousand_pages', { workspace: scaleSpace.id });
  assert.equal(scaleRestore.pages, 1000);
  assert.equal(scaleRestore.topLevelBlocks, 980);
  assert.equal(scaleRestore.pageReferences, 980);
  assert.ok(scaleRestore.independentRoot);
  const archiveHash = createHash('sha256').update(await readFile(join(sandbox, 'm1-thousand.zhiye.zip'))).digest('hex');
  await writeFile(join(reportDir, 'm1-scale.json'), JSON.stringify({
    passed: true, fixture: '20 directories, 980 nested notes, 980 linked paragraphs',
    seedMs: scaleFixture.seedMs, reopenedInBrowserMs: reopenedMs, archiveSha256: archiveHash, ...scaleRestore,
  }, null, 2) + '\n');
  checks.push('千页 ZIP 在独立数据根恢复，逐页核对正文、引用、稳定身份及全部修订');

  const integrationSpace = await invoke('create_workspace', { name: '集成设置验收' });
  const integrationNote = await invoke('create_page', { workspace: integrationSpace.id, parentId: null });
  const settings = async () => {
    await button('切换工作空间').click();
    await button('集成与 API').click();
    await button('新建集成', dialog()).waitFor();
  };
  await page.reload();
  await button('切换工作空间').click();
  await button(integrationSpace.name).click();
  await title().waitFor();
  await settings();
  await dialog().getByText('已关闭', { exact: true }).waitFor();
  assert.equal(await field('单个上传附件上限（MiB）', dialog()).inputValue(), '100');
  await field('单个上传附件上限（MiB）', dialog()).fill('7');
  await button('保存上限', dialog()).click();
  await button('保存上限', dialog()).waitFor();
  await button('新建集成', dialog()).click();
  await field('集成名称', dialog()).fill('浏览器写入夹具');
  const lifetime = field('令牌有效期', dialog());
  assert.equal(await lifetime.inputValue(), '0');
  await lifetime.selectOption('1');
  await field('选择授权资源', dialog()).selectOption(integrationNote.id);
  await button('加入范围', dialog()).click();
  assert.ok(await dialog().getByRole('checkbox', { name: '创建页面或记录', exact: true }).isDisabled());
  assert.ok(!(await dialog().getByRole('checkbox', { name: '下载附件', exact: true }).isChecked()));
  assert.ok(await dialog().getByRole('checkbox', { name: '上传附件', exact: true }).isDisabled());
  await dialog().getByRole('checkbox', { name: '修改页面、属性和正文', exact: true }).check();
  await dialog().getByRole('checkbox', { name: '提交修改建议', exact: true }).check();
  assert.ok(await dialog().getByRole('checkbox', { name: '上传附件', exact: true }).isEnabled());
  const beforeIssue = Date.now();
  await button('生成令牌', dialog()).click();
  const credentialInput = dialog().locator('#integration-token');
  const writerToken = await credentialInput.inputValue();
  assert.equal(writerToken.length, 70);
  await button('完成', dialog()).click();
  assert.equal(await credentialInput.count(), 0);
  await dialog().getByText(/^有效至 /).waitFor();
  await button('启用本机 API', dialog()).click();
  await dialog().getByText(`http://127.0.0.1:46832/api/v1/workspaces/${integrationSpace.id}`, { exact: true }).waitFor();
  await button('关闭', dialog()).click();
  const registered = await invoke('list_integrations', { workspace: integrationSpace.id });
  assert.ok(registered[0].expiresAt >= beforeIssue + 3_600_000);
  assert.ok(registered[0].expiresAt <= Date.now() + 3_600_000);
  assert.equal(await invoke('api_attachment_limit', { workspace: integrationSpace.id }), 7);
  assert.deepEqual(registered[0].scopes, ['content:read', 'content:update', 'proposals:create']);
  assert.ok(!JSON.stringify(registered).includes(writerToken));
  checks.push('集成设置默认关闭 API 和写操作，单页创建受限，令牌仅在创建后显示');
  checks.push('桌面可为合成授权选择 1 小时有效期，并显示 Core 保存的到期时间');

  const context = { workspace: integrationSpace.id, id: integrationNote.id };
  const remote = await invoke('verify_patch_integration', { ...context, token: writerToken, title: '集成更新的标题' });
  await body().fill('保留的本机草稿正文');
  await save();
  await button('对比版本').click();
  await dialog().getByText('集成更新的标题', { exact: true }).waitFor();
  await dialog().getByText('保留的本机草稿正文', { exact: true }).waitFor();
  await field('保留正文', dialog()).selectOption('local');
  await button('保存所选版本', dialog()).click();
  await dialog().waitFor({ state: 'hidden' });
  assert.equal(await title().inputValue(), '集成更新的标题');
  const merged = await invoke('read_page', context);
  assert.equal(merged.revision, remote.revision + 1);
  assert.equal(merged.blocks[0].payload.inline[0].text, '保留的本机草稿正文');
  await button('历史').click();
  await dialog().getByRole('button', { name: /^版本 2/ }).click();
  await dialog().getByText('集成 · 浏览器写入夹具', { exact: true }).waitFor();
  await button('关闭', dialog()).click();
  checks.push('外部修改产生版本冲突后保留草稿，可独立采纳标题和正文，历史可查看集成来源');

  await button('新建页面').click();
  await title().waitFor();
  const freshTitle = '其他客户端再次更新的标题';
  await invoke('save_page', { ...context, revision: merged.revision, draft: { title: freshTitle, blocks: merged.blocks } });
  const currentTree = page.getByRole('navigation', { name: '页面树' });
  await button('集成更新的标题', currentTree).click();
  await button(freshTitle, currentTree).waitFor();
  assert.equal(await title().inputValue(), freshTitle);
  checks.push('打开外部已更新的页面同步刷新侧栏标题和修订');

  const archivedOutsideWindow = await invoke('create_page', { workspace: integrationSpace.id, parentId: null });
  const archivedInfo = await invoke('save_page', {
    workspace: integrationSpace.id, id: archivedOutsideWindow.id, revision: archivedOutsideWindow.revision,
    draft: { title: '外部接口已归档的页面', blocks: [] },
  });
  await invoke('archive_pages', {
    workspace: integrationSpace.id, pages: [{ id: archivedInfo.id, revision: archivedInfo.revision }],
  });
  await button('回收站').click();
  await dialog().getByText('外部接口已归档的页面', { exact: true }).waitFor();
  await button('关闭', dialog()).click();
  checks.push('从桌面打开回收站会读取外部归档后的最新状态');

  const base = await invoke('read_page', context);
  await invoke('verify_submit_proposal', { workspace: context.workspace, token: writerToken, key: 'browser-proposal', draft: {
    page_id: context.id, base_revision: base.revision, changes: {
      operations: [{ op: 'append', block: { type: 'paragraph', payload: { text: '候选正文联调' } } }],
    },
  } });
  await settings();
  await button('审阅修改建议', dialog()).click();
  await button('查看建议', dialog()).click();
  await dialog().getByRole('region', { name: '候选内容', exact: true }).getByText(/候选正文联调/).waitFor();
  await invoke('save_page', { ...context, revision: base.revision, draft: { title: '审阅期间修改的底稿', blocks: base.blocks } });
  await button('接受建议', dialog()).click();
  await dialog().getByText('需重新对比', { exact: true }).waitFor();
  assert.equal((await invoke('read_page', context)).title, '审阅期间修改的底稿');
  await button('按当前版本重新预览', dialog()).click();
  await dialog().getByRole('region', { name: /^原文/ }).getByText(/审阅期间修改的底稿/).waitFor();
  await button('接受建议', dialog()).click();
  await dialog().getByText(/已接受 · 已写入修订/).waitFor();
  await button('关闭', dialog()).click();
  await button('关闭', dialog()).click();
  assert.equal(await title().inputValue(), '审阅期间修改的底稿');
  assert.match(JSON.stringify((await invoke('read_page', context)).blocks), /候选正文联调/);
  await button('页面操作').click();
  await button('修改建议').click();
  await dialog().getByText(/已接受/).waitFor();
  await button('关闭', dialog()).click();
  checks.push('修改建议可从集成和页面审阅，原文更新阻断接受，显式重新预览后写入新修订');

  await invoke('create_integration', { workspace: context.workspace, draft: {
    name: '子树访问范围', scopes: ['content:read', 'changes:read'], grants: [{ kind: 'subtree', id: context.id }],
  } });
  await button('页面操作').click();
  await button('新建子页面').click();
  await title().fill('移出授权的子页面');
  await save();
  await button('页面操作').click();
  await button('移动页面').click();
  await field(/选择.+上级位置/, dialog()).selectOption('');
  await button('移动', dialog()).click();
  await dialog().getByText('子树访问范围', { exact: true }).waitFor();
  await dialog().getByText(/减少 1 页/).waitFor();
  await button('完成', dialog()).click();
  checks.push('移动结束显示实际失去访问权限的集成与页面数');

  await settings();
  await button('撤销 浏览器写入夹具', dialog()).click();
  await button('确认撤销', dialog()).click();
  await dialog().getByText(/已撤销/).waitFor();
  await assert.rejects(invoke('verify_read_integration', { ...context, token: writerToken }), { code: 'UNAUTHORIZED' });
  await button('关闭本机 API', dialog()).click();
  await dialog().getByText('已关闭', { exact: true }).waitFor();
  await button('关闭', dialog()).click();
  checks.push('桌面撤销后同一令牌无法读取，API 可从设置独立关闭');

  const locationSpace = await invoke('create_workspace', { name: '系统定位链接样本空间' });
  const linkPage = await invoke('create_page', { workspace: locationSpace.id, parentId: null });
  const firstBlock = { id: randomUUID(), type: 'paragraph', schemaVersion: 1,
    payload: { inline: [{ type: 'text', text: '起始块' }] }, children: [] };
  const locatedBlock = { ...firstBlock, id: randomUUID(), payload: { inline: [{ type: 'text', text: '系统定位专用关键词' }] } };
  const expectedTitle = '正文块定位目标';
  await invoke('save_page', { workspace: locationSpace.id, id: linkPage.id, revision: linkPage.revision,
    draft: { title: expectedTitle, blocks: [firstBlock, locatedBlock] } });
  const pageLink = `zhiye://open/workspaces/${locationSpace.id}/pages/${linkPage.id}`;
  const blockLink = `${pageLink}/blocks/${locatedBlock.id}`;
  const backLink = `zhiye://open/workspaces/${context.workspace}/pages/${context.id}`;
  await page.evaluate((url) => sessionStorage.setItem('test-start-location', url), blockLink);
  await page.reload();
  await body().locator(`[data-id="${locatedBlock.id}"][data-search-target="true"]`).waitFor();
  assert.equal(await title().inputValue(), expectedTitle);
  await button('页面操作').click();
  await button('页面链接').click();
  assert.equal(await field('知页定位链接', dialog()).inputValue(), pageLink);
  await button('完成', dialog()).click();
  await button('搜索').click();
  await field('搜索关键词', dialog()).fill('系统定位专用关键词');
  await dialog().getByText(expectedTitle, { exact: true }).waitFor();
  await button('定位链接', dialog()).click();
  assert.equal(await field('知页定位链接', dialog()).inputValue(), blockLink);
  await button('完成', dialog()).click();
  checks.push('启动定位链接切换工作空间并高亮指定正文块，页面与搜索结果显示可复制链接');

  const deliver = (url) => page.evaluate((target) => window.testDeliverDesktopLocation(target), url);
  await title().fill('跨空间前的未保存标题');
  const saveStatus = await page.evaluate((url) => {
    const before = document.querySelector('.save-indicator')?.textContent;
    window.testDeliverDesktopLocation(url);
    return before;
  }, backLink);
  assert.equal(saveStatus, '等待保存');
  await body().getByText('候选正文联调', { exact: true }).waitFor();
  const saved = await invoke('read_page', { workspace: locationSpace.id, id: linkPage.id });
  assert.equal(saved.title, '跨空间前的未保存标题');
  await deliver(`${pageLink}/blocks/${randomUUID()}`);
  await page.getByRole('status').filter({ hasText: '链接中的正文块已不可用' }).waitFor();
  assert.equal(await title().inputValue(), saved.title);
  assert.equal(await body().locator('[data-search-target="true"]').count(), 0);
  const archivedLink = `zhiye://open/workspaces/${context.workspace}/pages/${archivedInfo.id}`;
  await deliver(archivedLink);
  await page.getByRole('alert').filter({ hasText: '该页面在回收站中' }).waitFor();
  assert.equal(await title().inputValue(), saved.title);
  checks.push('运行中跳转前提交未保存草稿，失效块回退到所属页，归档目标保留当前页');

  await invoke('save_page', { workspace: locationSpace.id, id: linkPage.id, revision: saved.revision,
    draft: { title: '另一个客户端写入的标题', blocks: saved.blocks } });
  await title().fill('因版本冲突保留的草稿');
  await deliver(backLink);
  await button('对比版本').waitFor();
  assert.equal(await title().inputValue(), '因版本冲突保留的草稿');
  assert.equal((await invoke('read_page', { workspace: locationSpace.id, id: linkPage.id })).title, '另一个客户端写入的标题');
  checks.push('链接跳转遇到保存冲突时留在原编辑器，远端修订和本机草稿分别保留');

  const referenceSpace = await invoke('create_workspace', { name: '块引用验收空间' });
  const sourcePage = await invoke('create_page', { workspace: referenceSpace.id, parentId: null });
  const paragraphs = [block('paragraph', { inline: [{ type: 'text', text: '源页第一段' }] }),
    block('paragraph', { inline: [{ type: 'text', text: '明确选择的原文段落' }] })];
  await invoke('save_page', { workspace: referenceSpace.id, id: sourcePage.id, revision: sourcePage.revision,
    draft: { title: '块引用原文', blocks: paragraphs } });
  const referencePage = await invoke('create_page', { workspace: referenceSpace.id, parentId: null });
  await invoke('save_page', { workspace: referenceSpace.id, id: referencePage.id, revision: referencePage.revision,
    draft: { title: '块引用使用页', blocks: [] } });
  const referenceLink = `zhiye://open/workspaces/${referenceSpace.id}/pages/${referencePage.id}`;
  await page.evaluate((url) => sessionStorage.setItem('test-start-location', url), referenceLink);
  await page.reload();
  await page.waitForFunction(() => document.querySelector('.document-title')?.value === '块引用使用页');
  await body().locator('p').first().click();
  await page.keyboard.type('/');
  await button('块引用', page.locator('.slash-menu')).click();
  await button('块引用原文', dialog()).click();
  await field('筛选源块', dialog()).fill('明确选择');
  await dialog().getByRole('button', { name: /明确选择的原文段落/ }).click();
  const preview = body().locator('.block-reference');
  await preview.getByText('明确选择的原文段落', { exact: true }).waitFor();
  await save();
  const savedReference = await invoke('read_page', { workspace: referenceSpace.id, id: referencePage.id });
  const [inserted] = savedReference.blocks;
  assert.equal(inserted.type, 'blockReference');
  assert.deepEqual(inserted.payload, { pageId: sourcePage.id, blockId: paragraphs[1].id });
  assert.equal(savedReference.blocks.length, 1);
  await button('打开源块：块引用原文', preview).click();
  await body().locator(`[data-id="${paragraphs[1].id}"][data-search-target="true"]`).waitFor();
  await button('页面操作').click();
  await button('反向链接').click();
  await button('块引用使用页 块引用', dialog()).click();
  await body().locator(`[data-id="${inserted.id}"][data-search-target="true"]`).waitFor();
  checks.push('通过斜杠菜单选择原文的第二段，引用和反向链接均可跳到对应的原始块');

  const latestSource = await invoke('read_page', { workspace: referenceSpace.id, id: sourcePage.id });
  const changedText = '源内容已从另一个客户端改好';
  paragraphs[1].payload.inline[0].text = changedText;
  await invoke('save_page', { workspace: referenceSpace.id, id: sourcePage.id, revision: latestSource.revision,
    draft: { title: '原文更新后的标题', blocks: paragraphs } });
  await preview.getByText(changedText, { exact: true }).waitFor();
  await button('打开源块：原文更新后的标题', preview).waitFor();
  await invoke('archive_pages', { workspace: referenceSpace.id, pages: [{ id: sourcePage.id, revision: latestSource.revision + 1 }] });
  await preview.getByText('源块不可用', { exact: true }).waitFor();
  assert.equal(await preview.getByText(changedText).count(), 0);
  await invoke('restore_pages', { workspace: referenceSpace.id, pages: [{ id: sourcePage.id, revision: latestSource.revision + 2 }] });
  await preview.getByText(changedText, { exact: true }).waitFor();
  assert.deepEqual(await invoke('read_page', { workspace: referenceSpace.id, id: referencePage.id }), savedReference);
  await page.screenshot({ path: join(reportDir, 'block-reference.png') });
  checks.push('引用页打开时跟随源块更新、归档和恢复，引用页的原始块及修订号保持稳定');

  const relationSpace = await invoke('create_workspace', { name: '关联属性验收空间' });
  const workspace = relationSpace.id;
  const seedDatabase = async (name) => {
    const created = await invoke('create_collection', { workspace, parentId: null });
    await invoke('save_page', { workspace, id: created.page.id, revision: created.page.revision, draft: { title: name, blocks: [] } });
    return created.collection;
  };
  const left = await seedDatabase('跨库任务');
  const right = await seedDatabase('证据目录');
  const seedRecord = async (collection, title) => {
    const record = await invoke('create_page', { workspace, parentId: collection.containerPageId });
    return invoke('save_page', { workspace, id: record.id, revision: record.revision, draft: { title, blocks: [] } });
  };
  const relationTask = await seedRecord(left, '关联任务甲');
  const relationTarget = await seedRecord(right, '关联目标乙');
  const databaseUrl = `zhiye://open/workspaces/${workspace}/pages/${left.containerPageId}`;
  await page.evaluate((url) => sessionStorage.setItem('test-start-location', url), databaseUrl);
  await page.reload();
  await page.waitForFunction(() => document.querySelector('.document-title')?.value === '跨库任务');
  await button('管理属性').click();
  await field('属性名称', dialog()).fill('项目证据');
  await field('属性类型', dialog()).selectOption('relation');
  await field('目标数据库', dialog()).selectOption({ label: '证据目录' });
  await dialog().getByRole('checkbox', { name: '同时创建反向关联', exact: true }).check();
  await field('反向属性名称', dialog()).fill('来自任务');
  await button('创建属性', dialog()).click();
  await table().getByRole('columnheader', { name: '项目证据' }).waitFor();
  await button('关联任务甲', table()).click();
  await button('编辑属性').click();
  await dialog().getByText('添加关联', { exact: true }).click();
  await field('查找「项目证据」的关联记录', dialog()).fill('目标乙');
  await dialog().getByRole('checkbox', { name: '关联目标乙', exact: true }).check();
  await button('保存', dialog()).click();
  const recordPanel = page.getByRole('region', { name: '记录属性' });
  await button('关联目标乙', recordPanel).waitFor();
  await page.screenshot({ path: join(reportDir, 'relation-record.png') });
  await button('关联目标乙', recordPanel).click();
  await recordPanel.getByText('来自任务', { exact: true }).waitFor();
  await button('关联任务甲', recordPanel).click();
  await button('关联目标乙', recordPanel).waitFor();
  checks.push('从属性管理创建双向关联，按标题选择异库记录，从两端点击可定位对方');

  const beforeArchive = await invoke('read_page', { workspace, id: relationTarget.id });
  await invoke('archive_pages', { workspace, pages: [{ id: relationTarget.id, revision: beforeArchive.revision }] });
  await deliver(`zhiye://open/workspaces/${workspace}/pages/${left.containerPageId}`);
  await button('关联任务甲', table()).click();
  await recordPanel.getByText('部分关联不可用', { exact: true }).waitFor();
  assert.equal(await button('关联目标乙', recordPanel).count(), 0);
  await button('编辑属性').click();
  await field('标题', dialog()).fill('关联任务已改名');
  await button('保存', dialog()).click();
  await page.waitForFunction(() => document.querySelector('.document-title')?.value === '关联任务已改名');
  await invoke('restore_pages', { workspace, pages: [{ id: relationTarget.id, revision: beforeArchive.revision + 1 }] });
  await deliver(`zhiye://open/workspaces/${workspace}/pages/${left.containerPageId}`);
  await button('关联任务已改名', table()).click();
  await button('关联目标乙', recordPanel).waitFor();
  assert.equal((await invoke('read_page', { workspace, id: relationTask.id })).title, '关联任务已改名');
  checks.push('关联目标归档时隐去标题，编辑普通属性保留关联，恢复后可再打开');

  await deliver('zhiye://open/workspaces/' + workspace + '/pages/' + right.containerPageId);
  await button('管理属性').click();
  await field('属性名称', dialog()).fill('计划工时');
  await field('属性类型', dialog()).selectOption('number');
  await button('创建属性', dialog()).click();
  await table().getByRole('columnheader', { name: '计划工时' }).waitFor();
  await button('关联目标乙', table()).click();
  await button('编辑属性').click();
  await field('计划工时', dialog()).fill('4');
  await button('保存', dialog()).click();
  await recordPanel.getByText('4', { exact: true }).waitFor();

  await deliver(databaseUrl);
  await table().getByRole('columnheader', { name: '项目证据' }).waitFor();
  const drainDerived = async () => {
    for (let attempts = 0; attempts < 80; attempts++) {
      if (!await invoke('verify_advance_derived', { workspace })) return;
    }
    throw new Error('Derived work remained after 80 batches');
  };
  await button('管理属性').click();
  await field('属性名称', dialog()).fill('合计证据');
  await field('属性类型', dialog()).selectOption('rollup');
  await field('关联属性', dialog()).selectOption({ label: '项目证据' });
  await field('汇总方式', dialog()).selectOption('sum');
  await field('目标数值属性', dialog()).selectOption({ label: '计划工时' });
  await button('创建属性', dialog()).click();
  await dialog().waitFor({ state: 'hidden' });
  await drainDerived();
  await table().getByText('4', { exact: true }).waitFor();

  await button('管理属性').click();
  await field('属性名称', dialog()).fill('翻倍结果');
  await field('属性类型', dialog()).selectOption('formula');
  await field('插入属性', dialog()).selectOption({ label: '合计证据' });
  const expression = field('公式表达式', dialog());
  assert.equal(await expression.inputValue(), 'prop("合计证据")');
  await expression.evaluate((editor) => new Promise((resolve) => requestAnimationFrame(() => resolve(editor.selectionEnd))));
  await expression.press('End');
  await expression.pressSequentially(' * 2');
  assert.equal(await expression.inputValue(), 'prop("合计证据") * 2');
  await button('创建属性', dialog()).click();
  await dialog().waitFor({ state: 'hidden' });
  await drainDerived();
  await table().getByText('8', { exact: true }).waitFor();

  const inputSchema = await invoke('read_collection', { workspace, collection: right.id });
  const inputField = inputSchema.properties.find((field) => field.name === '计划工时');
  const saveSourceAmount = async (value) => {
    const targetRecord = await invoke('read_record', { workspace, collection: right.id, id: relationTarget.id });
    await invoke('save_record', {
      workspace, collection: right.id, id: relationTarget.id, revision: targetRecord.revision,
      draft: { title: targetRecord.title, schemaRevision: inputSchema.schemaRevision, properties: { [inputField.id]: value } },
    });
  };
  const notifyWorkspace = (workspace) => page.evaluate((id) => window.testWorkspaceContentChanged(id), workspace);
  const rowId = (await invoke('list_pages', { workspace })).find((item) => item.title === '关联任务已改名').id;
  await saveSourceAmount(6);
  const wrongWorkspaceReads = await page.evaluate(async (other) => {
    const before = window.testViewRecordReads;
    window.testWorkspaceContentChanged(other);
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    return window.testViewRecordReads - before;
  }, randomUUID());
  assert.equal(wrongWorkspaceReads, 0);
  await notifyWorkspace(workspace);
  await table().getByText('计算中…', { exact: true }).first().waitFor();
  const waiting = await invoke('read_record', { workspace, collection: left.id, id: rowId });
  assert.equal(waiting.computedPending, true);
  assert.equal(waiting.computed, undefined);
  await drainDerived();
  await table().getByText('12', { exact: true }).waitFor();
  checks.push('外部写入刷新已打开的表格及派生字段，其他工作空间的通知不重复读取当前视图');

  await button('关联任务已改名', table()).click();
  await recordPanel.getByText('12', { exact: true }).waitFor();
  await button('编辑属性', recordPanel).click();
  const pendingTitle = field('标题', dialog());
  await pendingTitle.fill('保留的记录表单草稿');
  await saveSourceAmount(8);
  await notifyWorkspace(workspace);
  const panelBehindDialog = page.locator('.record-panel');
  await panelBehindDialog.getByText('计算中…', { exact: true }).first().waitFor();
  await drainDerived();
  await panelBehindDialog.getByText('16', { exact: true }).waitFor();
  assert.equal(await pendingTitle.inputValue(), '保留的记录表单草稿');
  await button('保存', dialog()).click();
  await dialog().waitFor({ state: 'hidden' });
  await recordPanel.getByText('16', { exact: true }).waitFor();
  const savedRecordForm = await invoke('read_record', { workspace, collection: left.id, id: rowId });
  assert.equal(savedRecordForm.title, '保留的记录表单草稿');
  assert.equal(savedRecordForm.revision, waiting.revision + 1);
  await page.screenshot({ path: join(reportDir, 'computed-record.png') });
  checks.push('记录属性在编辑对话框打开时刷新，保留并可提交表单草稿');

  {
    let conflictSchema = await invoke('read_collection', { workspace, collection: left.id });
    for (const [name, type] of [['本机备注', 'text'], ['远端计数', 'number']]) {
      conflictSchema = await invoke('save_property', { workspace, collection: left.id, revision: conflictSchema.schemaRevision,
        draft: { id: null, name, type, options: [] } });
    }
    const conflictField = (name) => conflictSchema.properties.find((field) => field.name === name).id;
    const relationId = conflictField('项目证据');
    const archivedMember = await seedRecord(right, '归档的关联成员');
    const localMember = await seedRecord(right, '本机新关联');
    const remoteMember = await seedRecord(right, '远端新关联');
    const related = (id) => invoke('read_record', { workspace, collection: right.id, id });
    const head = () => invoke('read_record', { workspace, collection: left.id, id: rowId });
    async function reviseRecord(title, properties, add = []) {
      const before = await head();
      const values = { ...before.properties, ...properties };
      if (add.length) values[relationId] = { add, remove: [] };
      else delete values[relationId];
      return invoke('save_record', {
        workspace, collection: left.id, id: rowId, revision: before.revision,
        draft: { title, schemaRevision: conflictSchema.schemaRevision, properties: values },
      });
    }
    await reviseRecord(savedRecordForm.title, {}, [archivedMember.id]);
    await invoke('archive_pages', { workspace, pages: [{ id: archivedMember.id, revision: (await related(archivedMember.id)).revision }] });
    await drainDerived();
    const recordBase = await head();
    assert.ok(recordBase.relations[relationId].restricted);

    await button('编辑属性', recordPanel).click();
    await field('标题', dialog()).fill('冲突时保留的草稿');
    await field('本机备注', dialog()).fill('保留本机备注');
    await dialog().getByText('添加关联', { exact: true }).click();
    await field('查找「项目证据」的关联记录', dialog()).fill('本机新关联');
    await dialog().getByRole('checkbox', { name: '本机新关联', exact: true }).check();
    const firstExternal = await reviseRecord('外部版本甲', { [conflictField('远端计数')]: 21 }, [remoteMember.id]);
    await notifyWorkspace(workspace);
    await button('保存', dialog()).click();
    const warning = '记录已有更新。已保留表单输入，请对比最新版本。';
    await dialog().getByText(warning, { exact: true }).waitFor();
    assert.equal(await field('标题', dialog()).inputValue(), '冲突时保留的草稿');
    assert.equal((await head()).revision, firstExternal.revision);
    async function openComparison(externalTitle) {
      await button('对比版本', dialog()).click();
      const comparison = page.getByRole('dialog', { name: '对比记录版本', exact: true });
      await comparison.getByText(externalTitle, { exact: true }).waitFor();
      assert.equal(await button('保存所选属性', comparison).isDisabled(), true);
      assert.equal(await field('保留本机备注', comparison).inputValue(), 'local');
      assert.equal(await field('保留远端计数', comparison).inputValue(), 'saved');
      await field('保留标题', comparison).selectOption('saved');
      await field('保留项目证据', comparison).selectOption('local');
      return comparison;
    }
    const oldComparison = await openComparison('外部版本甲');
    const secondExternal = await reviseRecord('外部版本乙', { [conflictField('远端计数')]: 25 });
    await notifyWorkspace(workspace);
    await button('保存所选属性', oldComparison).click();
    await dialog().getByText(warning, { exact: true }).waitFor();
    assert.equal(await field('标题', dialog()).inputValue(), '冲突时保留的草稿');
    assert.equal(await field('本机备注', dialog()).inputValue(), '保留本机备注');
    await button('移除关联「本机新关联」', dialog()).waitFor();
    assert.equal((await head()).revision, secondExternal.revision);
    await button('导出草稿', dialog()).waitFor();
    const latestComparison = await openComparison('外部版本乙');
    await latestComparison.getByText('25', { exact: true }).waitFor();
    await page.screenshot({ path: join(reportDir, 'record-conflict.png') });
    await button('保存所选属性', latestComparison).click();
    await dialog().waitFor({ state: 'hidden' });
    const merged = await head();
    assert.equal(merged.revision, recordBase.revision + 3);
    assert.equal(merged.title, '外部版本乙');
    assert.equal(merged.properties[conflictField('远端计数')], 25);
    assert.equal(merged.properties[conflictField('本机备注')], '保留本机备注');
    assert.deepEqual(new Set(merged.properties[relationId]), new Set([relationTarget.id, localMember.id, remoteMember.id]));
    assert.ok(merged.relations[relationId].restricted);
    const archivedPage = await invoke('read_page', { workspace, id: archivedMember.id });
    await invoke('restore_pages', { workspace, pages: [{ id: archivedMember.id, revision: archivedPage.revision }] });
    const restoredRelation = await head();
    assert.deepEqual(new Set(restoredRelation.properties[relationId]),
      new Set([relationTarget.id, localMember.id, remoteMember.id, archivedMember.id]));
    checks.push('记录属性可分字段解决冲突，二次外部写入拒绝旧对比，草稿、本机与远端关联及归档边均保留');
  }

  await deliver(databaseUrl);
  await table().getByRole('columnheader', { name: '翻倍结果' }).waitFor();
  await button('管理属性').click();
  await field('属性名称', dialog()).fill('循环引用');
  await field('属性类型', dialog()).selectOption('formula');
  await field('公式表达式', dialog()).fill('unknown(1)');
  await button('创建属性', dialog()).click();
  await dialog().getByText('未知函数。', { exact: true }).waitFor();
  await field('公式表达式', dialog()).fill('prop("循环引用")');
  await button('创建属性', dialog()).click();
  await dialog().waitFor({ state: 'hidden' });
  await drainDerived();
  await table().getByText('计算依赖存在循环', { exact: true }).waitFor();
  checks.push('公式管理提示非法函数，成功保存的循环依赖显示字段错误');

  const scheduleSpace = (await invoke('create_workspace', { name: '数据库视图验收' })).id;
  const projectDb = await invoke('create_collection', { workspace: scheduleSpace, parentId: null });
  const projectPage = await invoke('save_page', {
    workspace: scheduleSpace, id: projectDb.page.id, revision: projectDb.page.revision,
    draft: { title: '日程总览', blocks: [] },
  });
  const projectId = projectDb.collection.id;
  let scheduleSchema = await invoke('read_collection', { workspace: scheduleSpace, collection: projectId });
  const scheduleFields = {};
  for (const [name, type] of [['起始日期', 'date'], ['结束日期', 'date'], ['会议时间', 'dateTime'], ['图片封面', 'attachment']]) {
    scheduleSchema = await invoke('save_property', {
      workspace: scheduleSpace, collection: projectId, revision: scheduleSchema.schemaRevision,
      draft: { id: null, name, type, options: [] },
    });
    scheduleFields[name] = scheduleSchema.properties.find((item) => item.name === name).id;
  }
  const scheduleRecord = async (title, values, image) => {
    const row = await invoke('create_page', { workspace: scheduleSpace, parentId: projectPage.id });
    if (image) {
      const file = (await invoke('pick_images', { workspace: scheduleSpace, page: row.id })).added[0].attachment;
      values[scheduleFields['图片封面']] = [file.id];
    }
    return invoke('save_record', {
      workspace: scheduleSpace, collection: projectId, id: row.id, revision: row.revision,
      draft: { title, schemaRevision: scheduleSchema.schemaRevision, properties: values },
    });
  };
  const research = await scheduleRecord('全天研究', {
    [scheduleFields['起始日期']]: '2026-11-16', [scheduleFields['结束日期']]: '2026-11-24',
  }, true);
  const meeting = await scheduleRecord('带时区会议', { [scheduleFields['会议时间']]: '2026-11-16T00:30:00+09:30' });
  const memo = await invoke('create_page', { workspace: scheduleSpace, parentId: null });
  await invoke('save_page', {
    workspace: scheduleSpace, id: memo.id, revision: memo.revision,
    draft: { title: '排期嵌入页', blocks: [] },
  });
  const scheduleLocation = (id) => 'zhiye://open/workspaces/' + scheduleSpace + '/pages/' + id;
  await deliver(scheduleLocation(projectPage.id));
  const viewbar = page.getByRole('group', { name: '数据库视图' });
  const dateCell = (day) => page.getByRole('region', { name: day, exact: true });
  await button('日历', viewbar).click();
  await field('跳转日期').fill('2026-11-16');
  const scheduledCard = dateCell('2026-11-16').locator('.calendar-event').filter({ hasText: '全天研究' });
  await scheduledCard.dragTo(dateCell('2026-11-20'));
  await button('全天研究', dateCell('2026-11-20')).waitFor();
  assert.equal((await invoke('read_record', { workspace: scheduleSpace, collection: projectId, id: research.id }))
    .properties[scheduleFields['起始日期']], '2026-11-20');
  await button('未排期').click();
  await button('带时区会议', page.getByRole('region', { name: '数据库记录' })).waitFor();
  checks.push('日历按全天日期显示和拖动原属性，未排期记录仍可打开');

  await button('新建视图', viewbar).click();
  await field('视图名称', dialog()).fill('跨时区日历');
  await field('视图类型', dialog()).selectOption('calendar');
  await field('日历日期属性', dialog()).selectOption({ label: '会议时间' });
  await button('保存', dialog()).click();
  await page.getByRole('button', { name: '跨时区日历', pressed: true }).waitFor();
  await field('跳转日期').fill('2026-11-16');
  await button('带时区会议', dateCell('2026-11-15')).waitFor();
  await dateCell('2026-11-15').locator('.calendar-event').dragTo(dateCell('2026-11-18'));
  await button('带时区会议', dateCell('2026-11-18')).waitFor();
  assert.equal((await invoke('read_record', { workspace: scheduleSpace, collection: projectId, id: meeting.id }))
    .properties[scheduleFields['会议时间']], '2026-11-19T00:30:00+09:30');
  await page.screenshot({ path: join(reportDir, 'calendar.png') });
  checks.push('同类型日历各自保存设置，带时区日期按 UTC 投影且拖动保留原偏移');

  await button('时间线', viewbar).click();
  await field('跳转日期').fill('2026-11-18');
  await field('时间线缩放').selectOption('week');
  const track = page.getByRole('region', { name: '时间线', exact: true });
  const bar = track.getByRole('img', { name: '2026-11-20 至 2026-11-24' });
  const rail = bar.locator('xpath=..');
  const railBounds = await boundsFor(rail);
  await bar.dragTo(rail, { sourcePosition: { x: 8, y: 10 },
    targetPosition: { x: Math.round(railBounds.width * 5.5 / 7), y: 14 } });
  await track.getByRole('img', { name: '2026-11-23 至 2026-11-27' }).waitFor();
  await page.screenshot({ path: join(reportDir, 'timeline.png') });
  await button('画廊', viewbar).click();
  await page.getByRole('img', { name: '海湾.png', exact: true }).waitFor();
  await page.screenshot({ path: join(reportDir, 'gallery.png') });
  await button('全天研究', page.getByRole('list', { name: '画廊' })).click();
  await page.waitForFunction(() => document.querySelector('.document-title')?.value === '全天研究');
  checks.push('时间线缩放与拖动延续起止区间，画廊加载同一记录自有图片并打开原页');

  const otherCover = await invoke('attach_image_bytes', {
    workspace: scheduleSpace, page: research.id, name: '手选封面.png', bytes: [...await readFile(imageFixture)],
  });
  await pageAction('页面外观');
  const recordAppearance = page.getByRole('dialog', { name: '页面外观', exact: true });
  await field('封面', recordAppearance).selectOption(otherCover.attachment.id);
  await button('保存外观', recordAppearance).click();
  await page.getByRole('img', { name: '页面封面', exact: true }).waitFor();
  await deliver(scheduleLocation(projectPage.id));
  await button('画廊', viewbar).click();
  await page.getByRole('img', { name: '手选封面.png', exact: true }).waitFor();
  await page.screenshot({ path: join(reportDir, 'gallery-record-cover.png') });
  checks.push('在记录页指定第二张图片为封面后，默认画廊投影显示所选图片');

  await deliver(scheduleLocation(memo.id));
  await page.waitForFunction(() => document.querySelector('.document-title')?.value === '排期嵌入页');
  await button('插入').click();
  await button('数据库视图', page.locator('.insert-menu')).click();
  await field('选择数据库', dialog()).selectOption({ label: '日程总览' });
  await field('选择保存视图', dialog()).selectOption({ label: '跨时区日历' });
  await button('插入视图', dialog()).click();
  const embed = page.locator('.embedded-view');
  await field('跳转日期', embed).fill('2026-11-16');
  await button('带时区会议', embed).waitFor();
  await save();
  const embeddedPage = await invoke('read_page', { workspace: scheduleSpace, id: memo.id });
  const savedEmbed = embeddedPage.blocks.find((item) => item.type === 'collectionView');
  assert.equal(savedEmbed.payload.collectionId, projectId);
  const liveMeeting = await invoke('read_page', { workspace: scheduleSpace, id: meeting.id });
  const changedMeetingTitle = '外部更新的会议';
  await invoke('save_page', {
    workspace: scheduleSpace, id: meeting.id, revision: liveMeeting.revision,
    draft: { title: changedMeetingTitle, blocks: liveMeeting.blocks },
  });
  await notifyWorkspace(scheduleSpace);
  await button(changedMeetingTitle, embed).waitFor();
  assert.equal(await field('跳转日期', embed).inputValue(), '2026-11-16');
  checks.push('正文嵌入的保存视图刷新外部写入并保留当前日历日期');
  await button('打开数据库', embed).click();
  await page.getByRole('button', { name: '跨时区日历', pressed: true }).waitFor();
  await button('视图设置').click();
  await button('删除视图', dialog()).click();
  await button(changedMeetingTitle, table()).waitFor();
  await deliver(scheduleLocation(memo.id));
  await embed.getByText('此视图暂不可用。', { exact: true }).waitFor();
  checks.push('正文嵌入保存视图并可打开原视图，删除视图后保留记录及嵌入块');

  const richBlocksPage = await invoke('create_page', { workspace: scheduleSpace, parentId: null });
  await invoke('save_page', {
    workspace: scheduleSpace, id: richBlocksPage.id, revision: richBlocksPage.revision,
    draft: { title: '实验记录与原件', blocks: [] },
  });
  const richBlocksUrl = scheduleLocation(richBlocksPage.id);
  await deliver(richBlocksUrl);
  await body().waitFor();
  const insertBlock = async (kind) => {
    await button('插入').click();
    await button(kind, page.locator('.insert-menu')).click();
  };
  const tail = body().locator(':scope > p').last();
  const fold = body().locator('.fold-block');
  await tail.click();
  await insertBlock('折叠');
  await field('折叠标题', fold).fill('换算过程');
  await fold.locator('.fold-body p').click();
  await page.keyboard.insertText('实验参数');
  await insertBlock('二级标题');
  const nestedHeading = fold.getByRole('heading', { name: '实验参数', exact: true });
  await nestedHeading.waitFor();
  await button('收起折叠块', fold).click();
  await nestedHeading.waitFor({ state: 'hidden' });
  await tail.click();
  await insertBlock('目录');
  await save();
  const readRichBlocks = () => invoke('read_page', { workspace: scheduleSpace, id: richBlocksPage.id });
  await page.getByText('已保存', { exact: true }).waitFor();
  const beforeOutline = await readRichBlocks();
  const savedFold = beforeOutline.blocks.find((block) => block.type === 'toggleBlock');
  assert.equal(savedFold.payload.collapsed, true);
  const sectionLink = button('实验参数', page.getByRole('navigation', { name: '目录', exact: true }));
  await sectionLink.click();
  await nestedHeading.waitFor();
  await fold.locator('h2[data-search-target="true"]').waitFor();
  await save();
  assert.equal((await readRichBlocks()).revision, beforeOutline.revision);
  checks.push('目录定位到收起折叠内的标题，保留原折叠状态与正文修订');

  await tail.click();
  await insertBlock('提示块');
  const callout = body().locator('.callout-block');
  await field('提示样式', callout).selectOption('tip');
  await callout.locator('.callout-body p').click();
  await page.keyboard.insertText('核对实际用量。');
  await tail.click();
  await insertBlock('公式');
  const math = body().locator('.math-block');
  const latex = '\\frac{a}{b}';
  await field('LaTeX 公式', math).fill(latex);
  await button('保存公式', math).click();
  await math.locator('.katex').waitFor();
  await button('编辑公式', math).click();
  await field('LaTeX 公式', math).fill('\\frac{a');
  await button('保存公式', math).click();
  await math.getByRole('status').getByText('LaTeX 排版遇到格式错误。', { exact: false }).waitFor();
  await button('编辑公式', math).click();
  await field('LaTeX 公式', math).fill(latex);
  await button('保存公式', math).click();
  await math.locator('.katex').waitFor();
  await page.evaluate(() => document.fonts.ready);
  assert.equal(await page.evaluate(() => document.fonts.check('16px KaTeX_Main')), true);
  checks.push('提示样式与正文可编辑，纯本地公式排版和格式错误回退可用');

  await tail.click();
  await page.keyboard.insertText('/');
  await button('附件块', page.locator('.slash-menu')).click();
  await button('添加附件', dialog()).click();
  await button('插入附件块 研究备注.txt', dialog()).click();
  const fileCard = body().locator('.file-card');
  await button('研究备注.txt', fileCard).waitFor();
  await button('研究备注.txt', fileCard).click();
  await dialog().getByRole('heading', { name: '研究备注.txt', exact: true }).waitFor();
  await button('另存原件', dialog()).click();
  await dialog().getByText('附件原件已另存。', { exact: true }).waitFor();
  assert.equal(await readFile(join(sandbox, 'saved-attachment.bin'), 'utf8'), '知页附件块原件\n');
  await button('返回列表', dialog()).click();
  await button('重命名 研究备注.txt', dialog()).click();
  await field('附件名称', dialog()).fill('实验原件.txt');
  await button('保存名称', dialog()).click();
  await dialog().getByText('附件名称已更新。', { exact: true }).waitFor();
  await button('关闭', dialog()).click();
  await button('实验原件.txt', fileCard).waitFor();
  await save();
  await page.getByText('已保存', { exact: true }).waitFor();
  const persistedRichBlocks = await readRichBlocks();
  const attachmentBlock = persistedRichBlocks.blocks.find((block) => block.type === 'attachmentFile');
  const savedCallout = persistedRichBlocks.blocks.find((block) => block.type === 'callout');
  assert.ok(attachmentBlock);
  assert.equal(savedCallout.payload.tone, 'tip');
  assert.equal(persistedRichBlocks.blocks.find((block) => block.type === 'mathBlock').payload.latex, latex);
  assert.equal(persistedRichBlocks.blocks.find((block) => block.id === savedFold.id).payload.collapsed, true);
  await page.screenshot({ path: join(reportDir, 'content-blocks.png') });
  checks.push('斜杠菜单插入原件卡片，打开、另存和重命名后同一附件 ID 保持一致');

  await deliver(scheduleLocation(meeting.id));
  await title().waitFor();
  await page.waitForFunction((title) => document.querySelector('.document-title')?.value === title, changedMeetingTitle);
  await deliver(richBlocksUrl);
  await button('实验原件.txt', fileCard).waitFor();
  await math.locator('.katex').waitFor();
  await button('展开折叠块', fold).waitFor();
  await pageAction('导出 Markdown');
  await page.getByRole('status').filter({ hasText: '导出报告位于同目录的「images」文件夹。' }).waitFor();
  const derivedMarkdown = await readFile(join(sandbox, 'page.md'), 'utf8');
  for (const part of ['**目录**\n\n- 实验参数', '> **换算过程**', '> [!TIP]', latex, '[附件](<images/']) {
    assert.ok(derivedMarkdown.includes(part), part);
  }
  const markdownReport = await readFile(join(sandbox, 'images', '导出报告.txt'), 'utf8');
  for (const type of ['toggleBlock', 'callout', 'tableOfContents', 'mathBlock', 'attachmentFile']) {
    const block = persistedRichBlocks.blocks.find((block) => block.type === type);
    assert.ok(markdownReport.split('\n').some((line) => line.includes(block.id) && line.includes(type)), type);
  }
  assert.equal(await readFile(join(sandbox, 'images', attachmentBlock.payload.attachmentId + '.txt'), 'utf8'), '知页附件块原件\n');
  checks.push('重新打开保留内容块，Markdown 展平折叠并导出逐块转换说明与原件字节');

  const pdfPage = await invoke('create_page', { workspace: scheduleSpace, parentId: null });
  await invoke('save_page', { workspace: scheduleSpace, id: pdfPage.id, revision: pdfPage.revision,
    draft: { title: '实验文献与扫描件', blocks: [] } });
  await deliver(scheduleLocation(pdfPage.id));
  await page.waitForFunction(() => document.querySelector('.document-title')?.value === '实验文献与扫描件');
  const pdfs = await invoke('verify_add_pdf_attachments', { workspace: scheduleSpace, page: pdfPage.id });
  const originalPdf = pdfs.find((item) => item.name === '研究样本.pdf');
  const findPdf = () => dialog().getByRole('button', { name: /研究样本\.pdf · PDF 第 2 页.*赤铜配比/ });
  const pdfCanvas = (value) => dialog().locator('canvas[aria-label="PDF 第 ' + value + ' 页"]');
  await button('搜索').click();
  await field('搜索关键词', dialog()).fill('赤铜配比');
  await findPdf().waitFor({ timeout: 18000 });
  await page.screenshot({ path: join(reportDir, 'pdf-search.png') });
  await findPdf().click();
  await pdfCanvas(2).waitFor();
  const ink = await pdfCanvas(2).evaluate((canvas) => {
    const pixels = canvas.getContext('2d').getImageData(0, 0, canvas.width, Math.floor(canvas.height / 2)).data;
    let visible = 0;
    for (let offset = 0; offset < pixels.length; offset += 4) {
      if (pixels[offset + 3] > 0 && pixels[offset] < 150) visible += 1;
    }
    return visible;
  });
  assert.ok(ink > 1000);
  await page.screenshot({ path: join(reportDir, 'pdf-page-two.png') });
  await button('上一页', dialog()).click();
  await pdfCanvas(1).waitFor();
  checks.push('普通中文 PDF 按页提取，搜索点击直接绘制命中的原件第 2 页');

  await button('返回列表', dialog()).click();
  for (const [name, status] of [['扫描样本.pdf', '无文本层，未进行 OCR'],
    ['加密样本.pdf', 'PDF 加密，无法提取'], ['损坏样本.pdf', 'PDF 无法读取或提取']]) {
    const row = dialog().locator('.attachment-row').filter({ hasText: name });
    await row.getByText(status, { exact: false }).waitFor({ timeout: 15000 });
  }
  await page.screenshot({ path: join(reportDir, 'pdf-statuses.png') });
  await button('关闭', dialog()).click();
  await button('搜索').click();
  await field('搜索关键词', dialog()).fill('Gamma Zircon Raster');
  await dialog().getByText('没有找到匹配的页面。', { exact: true }).waitFor();
  checks.push('扫描件仅记录无文本层，加密和损坏状态独立显示，扫描内容没有文字命中');

  await field('搜索关键词', dialog()).fill('赤铜配比');
  await findPdf().waitFor();
  const originalStatus = (await invoke('poll_pdf_index', { workspace: scheduleSpace })).progress;
  await button('重建索引', dialog()).click();
  for (let attempt = 0; attempt < 90; attempt += 1) {
    const status = (await invoke('poll_pdf_index', { workspace: scheduleSpace })).progress;
    if (!status.pending && status.revision >= originalStatus.revision + 5) break;
    if (attempt === 89) throw new Error('PDF index rebuild did not finish');
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  await findPdf().waitFor();
  await button('定位链接', dialog()).click();
  await dialog().getByRole('heading', { name: 'PDF 页码链接' }).waitFor();
  const pdfLink = await field('知页定位链接', dialog()).inputValue();
  assert.equal(pdfLink, scheduleLocation(pdfPage.id) + '/attachments/' + originalPdf.id + '/pdf/' + originalPdf.sha256 + '/2');
  await button('完成', dialog()).click();
  await deliver(pdfLink);
  await pdfCanvas(2).waitFor();
  await button('关闭', dialog()).click();
  checks.push('PDF 重建索引后原件版本、复制的定位链接和准确页码仍有效');

  const runMigration = async (format, label) => {
    await button('导入资料').click();
    await field('文件格式', dialog()).selectOption(format);
    await button(label, dialog()).click();
    await reportDialog().waitFor();
  };
  const notionGuide = '迁移手册 11111111111111111111111111111111.md';
  await runMigration('notionZip', '选择 Notion ZIP');
  await reportDialog().getByText('已导入 · 2 条记录').waitFor();
  await reportDialog().getByText('未支持', { exact: true }).waitFor();
  await reportDialog().getByText(/已将 1 行的属性与同名 Markdown 正文合并/).waitFor();
  await button('保存报告', reportDialog()).click();
  await page.locator('.app-shell[aria-busy="false"]').waitFor();
  const migrationText = await readFile(join(sandbox, 'import-report.txt'), 'utf8');
  assert.match(migrationText, /未支持路径\.txt · 未支持/);
  assert.match(migrationText, /原件 Notion 样例\.zip · 页面 [a-f0-9-]+ · 附件 [a-f0-9-]+/);
  await writeFile(join(reportDir, 'notion-report.txt'), migrationText);
  await page.screenshot({ path: join(reportDir, 'notion-report.png') });
  await button(`打开 ${notionGuide}`, reportDialog()).click();
  await body().getByRole('link', { name: '↗ 章节入口' }).click();
  await body().getByText('Notion 子章节正文。', { exact: true }).waitFor();
  const migrationPages = await invoke('list_pages', { workspace: scheduleSpace });
  const notionRoot = migrationPages.find((item) => item.title === 'Notion 样例');
  const guide = migrationPages.find((item) => item.title === '迁移手册');
  const importedTable = migrationPages.find((item) => item.title === '迁移清单');
  assert.equal(guide.parentId, notionRoot.id);
  assert.equal(importedTable.parentId, guide.id);
  assert.equal(migrationPages.filter((item) => item.parentId === importedTable.id).length, 2);
  await button('迁移清单').click();
  await table().getByText('已核验', { exact: true }).waitFor();
  await button('检查导入', table()).click();
  await body().getByText('一条带正文的记录。', { exact: true }).waitFor();
  checks.push('Notion 分卷按原层级创建页面，转向本地页面和数据库，CSV 与记录正文合并并完整导出报告');

  await runMigration('notionZip', '选择 Notion ZIP');
  await reportDialog().getByText('此来源已迁移到当前位置。', { exact: true }).waitFor();
  assert.equal((await invoke('list_pages', { workspace: scheduleSpace })).length, migrationPages.length);
  await button('关闭', reportDialog()).click();
  checks.push('重复选择同一 Notion 来源与位置时保留原分支和页数');

  await runMigration('obsidianVault', '选择资料库目录');
  await reportDialog().getByText('2 处链接失配。', { exact: true }).waitFor();
  await reportDialog().getByText(/图片|显示尺寸留存/).first().waitFor();
  const canvasPreview = page.getByRole('dialog', { name: '草图.canvas' });
  await button('查看原件 草图.canvas', reportDialog()).click();
  await canvasPreview.getByText('此文件可另存为原件。').waitFor();
  await button('关闭', canvasPreview).click();
  await reportDialog().getByText('2 处链接失配。', { exact: true }).waitFor({ timeout: 3000 });
  await button('保存报告', reportDialog()).click();
  await page.locator('.app-shell[aria-busy="false"]').waitFor();
  const obsidianText = await readFile(join(sandbox, 'import-report.txt'), 'utf8');
  assert.match(obsidianText, /迁移已完成。.*2 处链接失配。/);
  assert.match(obsidianText, /草图\.canvas · 已留存原件/);
  assert.match(obsidianText, /\[\[找不到的笔记\]\]/);
  await writeFile(join(reportDir, 'obsidian-report.txt'), obsidianText);
  await page.screenshot({ path: join(reportDir, 'obsidian-report.png') });
  await button('查看原件 草图.canvas', reportDialog()).click();
  await canvasPreview.waitFor();
  await page.keyboard.press('Escape');
  await canvasPreview.waitFor({ state: 'hidden' });
  await reportDialog().getByText('2 处链接失配。', { exact: true }).waitFor();
  await button('关闭', reportDialog()).click();
  checks.push('关闭导入报告中的原件预览后保留报告、失配位置和 TXT 导出');
  await button('索引').click();
  const previewImage = body().getByRole('img', { name: '示意图.png' });
  await previewImage.waitFor();
  assert.ok(await previewImage.evaluate((image) => image.decode().then(() => image.naturalWidth)));
  await body().getByRole('link', { name: '↗ 主要笔记' }).click();
  await body().getByText('Obsidian 导入正文。', { exact: true }).waitFor();
  checks.push('Obsidian 路径 Wiki 链接可打开目标，图片使用本地原件，重名与缺页可查，原件按钮打开附件');

  await button('备份与恢复').click();
  await field('启用自动备份', dialog()).uncheck();
  await field('每日保留份数', dialog()).fill('2');
  await field('每周保留份数', dialog()).fill('1');
  await button('选择目录', dialog()).click();
  const chosen = join(sandbox, 'automatic-backups');
  await dialog().getByText(/automatic-backups$/).waitFor();
  await button('保存备份设置', dialog()).click();
  await dialog().getByText('备份设置已保存。', { exact: true }).waitFor();
  const configured = (await invoke('backup_status', { workspace: scheduleSpace })).policy;
  assert.equal(configured.enabled, false);
  assert.equal(configured.keepDaily, 2);
  assert.equal(configured.keepWeekly, 1);
  await rmdir(chosen);
  await button('立即运行备份', dialog()).click();
  await dialog().getByRole('alert').filter({ hasText: '最近备份失败' }).waitFor();
  await page.screenshot({ path: join(reportDir, 'backup-failed.png') });
  await mkdir(chosen);
  await button('立即运行备份', dialog()).click();
  await dialog().getByText('自动备份已保存。', { exact: true }).waitFor();
  await dialog().getByText(/^最近成功于/).click();
  await dialog().locator('details[open] code').waitFor();
  const scheduledFolders = await readdir(chosen);
  assert.equal(scheduledFolders.length, 1);
  const [dated] = scheduledFolders;
  assert.deepEqual(await readdir(join(chosen, dated)), ['知页整库恢复.zhiye-recovery.zip']);
  await button('立即运行备份', dialog()).click();
  await dialog().getByText('今天已经有一份自动备份。', { exact: true }).waitFor();
  await page.screenshot({ path: join(reportDir, 'backup-settings.png') });
  checks.push('配置外部备份目录和保留数量，显示失败，恢复目录后重试成功，当日只生成一份自动备份');

  const sourceBefore = (await invoke('list_workspaces')).workspaces.find((item) => item.id === scheduleSpace);
  await dialog().getByRole('button', { name: /^创建整库恢复包/ }).click();
  await page.getByRole('status').filter({ hasText: '整库恢复包已保存。' }).waitFor();
  const later = await invoke('create_page', { workspace: scheduleSpace, parentId: null });
  await invoke('save_page', { workspace: scheduleSpace, id: later.id, revision: later.revision, draft: { title: '快照后新增笔记', blocks: [] } });
  await button('备份与恢复').click();
  await dialog().getByRole('button', { name: /^从备份恢复/ }).click();
  await page.getByRole('status').filter({ hasText: '已恢复到新的工作空间。' }).waitFor();
  const recoveredSpace = (await invoke('list_workspaces')).workspaces.find((item) => item.name === `${sourceBefore.name}（恢复）`);
  assert.ok(recoveredSpace);
  assert.notEqual(recoveredSpace.id, scheduleSpace);
  const recoveredPages = await invoke('list_pages', { workspace: recoveredSpace.id });
  assert.ok(recoveredPages.some((item) => item.id === notionRoot.id));
  assert.ok(!recoveredPages.some((item) => item.id === later.id));
  assert.ok((await invoke('list_pages', { workspace: scheduleSpace })).some((item) => item.id === later.id));
  checks.push('桌面入口创建整库恢复包并恢复到新空间，旧页面 ID 可用，原工作空间保留快照后的新写入');

  const appearanceTitle = '页面外观验收';
  await button('新建根页面').click();
  await field('页面标题').fill(appearanceTitle);
  await body().fill('保留的试验正文。');
  const appearance = page.getByRole('dialog', { name: '页面外观' });
  const openAppearance = async () => {
    await button('页面操作').click();
    await button('页面外观').click();
    await appearance.waitFor();
  };
  const cover = page.getByRole('img', { name: '页面封面', exact: true });
  await openAppearance();
  await field('图标', appearance).selectOption('book');
  await button('导入图片', appearance).click();
  await button('保存外观', appearance).click();
  await cover.waitFor();
  assert.ok(await cover.evaluate((image) => image.decode().then(() => image.naturalWidth)));
  const appearanceNote = (await invoke('list_pages', { workspace: recoveredSpace.id })).find((item) => item.title === appearanceTitle);
  const appearanceSaved = await invoke('read_page', { workspace: recoveredSpace.id, id: appearanceNote.id });
  assert.equal(appearanceSaved.icon, 'book');
  assert.equal(appearanceSaved.blocks[0].payload.inline[0].text, '保留的试验正文。');
  assert.equal(appearanceSaved.cover.mediaType, 'image/png');
  assert.equal(await page.locator('.tree-row').filter({ has: button(appearanceTitle) }).locator('svg.lucide-book-open').count(), 1);
  await page.screenshot({ path: join(reportDir, 'page-appearance.png') });
  checks.push('打开页面外观先保存编辑内容，从本页导入封面并同步页面树图标');

  await button('页面操作').click();
  await button('导出 Markdown').click();
  await page.getByRole('status').filter({ hasText: 'Markdown 已导出。' }).waitFor();
  const exported = await readFile(join(sandbox, 'page.md'), 'utf8');
  assert.match(exported, /^# 页面外观验收\n\n!\[页面封面\]\(images\/[a-f0-9-]+\.png\)/);
  assert.deepEqual(await readFile(join(sandbox, 'images', `${appearanceSaved.cover.attachmentId}.png`)), await readFile(imageFixture));
  const appearanceLink = await invoke('make_location', { workspace: recoveredSpace.id, page: appearanceSaved.id, block: null });
  await page.evaluate((url) => sessionStorage.setItem('test-start-location', url), appearanceLink);
  await page.reload();
  await cover.waitFor();
  await body().getByText('保留的试验正文。', { exact: true }).waitFor();
  checks.push('页面封面跨重开显示，Markdown 导出保留相对图片路径与原件');

  await openAppearance();
  await field('图标', appearance).selectOption('file');
  await field('封面', appearance).selectOption('');
  await button('保存外观', appearance).click();
  await cover.waitFor({ state: 'hidden' });
  const retainedAssets = await invoke('list_attachments', { workspace: recoveredSpace.id, page: appearanceSaved.id });
  assert.ok(retainedAssets.some((item) => item.id === appearanceSaved.cover.attachmentId));
  await button('历史').click();
  await dialog().getByRole('button', { name: new RegExp(`^版本 ${appearanceSaved.revision}\\b`) }).click();
  await dialog().getByText('此版本含页面封面。', { exact: true }).waitFor();
  await button('恢复为新版本', dialog()).click();
  await cover.waitFor();
  const restoredAppearance = await invoke('read_page', { workspace: recoveredSpace.id, id: appearanceSaved.id });
  assert.equal(restoredAppearance.icon, appearanceSaved.icon);
  assert.deepEqual(restoredAppearance.cover, appearanceSaved.cover);
  assert.equal(restoredAppearance.revision, appearanceSaved.revision + 2);
  checks.push('清除封面保留本页原件，从历史恢复封面与图标生成新修订');

  const historyBeforeExport = await invoke('page_history', { workspace: recoveredSpace.id, id: appearanceSaved.id });
  assert.ok(historyBeforeExport.length > 1);
  await button('备份与恢复').click();
  await dialog().getByRole('checkbox', { name: '内容包包含修订历史' }).uncheck();
  await dialog().getByRole('button', { name: /^导出工作空间/ }).click();
  await page.getByText('工作空间包已导出。', { exact: true }).waitFor();
  const currentOnly = await invoke('verify_restore_content');
  const importedHistory = await invoke('page_history', { workspace: currentOnly.id, id: appearanceSaved.id });
  assert.deepEqual(importedHistory.map(({ revision }) => revision), [restoredAppearance.revision]);
  const importedPage = await invoke('read_page', { workspace: currentOnly.id, id: appearanceSaved.id });
  assert.equal(importedPage.revision, restoredAppearance.revision);
  assert.equal(importedPage.id, restoredAppearance.id);
  assert.deepEqual(importedPage.blocks, restoredAppearance.blocks);
  assert.deepEqual(importedPage.cover, restoredAppearance.cover);
  assert.deepEqual(await invoke('page_history', { workspace: recoveredSpace.id, id: appearanceSaved.id }), historyBeforeExport);
  checks.push('内容包可省略较早修订，恢复后核对当前正文、ID、封面及源库历史');

  const archiveBeforeMerge = await readFile(backup);
  const beforeMerge = await invoke('list_pages', { workspace: recoveredSpace.id });
  const corruptedArchive = Buffer.from(archiveBeforeMerge);
  const imageOffset = corruptedArchive.indexOf(await readFile(imageFixture));
  assert.ok(imageOffset > 0);
  corruptedArchive[imageOffset + 10] ^= 1;
  await writeFile(backup, corruptedArchive);
  await button('备份与恢复').click();
  const mergeButton = dialog().getByRole('button', { name: /^合并内容包/ });
  await mergeButton.click();
  await dialog().getByRole('alert').filter({ hasText: /附件|文件/ }).waitFor();
  assert.deepEqual(await invoke('list_pages', { workspace: recoveredSpace.id }), beforeMerge);
  const failedMerge = await invoke('latest_migration', { workspace: recoveredSpace.id, nativeContent: true });
  assert.equal(failedMerge.status, 'failed');
  await page.screenshot({ path: join(reportDir, 'content-merge-failed.png') });
  checks.push('内容包合并入口报告损坏原件，原空间的页面和修订不变，批次可重试');

  await writeFile(backup, archiveBeforeMerge);
  await mergeButton.click();
  await page.getByRole('status').filter({ hasText: /已将 \d+ 页合并到当前工作空间/ }).waitFor();
  const nativeMergeBatch = await invoke('latest_migration', { workspace: recoveredSpace.id, nativeContent: true });
  assert.equal(nativeMergeBatch.status, 'completed');
  assert.match(nativeMergeBatch.detail, /已合并 \d+ 页/);
  assert.equal(await field('页面标题').inputValue(), `${recoveredSpace.name}（导入）`);
  const afterMerge = await invoke('list_pages', { workspace: recoveredSpace.id });
  assert.equal(afterMerge.length, beforeMerge.length * 2 + 1);
  const copiedNote = afterMerge.find((item) => item.title === appearanceTitle && item.parentId === nativeMergeBatch.pageId);
  assert.ok(copiedNote && copiedNote.id !== appearanceSaved.id);
  const copiedAppearance = await invoke('read_page', { workspace: recoveredSpace.id, id: copiedNote.id });
  assert.notEqual(copiedAppearance.cover.attachmentId, restoredAppearance.cover.attachmentId);
  assert.deepEqual(await invoke('read_page', { workspace: recoveredSpace.id, id: appearanceSaved.id }), restoredAppearance);
  await button('备份与恢复').click();
  await dialog().getByText('最近一次合并完成 · workspace.zhiye.zip', { exact: true }).click();
  await dialog().getByText(/^已合并 \d+ 页/).waitFor();
  await page.screenshot({ path: join(reportDir, 'content-merge-result.png') });
  await button('关闭', dialog()).click();
  checks.push('内容包成功合并生成独立分支，复制封面映射到新附件，备份入口保留结果');

  {
    const cleanupSpace = await invoke('create_workspace', { name: '清空回收站验收夹具' });
    const workspace = cleanupSpace.id;
    const saveFixture = async (name, parentId, blocks = []) => {
      const page = await invoke('create_page', { workspace, parentId });
      return invoke('save_page', { workspace, id: page.id, revision: page.revision, draft: { title: name, blocks } });
    };
    const anchor = { id: randomUUID(), type: 'paragraph', schemaVersion: 1, children: [],
      payload: { inline: [{ type: 'text', text: '清空测试的目标原文' }] } };
    const doomed = await saveFixture('清空测试目录', null, [anchor]);
    const child = await saveFixture('清空测试子页', doomed.id);
    const survivor = await saveFixture('保留页面与引用', null, [{
      id: randomUUID(), type: 'blockReference', schemaVersion: 1, children: [],
      payload: { pageId: doomed.id, blockId: anchor.id },
    }]);
    const extra = await saveFixture('确认后才归档的页面', null);
    const asset = await invoke('add_attachments', { workspace, page: doomed.id });
    assert.equal(asset.added.length, 1);
    const startUrl = `zhiye://open/workspaces/${workspace}/pages/${survivor.id}`;
    await page.evaluate((url) => sessionStorage.setItem('test-start-location', url), startUrl);
    await page.reload();
    await body().getByText('清空测试的目标原文', { exact: true }).waitFor();
    await invoke('archive_pages', { workspace, pages: [{ id: doomed.id, revision: doomed.revision }] });
    await button('回收站').click();
    await button('清空回收站', dialog()).click();
    const clearDialog = page.getByRole('dialog', { name: '清空回收站' });
    await clearDialog.getByText(/永久删除 2 个页面/).waitFor();
    assert.ok(await button('取消', clearDialog).evaluate((control) => document.activeElement === control));
    await page.keyboard.press('Escape');
    await dialog().getByText('清空测试目录', { exact: true }).waitFor();
    assert.ok(await invoke('read_page', { workspace, id: child.id }));

    await button('清空回收站', dialog()).click();
    await clearDialog.waitFor();
    await invoke('archive_pages', { workspace, pages: [{ id: extra.id, revision: extra.revision }] });
    await button('确认永久删除', clearDialog).click();
    await dialog().getByRole('alert').filter({ hasText: /回收站或相关记录已变化/ }).waitFor();
    assert.ok(await invoke('read_page', { workspace, id: doomed.id }));
    await button('清空回收站', dialog()).click();
    await clearDialog.getByText(/永久删除 3 个页面/).waitFor();
    await page.screenshot({ path: join(reportDir, 'trash-confirmation.png') });
    await button('确认永久删除', clearDialog).click();
    await page.getByRole('status').filter({ hasText: '已永久删除 3 个页面' }).waitFor();
    await assert.rejects(invoke('read_page', { workspace, id: doomed.id }), { code: 'PAGE_NOT_FOUND' });
    await assert.rejects(invoke('read_page', { workspace, id: child.id }), { code: 'PAGE_NOT_FOUND' });
    assert.deepEqual(await invoke('list_pages', { workspace }), [survivor]);
    await body().getByText('源块不可用', { exact: true }).waitFor();
    checks.push('清空回收站默认聚焦取消，旧确认在回收站变化后失效，重新确认后引用显示不可用');

    await button('回收站').click();
    await dialog().getByText('回收站为空。', { exact: true }).waitFor();
    await button('检查附件清理', dialog()).click();
    await dialog().getByText(/1 份原件正在等待七天保留期/).waitFor();
    assert.equal(await button('确认清理', dialog()).isDisabled(), true);
    await button('关闭', dialog()).click();
    checks.push('清空后的孤立原件在回收站清理面板进入七天保留期');

    const { DatabaseSync } = await import('node:sqlite');
    const fixtureDb = new DatabaseSync(join(sandbox, 'workspaces', workspace, 'workspace.sqlite'));
    try {
      fixtureDb.exec('UPDATE blob_quarantine SET unreferenced_since = 0');
    } finally {
      fixtureDb.close();
    }
    await button('检查附件清理', dialog()).click();
    await dialog().getByText(/可清理 1 份无引用的原件/).waitFor();
    await button('确认清理', dialog()).click();
    await page.getByRole('status').filter({ hasText: '已清理 1 份原件' }).waitFor();
    const hash = asset.added[0].attachment.sha256;
    await assert.rejects(readFile(join(sandbox, 'workspaces', workspace, 'blobs', hash)), { code: 'ENOENT' });
    assert.deepEqual(await invoke('list_pages', { workspace }), [survivor]);
    checks.push('原件保留期结束后从正常入口确认清理，原件文件移除且保留页面仍可读');
  }

  if (process.argv.includes('--keyboard')) checks.push(...await verifyKeyboard({ page, invoke, origin, reportDir }));

  const conversionWorkspace = (await invoke('create_workspace', { name: '属性转换验收' })).id;
  const conversionCollection = (await invoke('create_collection', { workspace: conversionWorkspace, parentId: null })).collection;
  const collectionArgs = { workspace: conversionWorkspace, collection: conversionCollection.id };
  const originalSchema = await invoke('read_collection', collectionArgs);
  const sourceSchema = await invoke('save_property', { ...collectionArgs, revision: originalSchema.schemaRevision,
    draft: { id: null, name: '原始评分', type: 'text', options: [] } });
  const rating = sourceSchema.properties.find(({ name }) => name === '原始评分');
  const createRating = async (name, value) => {
    const record = await invoke('create_page', { workspace: conversionWorkspace, parentId: conversionCollection.containerPageId });
    return invoke('save_record', { ...collectionArgs, id: record.id, revision: record.revision,
      draft: { title: name, schemaRevision: sourceSchema.schemaRevision, properties: { [rating.id]: value } } });
  };
  const zero = await createRating('零分记录', '0');
  const unavailable = await createRating('待判断记录', '确认后填写');
  await deliver(await invoke('make_location', { workspace: conversionWorkspace, page: conversionCollection.containerPageId }));
  const openConversion = async () => {
    await button('管理属性').click();
    await field('要编辑的属性', dialog()).selectOption(rating.id);
    assert.equal(await field('属性类型', dialog()).isEnabled(), true, '已有文本属性应提供类型转换入口');
    await field('属性类型', dialog()).selectOption('number');
    await button('预览转换', dialog()).click();
    await dialog().getByRole('heading', { name: '原始评分 · 文本 → 数字' }).waitFor();
  };
  await openConversion();
  await dialog().getByText('可转换 1 条记录，1 条记录将留空。原值保留在页面历史中。').waitFor();
  await dialog().getByRole('row', { name: /待判断记录.*确认后填写.*留空/ }).waitFor();
  await page.screenshot({ path: join(reportDir, 'property-conversion.png') });
  await button('取消', dialog()).click();
  assert.deepEqual(await invoke('read_collection', collectionArgs), sourceSchema);
  await openConversion();
  const laterRating = await createRating('确认前新增评分', '42');
  await button('确认转换', dialog()).click();
  await dialog().getByRole('alert').filter({ hasText: '请重新预览' }).waitFor();
  await button('返回修改', dialog()).click();
  await button('预览转换', dialog()).click();
  await dialog().getByText('可转换 2 条记录，1 条记录将留空。原值保留在页面历史中。').waitFor();
  await button('确认转换', dialog()).click();
  await dialog().waitFor({ state: 'hidden' });
  const currentSchema = await invoke('read_collection', collectionArgs);
  assert.equal(currentSchema.properties.find(({ id }) => id === rating.id).type, 'number');
  for (const [record, expected] of [[zero, 0], [laterRating, 42], [unavailable, undefined]]) {
    const stored = await invoke('read_page', { workspace: conversionWorkspace, id: record.id });
    assert.equal(stored.properties[rating.id], expected);
    assert.equal(stored.revision, record.revision + 1);
  }
  await button('待判断记录', table()).click();
  await button('历史').click();
  await dialog().getByRole('button', { name: new RegExp(`版本 ${unavailable.revision}`) }).click();
  await dialog().getByText('确认后填写', { exact: true }).waitFor();
  await dialog().getByText('此版本包含已变更类型的属性。恢复时按当前类型转换，无法转换的值将留空。').waitFor();
  checks.push('桌面属性类型转换预览显示留空记录，取消及旧预览保留原数据，确认后可查看原值的历史');

  await button('关闭', dialog()).click();

  {
    const workspace = (await invoke('create_workspace', { name: '归档后保存故障' })).id;
    const createNote = async (name, text, parentId = null) => {
      const note = await invoke('create_page', { workspace, parentId });
      const paragraph = block('paragraph', { inline: [{ type: 'text', text }] });
      return invoke('save_page', { workspace, id: note.id, revision: note.revision,
        draft: { title: name, blocks: [paragraph] } });
    };
    const openNote = (id) => invoke('make_location', { workspace, page: id }).then(deliver);
    const saveError = page.locator('.save-error');
    const savedDraftFile = join(sandbox, 'editor-draft.json');
    const root = await createNote('归档分支验收', '分支正文');
    const child = await createNote('原始标题', '已经提交的正文', root.id);
    await openNote(child.id);
    await body().getByText('已经提交的正文', { exact: true }).waitFor();
    const original = await invoke('read_page', { workspace, id: child.id });
    const remote = await invoke('save_page', { workspace, id: child.id, revision: original.revision,
      draft: { title: '外部更新的标题', blocks: original.blocks } });
    assert.equal(await invoke('archive_pages', { workspace, pages: [{ id: root.id, revision: root.revision }] }), 2);
    await body().fill('分支恢复后要保留的本机草稿');
    await save();
    await button('对比版本').waitFor();

    await button('回收站').click();
    await dialog().getByText(root.title, { exact: true }).waitFor();
    await button('恢复', dialog()).click();
    await dialog().waitFor({ state: 'hidden' });
    assert.equal(await title().inputValue(), child.title);
    await body().getByText('分支恢复后要保留的本机草稿', { exact: true }).waitFor();
    const restored = await invoke('read_page', { workspace, id: child.id });
    assert.equal(restored.trashRootId, null);
    assert.equal(restored.parentId, root.id);
    assert.equal(restored.title, remote.title);
    assert.deepEqual(restored.blocks, original.blocks);
    await button('对比版本').click();
    await dialog().getByText(remote.title, { exact: true }).waitFor();
    await field('保留正文', dialog()).selectOption('local');
    await button('保存所选版本', dialog()).click();
    await dialog().waitFor({ state: 'hidden' });
    const combined = await invoke('read_page', { workspace, id: child.id });
    assert.equal(combined.revision, restored.revision + 1);
    assert.equal(combined.title, remote.title);
    assert.equal(combined.blocks[0].id, original.blocks[0].id);
    assert.equal(combined.blocks[0].payload.inline[0].text, '分支恢复后要保留的本机草稿');
    checks.push('恢复被归档的当前分支时保留子页草稿，版本对比保存外部标题和本机正文');

    const removed = await createNote('将被永久移除的页面', '用于验证关闭的正文');
    await openNote(removed.id);
    await body().getByText('用于验证关闭的正文', { exact: true }).waitFor();
    await invoke('archive_pages', { workspace, pages: [{ id: removed.id, revision: removed.revision }] });
    const plan = await invoke('preview_empty_trash', { workspace });
    assert.equal((await invoke('empty_trash', { workspace, confirmation: plan.confirmation })).pages, 1);
    await title().fill('关闭前要导出的本机标题');
    await body().fill('关闭前要导出的本机正文');
    await save();
    await saveError.getByText('找不到这个页面。').waitFor();
    await page.evaluate(() => window.testRequestDesktopClose());
    assert.equal(await page.evaluate(() => window.testWindowDestroyed), undefined);
    await button('关闭页面', saveError).click();
    await dialog().getByRole('heading', { name: '关闭未保存的页面' }).waitFor();
    assert.equal(await button('返回草稿', dialog()).evaluate((node) => node === document.activeElement), true);
    await page.keyboard.press('Escape');
    await dialog().waitFor({ state: 'hidden' });
    assert.equal(await title().inputValue(), '关闭前要导出的本机标题');
    await button('导出草稿', saveError).click();
    await saveError.getByText('草稿已导出。').waitFor();
    const exported = await readFile(savedDraftFile, 'utf8');
    const document = JSON.parse(exported);
    assert.equal(document.pageId, removed.id);
    assert.equal(document.baseRevision, removed.revision);
    assert.equal(document.draft.title, '关闭前要导出的本机标题');
    assert.equal(document.draft.blocks[0].payload.inline[0].text, '关闭前要导出的本机正文');

    await button('关闭页面', saveError).click();
    await button('放弃草稿并关闭', dialog()).click();
    await title().waitFor({ state: 'hidden' });
    assert.equal(await readFile(savedDraftFile, 'utf8'), exported);
    await page.evaluate(() => window.testRequestDesktopClose());
    assert.equal(await page.evaluate(() => window.testWindowDestroyed), true);
    await openNote(child.id);
    await body().getByText('分支恢复后要保留的本机草稿', { exact: true }).waitFor();
    assert.deepEqual(await invoke('read_page', { workspace, id: child.id }), combined);
    await assert.rejects(invoke('read_page', { workspace, id: removed.id }), { code: 'PAGE_NOT_FOUND' });
    checks.push('已永久移除页面可取消关闭、导出草稿、确认后关闭并继续打开其他页面');
  }

  // Freeze the debounce clock only after the pointer/IME cases have finished.
  const startupSpace = (await invoke('list_workspaces')).workspaces[0];
  await button('切换工作空间').click();
  await button(startupSpace.name, page.locator('.workspace-menu')).click();
  await page.waitForFunction(() => document.querySelector('.app-shell')?.getAttribute('aria-busy') === 'false');
  const nativePagesBefore = new Set((await invoke('list_pages', { workspace: startupSpace.id })).map(({ id }) => id));
  await page.evaluate(() => window.testNativeMenuCommand('new-page'));
  await page.waitForFunction(() => document.querySelector('textarea[aria-label="页面标题"]')?.value === ''
    && document.querySelector('.app-shell')?.getAttribute('aria-busy') === 'false');
  const nativePagesAdded = (await invoke('list_pages', { workspace: startupSpace.id }))
    .filter(({ id }) => !nativePagesBefore.has(id));
  assert.equal(nativePagesAdded.length, 1);
  await page.clock.pauseAt(new Date());
  await title().fill('原生菜单回归');
  await body().fill('通过原生菜单保存的正文。');
  await page.evaluate(() => window.testNativeMenuCommand('save'));
  await page.getByRole('status').filter({ hasText: /^已保存$/ }).waitFor();
  const nativeSaved = await invoke('read_page', { workspace: startupSpace.id, id: nativePagesAdded[0].id });
  assert.equal(nativeSaved.title, '原生菜单回归');
  assert.equal(nativeSaved.blocks[0].payload.inline[0].text, '通过原生菜单保存的正文。');
  await page.clock.resume();
  await page.reload();
  assert.equal(await title().inputValue(), '原生菜单回归');
  await body().getByText('通过原生菜单保存的正文。', { exact: true }).waitFor();
  checks.push('原生菜单在自动保存计时暂停时提交正文，重新加载自动回到同一页');

  await page.clock.pauseAt(new Date());
  await title().fill('系统退出提交的标题');
  await page.evaluate(() => {
    const adapter = window.__TAURI_INTERNALS__;
    const invoke = adapter.invoke;
    adapter.invoke = async (...args) => {
      if (args[0] !== 'save_page') return invoke(...args);
      adapter.invoke = invoke;
      await new Promise((resolve) => { window.testReleaseQuitSave = resolve; });
      return invoke(...args);
    };
    window.testPendingQuit = window.testRequestSystemQuit('save-then-quit');
  });
  await page.locator('.app-shell[aria-busy="true"]').waitFor();
  await page.evaluate(() => window.testRequestSystemQuit('quit-during-save'));
  const busyQuitReply = { request: 'quit-during-save', saved: false };
  assert.deepEqual(await page.evaluate(() => window.testQuitReplies), [busyQuitReply]);
  assert.deepEqual(await invoke('read_page', { workspace: startupSpace.id, id: nativeSaved.id }), nativeSaved);
  await page.evaluate(async () => {
    window.testReleaseQuitSave();
    await window.testPendingQuit;
  });
  assert.deepEqual(await page.evaluate(() => window.testQuitReplies), [
    busyQuitReply, { request: 'save-then-quit', saved: true },
  ]);
  assert.equal(await page.evaluate(() => window.testWindowDestroyed), undefined);
  const quitSaved = await invoke('read_page', { workspace: startupSpace.id, id: nativeSaved.id });
  assert.equal(quitSaved.title, '系统退出提交的标题');
  assert.equal(quitSaved.revision, nativeSaved.revision + 1);
  assert.deepEqual(quitSaved.blocks, nativeSaved.blocks);
  checks.push('保存进行中的第二次退出请求被拒绝，原请求只提交一次');
  checks.push('系统退出等到实际提交完成后才允许退出');

  await invoke('archive_pages', { workspace: startupSpace.id, pages: [{ id: quitSaved.id, revision: quitSaved.revision }] });
  await title().fill('失败后应保留的系统退出草稿');
  await page.evaluate(() => window.testRequestSystemQuit('failed-quit'));
  assert.deepEqual(await page.evaluate(() => window.testQuitReplies.at(-1)), { request: 'failed-quit', saved: false });
  assert.equal(await page.evaluate(() => window.testWindowDestroyed), undefined);
  assert.equal(await title().inputValue(), '失败后应保留的系统退出草稿');
  assert.equal((await invoke('read_page', { workspace: startupSpace.id, id: quitSaved.id })).title, quitSaved.title);
  await button('导出草稿').waitFor();
  await page.clock.resume();
  checks.push('系统退出遇到保存失败时取消退出并保留可导出的草稿');

  assert.deepEqual(errors, []);
  assert.deepEqual(external, []);
  await writeFile(join(reportDir, 'report.json'), JSON.stringify({
    passed: true, browser: `Chromium ${browserVersion}`, platform: `${process.platform} ${process.arch}`,
    frontend: 'production Vite bundle', backend: 'zhiye-core with real SQLite via isolated test adapter',
    nativeDialogs: false, checks, pageErrors: errors, externalRequests: external,
  }, null, 2) + '\n');
  process.stdout.write(checks.map((check) => `PASS ${check}\n`).join(''));
} catch (error) {
  process.stderr.write(JSON.stringify({ checks, errors, external }) + '\n');
  if (browser) {
    const [page] = browser.contexts().flatMap((context) => context.pages());
    await page?.screenshot({ path: join(reportDir, 'failure.png'), timeout: 2500,
      mask: [page.locator('#integration-token')] }).catch(() => {});
  }
  const safeStack = (error instanceof Error ? String(error.stack) : JSON.stringify(error))
    .replace(/zhiye_[a-f\d]{64}/g, '[test credential]');
  throw new Error(safeStack);
} finally {
  await browser?.close();
  server.close();
  await stopBackend();
  await rm(sandbox, { recursive: true, force: true });
}
