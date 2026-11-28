// SPDX-License-Identifier: AGPL-3.0-or-later

import { Node } from '@tiptap/core';
import type { Editor } from '@tiptap/core';
import { Fragment } from '@tiptap/pm/model';
import type { Node as ProseNode } from '@tiptap/pm/model';
import type { EditorView, NodeView } from '@tiptap/pm/view';
import { TextSelection } from '@tiptap/pm/state';

/** Blocks nested under the block above with Tab, as Notion's child blocks look. */
export const IndentBlock = Node.create({
  name: 'indentBlock', group: 'block', content: 'block+', defining: true,
  parseHTML() { return [{ tag: 'div[data-zhiye-indent]' }]; },
  renderHTML({ HTMLAttributes }) { return ['div', { ...HTMLAttributes, 'data-zhiye-indent': '', class: 'indent-block' }, 0]; },
});

/**
 * A column's relative width in hundredths of an even share (100), set by dragging the gap
 * between columns. Whole numbers, because Core keeps blocks with fractions opaque.
 */
export function columnWidth(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isInteger(value) && value >= 10 && value <= 1000 ? value : undefined;
}

function gridColumns(node: ProseNode) {
  const widths: string[] = [];
  node.forEach((column) => { widths.push(`minmax(0, ${columnWidth(column.attrs.width) ?? 100}fr)`); });
  return widths.join(' ');
}

/** Columns with a drag handle in each gap, as in Notion. */
class ColumnListView implements NodeView {
  dom: HTMLElement;
  contentDOM: HTMLElement;
  private handles: HTMLElement[] = [];

  constructor(private node: ProseNode, private view: EditorView, private getPos: () => number | undefined) {
    this.dom = document.createElement('div');
    this.dom.className = 'column-list-frame';
    this.contentDOM = document.createElement('div');
    this.contentDOM.className = 'column-list';
    this.dom.append(this.contentDOM);
    this.render();
  }

  private render() {
    const { node } = this;
    if (typeof node.attrs.id === 'string') this.dom.dataset.id = node.attrs.id;
    this.contentDOM.dataset.zhiyeColumns = String(node.childCount);
    this.contentDOM.style.gridTemplateColumns = gridColumns(node);
    while (this.handles.length > node.childCount - 1) this.handles.pop()?.remove();
    while (this.handles.length < node.childCount - 1) {
      const index = this.handles.length;
      const handle = document.createElement('div');
      handle.className = 'column-resize';
      handle.contentEditable = 'false';
      handle.setAttribute('aria-hidden', 'true');
      handle.title = '拖动调整栏宽';
      handle.addEventListener('pointerdown', (event) => this.drag(event, index));
      this.dom.append(handle);
      this.handles.push(handle);
    }
    requestAnimationFrame(() => this.place());
  }

  /** Puts each handle in the middle of the gap after column index. */
  private place() {
    const frame = this.dom.getBoundingClientRect();
    const columns = [...this.contentDOM.children] as HTMLElement[];
    this.handles.forEach((handle, index) => {
      const left = columns[index]?.getBoundingClientRect();
      const right = columns[index + 1]?.getBoundingClientRect();
      if (!left || !right) return;
      handle.style.left = `${(left.right + right.left) / 2 - frame.left - 4}px`;
    });
  }

  private drag(event: PointerEvent, index: number) {
    if (!this.view.editable || event.button !== 0) return;
    event.preventDefault();
    const handle = event.currentTarget as HTMLElement;
    const columns = [...this.contentDOM.children] as HTMLElement[];
    const widths: number[] = [];
    this.node.forEach((column) => { widths.push(columnWidth(column.attrs.width) ?? 100); });
    const left = columns[index].getBoundingClientRect();
    const right = columns[index + 1].getBoundingClientRect();
    const pair = widths[index] + widths[index + 1];
    const span = right.right - left.left;
    let next = widths.slice();
    const pointer = event.pointerId;
    // The pointer soon leaves the thin handle, so follow it on the whole document.
    const target = handle.ownerDocument;
    const move = (moved: PointerEvent) => {
      if (moved.pointerId !== pointer) return;
      // Share the two columns' combined width by where the pointer is, keeping each at least 10%.
      const share = Math.min(0.9, Math.max(0.1, (moved.clientX - left.left) / span));
      next = widths.slice();
      next[index] = Math.min(1000, Math.max(10, Math.round(pair * share)));
      next[index + 1] = Math.min(1000, Math.max(10, pair - next[index]));
      this.contentDOM.style.gridTemplateColumns = next.map((width) => `minmax(0, ${width}fr)`).join(' ');
      this.place();
    };
    const finish = (released: PointerEvent) => {
      if (released.pointerId !== pointer) return;
      if (released.type === 'pointerup') move(released);
      target.removeEventListener('pointermove', move, true);
      target.removeEventListener('pointerup', finish, true);
      target.removeEventListener('pointercancel', finish, true);
      const position = this.position();
      if (position === undefined || !this.view.editable) return;
      const tr = this.view.state.tr;
      let offset = position + 1;
      this.node.forEach((column, _, child) => {
        if ((child === index || child === index + 1) && next[child] !== widths[child]) {
          tr.setNodeAttribute(offset, 'width', next[child] === 100 ? null : next[child]);
        }
        offset += column.nodeSize;
      });
      if (tr.docChanged) this.view.dispatch(tr);
    };
    target.addEventListener('pointermove', move, true);
    target.addEventListener('pointerup', finish, true);
    target.addEventListener('pointercancel', finish, true);
  }

  /** The list's document position; getPos can be unavailable to listeners added outside ProseMirror. */
  private position() {
    const known = this.getPos();
    if (known !== undefined) return known;
    try {
      const found = this.view.posAtDOM(this.contentDOM, 0) - 1;
      return this.view.state.doc.nodeAt(found)?.type.name === 'columnList' ? found : undefined;
    } catch {
      return undefined;
    }
  }

  update(node: ProseNode) {
    if (node.type !== this.node.type) return false;
    this.node = node;
    this.render();
    return true;
  }

  ignoreMutation(mutation: MutationRecord | { type: 'selection'; target: globalThis.Node }) {
    if (mutation.type === 'selection') return false;
    // The grid style and handle positions are this view's own; reading them back as content
    // would make ProseMirror redraw the columns in the middle of a drag.
    if (mutation.type === 'attributes' && (mutation.target === this.contentDOM || mutation.target === this.dom)) return true;
    return !this.contentDOM.contains(mutation.target);
  }

  stopEvent(event: Event) {
    return this.handles.includes(event.target as HTMLElement);
  }
}

/** Side-by-side columns, 2 to 5, each holding any blocks. */
export const ColumnList = Node.create({
  name: 'columnList', group: 'block', content: 'column{2,5}', isolating: true,
  parseHTML() { return [{ tag: 'div[data-zhiye-columns]' }]; },
  renderHTML({ node, HTMLAttributes }) {
    return ['div', { ...HTMLAttributes, 'data-zhiye-columns': String(node.childCount), class: 'column-list',
      style: `grid-template-columns: ${gridColumns(node)}` }, 0];
  },
  addNodeView() {
    return ({ node, view, getPos }) => new ColumnListView(node, view, getPos);
  },
});

export const Column = Node.create({
  name: 'column', content: 'block+', isolating: true,
  addAttributes() { return { width: { default: null, rendered: false } }; },
  parseHTML() { return [{ tag: 'div[data-zhiye-column]' }]; },
  renderHTML({ HTMLAttributes }) { return ['div', { ...HTMLAttributes, 'data-zhiye-column': '', class: 'column' }, 0]; },
});

/** Inserts columns where the caret is, each starting with an empty line. */
export function insertColumns(editor: Editor, count: number) {
  const { schema } = editor.state;
  const columns = Array.from({ length: count }, () => schema.nodes.column.create(null, schema.nodes.paragraph.create()));
  const list = schema.nodes.columnList.create(null, Fragment.fromArray(columns));
  const { $from } = editor.state.selection;
  const empty = $from.parent.isTextblock && $from.parent.content.size === 0 && $from.depth > 0;
  const at = empty ? $from.before() : $from.after(Math.max(1, $from.depth));
  const tr = editor.state.tr;
  if (empty) tr.replaceWith(at, $from.after(), list);
  else tr.insert(at, list);
  tr.setSelection(TextSelection.near(tr.doc.resolve(at + 3)));
  editor.view.dispatch(tr.scrollIntoView());
  editor.view.focus();
}
