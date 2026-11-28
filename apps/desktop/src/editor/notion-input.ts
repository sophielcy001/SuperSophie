// SPDX-License-Identifier: AGPL-3.0-or-later

import { Extension, InputRule, wrappingInputRule } from '@tiptap/core';
import type { Editor } from '@tiptap/core';
import { Plugin, PluginKey, TextSelection } from '@tiptap/pm/state';
import type { Node } from '@tiptap/pm/model';
import type { EditorView, NodeView } from '@tiptap/pm/view';
import { blockKinds, blockRangePlugin, duplicateBlock, turnInto } from './block-actions';

/** Focuses the title field of the fold block at position once its node view is mounted. */
export function focusFoldTitle(editor: Editor, position: number, attempts = 10) {
  requestAnimationFrame(() => {
    if (editor.isDestroyed) return;
    const element = editor.view.nodeDOM(position);
    const input = element instanceof HTMLElement ? element.querySelector<HTMLInputElement>('.fold-header input') : null;
    // The fold's React view can mount a few frames later (notably in the macOS WebKit window).
    if (input) input.focus();
    else if (attempts > 1) focusFoldTitle(editor, position, attempts - 1);
  });
}

/**
 * Notion's Markdown-style shortcuts that differ from plain Markdown: "> " makes a fold
 * (toggle), a quotation mark makes a quote. Full-width forms typed with a Chinese input
 * method work too. Runs before StarterKit's own rules thanks to its priority.
 */
function foldRule(editor: Editor) {
  return new InputRule({
    find: /^\s*[>》]\s$/,
    handler: ({ state, range, chain }) => {
      const $from = state.doc.resolve(range.from);
      const toggle = state.schema.nodes.toggleBlock;
      if ($from.parent.type.name !== 'paragraph'
        || !$from.node(-1).canReplaceWith($from.index(-1), $from.index(-1) + 1, toggle)) return null;
      const title = $from.parent.textBetween(range.to - $from.start(), $from.parent.content.size).slice(0, 512);
      const start = $from.before();
      const end = $from.after();
      const fold = toggle.create({ title }, state.schema.nodes.paragraph.create());
      chain().command(({ tr }) => {
        tr.replaceWith(start, end, fold);
        tr.setSelection(TextSelection.create(tr.doc, start + 2));
        return true;
      }).run();
      // Like Notion, the typed line is the fold's title: keep typing there when it is empty.
      if (!title) focusFoldTitle(editor, start);
    },
  });
}

const codeLanguages: [string, string][] = [
  ['', '纯文本'], ['javascript', 'JavaScript'], ['typescript', 'TypeScript'], ['python', 'Python'], ['rust', 'Rust'],
  ['go', 'Go'], ['java', 'Java'], ['kotlin', 'Kotlin'], ['swift', 'Swift'], ['c', 'C'], ['cpp', 'C++'], ['csharp', 'C#'],
  ['sql', 'SQL'], ['bash', 'Shell'], ['json', 'JSON'], ['yaml', 'YAML'], ['html', 'HTML'], ['css', 'CSS'], ['markdown', 'Markdown'],
];

/** Code block with a language picker in its corner, as in Notion. */
class CodeBlockView implements NodeView {
  dom: HTMLElement;
  contentDOM: HTMLElement;
  private select: HTMLSelectElement;

  constructor(private node: Node, private view: EditorView, getPos: () => number | undefined) {
    this.dom = document.createElement('div');
    this.dom.className = 'code-block';
    this.select = document.createElement('select');
    this.select.className = 'code-language';
    this.select.setAttribute('aria-label', '代码语言');
    this.select.contentEditable = 'false';
    const languages = [...codeLanguages];
    const current = String(node.attrs.language ?? '');
    if (current && !languages.some(([id]) => id === current)) languages.push([current, current]);
    for (const [id, label] of languages) this.select.add(new Option(label, id));
    this.select.value = current;
    this.select.disabled = !view.editable;
    this.select.addEventListener('change', () => {
      const position = getPos();
      if (position === undefined || !view.editable) return;
      view.dispatch(view.state.tr.setNodeAttribute(position, 'language', this.select.value || null));
      view.focus();
    });
    const pre = document.createElement('pre');
    this.contentDOM = document.createElement('code');
    pre.append(this.contentDOM);
    this.dom.append(this.select, pre);
    this.syncLanguage();
  }

  private syncLanguage() {
    // Block identity stays on the outer element, where search and block links look for it.
    if (typeof this.node.attrs.id === 'string') this.dom.dataset.id = this.node.attrs.id;
    else delete this.dom.dataset.id;
    const language = String(this.node.attrs.language ?? '');
    this.contentDOM.className = language ? `language-${language}` : '';
    if (this.select.value !== language) {
      if (![...this.select.options].some((option) => option.value === language)) this.select.add(new Option(language, language));
      this.select.value = language;
    }
  }

  update(node: Node) {
    if (node.type !== this.node.type) return false;
    this.node = node;
    this.select.disabled = !this.view.editable;
    this.syncLanguage();
    return true;
  }

  stopEvent(event: Event) {
    return event.target === this.select;
  }

  ignoreMutation(mutation: MutationRecord | { type: 'selection'; target: globalThis.Node }) {
    return mutation.type !== 'selection' && !this.contentDOM.contains(mutation.target);
  }
}

function indent(editor: Editor): boolean {
  const { selection } = editor.state;
  const { $from } = selection;
  if (!(selection instanceof TextSelection) || !$from.parent.isTextblock || $from.parent.type.spec.code || $from.depth < 1) return false;
  const container = $from.node(-1).type.name;
  // Lists and tables keep their own Tab; the first block has nothing above it to nest under.
  if (container === 'listItem' || container === 'taskItem' || container.startsWith('table') || $from.index(-1) === 0) return false;
  if (!editor.commands.wrapIn('indentBlock')) return true;
  const $caret = editor.state.selection.$from;
  for (let depth = $caret.depth; depth > 0; depth--) {
    if ($caret.node(depth).type.name !== 'indentBlock') continue;
    const wrapper = $caret.before(depth);
    // Join an indented run with the one right above, so siblings share one container.
    if (editor.state.doc.resolve(wrapper).nodeBefore?.type.name === 'indentBlock') {
      editor.view.dispatch(editor.state.tr.join(wrapper));
    }
    break;
  }
  return true;
}

export const NotionInput = Extension.create({
  name: 'notionInput',
  priority: 1000,
  addInputRules() {
    const { schema } = this.editor;
    return [
      foldRule(this.editor),
      wrappingInputRule({ find: /^\s*["“”]\s$/, type: schema.nodes.blockquote }),
      // "【】" is what "[]" becomes with a Chinese input method.
      wrappingInputRule({ find: /^\s*【\s?】\s$/, type: schema.nodes.taskList }),
    ];
  },
  addKeyboardShortcuts() {
    const shortcuts: Record<string, () => boolean> = {
      'Mod-d': () => duplicateBlock(this.editor),
      // Tab nests the line under the block above it, as Notion's child blocks; Shift+Tab brings it back.
      Tab: () => indent(this.editor),
      'Shift-Tab': () => {
        const { $from } = this.editor.state.selection;
        if (!$from.parent.isTextblock || $from.depth < 2 || $from.node(-1).type.name !== 'indentBlock') return false;
        return this.editor.commands.lift('indentBlock');
      },
      // Backspace at the start of a heading turns it back into text first, as in Notion.
      Backspace: () => {
        const { selection } = this.editor.state;
        const { $from } = selection;
        if (!selection.empty || $from.parentOffset !== 0 || $from.parent.type.name !== 'heading') return false;
        return this.editor.commands.setParagraph();
      },
    };
    for (const { kind, shortcut } of blockKinds) {
      if (shortcut) shortcuts[`Mod-Alt-${shortcut}`] = () => turnInto(this.editor, kind);
    }
    return shortcuts;
  },
  addProseMirrorPlugins() {
    return [blockRangePlugin, new Plugin({
      key: new PluginKey('codeBlockView'),
      props: { nodeViews: { codeBlock: (node, view, getPos) => new CodeBlockView(node, view, getPos) } },
    })];
  },
});
