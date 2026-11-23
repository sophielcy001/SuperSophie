// SPDX-License-Identifier: AGPL-3.0-or-later

import { X } from 'lucide-react';
import { useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { FormEvent, ReactNode, RefObject } from 'react';
import type { BlobCleanupPreview, HistoryEntry, Page, PageInfo, PropertyDefinition, TrashPreview } from './contracts';
import { PageIcon } from './icons';
import { BatchCheckbox } from './organize/BatchControls';
import { pageTrail } from './PageTree';
import { FieldValue, autoField } from './collections/fields';
import { composing } from './keyboard';

export function Modal({ title, children, onClose, busy = false, className = '', initialFocus }: {
  title: string; children: ReactNode; onClose(): void; busy?: boolean; className?: string;
  initialFocus?: RefObject<HTMLElement | null>;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  useLayoutEffect(() => {
    const node = ref.current;
    if (!node) return;
    node.showModal();
    initialFocus?.current?.focus({ preventScroll: true });
    return () => { node.close(); };
  }, [initialFocus]);
  return <dialog ref={ref} className={`dialog ${className}`} aria-labelledby={titleId}
    onCancel={(event) => { event.preventDefault(); if (!busy) onClose(); }}
    onKeyDown={(event) => {
      if (event.key !== 'Tab' || event.defaultPrevented || composing(event.nativeEvent)) return;
      const candidates = event.currentTarget.querySelectorAll<HTMLElement>(
        'button, a[href], input, select, textarea, summary, [tabindex], [contenteditable="true"]',
      );
      const controls = Array.from(candidates).filter((node) => node.tabIndex >= 0 && !node.matches(':disabled')
        && node.getClientRects().length > 0 && getComputedStyle(node).visibility === 'visible');
      const first = controls[0];
      const last = controls.at(-1);
      const active = document.activeElement;
      if (!first || (event.shiftKey ? active === first : active === last)) {
        event.preventDefault();
        (event.shiftKey ? last : first)?.focus();
      }
    }}
    onClick={(event) => {
      if (event.target !== event.currentTarget || busy) return;
      const rect = event.currentTarget.getBoundingClientRect();
      if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) onClose();
    }}>
    <header className="dialog-header"><h2 id={titleId}>{title}</h2>
      <button type="button" className="icon-button" title="关闭" aria-label="关闭" onClick={onClose} disabled={busy}><X size={18} /></button>
    </header>
    {children}
  </dialog>;
}

export function WorkspaceDialog({ busy, onCreate, onClose }: { busy: boolean; onCreate(name: string): void; onClose(): void }) {
  const [name, setName] = useState('');
  const input = useRef<HTMLInputElement>(null);
  function submit(event: FormEvent) { event.preventDefault(); if (name.trim()) onCreate(name.trim()); }
  return <Modal title="新建工作空间" onClose={onClose} busy={busy} initialFocus={input}>
    <form className="dialog-form" onSubmit={submit}>
      <label htmlFor="workspace-name">名称</label>
      <input id="workspace-name" ref={input} placeholder="例如：阅读与研究" maxLength={64} value={name} disabled={busy}
        onChange={(event) => setName(event.currentTarget.value)} />
      <footer className="dialog-actions"><button type="button" className="secondary-button" disabled={busy} onClick={onClose}>取消</button>
        <button type="submit" className="primary-button" disabled={busy || !name.trim()}>创建</button></footer>
    </form>
  </Modal>;
}

export function MoveDialog({ pages, roots, batch = false, collectionByPage, busy, onMove, onClose }: {
  pages: PageInfo[]; roots: PageInfo[]; batch?: boolean; collectionByPage: ReadonlyMap<string, string>;
  busy: boolean; onMove(id: string | null): void; onClose(): void;
}) {
  const initialParent = roots[0]?.parentId;
  const [parent, setParent] = useState(roots.every((page) => page.parentId === initialParent) ? initialParent ?? '' : '');
  const select = useRef<HTMLSelectElement>(null);
  const byId = useMemo(() => new Map(pages.map((item) => [item.id, item])), [pages]);
  const rootIds = new Set(roots.map((page) => page.id));
  const choices = pages.flatMap((candidate) => {
    if (candidate.trashRootId || collectionByPage.has(candidate.id)) return [];
    const path = pageTrail(byId, candidate.id);
    return path.some((item) => rootIds.has(item.id)) ? [] : [{
      id: candidate.id,
      label: path.map((item) => item.title).join(' / '),
    }];
  });
  const canMove = roots.some((page) => (page.parentId ?? '') !== parent);
  return <Modal title={batch ? '批量移动' : '移动页面'} onClose={onClose} busy={busy} initialFocus={select}>
    <form className="dialog-form" onSubmit={(event) => { event.preventDefault(); if (canMove && !busy) onMove(parent || null); }}>
      <label htmlFor="move-parent">{batch ? '目标位置' : `选择「${roots[0]?.title}」的上级位置`}</label>
      <select ref={select} id="move-parent" value={parent} disabled={busy} onChange={(event) => setParent(event.currentTarget.value)}>
        <option value="">工作空间根目录</option>
        {choices.map((item) => <option key={item.id} value={item.id}>{item.label}</option>)}
      </select>
      {batch && <p className="library-hint">移动 {roots.length} 个选中的分支，分支内的层级会保留。</p>}
      <footer className="dialog-actions"><button type="button" className="secondary-button" onClick={onClose} disabled={busy}>取消</button>
        <button type="submit" className="primary-button" disabled={busy || !canMove}>移动</button></footer>
    </form>
  </Modal>;
}

export type TrashReview = { kind: 'pages'; preview: TrashPreview } | { kind: 'blobs'; preview: BlobCleanupPreview };

export function TrashDialog({ pages, busy, review, onRestore, onReview, onConfirm, onBack, onClose }: {
  pages: PageInfo[];
  busy: boolean;
  review?: TrashReview;
  onRestore(pages: PageInfo[]): void;
  onReview(kind: TrashReview['kind']): void;
  onConfirm(review: TrashReview): void;
  onBack(): void;
  onClose(): void;
}) {
  const [ids, setIds] = useState(() => new Set<string>());
  const cancel = useRef<HTMLButtonElement>(null);
  const roots = pages.filter((page) => page.trashRootId === page.id).sort((a, b) => b.updatedAt - a.updatedAt);
  const selected = roots.filter((page) => ids.has(page.id));
  const batch = { ids, select: (items: readonly string[], checked: boolean) => setIds((previous) => {
    const next = new Set(previous);
    for (const id of items) { if (checked) next.add(id); else next.delete(id); }
    return next;
  }) };
  if (review?.kind === 'pages') {
    const { pages, branches, affectedRecords, templates } = review.preview;
    return <Modal title="清空回收站" onClose={onBack} busy={busy} initialFocus={cancel}>
      <p className="dialog-copy">{pages > 0
        ? `将永久删除 ${pages} 个页面及其修订，共 ${branches} 个分支。清空前保存的备份仍可恢复这些页面。`
        : '回收站为空。'}</p>
      {affectedRecords > 0 && <p className="dialog-copy">{affectedRecords} 条保留记录会生成新修订以移除已删关联。</p>}
      {templates > 0 && <p className="dialog-copy">{templates} 个仅适用于已删数据库的模板也会删除。</p>}
      <footer className="dialog-actions dialog-pad">
        <button ref={cancel} type="button" className="secondary-button" disabled={busy} onClick={onBack}>取消</button>
        <button type="button" className="danger-button" disabled={busy || pages === 0}
          onClick={() => onConfirm(review)}>确认永久删除</button>
      </footer>
    </Modal>;
  }
  if (review?.kind === 'blobs') {
    const { waiting, files, bytes } = review.preview;
    return <Modal title="清理附件原件" onClose={onBack} busy={busy} initialFocus={cancel}>
      <p className="dialog-copy">{files > 0
        ? `可清理 ${files} 份无引用的原件，共 ${bytes.toLocaleString('zh-CN')} 字节。`
        : '当前没有可清理的附件原件。'}</p>
      {waiting > 0 && <p className="dialog-copy">{waiting} 份原件正在等待七天保留期。</p>}
      <footer className="dialog-actions dialog-pad">
        <button ref={cancel} type="button" className="secondary-button" disabled={busy} onClick={onBack}>{files > 0 ? '取消' : '返回回收站'}</button>
        <button type="button" className="danger-button" disabled={busy || files === 0}
          onClick={() => onConfirm(review)}>确认清理</button>
      </footer>
    </Modal>;
  }
  return <Modal title="回收站" onClose={onClose} busy={busy}>
    <div className="dialog-list">
      {roots.length === 0 && <p className="dialog-empty">回收站为空。</p>}
      {roots.map((page) => <div key={page.id} className="dialog-list-row">
        <BatchCheckbox batch={batch} id={page.id} title={page.title} disabled={busy} />
        <PageIcon kind={page.icon} /><span>{page.title}</span>
        <button type="button" className="secondary-button" onClick={() => onRestore([page])} disabled={busy}>恢复</button>
      </div>)}
    </div>
    <footer className="dialog-actions dialog-pad">
      <button type="button" className="secondary-button" disabled={busy} onClick={() => onReview('blobs')}>检查附件清理</button>
      {roots.length > 0 && <>
        <button type="button" className="danger-button" disabled={busy} onClick={() => onReview('pages')}>清空回收站</button>
        <button type="button" className="primary-button" disabled={busy || !selected.length} onClick={() => onRestore(selected)}>恢复所选</button>
      </>}
    </footer>
  </Modal>;
}

const dateFormat = new Intl.DateTimeFormat('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' });

function excerpt(page: Page) {
  function text(blocks: Page['blocks']): string[] {
    return blocks.flatMap((block) => {
      const inline = Array.isArray(block.payload.inline) ? block.payload.inline : [];
      const line = inline.map((part: { text?: string; label?: string }) => part?.text ?? part?.label ?? '').join('');
      return [line || (typeof block.payload.text === 'string' ? block.payload.text : ''), ...text(block.children)].filter(Boolean);
    });
  }
  return text(page.blocks).join('\n').slice(0, 1800);
}

export function HistoryDialog({ entries, fields, busy, onClose, onRestore }: {
  entries: HistoryEntry[]; fields: PropertyDefinition[]; busy: boolean; onClose(): void; onRestore(revision: number): void;
}) {
  const [revision, setRevision] = useState(entries[0]?.revision ?? 0);
  const selected = entries.find((entry) => entry.revision === revision);
  return <Modal title="页面历史" onClose={onClose} busy={busy} className="history-dialog">
    <div className="history-body">
      <div className="history-list" aria-label="保存记录">
        {entries.map((entry, index) => <button type="button" key={entry.revision}
          className={`history-row${revision === entry.revision ? ' highlighted' : ''}`}
          disabled={busy} onClick={() => setRevision(entry.revision)}>
          <strong>{index === 0 ? '当前版本' : `版本 ${entry.revision}`}</strong><small>{dateFormat.format(entry.updatedAt)}</small>
        </button>)}
      </div>
      {selected && <div className="history-preview"><h3><PageIcon kind={selected.icon} />{selected.title}</h3>
        {selected.attribution && <p className="integration-source">集成 · {selected.attribution.integrationName}</p>}
        {selected.cover && <p className="library-hint">此版本含页面封面。</p>}
        <p>{excerpt(selected) || '此版本没有正文。'}</p>
        {selected.historicalProperties?.length ? <p className="library-hint">
          此版本包含已变更类型的属性。恢复时按当前类型转换，无法转换的值将留空。
        </p> : null}
        {selected.properties && <dl className="record-panel-values">{fields.filter((field) => !autoField(field.type)).map((current) => {
          const field = selected.historicalProperties?.find(({ id }) => id === current.id) ?? current;
          return <div key={field.id}><dt>{field.name}</dt><dd><FieldValue field={field} value={selected.properties?.[field.id]} /></dd></div>;
        })}</dl>}
      </div>}
    </div>
    <footer className="dialog-actions history-actions"><button type="button" className="primary-button"
      disabled={busy || !selected || revision === entries[0]?.revision} onClick={() => onRestore(revision)}>恢复为新版本</button></footer>
  </Modal>;
}
