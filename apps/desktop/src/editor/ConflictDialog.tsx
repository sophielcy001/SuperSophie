// SPDX-License-Identifier: AGPL-3.0-or-later

import { useState } from 'react';
import type { FormEvent } from 'react';
import type { Block, Page, PageDraft } from '../contracts';
import { errorMessage } from '../contracts';
import { Modal } from '../dialogs';

function preview(blocks: Block[]): string {
  return blocks.map((block) => {
    const { inline, text, alt, title, latex } = block.payload;
    if (Array.isArray(inline)) {
      return inline.map((item) => {
        if (item?.type === 'hardBreak') return '\n';
        if (typeof item?.text === 'string') return item.text;
        if (typeof item?.label === 'string') return `[[${item.label}]]`;
        return '［保留内容］';
      }).join('');
    }
    if (typeof text === 'string') return text;
    if (block.type === 'attachmentImage') return `［图片］${typeof alt === 'string' ? alt : ''}`;
    if (block.type === 'attachmentFile') return '［附件］';
    if (block.type === 'mathBlock' && typeof latex === 'string') return `［公式］${latex}`;
    if (block.type === 'tableOfContents') return '［目录］';
    if (block.type === 'toggleBlock' && typeof title === 'string' && block.children.length) return `［折叠］${title}\n${preview(block.children)}`;
    if (block.children.length) return preview(block.children);
    return `［${block.type}］`;
  }).join('\n');
}

type Version = 'saved' | 'local';

export function ConflictDialog({ saved, local, onClose, onSave }: {
  saved: Page; local: PageDraft; onClose(): void; onSave(draft: PageDraft): Promise<void>;
}) {
  const [title, setTitle] = useState<Version>('saved');
  const [body, setBody] = useState<Version>('saved');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError('');
    try {
      await onSave({ title: title === 'local' ? local.title : saved.title,
        blocks: body === 'local' ? local.blocks : saved.blocks });
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  return <Modal title="对比保存版本和草稿" busy={busy} onClose={onClose} className="conflict-dialog">
    <p className="dialog-copy">页面已有新的保存版本。可以分别选择要保留的标题和正文。</p>
    <div className="conflict-versions">
      <section aria-label="已保存版本">
        <h3>已保存版本 {saved.revision}</h3><p>{saved.title}</p><pre>{preview(saved.blocks) || '（空正文）'}</pre>
      </section>
      <section aria-label="本机草稿">
        <h3>本机草稿</h3><p>{local.title}</p><pre>{preview(local.blocks) || '（空正文）'}</pre>
      </section>
    </div>
    <form className="dialog-form" onSubmit={(event) => { void submit(event); }}>
      {error && <p className="dialog-error" role="alert">{error}</p>}
      <label htmlFor="conflict-title">保留标题</label>
      <select id="conflict-title" disabled={busy} value={title} onChange={(event) => setTitle(event.currentTarget.value as Version)}>
        <option value="saved">已保存版本</option><option value="local">本机草稿</option>
      </select>
      <label htmlFor="conflict-body">保留正文</label>
      <select id="conflict-body" disabled={busy} value={body} onChange={(event) => setBody(event.currentTarget.value as Version)}>
        <option value="saved">已保存版本</option><option value="local">本机草稿</option>
      </select>
      <div className="dialog-actions">
        <button type="button" className="secondary-button" disabled={busy} onClick={onClose}>返回草稿</button>
        <button type="submit" className="primary-button" disabled={busy}>保存所选版本</button>
      </div>
    </form>
  </Modal>;
}
