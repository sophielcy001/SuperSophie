// SPDX-License-Identifier: AGPL-3.0-or-later

import { ImagePlus } from 'lucide-react';
import { useId, useRef, useState } from 'react';
import type { Attachment, Page } from '../contracts';
import { Modal } from '../dialogs';
import { PageIcon, pageIconOptions } from '../icons';
import { AttachmentFigure } from './AttachmentImage';
import { isImageAttachment } from './rich-content';

export function PageAppearanceDialog({ workspace, page, attachments, busy, onClose, onImport, onSave }: {
  workspace: string; page: Page; attachments: Attachment[]; busy: boolean;
  onClose(): void; onImport(): Promise<Attachment | undefined>;
  onSave(icon: string, coverAttachmentId: string | null): void;
}) {
  const id = useId();
  const first = useRef<HTMLSelectElement>(null);
  const originalCover = page.cover?.attachmentId ?? '';
  const [icon, setIcon] = useState(page.icon);
  const [cover, setCover] = useState(originalCover);
  const images = attachments.filter(isImageAttachment);
  const selected = images.find((image) => image.id === cover);
  const changed = icon !== page.icon || cover !== originalCover;

  return <Modal title="页面外观" onClose={onClose} busy={busy} initialFocus={first}>
    <form className="dialog-form" onSubmit={(event) => {
      event.preventDefault();
      if (changed && !busy) onSave(icon, cover || null);
    }}>
      <label htmlFor={`${id}-icon`}>图标</label>
      <div className="page-icon-picker">
        <PageIcon kind={icon} size={20} />
        <select ref={first} id={`${id}-icon`} value={icon} disabled={busy} onChange={(event) => setIcon(event.currentTarget.value)}>
          {!pageIconOptions.some(({ id }) => id === page.icon) && <option value={page.icon}>当前图标</option>}
          {pageIconOptions.map(({ id, label }) => <option key={id} value={id}>{label}</option>)}
        </select>
      </div>
      <label htmlFor={`${id}-cover`}>封面</label>
      <div className="page-cover-picker">
        <select id={`${id}-cover`} value={cover} disabled={busy} onChange={(event) => setCover(event.currentTarget.value)}>
          <option value="">无封面</option>
          {images.map(({ id, name }) => <option key={id} value={id}>{name}</option>)}
        </select>
        <button type="button" className="secondary-button" disabled={busy} onClick={async () => {
          const image = await onImport();
          if (image) setCover(image.id);
        }}><ImagePlus size={16} aria-hidden />导入图片</button>
      </div>
      {selected && <div className="page-cover page-cover-preview">
        <AttachmentFigure key={selected.id} workspace={workspace} pageId={page.id} defaultAlt="封面预览"
          attachment={{ attachmentId: selected.id, mediaType: selected.mediaType, alt: '' }} />
      </div>}
      <footer className="dialog-actions"><button type="button" className="secondary-button" disabled={busy} onClick={onClose}>取消</button>
        <button type="submit" className="primary-button" disabled={busy || !changed}>保存外观</button></footer>
    </form>
  </Modal>;
}
