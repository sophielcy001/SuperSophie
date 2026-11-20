// SPDX-License-Identifier: AGPL-3.0-or-later

import { useRef, useState } from 'react';
import { Modal } from '../dialogs';
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

export function WebLinkDialog({ initialHref, nameField, onSave, onRemove, onClose }: {
  initialHref?: string; nameField: boolean; onSave(href: string, label: string): void;
  onRemove(): void; onClose(): void;
}) {
  const [input, setInput] = useState(initialHref ?? '');
  const [label, setLabel] = useState('');
  const [attempted, setAttempted] = useState(false);
  const addressField = useRef<HTMLInputElement>(null);
  const href = webAddress(input.trim());
  return <Modal title="网页链接" onClose={onClose} initialFocus={addressField}>
    <form className="dialog-form" onSubmit={(event) => {
      event.preventDefault();
      setAttempted(true);
      if (href) onSave(href, label.trim() || href);
    }}>
      <label htmlFor="web-link-url">网址</label>
      <input id="web-link-url" inputMode="url" autoComplete="off" spellCheck={false} ref={addressField}
        maxLength={4096} placeholder="https://example.com" value={input}
        onChange={(event) => setInput(event.currentTarget.value)} />
      {nameField && <>
        <label htmlFor="web-link-label">显示文字</label>
        <input id="web-link-label" value={label} maxLength={500} onChange={(event) => setLabel(event.currentTarget.value)} />
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
