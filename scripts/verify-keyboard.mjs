// SPDX-License-Identifier: AGPL-3.0-or-later

import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';

export async function verifyKeyboard({ page, invoke, origin, reportDir }) {
  const workspaceName = '键盘回归验收';
  const { id: workspace } = await invoke('create_workspace', { name: workspaceName });
  async function add(title, parentId = null, blocks = []) {
    const page = await invoke('create_page', { workspace, parentId });
    await invoke('save_page', { workspace, id: page.id, revision: page.revision, draft: { title, blocks } });
    return page.id;
  }
  const parent = await add('键盘甲目录');
  await add('键盘甲子页', parent);
  await add('键盘乙笔记');
  const button = (name, scope = page) => scope.getByRole('button', { name, exact: true });
  const title = page.getByRole('textbox', { name: '页面标题', exact: true });
  const body = page.getByRole('textbox', { name: '页面正文', exact: true });
  const tree = page.getByRole('navigation', { name: '页面树' });
  const sidebar = page.getByRole('complementary', { name: '工作空间侧栏' });
  const workspaceButton = button('切换工作空间');
  const press = (key) => page.keyboard.press(key);
  const dialog = (name) => page.getByRole('dialog', { name, exact: true });
  const folder = button('键盘甲目录', tree);
  const child = button('键盘甲子页', tree);
  const tail = button('键盘乙笔记', tree);
  const assertFocused = async (target) => {
    assert.equal(await target.evaluate((node) => node === document.activeElement), true, 'focus is on the expected control');
  };
  await page.emulateMedia({ colorScheme: 'light' });

  async function showFixture() {
    await page.goto(origin);
    await workspaceButton.click();
    await button(workspaceName).click();
    await folder.waitFor();
    await page.waitForFunction(() => document.querySelector('.app-shell')?.getAttribute('aria-busy') === 'false');
  }

  const cases = [];
  async function check(name, run) {
    await showFixture();
    try {
      await run();
      cases.push({ name, passed: true });
    } catch (error) {
      cases.push({ name, passed: false, detail: String(error?.message ?? error).slice(0, 400) });
    }
  }

  await check('首个 Tab 能跳过侧栏直达页面内容', async () => {
    await page.reload();
    await workspaceButton.waitFor();
    await press('Tab');
    await assertFocused(page.getByRole('link', { name: '跳到页面内容' }));
    await press('Enter');
    assert.equal(await page.evaluate(() => document.activeElement?.id), 'page-content');
  });

  await check('标题和正文都有可见键盘焦点', async () => {
    await folder.focus();
    await press('Tab');
    for (const target of [title, body]) {
      await target.focus();
      assert.deepEqual(await target.evaluate((node) => {
        const { outlineStyle, outlineWidth } = getComputedStyle(node);
        return [node.matches(':focus-visible'), outlineStyle, parseFloat(outlineWidth) >= 2];
      }), [true, 'solid', true]);
    }
  });

  await check('页面树支持上下与首尾移动、展开、返回父级和 Enter 打开', async () => {
    await folder.focus();
    await press('ArrowDown');
    await assertFocused(child);
    await press('ArrowLeft');
    await assertFocused(folder);
    await press('ArrowLeft');
    await child.waitFor({ state: 'hidden', timeout: 2000 });
    await press('ArrowRight');
    await child.waitFor({ timeout: 2000 });
    await press('ArrowRight');
    await assertFocused(child);
    await press('End');
    await assertFocused(tail);
    await press('Home');
    await assertFocused(folder);
    await press('ArrowDown');
    await press('Enter');
    await page.waitForFunction(() => document.querySelector('.document-title')?.value === '键盘甲子页');
    await assertFocused(child);
  });

  await check('下拉可按 Esc 关闭，新建弹窗的 Tab 循环与关闭后焦点正确', async () => {
    await workspaceButton.focus();
    await press('Enter');
    const create = button('新建工作空间');
    await create.focus();
    await press('Escape');
    assert.equal(await workspaceButton.getAttribute('aria-expanded'), 'false');
    await assertFocused(workspaceButton);
    await press('Enter');
    await create.focus();
    await press('Enter');
    const modal = dialog('新建工作空间');
    await assertFocused(modal.getByRole('textbox', { name: '名称' }));
    await press('Shift+Tab');
    await assertFocused(button('关闭', modal));
    await press('Shift+Tab');
    await assertFocused(button('取消', modal));
    await press('Tab');
    await assertFocused(button('关闭', modal));
    await press('Escape');
    await modal.waitFor({ state: 'hidden' });
    await assertFocused(workspaceButton);
  });

  await check('搜索高亮可读，Enter 打开方向键选中的页面', async () => {
    await press('Control+k');
    const modal = dialog('搜索页面');
    const input = modal.getByRole('textbox', { name: '搜索关键词' });
    await input.fill('键盘甲');
    const rows = modal.locator('.picker-row');
    await rows.nth(1).waitFor();
    await press('ArrowDown');
    const expected = await rows.nth(1).locator('strong').innerText();
    const described = await input.evaluate((node) => document.getElementById(node.getAttribute('aria-describedby'))?.textContent);
    assert.ok(described?.includes(expected), 'highlighted result is announced to assistive technology');
    await press('Enter');
    await modal.waitFor({ state: 'hidden' });
    assert.equal(await title.inputValue(), expected);
  });

  await check('任务复选框各有中文名称，子项改名和空格切换保持独立', async () => {
    const block = (type, payload, children = []) => ({ id: randomUUID(), type, schemaVersion: 1, payload, children });
    const paragraph = (inline) => block('paragraph', { inline });
    const text = (value) => paragraph([{ type: 'text', text: value }]);
    const task = (checked, ...children) => block('taskItem', { checked }, children);
    const nested = task(true, text('核对许可'));
    const parentTask = task(false, text('准备发布'), block('taskList', {}, [nested]));
    const referenceTask = task(false, paragraph([{ type: 'pageReference', pageId: parent, label: '键盘甲目录' }]));
    const id = await add('任务复选框名称', null, [block('taskList', {}, [parentTask, referenceTask, task(false, paragraph([]))])]);
    await showFixture();
    await button('任务复选框名称', tree).click();
    const checkboxes = body.getByRole('checkbox');
    await checkboxes.last().waitFor();
    const names = ['任务：准备发布', '任务：核对许可', '任务：键盘甲目录', '空白任务'];
    assert.deepEqual(await checkboxes.evaluateAll((items) => items.map((item) => item.getAttribute('aria-label'))), names);
    const checkbox = (name) => body.getByRole('checkbox', { name, exact: true });
    assert.equal(await checkbox(names[0]).isChecked(), false);
    assert.equal(await checkbox(names[1]).isChecked(), true);

    const nestedText = body.getByText('核对许可', { exact: true });
    await nestedText.click();
    await nestedText.evaluate((node) => node.ownerDocument.getSelection().collapse(node, node.childNodes.length));
    await page.keyboard.insertText('与签名');
    const renamed = checkbox('任务：核对许可与签名');
    await renamed.waitFor();
    assert.equal(await checkbox(names[0]).count(), 1);
    await renamed.focus();
    await press('Space');
    assert.equal(await renamed.isChecked(), false);
    assert.equal(await checkbox(names[0]).isChecked(), false);
    await page.getByRole('status').filter({ hasText: /^已保存$/ }).waitFor();

    const saved = await invoke('read_page', { workspace, id });
    const [savedParent] = saved.blocks[0].children;
    const [savedNested] = savedParent.children[1].children;
    assert.equal(savedParent.id, parentTask.id);
    assert.equal(savedParent.payload.checked, false);
    assert.equal(savedNested.id, nested.id);
    assert.equal(savedNested.payload.checked, false);
    assert.equal(savedNested.children[0].payload.inline.map((part) => part.text).join(''), '核对许可与签名');
  });

  for (const field of ['标题', '正文']) {
    await check(`${field}组字跨过自动保存间隔，取消不生成修订，提交只保存一次`, async () => {
      const initialTitle = `${field}组字基线`;
      const id = await add(initialTitle);
      const read = () => invoke('read_page', { workspace, id });
      const initial = await read();
      const block = { id: randomUUID(), type: 'paragraph', schemaVersion: 1,
        payload: { inline: [{ type: 'text', text: '正文基准' }] }, children: [] };
      await invoke('save_page', { workspace, id, revision: initial.revision,
        draft: { title: initialTitle, blocks: [block] } });
      await showFixture();
      await button(initialTitle, tree).click();
      await page.waitForFunction((value) => document.querySelector('.document-title')?.value === value, initialTitle);
      await body.locator(`[data-id="${block.id}"]`).waitFor();
      const starting = await read();
      const titleComposition = field === '标题';
      if (titleComposition) {
        await title.focus();
        await title.evaluate((node) => node.setSelectionRange(node.value.length, node.value.length));
      } else {
        await body.focus();
        await press('End');
      }
      const baseText = titleComposition ? initialTitle : '正文基准';
      const target = titleComposition ? '.document-title' : '.document-body';
      const shown = (text) => page.waitForFunction(({ target, text }) => {
        const node = document.querySelector(target);
        return (node?.value ?? node?.textContent) === text;
      }, { target, text });
      const ime = await page.context().newCDPSession(page);
      const compose = (text) => ime.send('Input.imeSetComposition', {
        text, selectionStart: text.length, selectionEnd: text.length,
      });
      try {
        await compose('未选候选');
        await shown(baseText + '未选候选');
        await page.waitForTimeout(950);
        const duringComposition = await read();
        assert.equal(duringComposition.revision, starting.revision, 'an IME candidate must not become a revision');
        assert.equal(duringComposition.title, starting.title);
        assert.deepEqual(duringComposition.blocks, starting.blocks);
        await page.getByRole('status').filter({ hasText: /^输入法组字中$/ }).waitFor();

        await compose('');
        await shown(baseText);
        await page.getByRole('status').filter({ hasText: /^已保存$/ }).waitFor();
        await page.waitForTimeout(950);
        assert.equal((await read()).revision, starting.revision, 'cancelling IME must not create a revision');

        await compose('确认文字');
        await shown(baseText + '确认文字');
        await ime.send('Input.insertText', { text: '确认文字' });
        await shown(baseText + '确认文字');
        if (titleComposition) {
          await page.getByRole('status').filter({ hasText: /^已保存$/ }).waitFor();
        } else {
          await folder.click();
          await page.waitForFunction(() => document.querySelector('.document-title')?.value === '键盘甲目录');
        }
        const saved = await read();
        assert.equal(saved.revision, starting.revision + 1);
        assert.equal(saved.title, titleComposition ? `${initialTitle}确认文字` : initialTitle);
        assert.deepEqual(saved.blocks.map(({ id }) => id), [block.id]);
        assert.equal(saved.blocks[0].payload.inline.map(({ text }) => text).join(''),
          titleComposition ? '正文基准' : '正文基准确认文字');
      } finally {
        await ime.detach();
      }
    });
  }

  await check('斜杠菜单可用键盘插入块，直接回车仍可写下原斜杠', async () => {
    await body.focus();
    await page.keyboard.type('/');
    const list = page.locator('.slash-menu');
    await list.waitFor({ timeout: 2000 });
    // Notion's order: 正文, 页面, then the headings.
    for (let step = 0; step < 3; step++) await press('ArrowDown');
    assert.equal(await list.locator('.highlighted').innerText({ timeout: 2000 }), '一级标题');
    await press('Enter');
    await list.waitFor({ state: 'hidden' });
    await page.keyboard.type('Heading');
    await body.getByRole('heading', { level: 1, name: 'Heading' }).waitFor();
    await press('Enter');
    await page.keyboard.type('/');
    await list.waitFor();
    await press('Enter');
    assert.equal(await body.locator('p').first().innerText(), '/');
  });

  await check('命令面板能筛选执行，显式深色跨重启并能切回跟随系统', async () => {
    const open = async () => {
      await press('Control+Shift+p');
      const modal = dialog('命令面板');
      await modal.waitFor({ timeout: 2000 });
      const input = modal.getByRole('combobox', { name: '筛选命令' });
      await assertFocused(input);
      return { modal, input };
    };
    let { modal, input } = await open();
    await input.fill('主题');
    const first = modal.getByRole('option', { name: '使用浅色主题' });
    assert.equal(await input.getAttribute('aria-activedescendant'), await first.getAttribute('id'));
    await press('ArrowDown');
    const command = modal.getByRole('option', { name: '使用深色主题' });
    assert.equal(await input.getAttribute('aria-activedescendant'), await command.getAttribute('id'));
    await press('Enter');
    await modal.waitFor({ state: 'hidden' });
    await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark');
    const colors = await page.evaluate(() => {
      const read = (selector, property) => getComputedStyle(document.querySelector(selector))[property];
      return [read('.main-pane', 'backgroundColor'), read('.document-title', 'color'), read('.sidebar', 'backgroundColor')];
    });
    const luminance = (color) => color.match(/\d+/g).slice(0, 3).map(Number).map((channel) => {
      const normal = channel / 255;
      return normal <= 0.04045 ? normal / 12.92 : ((normal + 0.055) / 1.055) ** 2.4;
    }).reduce((sum, channel, index) => sum + channel * [0.2126, 0.7152, 0.0722][index], 0);
    assert.ok(luminance(colors[0]) < 0.14, 'the document uses a dark surface');
    assert.ok(luminance(colors[2]) < 0.14, 'the sidebar uses a dark surface');
    assert.ok((luminance(colors[1]) + 0.05) / (luminance(colors[0]) + 0.05) >= 4.5);
    await page.screenshot({ path: join(reportDir, 'dark.png') });
    await page.reload();
    await title.waitFor();
    assert.equal(await page.evaluate(() => document.documentElement.dataset.theme), 'dark');
    ({ modal, input } = await open());
    await input.fill('跟随系统');
    await press('Enter');
    await modal.waitFor({ state: 'hidden' });
    await page.waitForFunction(() => document.documentElement.dataset.theme === 'system');
    await page.emulateMedia({ colorScheme: 'dark' });
    await page.waitForFunction(() => getComputedStyle(document.documentElement).colorScheme === 'dark');
    await page.emulateMedia({ colorScheme: 'light' });
    await page.waitForFunction(() => getComputedStyle(document.documentElement).colorScheme === 'light');
  });

  await check('系统强制颜色保留选中轮廓，减少动态效果关闭折叠过渡', async () => {
    try {
      await page.emulateMedia({ forcedColors: 'active', reducedMotion: 'reduce' });
      await page.waitForFunction(() => matchMedia('(forced-colors: active)').matches && matchMedia('(prefers-reduced-motion: reduce)').matches);
      const selected = tree.locator('.tree-row.selected');
      assert.equal(await selected.evaluate((element) => getComputedStyle(element).outlineStyle), 'solid');
      const arrow = tree.locator('.tree-toggle svg').first();
      assert.ok(await arrow.evaluate((element) => parseFloat(getComputedStyle(element).transitionDuration) <= 0.001));
    } finally {
      await page.emulateMedia({ forcedColors: 'none', reducedMotion: 'no-preference' });
    }
  });

  await check('专注模式保留现有页面，先关闭命令面板再用 Esc 退出', async () => {
    const hadTitle = await title.inputValue();
    await body.focus();
    await press('Control+Shift+f');
    await button('退出专注模式').waitFor({ timeout: 2000 });
    await sidebar.waitFor({ state: 'hidden' });
    await assertFocused(body);
    assert.equal(await title.inputValue(), hadTitle);
    await press('Control+Shift+p');
    const modal = dialog('命令面板');
    await modal.waitFor({ timeout: 2000 });
    await press('Escape');
    await modal.waitFor({ state: 'hidden' });
    await button('退出专注模式').waitFor();
    // As in Notion, the first Esc in the editor selects the block; the next one leaves focus mode.
    await press('Escape');
    await body.locator('.ProseMirror-selectednode').waitFor({ timeout: 2000 });
    await page.evaluate(() => {
      window.lastFocusKey = null;
      window.addEventListener('keydown', (event) => {
        const target = event.target;
        const requested = { key: event.key, repeat: event.repeat, focused: target?.className, dialog: !!document.querySelector('dialog[open]') };
        setTimeout(() => { window.lastFocusKey = { ...requested, prevented: event.defaultPrevented }; }, 0);
      }, { capture: true, once: true });
    });
    await press('Escape');
    await sidebar.waitFor({ timeout: 2000 }).catch(async (cause) => {
      throw new Error(`Escape did not exit focus mode. ${JSON.stringify(await page.evaluate(() => window.lastFocusKey))}`, { cause });
    });
    assert.equal(await title.inputValue(), hadTitle);
    await button('收起侧栏').focus();
    await press('Enter');
    await sidebar.waitFor({ state: 'hidden' });
    await press('Control+Shift+f');
    await button('退出专注模式').waitFor();
    await press('Escape');
    await button('展开侧栏').waitFor();
    assert.equal(await sidebar.isVisible(), false);
  });

  await check('输入法组字期间组合快捷键不会打开面板或改变界面', async () => {
    await body.focus();
    for (const key of ['k', 'p', 'f']) {
      await body.dispatchEvent('keydown', { key, ctrlKey: true, shiftKey: key !== 'k', isComposing: true });
    }
    assert.equal(await page.getByRole('dialog').count(), 0);
    assert.equal(await sidebar.isVisible(), true);
    await assertFocused(body);
  });

  await writeFile(join(reportDir, 'keyboard.json'), JSON.stringify({ cases }, null, 2) + '\n');
  assert.ok(cases.every(({ passed }) => passed), cases.filter((item) => !item.passed).map((item) => item.name).join(' | '));
  return cases.map(({ name }) => name);
}
