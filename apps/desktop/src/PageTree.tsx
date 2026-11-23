// SPDX-License-Identifier: AGPL-3.0-or-later

import { ChevronRight } from 'lucide-react';
import { useId, useMemo, useState } from 'react';
import type { KeyboardEvent } from 'react';
import type { PageInfo } from './contracts';
import { PageIcon } from './icons';
import { BatchCheckbox } from './organize/BatchControls';
import type { BatchSelection } from './organize/BatchControls';
import { composing } from './keyboard';

export function pageTrail(byId: ReadonlyMap<string, PageInfo>, id: string) {
  const seen = new Set<string>();
  const trail: PageInfo[] = [];
  let current = byId.get(id);
  while (current && !seen.has(current.id)) {
    seen.add(current.id);
    trail.push(current);
    current = current.parentId ? byId.get(current.parentId) : undefined;
  }
  return trail.reverse();
}

const LARGE_TREE = 1_000;
const PAGE_BATCH = 64;

export function PageTree({ byId, collectionByPage, selected, disabled, batch, onOpen }: {
  byId: ReadonlyMap<string, PageInfo>; collectionByPage: ReadonlyMap<string, string>; selected?: string; disabled: boolean; onOpen(id: string): void;
  batch?: BatchSelection;
}) {
  const [expansion, setExpansion] = useState(() => new Map<string, boolean>());
  const [shown, setShown] = useState(() => new Map<string | null, number>());
  const treeId = useId();
  const branch = useMemo(() => new Map((selected ? pageTrail(byId, selected) : [])
    .map((page) => [page.parentId, page])), [byId, selected]);
  const { children, large } = useMemo(() => {
    const children = new Map<string | null, PageInfo[]>();
    let count = 0;
    for (const page of byId.values()) {
      if (page.trashRootId || (page.parentId && collectionByPage.has(page.parentId))) continue;
      const list = children.get(page.parentId) ?? [];
      list.push(page);
      children.set(page.parentId, list);
      count++;
    }
    return { children, large: count > LARGE_TREE };
  }, [byId, collectionByPage]);

  function childrenOf(parent: string | null) {
    if (parent && collectionByPage.has(parent)) {
      const record = branch.get(parent);
      return record && !record.trashRootId ? [record] : [];
    }
    return children.get(parent) ?? [];
  }

  function navigate(event: KeyboardEvent<HTMLElement>) {
    if (disabled || event.defaultPrevented || composing(event.nativeEvent)
      || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    const target = event.target;
    if (!(target instanceof HTMLButtonElement) || !target.matches('.tree-link, .tree-more')) return;
    const { key } = event;
    if (key === 'ArrowUp' || key === 'ArrowDown' || key === 'Home' || key === 'End') {
      const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('.tree-link:not(:disabled), .tree-more:not(:disabled)'));
      const index = buttons.indexOf(target);
      const next = key === 'Home' ? buttons[0] : key === 'End' ? buttons.at(-1) : buttons[index + (key === 'ArrowUp' ? -1 : 1)];
      event.preventDefault();
      next?.focus();
      return;
    }
    if (key !== 'ArrowLeft' && key !== 'ArrowRight') return;
    const row = target.closest('.tree-branch');
    if (!row) return;
    const rowButton = (branch: Element | null) => branch?.querySelector<HTMLButtonElement>(':scope > .tree-row > .tree-link');
    const toggle = row.querySelector<HTMLButtonElement>(':scope > .tree-row > .tree-toggle');
    const expanded = toggle?.getAttribute('aria-expanded') === 'true';
    event.preventDefault();
    if (target.classList.contains('tree-more')) {
      if (key === 'ArrowLeft') rowButton(row)?.focus();
    } else if (key === 'ArrowRight') {
      if (toggle && !expanded) toggle.click();
      else row.querySelector<HTMLButtonElement>(':scope > .tree-children > .tree-branch > .tree-row > .tree-link')?.focus();
    } else if (expanded && !toggle?.disabled) {
      toggle?.click();
    } else {
      rowButton(row.parentElement?.closest('.tree-branch') ?? null)?.focus();
    }
  }

  function rows(parent: string | null, depth: number) {
    const siblings = childrenOf(parent);
    const limit = large ? shown.get(parent) ?? PAGE_BATCH : siblings.length;
    const visible = siblings.length > limit ? siblings.slice(0, limit) : siblings;
    const currentChild = branch.get(parent);
    if (currentChild && !currentChild.trashRootId && siblings.length > limit && !visible.includes(currentChild)) {
      visible.push(currentChild);
    }
    return <div className="tree-children">
      {visible.map((page) => {
        const expandable = childrenOf(page.id).length > 0;
        const ancestor = branch.has(page.id);
        const expanded = ancestor || (expansion.get(page.id) ?? !large);
        return <div className="tree-branch" key={page.id}>
          <div className={`tree-row${selected === page.id ? ' selected' : ''}${batch?.ids.has(page.id) ? ' batch-selected' : ''}`}
            style={{ paddingInlineStart: `${6 + Math.min(depth, 10) * 15}px` }}>
            {batch && <BatchCheckbox batch={batch} id={page.id} title={page.title} disabled={disabled} />}
            {expandable ? <button type="button" className="tree-toggle" aria-label={`${expanded ? '折叠' : '展开'} ${page.title}`}
              aria-expanded={expanded} disabled={disabled || ancestor}
              onClick={() => setExpansion((old) => new Map(old).set(page.id, !expanded))}>
              <ChevronRight className={expanded ? 'expanded' : ''} size={13} /></button>
              : <span className="tree-spacer" />}
            <button id={`${treeId}-${page.id}`} type="button" className="tree-link" aria-current={selected === page.id ? 'page' : undefined}
              title={page.title} aria-disabled={disabled || undefined} onClick={() => { if (!disabled) onOpen(page.id); }}>
              <PageIcon kind={page.icon} size={16} /><span>{page.title}</span>
            </button>
          </div>
          {expandable && expanded ? rows(page.id, depth + 1) : null}
        </div>;
      })}
      {visible.length < siblings.length && <button type="button" className="tree-more"
        style={{ paddingInlineStart: `${27 + Math.min(depth, 10) * 15}px` }}
        aria-label={parent ? `显示更多页面 · ${byId.get(parent)?.title}` : '显示更多页面'}
        disabled={disabled} onClick={(event) => {
          const button = event.currentTarget;
          const first = siblings[limit];
          setShown((old) => new Map(old).set(parent, limit + PAGE_BATCH));
          requestAnimationFrame(() => {
            if (document.activeElement === button || (!button.isConnected && document.activeElement === document.body)) {
              document.getElementById(`${treeId}-${first.id}`)?.focus();
            }
          });
        }}>
        显示更多页面
      </button>}
    </div>;
  }

  return <nav className="page-tree" aria-label="页面树" aria-describedby={`${treeId}-help`} onKeyDown={navigate}>
    <span className="visually-hidden" id={`${treeId}-help`}>方向键浏览页面，Enter 打开当前页。</span>
    {rows(null, 0)}
  </nav>;
}
