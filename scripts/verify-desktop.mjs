// SPDX-License-Identifier: AGPL-3.0-or-later

import assert from 'node:assert/strict';
import { copyFile, mkdir, mkdtemp, readFile, readdir, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { parseArgs } from 'node:util';
import { environment, fileSha256 } from './performance/measure.mjs';
import { verifyKeyboard } from './verify-keyboard.mjs';
import { startWebView2, tauriIpcOrigin, tauriOrigin } from './webview2.mjs';

const { positionals, values: { keyboard, installed, scratch: scratchPath } } = parseArgs({ allowPositionals: true, options: {
  keyboard: { type: 'boolean' }, installed: { type: 'boolean' }, scratch: { type: 'string' },
} });
assert.ok(positionals.length <= 1 && (!installed || positionals[0]),
  'usage: node scripts/verify-desktop.mjs [executable] [--keyboard] [--installed] [--scratch directory]');
assert.equal(process.platform, 'win32', 'Desktop verification requires Windows WebView2');
const executable = resolve(positionals[0] ?? 'artifacts/Zhiye-Demo-win-x64.exe');
const scratch = resolve(scratchPath ?? '.verify');
await mkdir(scratch, { recursive: true });
const test = await mkdtemp(join(scratch, 'desktop-'));
const data = join(test, 'workspaces');
const running = new Set();
const errors = [];
const externalRequests = [];
const report = { executable, launch: installed ? 'original executable' : 'isolated copy', testDirectory: test,
  environment: environment(), pageErrors: errors, blockedExternalRequests: externalRequests };
const pdf = samplePdf();
const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO7+1gAAAABJRU5ErkJggg==', 'base64');

function samplePdf() {
  const drawing = ['0 0 1 rg 10 10 180 90 re f', '1 0 0 rg 10 10 180 90 re f\n0 0 0 rg BT /F1 12 Tf 100 47 Td (PAGE TWO) Tj ET'];
  const objects = [
    '<< /Type /Catalog /Pages 2 0 R >>',
    '<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 110] /Contents 5 0 R >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 110] /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> >> >>',
    ...drawing.map((commands) => `<< /Length ${commands.length} >>\nstream\n${commands}\nendstream`),
    '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
  ];
  const chunks = ['%PDF-1.4\n'];
  const offsets = [0];
  for (const [index, value] of objects.entries()) {
    offsets.push(chunks.join('').length);
    chunks.push(`${index + 1} 0 obj\n${value}\nendobj\n`);
  }
  const crossRef = chunks.join('').length;
  chunks.push(`xref\n0 ${offsets.length}\n0000000000 65535 f \n`);
  for (const offset of offsets.slice(1)) chunks.push(`${String(offset).padStart(10, '0')} 00000 n \n`);
  chunks.push(`trailer\n<< /Size ${offsets.length} /Root 1 0 R >>\nstartxref\n${crossRef}\n%%EOF\n`);
  return Buffer.from(chunks.join(''), 'ascii');
}

async function start(label) {
  const app = installed ? executable : join(test, `${label}.exe`);
  if (!installed) await copyFile(executable, app);
  const instance = await startWebView2(app, data, join(test, 'webview', label));
  running.add(instance);
  const { browser, page } = instance;
  report.engine = `WebView2 ${browser.version()}`;
  page.setDefaultTimeout(8000);
  page.on('pageerror', (error) => errors.push(error.message));
  await page.route(/^https?:/, (route) => {
    const url = new URL(route.request().url());
    if (url.origin === tauriOrigin || url.origin === tauriIpcOrigin) return route.continue();
    externalRequests.push(url.origin);
    return route.abort();
  });
  return instance;
}

const title = (page) => page.getByRole('textbox', { name: '页面标题' });
const body = (page) => page.getByRole('textbox', { name: '页面正文' });
const tree = (page) => page.getByRole('navigation', { name: '页面树' });
const menu = (page) => page.getByRole('button', { name: '页面操作' }).click();
const findPage = (page, name) => tree(page).getByRole('button', { name, exact: true });

async function waitForTitle(page, expected) {
  await page.waitForFunction((value) => document.querySelector('[aria-label="页面标题"]')?.value === value, expected);
}

async function stop(instance) {
  await instance.close();
  running.delete(instance);
}

try {
  report.executableSha256 = await fileSha256(executable);
  const first = await start('first');
  const page = first.page;
  await findPage(page, '笔记').click();
  await menu(page);
  await page.getByRole('button', { name: '新建子页面', exact: true }).click();
  await waitForTitle(page, '');
  await title(page).fill('桌面回归验证');
  await title(page).press('Enter');
  await page.locator('.document-body:focus').waitFor();
  await page.keyboard.type('/');
  await page.locator('.slash-menu').getByRole('button', { name: '二级标题', exact: true }).click();
  await page.keyboard.insertText('中文标题与引用');
  await page.keyboard.press('Enter');
  await page.keyboard.insertText('海棠书签的测试内容');
  await body(page).getByRole('heading', { name: '中文标题与引用', level: 2 }).waitFor();
  await page.getByRole('button', { name: '引用页面', exact: true }).click();
  const picker = page.getByRole('dialog', { name: '引用页面' });
  await picker.getByRole('textbox', { name: '查找引用页面' }).fill('阅读笔记');
  await picker.getByRole('button', { name: '阅读笔记 笔记', exact: true }).click();
  const reference = body(page).getByRole('link', { name: /阅读笔记/ });
  await reference.waitFor();
  await page.getByRole('status').filter({ hasText: /^已保存$/ }).waitFor();

  const editorBlocks = body(page).locator(':scope > [data-id]');
  const originalBlockIds = await editorBlocks.evaluateAll((nodes) => nodes.map((node) => node.dataset.id));
  assert.equal(originalBlockIds.length, 2);
  const reorderedBlockIds = [...originalBlockIds].reverse();
  const waitForBlocks = (ids) => page.waitForFunction((expected) => {
    const actual = Array.from(document.querySelectorAll('.document-body > [data-id]'), (node) => node.dataset.id);
    return actual.length === expected.length && actual.every((id, index) => id === expected[index]);
  }, ids);
  const heading = body(page).getByRole('heading', { name: '中文标题与引用' });
  const paragraph = body(page).locator('p').filter({ hasText: '海棠书签的测试内容' });
  await paragraph.click({ position: { x: 12, y: 9 } });
  await page.keyboard.press('Alt+Shift+ArrowUp');
  await waitForBlocks(reorderedBlockIds);
  await page.getByRole('button', { name: '撤销', exact: true }).click();
  await waitForBlocks(originalBlockIds);
  await heading.hover();
  const grip = page.locator('.block-drag-outer');
  await grip.waitFor({ state: 'visible' });
  const handleBounds = await grip.boundingBox();
  const target = await paragraph.boundingBox();
  assert.ok(handleBounds && target);
  await page.mouse.move(handleBounds.x + handleBounds.width / 2, handleBounds.y + handleBounds.height / 2);
  await page.mouse.down();
  await page.mouse.move(target.x + target.width / 3, target.y + target.height * .8, { steps: 12 });
  await page.locator('.block-drop-line').waitFor();
  await page.mouse.up();
  await waitForBlocks(reorderedBlockIds);
  await page.keyboard.press('Control+s');
  await page.getByRole('status').filter({ hasText: /^已保存$/ }).waitFor();

  await page.keyboard.press('Control+k');
  const search = page.getByRole('dialog', { name: '搜索页面' });
  await search.getByRole('textbox', { name: '搜索关键词' }).fill('海棠书签');
  const searchHit = search.getByRole('button', { name: /桌面回归验证/ });
  await searchHit.locator('mark').filter({ hasText: /^海棠书签$/ }).waitFor();
  await searchHit.click();
  await waitForTitle(page, '桌面回归验证');
  await body(page).locator('[data-search-target="true"]').filter({ hasText: '海棠书签' }).waitFor();
  await reference.click();
  await waitForTitle(page, '阅读笔记');
  await findPage(page, '桌面回归验证').click();
  await menu(page);
  await page.getByRole('button', { name: '移动页面', exact: true }).click();
  const move = page.getByRole('dialog', { name: '移动页面' });
  await move.getByRole('combobox').selectOption({ label: '收集箱' });
  await move.getByRole('button', { name: '移动', exact: true }).click();
  await page.getByRole('navigation', { name: '页面位置' }).getByRole('button', { name: '收集箱' }).waitFor();
  await page.getByRole('button', { name: '历史', exact: true }).click();
  const history = page.getByRole('dialog', { name: '页面历史' });
  await history.getByText('当前版本', { exact: true }).waitFor();
  assert.ok(await history.getByRole('button', { name: /版本 2/ }).count());
  await history.getByRole('button', { name: '关闭', exact: true }).click();
  await menu(page);
  await page.getByRole('button', { name: '移入回收站', exact: true }).click();
  await page.getByRole('dialog', { name: '移入回收站' }).getByRole('button', { name: '移入回收站', exact: true }).click();
  await findPage(page, '桌面回归验证').waitFor({ state: 'detached' });
  await page.getByRole('button', { name: '回收站', exact: true }).click();
  await page.getByRole('dialog', { name: '回收站' }).getByRole('button', { name: '恢复', exact: true }).click();
  await waitForTitle(page, '桌面回归验证');
  await body(page).getByRole('heading', { name: '中文标题与引用' }).waitFor();
  await page.screenshot({ path: join(test, 'desktop.png') });

  const second = await start('second');
  await second.page.getByRole('alert').filter({ hasText: '另一个知页窗口使用' }).waitFor();
  await stop(first);
  await second.page.getByRole('button', { name: '重试', exact: true }).click();
  await findPage(second.page, '桌面回归验证').click();
  await body(second.page).getByText('海棠书签的测试内容', { exact: false }).waitFor();
  const workspaceLabel = await second.page.locator('.workspace-label small').innerText();
  assert.equal(workspaceLabel, '我的知识库');
  await stop(second);

  const workspaceId = (await readdir(data)).find((entry) => /^[0-9a-f-]{36}$/.test(entry));
  assert.ok(workspaceId);
  await seedAttachments(workspaceId);

  const badId = '98b0a708-8956-42a9-ae98-d6579f1e4686';
  await mkdir(join(data, badId));
  await writeFile(join(data, badId, 'workspace.sqlite'), 'Not a SQLite database');
  const third = await start('third');
  await findPage(third.page, '桌面回归验证').click();
  await third.page.getByRole('button', { name: '附件', exact: true }).click();
  const attachmentList = third.page.getByRole('dialog', { name: '页面附件' });
  await attachmentList.getByText('2 个文件').waitFor();
  await attachmentList.getByRole('button', { name: '重命名 预览图片.png', exact: true }).click();
  const rename = third.page.getByRole('dialog', { name: '重命名附件' });
  await rename.getByRole('textbox', { name: '附件名称' }).fill('已改名图片.png');
  await rename.getByRole('button', { name: '保存名称', exact: true }).click();
  await attachmentList.getByRole('button', { name: /已改名图片\.png/ }).first().click();
  const image = third.page.getByRole('img', { name: '已改名图片.png' });
  await image.waitFor();
  await third.page.waitForFunction(() => document.querySelector('.attachment-image')?.naturalWidth === 1);
  await third.page.getByRole('button', { name: '返回列表', exact: true }).click();
  await attachmentList.getByRole('button', { name: /预览文档\.pdf/ }).first().click();
  const firstPdf = third.page.locator('canvas[aria-label="PDF 第 1 页"]');
  await firstPdf.waitFor({ state: 'visible', timeout: 15_000 });
  assert.deepEqual(await firstPdf.evaluate((canvas) => [...canvas.getContext('2d').getImageData(50, 50, 1, 1).data]), [0, 0, 255, 255]);
  await third.page.getByRole('button', { name: '下一页', exact: true }).click();
  const secondPdf = third.page.locator('canvas[aria-label="PDF 第 2 页"]');
  await secondPdf.waitFor({ state: 'visible' });
  assert.deepEqual(await secondPdf.evaluate((canvas) => [...canvas.getContext('2d').getImageData(50, 50, 1, 1).data]), [255, 0, 0, 255]);
  assert.ok(await secondPdf.evaluate((canvas) => {
    const pixels = canvas.getContext('2d').getImageData(157, 77, 110, 25).data;
    for (let offset = 0; offset < pixels.length; offset += 4) {
      if (pixels[offset] < 30 && pixels[offset + 1] < 30 && pixels[offset + 2] < 30 && pixels[offset + 3] === 255) return true;
    }
    return false;
  }), 'The second page should render black PDF text over the red background');
  await third.page.getByRole('dialog', { name: '预览文档.pdf' }).getByRole('button', { name: '关闭', exact: true }).click();
  await third.page.getByRole('button', { name: '1 个工作空间无法打开' }).click();
  await third.page.getByRole('dialog', { name: '无法打开的工作空间' }).getByText(badId).waitFor();
  await third.page.getByRole('dialog', { name: '无法打开的工作空间' }).getByRole('button', { name: '备份与恢复' }).click();
  await third.page.getByRole('dialog', { name: '备份与恢复' }).getByRole('button', { name: '从备份恢复' }).waitFor();
  assert.equal((await readFile(join(data, badId, 'workspace.sqlite'))).toString(), 'Not a SQLite database');
  const healthy = await third.page.evaluate(async () => {
    const workspaces = await window.__TAURI_INTERNALS__.invoke('list_workspaces');
    return workspaces.workspaces[0].id;
  });
  const database = new DatabaseSync(join(data, healthy, 'workspace.sqlite'), { readOnly: true });
  try {
    const note = database.prepare('SELECT title, parent_id, revision, blocks FROM pages WHERE title = ?').get('桌面回归验证');
    assert.ok(note?.parent_id && note.revision > 1);
    assert.deepEqual(JSON.parse(note.blocks).map((block) => block.id), reorderedBlockIds);
    assert.ok(database.prepare('SELECT title FROM pages WHERE id = ?').get(note.parent_id).title === '收集箱');
  } finally {
    database.close();
  }
  const keyboardChecks = keyboard ? await verifyKeyboard({
    page: third.page,
    origin: new URL(third.page.url()).origin,
    reportDir: test,
    invoke: (command, args) => third.page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args }),
  }) : [];
  assert.deepEqual(errors, []);
  assert.deepEqual(externalRequests, []);
  Object.assign(report, { screenshot: join(test, 'desktop.png'), checks: [
    'edit', 'keyboard-and-pointer-block-move', 'block-order-and-ids-after-restart', 'search-snippet-and-block-location',
    'reference', 'move', 'history', 'archive', 'restore', 'restart', 'lock-retry', 'damaged-workspace-recovery-entry',
    'attachment-rename-and-image-preview', 'pdf-text-preview-and-page-turn', ...keyboardChecks,
  ], passed: true });
} catch (error) {
  report.passed = false;
  report.error = error.stack ?? String(error);
  process.exitCode = 1;
} finally {
  const cleanup = await Promise.allSettled([...running].map((instance) => instance.close()));
  const failures = cleanup.filter((result) => result.status === 'rejected').map(({ reason }) => String(reason));
  if (failures.length) {
    report.passed = false;
    report.cleanupErrors = failures;
    process.exitCode = 1;
  }
  await writeFile(join(test, 'report.json'), JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report, null, 2));
}

async function seedAttachments(workspace) {
  const { createHash, randomUUID } = await import('node:crypto');
  const root = join(data, workspace);
  const directory = join(root, 'blobs');
  await mkdir(directory, { recursive: true });
  const db = new DatabaseSync(join(root, 'workspace.sqlite'));
  try {
    const page = db.prepare('SELECT id FROM pages WHERE title=?').get('桌面回归验证');
    const insert = db.prepare('INSERT INTO attachments VALUES(?,?,?,?,?,?,?)');
    for (const [name, bytes, type] of [['预览图片.png', png, 'image/png'], ['预览文档.pdf', pdf, 'application/pdf']]) {
      const digest = createHash('sha256').update(bytes).digest('hex');
      await writeFile(join(directory, digest), bytes);
      insert.run(randomUUID(), page.id, name, digest, bytes.length, type, Date.now());
    }
  } finally {
    db.close();
  }
}
