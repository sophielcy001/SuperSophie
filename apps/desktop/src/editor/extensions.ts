// SPDX-License-Identifier: AGPL-3.0-or-later

import { Extension, Node } from '@tiptap/core';
import type { Editor } from '@tiptap/core';
import { TaskItem, TaskList } from '@tiptap/extension-list';
import UniqueID from '@tiptap/extension-unique-id';
import { TableKit } from '@tiptap/extension-table';
import { Placeholder } from '@tiptap/extensions';
import type { ResolvedPos } from '@tiptap/pm/model';
import { NodeSelection, Plugin, PluginKey } from '@tiptap/pm/state';
import type { Transaction } from '@tiptap/pm/state';
import { Decoration, DecorationSet } from '@tiptap/pm/view';
import StarterKit from '@tiptap/starter-kit';
import { blockTypes, cursorParagraphAttributes, needsCursorParagraph } from './blocks';
import { BackgroundColor, BlockColor, Comment, TextColor } from './marks';
import { Column, ColumnList, IndentBlock } from './LayoutBlocks';
import { onPageDirectory, referenceLabel, referenceTrashed } from './page-directory';
import { NotionInput } from './notion-input';
import { inlineText, webAddress } from './rich-content';

const PageReference = Node.create({
  name: 'pageReference',
  group: 'inline',
  inline: true,
  atom: true,
  addAttributes() {
    return { pageId: { default: '' }, label: { default: '页面' } };
  },
  parseHTML() {
    return [{
      tag: 'span[data-page-reference]',
      getAttrs: (element) => ({
        pageId: element.getAttribute('data-page-reference'),
        label: element.getAttribute('data-label') ?? '页面',
      }),
    }];
  },
  renderHTML({ node }) {
    return ['span', {
      class: 'page-reference',
      'data-page-reference': node.attrs.pageId,
      'data-label': node.attrs.label,
      role: 'link',
      tabindex: '0',
    }, `↗ ${node.attrs.label}`];
  },
  addNodeView() {
    return ({ node: initial }) => {
      let node = initial;
      const dom = document.createElement('span');
      dom.className = 'page-reference';
      dom.setAttribute('role', 'link');
      dom.tabIndex = 0;
      const render = () => {
        const { pageId, label } = node.attrs;
        dom.dataset.pageReference = pageId;
        dom.dataset.label = label;
        dom.textContent = `↗ ${referenceLabel(pageId, label) || '无标题'}`;
        dom.classList.toggle('is-trashed', referenceTrashed(pageId));
      };
      render();
      const unsubscribe = onPageDirectory(render);
      return {
        dom,
        update(next) {
          if (next.type !== node.type) return false;
          node = next;
          render();
          return true;
        },
        ignoreMutation: () => true,
        destroy: unsubscribe,
      };
    };
  },
});

const UnknownBlock = Node.create({
  name: 'unknownBlock',
  group: 'block',
  atom: true,
  addAttributes() {
    return { raw: { default: null, rendered: false } };
  },
  renderHTML({ node, HTMLAttributes }) {
    return ['div', { ...HTMLAttributes, class: 'unknown-block', contenteditable: 'false' },
      `已保留的块 · ${node.attrs.raw?.type ?? '未知类型'}`];
  },
});

const searchBlockKey = new PluginKey<DecorationSet>('searchBlock');

export function markSearchBlock(editor: Editor, position: number | null, transaction: Transaction = editor.state.tr) {
  editor.view.dispatch(transaction.setMeta(searchBlockKey, position));
}

export function focusLocatedBlock(editor: Editor, position: number): boolean {
  const node = editor.state.doc.nodeAt(position);
  if (!node || !(editor.view.nodeDOM(position) instanceof HTMLElement)) return false;
  markSearchBlock(editor, position);
  if ((node.isAtom || node.type.name === 'toggleBlock') && NodeSelection.isSelectable(node)) {
    editor.commands.setNodeSelection(position);
    editor.commands.focus(undefined, { scrollIntoView: false });
  } else {
    editor.commands.focus(position + 1, { scrollIntoView: false });
  }
  requestAnimationFrame(() => {
    if (editor.isDestroyed || searchBlockKey.getState(editor.state)?.find(
      position, position, (spec) => spec.searchTarget,
    ).length !== 1) return;
    const element = editor.view.nodeDOM(position);
    if (element instanceof HTMLElement) element.scrollIntoView({ block: 'center', inline: 'nearest' });
  });
  return true;
}

const SearchBlock = Extension.create({
  name: 'searchBlock',
  addProseMirrorPlugins() {
    return [new Plugin<DecorationSet>({
      key: searchBlockKey,
      state: {
        init: () => DecorationSet.empty,
        apply(transaction, previous) {
          const requested: number | null | undefined = transaction.getMeta(searchBlockKey);
          if (requested === undefined && !transaction.docChanged && !transaction.selectionSet) return previous;
          const position = requested === undefined && !transaction.docChanged
            ? previous.find(undefined, undefined, (spec) => spec.searchTarget)[0]?.from : requested;
          const decorations: Decoration[] = [];
          const revealed = new Set<number>();
          function revealFolds(location: ResolvedPos) {
            for (let depth = 1; depth <= location.depth; depth += 1) {
              const node = location.node(depth);
              if (node.type.name !== 'toggleBlock' || node.attrs.collapsed !== true) continue;
              const start = location.before(depth);
              if (revealed.has(start)) continue;
              revealed.add(start);
              decorations.push(Decoration.node(start, start + node.nodeSize,
                { 'data-reveal-fold': 'true' }, { revealFold: true }));
            }
          }
          if (typeof position === 'number') {
            const node = transaction.doc.nodeAt(position);
            if (node) {
              revealFolds(transaction.doc.resolve(position));
              decorations.push(Decoration.node(position, position + node.nodeSize,
                { 'data-search-target': 'true' }, { searchTarget: true }));
            }
          }
          revealFolds(transaction.selection.$from);
          return DecorationSet.create(transaction.doc, decorations);
        },
      },
      props: { decorations: (state) => searchBlockKey.getState(state) },
    })];
  },
});

const CursorParagraph = Extension.create({
  name: 'cursorParagraph',
  addGlobalAttributes() {
    return [{
      types: ['paragraph'],
      attributes: { editorCursor: { default: false, rendered: false, keepOnSplit: false } },
    }];
  },
  addProseMirrorPlugins() {
    return [new Plugin({
      appendTransaction(transactions, previous, state) {
        if (!transactions.some((transaction) => transaction.docChanged)) return;
        const last = state.doc.lastChild;
        const tr = state.tr;
        if (previous.doc.lastChild?.attrs.editorCursor && previous.doc.lastChild !== last) {
          state.doc.descendants((node, position) => {
            if (node.attrs.editorCursor && (node !== last || node.content.size > 0)) {
              tr.setNodeAttribute(position, 'editorCursor', false);
            }
          });
        }
        if (last && needsCursorParagraph.has(last.type.name)) {
          tr.insert(tr.doc.content.size, state.schema.nodes.paragraph.create(cursorParagraphAttributes()));
        }
        return tr.docChanged ? tr : null;
      },
    })];
  },
});

export const editorExtensions = [
  StarterKit.configure({
    heading: { levels: [1, 2, 3] },
    link: {
      openOnClick: false, defaultProtocol: 'https',
      HTMLAttributes: { target: null, rel: null, 'data-web-link': 'true', title: '按住 Ctrl 或 ⌘ 点击打开链接' },
      isAllowedUri: (url) => Boolean(webAddress(url)),
    },
    trailingNode: false,
  }),
  TableKit.configure({ table: { renderWrapper: true, allowTableNodeSelection: true } }),
  // Hint in the empty line under the caret, as Notion does.
  Placeholder.configure({
    includeChildren: false,
    placeholder: ({ node }) => node.type.name === 'heading'
      ? `${['', '一', '二', '三'][node.attrs.level as number] ?? ''}级标题`
      : node.type.name === 'paragraph' ? '输入 / 唤出命令…' : '',
  }),
  TaskList,
  TaskItem.configure({
    nested: true,
    a11y: {
      checkboxLabel(node) {
        const label = node.firstChild ? inlineText(node.firstChild) : '';
        return label ? `任务：${label}` : '空白任务';
      },
    },
  }),
  PageReference,
  TextColor,
  BackgroundColor,
  BlockColor,
  Comment,
  IndentBlock,
  ColumnList,
  Column,
  NotionInput,
  UnknownBlock,
  SearchBlock,
  CursorParagraph,
  UniqueID.configure({ types: blockTypes, generateID: () => crypto.randomUUID() }),
];
