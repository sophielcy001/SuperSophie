// SPDX-License-Identifier: AGPL-3.0-or-later

import type { Editor } from '@tiptap/react';
import type { Node } from '@tiptap/pm/model';
import type { LucideIcon } from 'lucide-react';
import { ArrowDown, ArrowUp, Check, ChevronLeft, ChevronRight, Copy, Palette, Trash2 } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import type { KeyboardEvent } from 'react';
import { composing } from '../keyboard';
import { blockKinds, currentKind, deleteBlock, duplicateBlock, focusBlock, turnInto } from './block-actions';
import type { BlockKind } from './block-actions';
import { blockMove, moveBlock } from './block-move';
import { blockColor, inlineColors } from './marks';
import type { MovableBlock } from './block-move';

export interface BlockMenuTarget {
  block: MovableBlock;
  doc: Node;
  left: number;
  top: number;
}

const textContainers = new Set(['listItem', 'taskItem']);

/** The text block whose kind "Turn into" changes, if the block has one. */
function turnTarget(node: Node, position: number): number | undefined {
  if (node.isTextblock) return position;
  if (textContainers.has(node.type.name) && node.firstChild?.isTextblock) return position + 1;
  // A list with a single item reads as one block.
  if (node.type.name.endsWith('List') && node.childCount === 1) return turnTarget(node.child(0), position + 1);
}

export function BlockMenu({ editor, target, kindIcons, shortcut, onClose, onMoved }: {
  editor: Editor;
  target: BlockMenuTarget;
  kindIcons: Record<BlockKind, LucideIcon>;
  shortcut(keys: string): string;
  onClose(): void;
  onMoved(index: number): void;
}) {
  const menu = useRef<HTMLDivElement>(null);
  const { block } = target;
  const textAt = turnTarget(block.node, block.position);
  const kind = textAt === undefined ? undefined : currentKind(editor.state.doc.resolve(textAt + 1));
  const textNode = textAt === undefined ? undefined : editor.state.doc.nodeAt(textAt);
  const colorable = textNode && 'blockColor' in textNode.attrs;
  const color = blockColor(textNode?.attrs.blockColor);
  const [view, setView] = useState<'main' | 'color'>('main');

  useEffect(() => {
    menu.current?.querySelector<HTMLButtonElement>('button')?.focus();
  }, [view]);

  useEffect(() => {
    function dismiss(event: PointerEvent) {
      if (event.target instanceof globalThis.Node && !menu.current?.contains(event.target)) onClose();
    }
    document.addEventListener('pointerdown', dismiss);
    return () => document.removeEventListener('pointerdown', dismiss);
  }, [onClose]);

  /** Runs an action only while the document still matches what the menu was opened for. */
  function act(action: () => void) {
    onClose();
    if (editor.state.doc !== target.doc || !editor.isEditable) return;
    action();
  }

  function move(direction: -1 | 1) {
    act(() => {
      const moved = moveBlock(editor.view, block, block.index + (direction < 0 ? -1 : 2));
      if (moved !== undefined) {
        editor.view.focus();
        onMoved(moved);
      }
    });
  }

  function keyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (composing(event.nativeEvent)) return;
    const items = [...(menu.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? [])];
    const index = items.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === 'Escape') {
      event.preventDefault();
      onClose();
      editor.commands.focus();
    } else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      const step = event.key === 'ArrowDown' ? 1 : -1;
      items[(index + step + items.length) % items.length]?.focus();
    } else if (event.key === 'Home' || event.key === 'End') {
      event.preventDefault();
      items[event.key === 'Home' ? 0 : items.length - 1]?.focus();
    } else if (event.key === 'Tab') {
      onClose();
    }
  }

  const style = {
    left: Math.max(8, Math.min(target.left, window.innerWidth - 268)),
    top: Math.max(8, Math.min(target.top, window.innerHeight - (kind ? 560 : 200))),
  };
  const item = (label: string, Icon: LucideIcon, onClick: () => void, keys?: string, disabled?: boolean) =>
    <button type="button" role="menuitem" className="menu-item" disabled={disabled} onClick={onClick}>
      <Icon size={17} />{label}{keys && <kbd className="command-shortcut">{shortcut(keys)}</kbd>}
    </button>;

  function paint(next: string | null) {
    act(() => {
      if (textAt === undefined) return;
      editor.view.dispatch(editor.state.tr.setNodeAttribute(textAt, 'blockColor', next));
      editor.view.focus();
    });
  }

  if (view === 'color') {
    const choice = (label: string, value: string | null, swatch: { text?: string; background?: string }) =>
      <button key={label} type="button" role="menuitemradio" aria-checked={color === (value ?? undefined)} className="menu-item"
        onClick={() => paint(value)}>
        <span className="color-swatch" data-text-color={swatch.text} data-background-color={swatch.background} aria-hidden>A</span>
        {label}{color === (value ?? undefined) && <Check size={14} className="menu-check" aria-hidden />}
      </button>;
    return <div ref={menu} className="menu block-menu" role="menu" aria-label="块颜色" style={style} onKeyDown={keyDown}>
      <button type="button" role="menuitem" className="menu-item" onClick={() => setView('main')}><ChevronLeft size={17} />返回</button>
      <div className="menu-heading" aria-hidden>文字颜色</div>
      {choice('默认', null, {})}
      {inlineColors.map(({ id, label }) => choice(`${label}文字`, id, { text: id }))}
      <div className="menu-heading" aria-hidden>背景颜色</div>
      {inlineColors.map(({ id, label }) => choice(`${label}背景`, `${id}_background`, { background: id }))}
    </div>;
  }

  return <div ref={menu} className="menu block-menu" role="menu" aria-label="块操作" style={style} onKeyDown={keyDown}>
    {item('创建副本', Copy, () => act(() => duplicateBlock(editor, { node: block.node, position: block.position })), 'D')}
    {item('上移', ArrowUp, () => move(-1), '⇧↑', !blockMove(block, block.index - 1))}
    {item('下移', ArrowDown, () => move(1), '⇧↓', !blockMove(block, block.index + 2))}
    {item('删除', Trash2, () => act(() => deleteBlock(editor, { node: block.node, position: block.position })))}
    {colorable && <button type="button" role="menuitem" aria-haspopup="menu" className="menu-item" onClick={() => setView('color')}>
      <Palette size={17} />颜色<ChevronRight size={14} className="menu-check" aria-hidden /></button>}
    {kind && textAt !== undefined && <>
      <div className="menu-heading" aria-hidden>转换为</div>
      {blockKinds.map(({ kind: next, label }) => {
        const Icon = kindIcons[next];
        return <button key={next} type="button" role="menuitemradio" aria-checked={next === kind} className="menu-item"
          onClick={() => act(() => { focusBlock(editor, textAt); turnInto(editor, next); })}>
          <Icon size={17} />{label}{next === kind && <Check size={14} className="menu-check" aria-hidden />}
        </button>;
      })}
    </>}
  </div>;
}
