// SPDX-License-Identifier: AGPL-3.0-or-later

import type { Editor } from '@tiptap/react';
import type { Node } from '@tiptap/pm/model';
import { GripVertical, Plus } from 'lucide-react';
import { useEffect, useRef } from 'react';
import { blockMove, blocksAt, moveBlock } from './block-move';
import type { MovableBlock } from './block-move';

interface Drag {
  grip: HTMLSpanElement;
  pointer: number;
  doc: Node;
  block: MovableBlock;
  rows: Element[];
  parent: Element | undefined;
  startX: number;
  startY: number;
  x: number;
  y: number;
  started: boolean;
}

function placeGrip(grip: HTMLElement, row: DOMRect | undefined, surface: DOMRect) {
  grip.hidden = !row;
  if (row) grip.style.top = `${row.top - surface.top}px`;
}

function dropGap(rows: Element[], y: number) {
  let first = 0;
  let end = rows.length;
  while (first < end) {
    const middle = Math.floor((first + end) / 2);
    const row = rows[middle].getBoundingClientRect();
    if (y < (row.top + row.bottom) / 2) end = middle;
    else first = middle + 1;
  }
  return first;
}

export function BlockDrag({ editor, disabled, onMove, onMenu }: {
  editor: Editor | null;
  disabled: boolean;
  onMove(index: number): void;
  /** A click on the grip without dragging opens the block menu, as in Notion. */
  onMenu?(block: MovableBlock, grip: DOMRect, doc: Node): void;
}) {
  const outerRef = useRef<HTMLSpanElement>(null);
  const innerRef = useRef<HTMLSpanElement>(null);
  const lineRef = useRef<HTMLSpanElement>(null);
  const addRef = useRef<HTMLSpanElement>(null);

  useEffect(() => {
    const outer = outerRef.current;
    const inner = innerRef.current;
    const line = lineRef.current;
    const add = addRef.current;
    const surface = outer?.parentElement;
    if (!editor || !outer || !inner || !line || !add || !surface || disabled) return;
    const view = editor.view;
    const scroll = surface.closest<HTMLElement>('.document-scroll');
    const hovered = new Map<HTMLElement, MovableBlock>();
    let hoveredDocument: Node | undefined;
    let drag: Drag | undefined;
    let frame = 0;

    function hideGrips() {
      hovered.clear();
      hoveredDocument = undefined;
      outer!.hidden = true;
      inner!.hidden = true;
      add!.hidden = true;
    }

    function cancel() {
      cancelAnimationFrame(frame);
      frame = 0;
      const active = drag;
      drag = undefined;
      line!.hidden = true;
      surface!.classList.remove('block-is-dragging');
      hideGrips();
      if (active?.grip.hasPointerCapture(active.pointer)) active.grip.releasePointerCapture(active.pointer);
    }

    function destination(active: Drag) {
      if (!active.started || !view.editable || view.composing || view.state.doc !== active.doc) return;
      const rect = surface!.getBoundingClientRect();
      const viewport = scroll?.getBoundingClientRect();
      if (active.x < rect.left - 24 || active.x > rect.right + 24
        || active.y < Math.max(rect.top - 8, viewport?.top ?? 0)
        || active.y > Math.min(rect.bottom, viewport?.bottom ?? window.innerHeight)) return;
      const group = active.parent?.getBoundingClientRect();
      if (group && (active.y < group.top - 8 || active.y > group.bottom + 8)) return;
      const gap = dropGap(active.rows, active.y);
      if (!blockMove(active.block, gap)) return;
      return { gap, rect };
    }

    function markDrop(active: Drag) {
      const point = destination(active);
      line!.hidden = !point;
      if (!point) return;
      const above = active.rows[point.gap - 1]?.getBoundingClientRect();
      const below = active.rows[point.gap]?.getBoundingClientRect();
      const boundary = below?.top ?? above!.bottom;
      const row = below ?? above!;
      line!.style.top = `${((above?.bottom ?? boundary) + boundary) / 2 - point.rect.top}px`;
      line!.style.left = `${row.left - point.rect.left}px`;
      line!.style.width = `${row.width}px`;
    }

    function scrollAtEdge() {
      frame = 0;
      if (!drag?.started || !scroll) return;
      const rect = scroll.getBoundingClientRect();
      if (drag.x < rect.left || drag.x > rect.right || drag.y < rect.top || drag.y > rect.bottom) return;
      const edge = 50;
      const nearTop = Math.max(0, 1 - (drag.y - rect.top) / edge);
      const nearBottom = Math.max(0, 1 - (rect.bottom - drag.y) / edge);
      const before = scroll.scrollTop;
      scroll.scrollTop += Math.round((nearBottom - nearTop) * 17);
      if (scroll.scrollTop === before) return;
      markDrop(drag);
      frame = requestAnimationFrame(scrollAtEdge);
    }

    function pointerMove(event: PointerEvent) {
      if (drag) {
        if (event.pointerId !== drag.pointer) return;
        drag.x = event.clientX;
        drag.y = event.clientY;
        if (!drag.started && Math.hypot(drag.x - drag.startX, drag.y - drag.startY) < 4) return;
        drag.started = true;
        surface!.classList.add('block-is-dragging');
        markDrop(drag);
        if (!frame) frame = requestAnimationFrame(scrollAtEdge);
        return;
      }
      const target = event.target;
      if (!(target instanceof Element) || target.closest('.block-drag-handle')) return;
      if (event.buttons || !view.editable || view.composing || !view.dom.contains(target)) {
        hideGrips();
        return;
      }
      const hit = view.posAtCoords({ left: event.clientX, top: event.clientY });
      const candidates = hit ? blocksAt(view.state.doc, hit.inside < 0 ? hit.pos : hit.inside,
        hit.inside < 0 ? hit.pos : hit.inside, hit.inside >= 0) : [];
      const root = candidates.find((block) => block.parentStart === 0);
      const nested = candidates.find((block) => block.parentStart > 0 &&
        (blockMove(block, block.index - 1) || blockMove(block, block.index + 2)));
      const bounds = surface!.getBoundingClientRect();
      hovered.clear();
      hoveredDocument = view.state.doc;
      for (const [grip, block] of [[outer!, root], [inner!, nested]] as const) {
        const element = block ? view.nodeDOM(block.position) : undefined;
        const rect = element instanceof Element ? element.getBoundingClientRect() : undefined;
        const row = rect && event.clientY >= rect.top && event.clientY <= rect.bottom ? rect : undefined;
        if (block && row) hovered.set(grip, block);
        placeGrip(grip, row, bounds);
        if (grip === outer) placeGrip(add!, row, bounds);
      }
    }

    function pointerDown(event: PointerEvent) {
      const grip = event.target instanceof Element ? event.target.closest<HTMLSpanElement>('.block-drag-handle') : null;
      const block = grip ? hovered.get(grip) : undefined;
      if (!grip || !block || drag || event.button !== 0 || !view.editable || view.composing
        || hoveredDocument !== view.state.doc) return;
      const rows: Element[] = [];
      let position = block.parentStart;
      for (let index = 0; index < block.siblings; index++) {
        const element = view.nodeDOM(position);
        if (!(element instanceof Element)) return;
        rows.push(element);
        position += block.parent.child(index).nodeSize;
      }
      const parent = block.parentStart ? view.nodeDOM(block.parentStart - 1) : undefined;
      event.preventDefault();
      grip.setPointerCapture(event.pointerId);
      drag = {
        grip, block, rows, doc: view.state.doc, parent: parent instanceof Element ? parent : undefined,
        pointer: event.pointerId, startX: event.clientX, startY: event.clientY,
        x: event.clientX, y: event.clientY, started: false,
      };
    }

    function pointerUp(event: PointerEvent) {
      const active = drag;
      if (!active) {
        pointerMove(event);
        return;
      }
      if (event.pointerId !== active.pointer) return;
      active.x = event.clientX;
      active.y = event.clientY;
      const point = destination(active);
      const grip = active.grip.getBoundingClientRect();
      cancel();
      if (!active.started) {
        if (view.state.doc === active.doc) onMenu?.(active.block, grip, active.doc);
        return;
      }
      if (point) {
        const moved = moveBlock(view, active.block, point.gap);
        if (moved !== undefined) {
          view.focus();
          onMove(moved);
        }
      }
    }

    // Like Notion's "+": start a new block below the hovered one with the insert menu open.
    function addBelow(event: MouseEvent) {
      const block = hovered.get(outer!);
      if (!block || drag || !view.editable || view.composing || hoveredDocument !== view.state.doc) return;
      event.preventDefault();
      const node = view.state.doc.nodeAt(block.position);
      if (!node) return;
      if (node.type.name === 'paragraph' && node.content.size === 0) {
        editor!.chain().focus().insertContentAt(block.position + 1, '/').run();
      } else {
        const at = block.position + node.nodeSize;
        editor!.chain().focus()
          .insertContentAt(at, { type: 'paragraph', content: [{ type: 'text', text: '/' }] })
          .setTextSelection(at + 2).run();
      }
      hideGrips();
    }
    function pointerOut() { if (!drag) hideGrips(); }
    function pointerCancel(event: PointerEvent) { if (drag?.pointer === event.pointerId) cancel(); }
    function escape(event: KeyboardEvent) {
      if (!drag || event.key !== 'Escape' || event.isComposing) return;
      event.preventDefault();
      event.stopPropagation();
      cancel();
    }

    const listeners = new AbortController();
    const options = { signal: listeners.signal };
    surface.addEventListener('pointermove', pointerMove, options);
    surface.addEventListener('pointerdown', pointerDown, options);
    add.addEventListener('click', addBelow, options);
    add.addEventListener('mousedown', (event) => event.preventDefault(), options);
    surface.addEventListener('pointerup', pointerUp, options);
    surface.addEventListener('pointerleave', pointerOut, options);
    surface.addEventListener('pointercancel', pointerCancel, options);
    surface.addEventListener('lostpointercapture', pointerCancel, options);
    document.addEventListener('keydown', escape, { ...options, capture: true });
    window.addEventListener('blur', cancel, options);
    return () => {
      cancel();
      listeners.abort();
    };
  }, [editor, disabled, onMove, onMenu]);

  return <>
    <span ref={addRef} className="block-drag-handle block-add-handle" hidden aria-hidden="true" title="在下方插入"><Plus size={16} /></span>
    <span ref={outerRef} className="block-drag-handle block-drag-outer" hidden aria-hidden="true" title="拖动移块，点击打开菜单"><GripVertical size={16} /></span>
    <span ref={innerRef} className="block-drag-handle block-drag-inner" hidden aria-hidden="true" title="拖动移块，点击打开菜单"><GripVertical size={16} /></span>
    <span ref={lineRef} className="block-drop-line" hidden aria-hidden="true" />
  </>;
}
