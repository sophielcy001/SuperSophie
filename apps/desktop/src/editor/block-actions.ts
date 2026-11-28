// SPDX-License-Identifier: AGPL-3.0-or-later

import type { Editor, JSONContent } from '@tiptap/core';
import type { Node, ResolvedPos } from '@tiptap/pm/model';
import { NodeSelection, Plugin, PluginKey, Selection, TextSelection } from '@tiptap/pm/state';
import { Decoration, DecorationSet } from '@tiptap/pm/view';

/** Block kinds that "Turn into" can switch between, as in Notion. */
export type BlockKind = 'paragraph' | 'h1' | 'h2' | 'h3' | 'bullet' | 'ordered' | 'task' | 'quote' | 'code' | 'toggle' | 'callout';

export const blockKinds: { kind: BlockKind; label: string; shortcut?: string }[] = [
  { kind: 'paragraph', label: '正文', shortcut: '0' },
  { kind: 'h1', label: '一级标题', shortcut: '1' },
  { kind: 'h2', label: '二级标题', shortcut: '2' },
  { kind: 'h3', label: '三级标题', shortcut: '3' },
  { kind: 'task', label: '待办清单', shortcut: '4' },
  { kind: 'bullet', label: '项目列表', shortcut: '5' },
  { kind: 'ordered', label: '编号列表', shortcut: '6' },
  { kind: 'toggle', label: '折叠', shortcut: '7' },
  { kind: 'code', label: '代码块', shortcut: '8' },
  { kind: 'quote', label: '引用' },
  { kind: 'callout', label: '提示块' },
];

const listItems = new Set(['listItem', 'taskItem']);
const lists: Record<string, BlockKind> = { bulletList: 'bullet', orderedList: 'ordered', taskList: 'task' };

/** The kind of the text block holding the caret, or undefined when the selection is not in text. */
export function currentKind($pos: ResolvedPos): BlockKind | undefined {
  const block = $pos.parent;
  if (!block.isTextblock) return;
  if (block.type.name === 'codeBlock') return 'code';
  if (block.type.name === 'heading') return (['h1', 'h2', 'h3'] as const)[Number(block.attrs.level) - 1] ?? 'h1';
  if ($pos.depth > 1 && listItems.has($pos.node(-1).type.name) && $pos.index(-1) === 0) {
    return lists[$pos.node(-2).type.name] ?? 'paragraph';
  }
  if ($pos.depth > 1 && $pos.node(-1).type.name === 'blockquote') return 'quote';
  return 'paragraph';
}

export function kindLabel(kind: BlockKind | undefined) {
  return blockKinds.find((item) => item.kind === kind)?.label ?? '正文';
}

/** Lifts the caret's block out of any lists and quotes, leaving a plain paragraph or heading. */
function unwrap(editor: Editor) {
  for (let guard = 0; guard < 8; guard++) {
    const { $from } = editor.state.selection;
    const kind = currentKind($from);
    if (kind === 'bullet' || kind === 'ordered' || kind === 'task') {
      const item = $from.node(-1).type.name;
      if (!editor.commands.liftListItem(item)) return false;
    } else if (kind === 'quote') {
      if (!editor.commands.lift('blockquote')) return false;
    } else return true;
  }
  return false;
}

/**
 * Turns the text block holding the caret into another kind, keeping its text.
 * Returns false when the selection is not in a text block.
 */
export function turnInto(editor: Editor, kind: BlockKind): boolean {
  const { $from } = editor.state.selection;
  const from = currentKind($from);
  if (!from || !editor.isEditable) return false;
  if (from === kind) return true;
  const chain = () => editor.chain().focus();
  if (kind === 'bullet' || kind === 'ordered' || kind === 'task') {
    // Only this item changes, as in Notion: it leaves its list and joins or starts one of the new kind.
    unwrap(editor);
    editor.commands.setParagraph();
    return chain()[({ bullet: 'toggleBulletList', ordered: 'toggleOrderedList', task: 'toggleTaskList' } as const)[kind]]().run();
  }
  unwrap(editor);
  switch (kind) {
    case 'paragraph': return chain().setParagraph().run();
    case 'h1': case 'h2': case 'h3': return chain().setHeading({ level: Number(kind[1]) as 1 | 2 | 3 }).run();
    case 'code': return chain().setCodeBlock().run();
    case 'quote': return chain().setParagraph().wrapIn('blockquote').run();
    case 'toggle': return chain().setParagraph().wrapIn('toggleBlock').run();
    case 'callout': return chain().setParagraph().wrapIn('callout').run();
  }
}

/** The block a block action applies to: a selected node, the list item, or the text block itself. */
export function actionBlock(editor: Editor): { node: Node; position: number } | undefined {
  const { selection } = editor.state;
  if (selection instanceof NodeSelection) return { node: selection.node, position: selection.from };
  const { $from } = selection;
  if (!$from.parent.isTextblock || $from.depth < 1) return;
  const depth = $from.depth > 1 && listItems.has($from.node(-1).type.name) && $from.index(-1) === 0
    ? $from.depth - 1 : $from.depth;
  return { node: $from.node(depth), position: $from.before(depth) };
}

/** Inserts a copy of the block right after it, like Notion's Duplicate (⌘D). */
export function duplicateBlock(editor: Editor, block = actionBlock(editor)): boolean {
  if (!block || !editor.isEditable) return false;
  const at = block.position + block.node.nodeSize;
  const { tr } = editor.state;
  tr.insert(at, editor.schema.nodeFromJSON(withNewIds(block.node.toJSON())));
  const inside = tr.doc.resolve(Math.min(at + 1, tr.doc.content.size));
  tr.setSelection(TextSelection.near(inside));
  editor.view.dispatch(tr.scrollIntoView());
  editor.view.focus();
  return true;
}

/** A copy of a block's JSON in which every block gets a fresh ID. */
function withNewIds(json: JSONContent): JSONContent {
  return {
    ...json,
    ...(json.attrs && 'id' in json.attrs ? { attrs: { ...json.attrs, id: crypto.randomUUID() } } : {}),
    ...(json.content ? { content: json.content.map(withNewIds) } : {}),
  };
}

export function deleteBlock(editor: Editor, block = actionBlock(editor)): boolean {
  if (!block || !editor.isEditable) return false;
  const { tr } = editor.state;
  tr.delete(block.position, block.position + block.node.nodeSize);
  if (!tr.doc.childCount) tr.insert(0, editor.schema.nodes.paragraph.create());
  tr.setSelection(TextSelection.near(tr.doc.resolve(Math.min(block.position, tr.doc.content.size))));
  editor.view.dispatch(tr.scrollIntoView());
  editor.view.focus();
  return true;
}

/** Places the caret at the start of the block at position, so caret-based actions apply to it. */
export function focusBlock(editor: Editor, position: number) {
  const node = editor.state.doc.nodeAt(position);
  if (!node?.isBlock) return;
  const { doc } = editor.state;
  const selection = node.isTextblock || (node.inlineContent === false && node.firstChild && !node.isAtom)
    ? TextSelection.near(doc.resolve(position + 1))
    : NodeSelection.create(doc, position);
  editor.view.dispatch(editor.state.tr.setSelection(selection));
}

interface BlockUnit { node: Node; position: number }

const unitContainers = new Set(['toggleBlock', 'callout', 'blockquote']);

/**
 * The blocks Notion's block selection steps through, in document order: text blocks,
 * list items (with their first paragraph), tables, media and other atoms, and the
 * containers that hold blocks. Content of collapsed folds is skipped.
 */
function blockUnits(doc: Node): BlockUnit[] {
  const units: BlockUnit[] = [];
  doc.descendants((node, position, parent, index) => {
    const name = node.type.name;
    if (listItems.has(name)) {
      units.push({ node, position });
      return true;
    }
    if (node.isTextblock) {
      if (!(parent && listItems.has(parent.type.name) && index === 0)) units.push({ node, position });
      return false;
    }
    if (name === 'table' || (node.isBlock && node.isAtom)) {
      units.push({ node, position });
      return false;
    }
    if (unitContainers.has(name)) {
      units.push({ node, position });
      return !(name === 'toggleBlock' && node.attrs.collapsed === true);
    }
    return true;
  });
  return units;
}

/** The block a Notion-style block selection currently covers, if any. */
export function selectedUnit(editor: Editor): BlockUnit | undefined {
  const { selection } = editor.state;
  if (!(selection instanceof NodeSelection)) return;
  return blockUnits(editor.state.doc).find((unit) => unit.position === selection.from);
}

/** Esc in Notion: select the block holding the caret. */
export function selectBlockAtCaret(editor: Editor): boolean {
  const { from } = editor.state.selection;
  let target: BlockUnit | undefined;
  for (const unit of blockUnits(editor.state.doc)) {
    if (unit.position < from && from < unit.position + unit.node.nodeSize) target = unit;
  }
  if (!target) return false;
  editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, target.position)).scrollIntoView());
  return true;
}

/** ↑／↓ while a block is selected select the previous or next block. */
export function stepBlockSelection(editor: Editor, direction: -1 | 1): boolean {
  const current = selectedUnit(editor);
  if (!current) return false;
  const units = blockUnits(editor.state.doc);
  const next = units[units.findIndex((unit) => unit.position === current.position) + direction];
  if (next) editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, next.position)).scrollIntoView());
  return true;
}

/** Enter on a selected block goes back to editing at its end. */
export function editSelectedBlock(editor: Editor): boolean {
  const current = selectedUnit(editor);
  if (!current || current.node.isAtom) return false;
  const end = editor.state.doc.resolve(current.position + current.node.nodeSize);
  editor.view.dispatch(editor.state.tr.setSelection(Selection.near(end, -1)).scrollIntoView());
  return true;
}

/** Several whole blocks selected with Shift+↑／↓ (unit positions of the anchor and head blocks). */
interface BlockRange { anchor: number; head: number }

export const blockRangeKey = new PluginKey<BlockRange | null>('blockRange');

/** Keeps the block range until the document or selection changes some other way, and highlights it. */
export const blockRangePlugin = new Plugin<BlockRange | null>({
  key: blockRangeKey,
  state: {
    init: () => null,
    apply(transaction, previous) {
      const requested: BlockRange | null | undefined = transaction.getMeta(blockRangeKey);
      if (requested !== undefined) return requested;
      return transaction.docChanged || transaction.selectionSet ? null : previous;
    },
  },
  props: {
    decorations(state) {
      const range = blockRangeKey.getState(state);
      if (!range) return null;
      const units = rangeUnits(state.doc, range);
      return DecorationSet.create(state.doc, units.map((unit) =>
        Decoration.node(unit.position, unit.position + unit.node.nodeSize, { class: 'block-range-selected' })));
    },
  },
});

function rangeUnits(doc: Node, range: BlockRange) {
  const units = blockUnits(doc);
  const from = units.findIndex((unit) => unit.position === Math.min(range.anchor, range.head));
  const to = units.findIndex((unit) => unit.position === Math.max(range.anchor, range.head));
  if (from < 0 || to < 0) return [];
  // Blocks nested in an already selected container are covered by it.
  return units.slice(from, to + 1).filter((unit, index, all) => !all.slice(0, index).some((outer) =>
    outer.position < unit.position && unit.position < outer.position + outer.node.nodeSize));
}

function showRange(editor: Editor, range: BlockRange) {
  const units = rangeUnits(editor.state.doc, range);
  if (!units.length) return false;
  const { doc } = editor.state;
  const first = units[0];
  const last = units[units.length - 1];
  const tr = editor.state.tr;
  if (range.anchor === range.head) tr.setSelection(NodeSelection.create(doc, range.head));
  else tr.setSelection(TextSelection.between(doc.resolve(first.position), doc.resolve(last.position + last.node.nodeSize)));
  tr.setMeta(blockRangeKey, range.anchor === range.head ? null : range);
  editor.view.dispatch(tr.scrollIntoView());
  return true;
}

/** Shift+↑／↓ in a block selection: grow or shrink it by one block. */
export function extendBlockSelection(editor: Editor, direction: -1 | 1): boolean {
  const range = blockRangeKey.getState(editor.state) ?? (() => {
    const unit = selectedUnit(editor);
    return unit ? { anchor: unit.position, head: unit.position } : undefined;
  })();
  if (!range) return false;
  const units = blockUnits(editor.state.doc);
  const next = units[units.findIndex((unit) => unit.position === range.head) + direction];
  if (!next) return true;
  return showRange(editor, { anchor: range.anchor, head: next.position });
}

/** Keys on a multi-block selection: Delete removes the blocks, ↑／↓ and Esc go back to one block. */
export function blockRangeKeyDown(editor: Editor, key: string): boolean {
  const range = blockRangeKey.getState(editor.state);
  if (!range) return false;
  if (key === 'Backspace' || key === 'Delete') {
    const units = rangeUnits(editor.state.doc, range);
    if (!units.length) return false;
    const first = units[0];
    const last = units[units.length - 1];
    const tr = editor.state.tr.delete(first.position, last.position + last.node.nodeSize).setMeta(blockRangeKey, null);
    if (!tr.doc.childCount) tr.insert(0, editor.schema.nodes.paragraph.create());
    tr.setSelection(Selection.near(tr.doc.resolve(Math.min(first.position, tr.doc.content.size))));
    editor.view.dispatch(tr.scrollIntoView());
    return true;
  }
  if (key === 'ArrowUp' || key === 'ArrowDown' || key === 'Escape') {
    return showRange(editor, { anchor: range.head, head: range.head });
  }
  return false;
}

/**
 * Typing while whole blocks are selected goes back to editing instead of replacing them,
 * as in Notion: the caret moves to the end of the (last) block, or to a new line after
 * a selected image or other atom. Returns false when no block selection is active.
 */
export function leaveBlockSelection(editor: Editor): boolean {
  const range = blockRangeKey.getState(editor.state);
  const units = range ? rangeUnits(editor.state.doc, range) : [];
  const target = units.at(-1) ?? selectedUnit(editor);
  if (!target) return false;
  const tr = editor.state.tr.setMeta(blockRangeKey, null);
  const end = target.position + target.node.nodeSize;
  if (target.node.isAtom) {
    tr.insert(end, editor.schema.nodes.paragraph.create());
    tr.setSelection(TextSelection.create(tr.doc, end + 1));
  } else {
    tr.setSelection(Selection.near(tr.doc.resolve(end), -1));
  }
  editor.view.dispatch(tr.scrollIntoView());
  return true;
}
