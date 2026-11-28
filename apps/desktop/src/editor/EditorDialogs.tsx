// SPDX-License-Identifier: AGPL-3.0-or-later

import { useRef, useState } from 'react';
import type { PageInfo } from '../contracts';
import { Modal } from '../dialogs';
import { CommentText } from './CommentText';
import { commentMention } from './page-directory';
import { webAddress } from './rich-content';

export function CloseDraftDialog({ busy, onClose, onDiscard }: {
  busy: boolean; onClose(): void; onDiscard(): void;
}) {
  const keepDraft = useRef<HTMLButtonElement>(null);
  return <Modal title="关闭未保存的页面" busy={busy} onClose={onClose} initialFocus={keepDraft}>
    <p className="dialog-copy">关闭页面会清除未保存的草稿。可先导出草稿留存。</p>
    <footer className="dialog-actions dialog-pad">
      <button ref={keepDraft} type="button" className="secondary-button" disabled={busy} onClick={onClose}>返回草稿</button>
      <button type="button" className="danger-button" disabled={busy} onClick={onDiscard}>放弃草稿并关闭</button>
    </footer>
  </Modal>;
}

/** What 「读取网页标题」 found: title, description and a preview image saved as an attachment. */
export interface WebSummary {
  title: string | null; description: string | null; image: { attachmentId: string; mediaType: string } | null; href?: string;
}

export function WebLinkDialog({ initialHref, nameField, onSave, onRemove, onClose, heading = '网页链接', labelText = '显示文字', onFetchTitle }: {
  initialHref?: string; nameField: boolean; onSave(href: string, label: string, summary?: WebSummary): void;
  onRemove(): void; onClose(): void; heading?: string; labelText?: string;
  /** Offers 「读取网页标题」, which visits the address only when pressed. */
  onFetchTitle?(href: string): Promise<WebSummary>;
}) {
  const [summary, setSummary] = useState<WebSummary>();
  const [input, setInput] = useState(initialHref ?? '');
  const [label, setLabel] = useState('');
  const [fetching, setFetching] = useState(false);
  const [fetchNote, setFetchNote] = useState<string>();
  const [attempted, setAttempted] = useState(false);
  const addressField = useRef<HTMLInputElement>(null);
  const href = webAddress(input.trim());
  return <Modal title={heading} onClose={onClose} initialFocus={addressField}>
    <form className="dialog-form" onSubmit={(event) => {
      event.preventDefault();
      setAttempted(true);
      // A summary read for another address does not belong to this one.
      if (href) onSave(href, label.trim() || href, summary?.href === href ? summary : undefined);
    }}>
      <label htmlFor="web-link-url">网址</label>
      <input id="web-link-url" inputMode="url" autoComplete="off" spellCheck={false} ref={addressField}
        maxLength={4096} placeholder="https://example.com" value={input}
        onChange={(event) => setInput(event.currentTarget.value)} />
      {nameField && <>
        <label htmlFor="web-link-label">{labelText}</label>
        <input id="web-link-label" value={label} maxLength={500} onChange={(event) => setLabel(event.currentTarget.value)} />
        {onFetchTitle && <div className="web-title-fetch">
          <button type="button" className="secondary-button" disabled={!href || fetching} onClick={() => {
            if (!href) return;
            setFetching(true);
            setFetchNote(undefined);
            onFetchTitle(href).then((found) => {
              const { title, description, image } = found;
              if (title) setLabel(title);
              setSummary({ ...found, href });
              if (!title) setFetchNote('这个网页没有可读取的标题。');
              else setFetchNote([description && `摘要：${description}`, image && '已保存预览图。'].filter(Boolean).join(' ') || '已读取标题。');
            }).catch((error: unknown) => setFetchNote(error instanceof Error ? error.message
              : typeof error === 'object' && error && 'message' in error ? String(error.message) : '无法读取网页标题。'))
              .finally(() => setFetching(false));
          }}>{fetching ? '正在读取…' : '读取网页标题'}</button>
          <span className="library-hint" role="status">{fetchNote ?? '保存后会自动访问这个网址，读取标题、摘要和预览图。'}</span>
        </div>}
      </>}
      {attempted && !href && <p className="library-hint" role="alert">请输入完整的 HTTP 或 HTTPS 地址。</p>}
      <footer className="dialog-actions library-template-actions">
        {initialHref && <button type="button" className="text-button" onClick={onRemove}>移除链接</button>}
        <button type="button" className="secondary-button" onClick={onClose}>取消</button>
        <button type="submit" className="primary-button">保存链接</button>
      </footer>
    </form>
  </Modal>;
}

export function ImageCaptionDialog({ initial, onSave, onClose }: {
  initial: string; onSave(alt: string): void; onClose(): void;
}) {
  const [alt, setAlt] = useState(initial);
  const field = useRef<HTMLInputElement>(null);
  return <Modal title="图片说明" initialFocus={field} onClose={onClose}>
    <form className="dialog-form" onSubmit={(event) => { event.preventDefault(); onSave(alt.trim()); }}>
      <label htmlFor="image-caption">说明文字</label>
      <input id="image-caption" ref={field} value={alt} maxLength={500}
        onChange={(event) => setAlt(event.currentTarget.value.replace(/[\u0000-\u001f\u007f-\u009f]/gu, ' '))} />
      <footer className="dialog-actions">
        <button type="button" className="secondary-button" onClick={onClose}>取消</button>
        <button type="submit" className="primary-button">保存说明</button>
      </footer>
    </form>
  </Modal>;
}

/** A personal comment on the selected text, as Notion's comment box. */
export function CommentDialog({ initial, onSave, onRemove, onClose, reply, pages = [] }: {
  initial?: string; onSave(text: string): void; onRemove(): void; onClose(): void;
  /** Writes a reply in the comment's thread instead of editing the comment. */
  reply?: boolean;
  /** Pages offered after typing "@", as in Notion's comments. */
  pages?: PageInfo[];
}) {
  const [text, setText] = useState(initial ?? '');
  const [caret, setCaret] = useState((initial ?? '').length);
  const [highlighted, setHighlighted] = useState(0);
  const field = useRef<HTMLTextAreaElement>(null);
  const value = text.trim();
  // "@" at the start or after a space, followed by part of a page title.
  const mention = /(?:^|\s)@([^\s@\[\]]{0,30})$/u.exec(text.slice(0, caret));
  const suggestions = mention ? pages.filter((page) => !page.trashRootId
    && (page.title || '无标题').toLowerCase().includes(mention[1].toLowerCase())).slice(0, 6) : [];
  function choose(page: PageInfo) {
    if (!mention) return;
    const start = caret - mention[1].length - 1;
    const token = commentMention(page) + ' ';
    const next = text.slice(0, start) + token + text.slice(caret);
    setText(next);
    setCaret(start + token.length);
    requestAnimationFrame(() => {
      field.current?.focus();
      field.current?.setSelectionRange(start + token.length, start + token.length);
    });
  }
  return <Modal title={reply ? '回复评论' : '评论'} initialFocus={field} onClose={onClose}>
    <form className="dialog-form" onSubmit={(event) => { event.preventDefault(); if (value) onSave(value); }}>
      <label htmlFor="comment-text">{reply ? '回复内容' : '评论内容'}</label>
      <textarea ref={field} id="comment-text" rows={4} maxLength={2000} value={text}
        aria-autocomplete="list" aria-controls={suggestions.length ? 'comment-mentions' : undefined}
        onChange={(event) => { setText(event.currentTarget.value); setCaret(event.currentTarget.selectionStart); setHighlighted(0); }}
        onSelect={(event) => setCaret(event.currentTarget.selectionStart)}
        onKeyDown={(event) => {
          if (!suggestions.length || event.nativeEvent.isComposing) return;
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault();
            setHighlighted((highlighted + (event.key === 'ArrowDown' ? 1 : -1) + suggestions.length) % suggestions.length);
          } else if (event.key === 'Enter' || event.key === 'Tab') {
            event.preventDefault();
            choose(suggestions[highlighted] ?? suggestions[0]);
          }
        }} />
      {/\]\(page:/.test(text) && <p className="comment-preview" aria-label="预览"><CommentText text={text} /></p>}
      {suggestions.length > 0 && <div id="comment-mentions" className="menu comment-mention-menu" role="listbox" aria-label="提及页面">
        {suggestions.map((page, index) => <button key={page.id} type="button" role="option" aria-selected={index === highlighted}
          className={`menu-item${index === highlighted ? ' highlighted' : ''}`}
          onMouseDown={(event) => event.preventDefault()} onClick={() => choose(page)}>{page.title || '无标题'}</button>)}
      </div>}
      <footer className="dialog-actions library-template-actions">
        {initial && !reply && <button type="button" className="text-button" onClick={onRemove}>解决并移除</button>}
        <button type="button" className="secondary-button" onClick={onClose}>取消</button>
        <button type="submit" className="primary-button" disabled={!value}>{reply ? '发送回复' : '保存评论'}</button>
      </footer>
    </form>
  </Modal>;
}
