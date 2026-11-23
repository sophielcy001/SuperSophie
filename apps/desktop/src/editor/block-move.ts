// SPDX-License-Identifier: AGPL-3.0-or-later

import { closeHistory } from '@tiptap/pm/history';
import { Fragment } from '@tiptap/pm/model';
import type { Node, ResolvedPos } from '@tiptap/pm/model';
import { NodeSelection, Selection } from '@tiptap/pm/state';
import type { EditorState } from '@tiptap/pm/state';
import { StepMap } from '@tiptap/pm/transform';
import type { EditorView } from '@tiptap/pm/view';

export interface MovableBlock {
  node: Node;
  parent: Node;
  parentStart: number;
  position: number;
  index: number;
  siblings: number;
}

const tableStructure = new Set(['tableRow', 'tableCell', 'tableHeader']);

function blockAt($pos: ResolvedPos, parentDepth: number, position: number): MovableBlock | undefined {
  const parent = $pos.node(parentDepth);
  const index = $pos.index(parentDepth);
  const node = parent.maybeChild(index);
  const last = parent.lastChild;
  const siblings = parent.childCount - (last?.attrs.editorCursor && last.content.size === 0 ? 1 : 0);
  if (!node?.isBlock || tableStructure.has(node.type.name) || index >= siblings || siblings < 2) return;
  return { node, parent, parentStart: $pos.start(parentDepth), position, index, siblings };
}

export function blocksAt(doc: Node, from: number, to: number, atBoundary: boolean): MovableBlock[] {
  const $pos = doc.resolve(from);
  const candidates = [];
  if (atBoundary && !$pos.textOffset) {
    const block = blockAt($pos, $pos.depth, from);
    if (block && from + block.node.nodeSize >= to) candidates.push(block);
  }
  for (let depth = $pos.depth; depth > 0; depth--) {
    const before = $pos.before(depth);
    const block = blockAt($pos, depth - 1, before);
    if (block && before + block.node.nodeSize >= to) candidates.push(block);
  }
  return candidates;
}

export function blockMove(block: MovableBlock, gap: number) {
  const { parent, index, position, node, siblings } = block;
  if (gap < 0 || gap > siblings || gap === index || gap === index + 1) return;

  const first = Math.min(index, gap);
  const last = Math.max(index + 1, gap);
  const newIndex = gap < index ? first : last - 1;
  const reordered: Node[] = [];
  let from = position;
  for (let i = first; i < last; i++) {
    const sibling = parent.child(i);
    if (i < index) from -= sibling.nodeSize;
    if (i !== index) reordered.push(sibling);
  }
  reordered.splice(newIndex - first, 0, node);
  const content = Fragment.fromArray(reordered);
  if (!parent.canReplace(first, last, content)) return;
  const offset = gap < index ? from - position : content.size - node.nodeSize;
  return { from, content, offset, newIndex };
}

export function selectedMoveBlock(state: EditorState) {
  const { from, to } = state.selection;
  return blocksAt(state.doc, from, to, state.selection instanceof NodeSelection)
    .find((block) => blockMove(block, block.index - 1) || blockMove(block, block.index + 2));
}

export function moveBlock(view: EditorView, block: MovableBlock, gap: number): number | undefined {
  if (!view.editable || view.composing || block.position >= view.state.doc.content.size
    || view.state.doc.resolve(block.position).parent !== block.parent) return;
  const move = blockMove(block, gap);
  if (!move) return;
  const { doc, selection } = view.state;
  const within = selection.from >= block.position && selection.to <= block.position + block.node.nodeSize;
  const anchor = within ? selection : Selection.near(doc.resolve(block.position));
  const transaction = closeHistory(view.state.tr.replaceWith(move.from, move.from + move.content.size, move.content));
  transaction.setSelection(anchor.getBookmark().map(StepMap.offset(move.offset)).resolve(transaction.doc));
  view.dispatch(transaction.scrollIntoView());
  // Keep typing immediately after a move in its own undo event.
  view.dispatch(closeHistory(view.state.tr));
  return move.newIndex;
}
