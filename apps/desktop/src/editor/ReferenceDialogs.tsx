// SPDX-License-Identifier: AGPL-3.0-or-later

import type { JSONContent } from '@tiptap/core';
import { ArrowUpRight, Link2, Search } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import type { BacklinkPage, BlockTarget, Page } from '../contracts';
import { errorMessage } from '../contracts';
import { desktop } from '../desktop';
import { Modal } from '../dialogs';
import { PageIcon } from '../icons';
import { fromBlocks } from './blocks';

const blockLabels: Record<string, string> = {
  paragraph: '正文', heading: '标题', blockquote: '引用', bulletList: '项目列表', orderedList: '编号列表',
  taskList: '待办清单', listItem: '列表项', taskItem: '待办项', codeBlock: '代码块', table: '表格',
  tableRow: '表格行', tableCell: '单元格', tableHeader: '表头', attachmentImage: '图片', horizontalRule: '分割线',
  attachmentFile: '附件', toggleBlock: '折叠', callout: '提示块', mathBlock: '公式',
};

function excerpt(node: JSONContent): string {
  const parts: string[] = [];
  let remaining = 145;
  function collect(item: JSONContent) {
    if (remaining <= 0 || item.type === 'unknownBlock' || item.type === 'blockReference') return;
    const text = item.text ?? item.attrs?.label ?? item.attrs?.alt ?? item.attrs?.title ?? item.attrs?.latex;
    if (typeof text === 'string') {
      if (parts.length) remaining = Math.max(0, remaining - 1);
      parts.push(text.slice(0, remaining));
      remaining -= text.length;
    }
    for (const child of item.content ?? []) collect(child);
  }
  collect(node);
  const summary = parts.join(' ').replace(/\s+/g, ' ').trim();
  return remaining < 0 ? `${summary}…` : summary;
}

export function BlockPicker({ page, preferred, excluded, onClose, onBack, onChoose }: {
  page: Page; preferred?: string | null; excluded?: string;
  onClose(): void; onBack(): void; onChoose(target: BlockTarget): void;
}) {
  const input = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState('');
  const [limit, setLimit] = useState(50);
  const candidates = useMemo(() => {
    const items: { blockId: string; label: string; text: string }[] = [];
    function collect(nodes: JSONContent[]) {
      for (const node of nodes) {
        const blockId = node.attrs?.id;
        const label = node.type && blockLabels[node.type];
        if (!label || !blockId || node.attrs?.editorCursor || blockId === excluded) continue;
        items.push({ blockId, label, text: excerpt(node) });
        collect(node.content ?? []);
      }
    }
    collect(fromBlocks(page.blocks).content ?? []);
    const match = items.findIndex((item) => item.blockId === preferred);
    if (match > 0) items.unshift(...items.splice(match, 1));
    return items;
  }, [page, preferred, excluded]);
  const term = query.trim().toLocaleLowerCase();
  const visible = candidates.filter((item) => `${item.label} ${item.text}`.toLocaleLowerCase().includes(term));

  return <Modal title="选择源块" className="picker-dialog" initialFocus={input} onClose={onClose}>
    <div className="picker-input">
      <Search size={17} aria-hidden />
      <input ref={input} aria-label="筛选源块" placeholder={page.title} value={query} maxLength={200}
        onChange={(event) => { setQuery(event.currentTarget.value); setLimit(50); }} />
    </div>
    <div className="picker-results" aria-label="源块">
      {visible.slice(0, limit).map(({ blockId, label, text }) =>
        <button type="button" className="picker-row" key={blockId} onClick={() => onChoose({ pageId: page.id, blockId })}>
          <span className="picker-page-icon"><Link2 size={16} aria-hidden /></span>
          <span className="picker-result-text"><strong>{text || label}</strong>{text && <small>{label}</small>}</span>
        </button>)}
      {!visible.length && <p className="dialog-empty">{query ? '没有匹配的块。' : '这个页面还没有可引用的块。'}</p>}
    </div>
    <footer className="picker-footer">
      <button type="button" className="secondary-button" onClick={onBack}>另选页面</button>
      {visible.length > limit && <button type="button" className="secondary-button" onClick={() => setLimit(limit + 50)}>加载更多</button>}
    </footer>
  </Modal>;
}

export function BacklinkDialog({ workspace, page, onClose, onOpen }: {
  workspace: string; page: string; onClose(): void; onOpen(id: string, blockId: string): void;
}) {
  const [items, setItems] = useState<BacklinkPage['items']>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [request, setRequest] = useState<{ after: string | null }>({ after: null });
  const [finished, setFinished] = useState<typeof request>();
  const [error, setError] = useState('');
  const loading = request !== finished;

  useEffect(() => {
    let alive = true;
    void desktop.backlinks(workspace, page, request.after).then((result) => {
      if (!alive) return;
      setItems((previous) => request.after ? [...previous, ...result.items] : result.items);
      setCursor(result.next_cursor);
      setError('');
    }).catch((error: unknown) => {
      if (alive) setError(errorMessage(error));
    }).finally(() => { if (alive) setFinished(request); });
    return () => { alive = false; };
  }, [workspace, page, request]);

  return <Modal title="反向链接" className="picker-dialog" onClose={onClose}>
    <div className="picker-results" aria-label="引用位置" aria-busy={loading}>
      {items.map((link) => <button type="button" key={link.block_id} className="picker-row"
        onClick={() => onOpen(link.page_id, link.block_id)}>
        <span className="picker-page-icon"><PageIcon kind={link.icon} /></span>
        <span className="picker-result-text"><strong>{link.title}</strong>
          <small>{link.target_kind === 'block' ? '块引用' : '页面链接'}</small></span>
        <ArrowUpRight size={15} aria-hidden />
      </button>)}
      {error && !loading && <p className="dialog-empty" role="alert">{error}</p>}
      {loading && <p className="dialog-empty" role="status">正在查找引用位置…</p>}
      {!loading && !error && !items.length && <p className="dialog-empty">还没有页面链接到这里。</p>}
    </div>
    {!loading && (cursor || error) && <footer className="picker-footer">
      <button type="button" className="secondary-button" onClick={() => setRequest({ after: error ? request.after : cursor })}>
        {error ? '重试' : '加载更多'}
      </button>
    </footer>}
  </Modal>;
}
