// SPDX-License-Identifier: AGPL-3.0-or-later

import type { JSONContent } from '@tiptap/core';
import type { Block, BlockTarget, ViewTarget } from '../contracts';
import { imageAttributes, tableAlignment, webAddress } from './rich-content';
import { calloutTone, mathSource, toggleTitle } from './content-shapes';

const containers = new Set(['blockquote', 'bulletList', 'orderedList', 'listItem', 'taskList', 'taskItem', 'toggleBlock', 'callout']);
const tableCells = new Set(['tableCell', 'tableHeader']);
const tableNodes = new Set(['table', 'tableRow', ...tableCells]);
const structuralNodes = new Set(['listItem', 'taskItem', 'tableRow', ...tableCells]);
const allowedMarks = new Set(['bold', 'italic', 'underline', 'strike', 'code']);
const blockKeys = new Set(['id', 'type', 'schemaVersion', 'payload', 'children']);
const numberingStyles = new Set(['1', 'a', 'A', 'i', 'I']);

export const needsCursorParagraph = new Set(['horizontalRule', 'unknownBlock', 'attachmentImage', 'attachmentFile',
  'blockReference', 'collectionView', 'mathBlock', 'tableOfContents', 'toggleBlock', 'callout', 'table']);

export function cursorParagraphAttributes() {
  return { id: crypto.randomUUID(), editorCursor: true };
}

export const blockTypes = [...new Set(['paragraph', 'heading', 'codeBlock', ...needsCursorParagraph, ...containers,
  'tableRow', ...tableCells])];

const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;

export function blockTarget(attrs: Record<string, unknown>): BlockTarget | undefined {
  const { pageId, blockId } = attrs;
  if (typeof pageId === 'string' && typeof blockId === 'string' && uuid.test(pageId) && uuid.test(blockId)) {
    return { pageId, blockId };
  }
}

export function viewTarget(attrs: Record<string, unknown>): ViewTarget | undefined {
  const { collectionId, viewId } = attrs;
  if (typeof collectionId === 'string' && typeof viewId === 'string' && uuid.test(collectionId) && uuid.test(viewId)) {
    return { collectionId, viewId };
  }
}

export function fileTarget(attrs: Record<string, unknown>) {
  const { attachmentId } = attrs;
  return typeof attachmentId === 'string' && uuid.test(attachmentId) ? { attachmentId } : undefined;
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function inlineMark(mark: unknown): NonNullable<JSONContent['marks']>[number] | undefined {
  if (!isObject(mark) || typeof mark.type !== 'string') return;
  const count = Object.keys(mark).length;
  if (count === 1 && allowedMarks.has(mark.type)) return { type: mark.type };
  const href = mark.type === 'link' && count === 2 ? webAddress(mark.href) : undefined;
  if (href) return { type: 'link', attrs: { href } };
}

function inlineNode(item: unknown): JSONContent | undefined {
  if (!isObject(item)) return;
  if (item.type === 'text' && typeof item.text === 'string' && item.text.length) {
    if (Object.keys(item).some((key) => !['type', 'text', 'marks'].includes(key))) return;
    if (item.marks === undefined) return { type: 'text', text: item.text };
    if (Array.isArray(item.marks)) {
      const marks = item.marks.map(inlineMark);
      if (marks.every((mark) => mark !== undefined)) return { type: 'text', text: item.text, marks };
    }
  }
  if (item.type === 'hardBreak' && Object.keys(item).length === 1) return { type: 'hardBreak' };
  if (item.type === 'pageReference' && Object.keys(item).length === 3 &&
    typeof item.pageId === 'string' && typeof item.label === 'string') {
    return { type: 'pageReference', attrs: { pageId: item.pageId, label: item.label } };
  }
}

function tableHasValidGrid(rows: JSONContent[]) {
  const width = rows[0]?.content?.reduce((sum, cell) => sum + Number(cell.attrs?.colspan ?? 1), 0) ?? 0;
  if (!width || width * rows.length > 20_000) return false;
  const occupied = new Int32Array(width);
  for (const [rowIndex, row] of rows.entries()) {
    let column = 0;
    for (const cell of row.content ?? []) {
      while (column < width && occupied[column] > rowIndex) column++;
      const end = column + Number(cell.attrs?.colspan ?? 1);
      const bottom = rowIndex + Number(cell.attrs?.rowspan ?? 1);
      if (end > width || bottom > rows.length) return false;
      for (; column < end; column++) {
        if (occupied[column] > rowIndex) return false;
        occupied[column] = bottom;
      }
    }
    if (occupied.some((bottom) => bottom <= rowIndex)) return false;
  }
  return true;
}

function tableNode(type: string, attrs: Record<string, unknown>, payload: Block['payload'], children: Block[]): JSONContent | undefined {
  if (!children.length && type !== 'tableRow') return;
  const content = children.map(blockNode);
  if (type === 'table' || type === 'tableRow') {
    if (Object.keys(payload).length) return;
    if (type === 'tableRow' ? content.some((cell) => !tableCells.has(cell.type ?? ''))
      : content.some((row) => row.type !== 'tableRow') || !tableHasValidGrid(content)) return;
  } else {
    if (content.some((node) => structuralNodes.has(node.type ?? ''))
      || Object.keys(payload).some((key) => !['align', 'colspan', 'rowspan', 'colwidth'].includes(key))) return;
    for (const key of ['colspan', 'rowspan']) {
      const span = payload[key] ?? 1;
      if (typeof span !== 'number' || !Number.isInteger(span) || span < 1 || span > 20_000) return;
      attrs[key] = span;
    }
    if (payload.colwidth !== undefined) {
      if (!Array.isArray(payload.colwidth) || payload.colwidth.length !== attrs.colspan
        || !payload.colwidth.every((width) => Number.isSafeInteger(width) && width >= 0)) return;
      attrs.colwidth = payload.colwidth;
    }
    if (payload.align !== undefined) {
      const align = tableAlignment(payload.align);
      if (!align) return;
      attrs.align = align;
    }
  }
  return { type, attrs, content };
}

function supportedNode(block: Block): JSONContent | undefined {
  if (block.schemaVersion !== 1 || !isObject(block.payload) || Object.keys(block).some((key) => !blockKeys.has(key))) return;
  const { type, id, payload, children } = block;
  const attrs: Record<string, unknown> = { id };
  const keys = Object.keys(payload);

  if (type === 'paragraph' || type === 'heading') {
    if (children.length || !Array.isArray(payload.inline) || keys.some((key) => key !== 'inline' && (type !== 'heading' || key !== 'level'))) return;
    if (type === 'heading') {
      if (typeof payload.level !== 'number' || ![1, 2, 3].includes(payload.level)) return;
      attrs.level = payload.level;
    }
    const content = payload.inline.map(inlineNode);
    if (content.some((node) => !node)) return;
    return { type, attrs, content: content as JSONContent[] };
  }
  if (type === 'codeBlock') {
    if (children.length || typeof payload.text !== 'string' || keys.some((key) => key !== 'text' && key !== 'language')) return;
    if (payload.language !== undefined && typeof payload.language !== 'string') return;
    return { type, attrs: { ...attrs, language: payload.language ?? null }, content: payload.text ? [{ type: 'text', text: payload.text }] : [] };
  }
  if (type === 'horizontalRule' && keys.length === 0 && children.length === 0) return { type, attrs };
  if (type === 'attachmentImage') {
    const image = keys.length === 3 && children.length === 0 ? imageAttributes(payload) : undefined;
    if (image) return { type, attrs: { ...attrs, ...image } };
    return;
  }
  if (type === 'blockReference') {
    const target = keys.length === 2 && children.length === 0 ? blockTarget(payload) : undefined;
    if (target) return { type, attrs: { id, ...target } };
    return;
  }
  if (type === 'collectionView') {
    const target = keys.length === 2 && children.length === 0 ? viewTarget(payload) : undefined;
    return target ? { type, attrs: { id, ...target } } : undefined;
  }
  if (type === 'attachmentFile') {
    const target = keys.length === 1 && children.length === 0 ? fileTarget(payload) : undefined;
    return target ? { type, attrs: { id, ...target } } : undefined;
  }
  if (type === 'mathBlock') {
    const latex = keys.length === 1 && children.length === 0 ? mathSource(payload.latex) : undefined;
    return latex === undefined ? undefined : { type, attrs: { id, latex } };
  }
  if (type === 'tableOfContents') {
    return !keys.length && !children.length ? { type, attrs } : undefined;
  }
  if (tableNodes.has(type)) return tableNode(type, attrs, payload, children);
  if (!containers.has(type) || children.length === 0) return;

  if (type === 'orderedList') {
    if (keys.some((key) => key !== 'start' && key !== 'type')
      || (payload.start !== undefined && (typeof payload.start !== 'number' || !Number.isSafeInteger(payload.start)))
      || (payload.type !== undefined && (typeof payload.type !== 'string' || !numberingStyles.has(payload.type)))) return;
    attrs.start = payload.start ?? 1;
    if (payload.type) attrs.type = payload.type;
  } else if (type === 'taskItem') {
    if (keys.some((key) => key !== 'checked') || (payload.checked !== undefined && typeof payload.checked !== 'boolean')) return;
    attrs.checked = payload.checked ?? false;
  } else if (type === 'toggleBlock') {
    const title = toggleTitle(payload.title);
    if (keys.length !== 2 || title === undefined || typeof payload.collapsed !== 'boolean') return;
    attrs.title = title;
    attrs.collapsed = payload.collapsed;
  } else if (type === 'callout') {
    const tone = calloutTone(payload.tone);
    if (keys.length !== 1 || !tone) return;
    attrs.tone = tone;
  } else if (keys.length) return;

  const content = children.map(blockNode);
  if (type === 'bulletList' || type === 'orderedList' || type === 'taskList') {
    if (content.some((node) => node.type !== (type === 'taskList' ? 'taskItem' : 'listItem'))) return;
  } else if (type === 'listItem' || type === 'taskItem') {
    if (content[0]?.type !== 'paragraph' || content.some((node) => structuralNodes.has(node.type ?? ''))) return;
  } else if (content.some((node) => structuralNodes.has(node.type ?? ''))) return;
  return { type, attrs, content };
}

export function blockNode(block: Block): JSONContent {
  return supportedNode(block) ?? { type: 'unknownBlock', attrs: { id: block.id, raw: block } };
}

export function fromBlocks(blocks: Block[]): JSONContent {
  const content = blocks.map((block) => {
    const node = blockNode(block);
    return structuralNodes.has(node.type ?? '')
      ? { type: 'unknownBlock', attrs: { id: block.id, raw: block } }
      : node;
  });
  const last = content.at(-1)?.type;
  if (!last || needsCursorParagraph.has(last)) {
    // Give the cursor paragraph its ID before mounting, so initialization does not trigger autosave.
    content.push({ type: 'paragraph', attrs: cursorParagraphAttributes() });
  }
  return { type: 'doc', content };
}

function asInline(node: JSONContent) {
  if (node.type === 'pageReference') return { type: node.type, pageId: node.attrs?.pageId, label: node.attrs?.label };
  if (node.type === 'hardBreak') return { type: 'hardBreak' };
  const marks = node.marks?.flatMap((mark) => {
    if (mark.type === 'link') {
      const href = webAddress(mark.attrs?.href);
      return href ? [{ type: 'link', href }] : [];
    }
    return [{ type: mark.type }];
  });
  return { type: 'text', text: node.text ?? '', ...(marks?.length ? { marks } : {}) };
}

function asBlock(node: JSONContent, seen: Set<string>): Block {
  const requested = node.attrs?.id;
  const id = typeof requested === 'string' && !seen.has(requested) ? requested : crypto.randomUUID();
  seen.add(id);
  if (node.type === 'unknownBlock' && isObject(node.attrs?.raw)) {
    const raw = node.attrs.raw as Block;
    const keepIds = (block: Block): Block => {
      const childId = seen.has(block.id) ? crypto.randomUUID() : block.id;
      seen.add(childId);
      return { ...block, id: childId, children: block.children.map(keepIds) };
    };
    return { ...raw, id, children: raw.children.map(keepIds) };
  }

  const type = node.type ?? 'paragraph';
  const block: Block = { id, type, schemaVersion: 1, payload: {}, children: [] };
  if (type === 'paragraph' || type === 'heading') {
    block.payload.inline = (node.content ?? []).map(asInline);
    if (type === 'heading') block.payload.level = node.attrs?.level ?? 1;
  } else if (type === 'codeBlock') {
    block.payload.text = (node.content ?? []).map((part) => part.text ?? '').join('');
    if (typeof node.attrs?.language === 'string') block.payload.language = node.attrs.language;
  } else if (type === 'attachmentImage') {
    block.payload = imageAttributes(node.attrs ?? {}) ?? {};
  } else if (type === 'blockReference') {
    const target = blockTarget(node.attrs ?? {});
    block.payload = target ? { ...target } : {};
  } else if (type === 'collectionView') {
    const target = viewTarget(node.attrs ?? {});
    block.payload = target ? { ...target } : {};
  } else if (type === 'attachmentFile') {
    block.payload = fileTarget(node.attrs ?? {}) ?? {};
  } else if (type === 'mathBlock') {
    block.payload.latex = mathSource(node.attrs?.latex) ?? '';
  } else if (containers.has(type) || tableNodes.has(type)) {
    if (type === 'orderedList') {
      block.payload.start = Number.isSafeInteger(node.attrs?.start) ? node.attrs?.start : 1;
      if (numberingStyles.has(node.attrs?.type)) block.payload.type = node.attrs?.type;
    }
    if (type === 'taskItem') block.payload.checked = node.attrs?.checked ?? false;
    if (type === 'toggleBlock') {
      block.payload.title = toggleTitle(node.attrs?.title) ?? '';
      block.payload.collapsed = node.attrs?.collapsed === true;
    }
    if (type === 'callout') block.payload.tone = calloutTone(node.attrs?.tone) ?? 'note';
    if (tableCells.has(type)) {
      for (const key of ['colspan', 'rowspan']) {
        if (typeof node.attrs?.[key] === 'number' && node.attrs[key] > 1) block.payload[key] = node.attrs[key];
      }
      if (Array.isArray(node.attrs?.colwidth)) block.payload.colwidth = node.attrs.colwidth;
      const align = tableAlignment(node.attrs?.align);
      if (align) block.payload.align = align;
    }
    block.children = (node.content ?? []).map((child) => asBlock(child, seen));
  }
  return block;
}

export function toBlocks(doc: JSONContent): Block[] {
  const seen = new Set<string>();
  const children = doc.content ?? [];
  const last = children.at(-1);
  const blocks = last?.attrs?.editorCursor && !last.content?.length ? children.slice(0, -1) : children;
  return blocks.map((node) => asBlock(node, seen));
}
