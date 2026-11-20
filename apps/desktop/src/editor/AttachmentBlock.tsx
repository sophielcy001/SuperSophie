// SPDX-License-Identifier: AGPL-3.0-or-later

import { Node } from '@tiptap/core';
import { NodeViewWrapper, ReactNodeViewRenderer } from '@tiptap/react';
import type { NodeViewProps } from '@tiptap/react';
import { ArrowUpRight, Paperclip } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { attachmentRenamed, sizeText } from '../attachments/Attachments';
import type { AttachmentChange } from '../attachments/Attachments';
import type { Attachment } from '../contracts';
import { desktop } from '../desktop';
import { fileTarget } from './blocks';

interface FileContext {
  workspace: string;
  pageId: string;
  openFile?(id: string): void;
}

export function FileCard({ workspace, pageId, attachmentId, openFile }: FileContext & { attachmentId: string }) {
  const card = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(false);
  const [file, setFile] = useState<Attachment | null>();

  useEffect(() => {
    if (!card.current || visible) return;
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) setVisible(true);
    });
    observer.observe(card.current);
    return () => observer.disconnect();
  }, [visible]);

  useEffect(() => {
    if (!visible) return;
    let active = true;
    let renamed = false;
    function update(event: Event) {
      const { workspace: origin, item } = (event as CustomEvent<AttachmentChange>).detail;
      if (origin === workspace && item.pageId === pageId && item.id === attachmentId) {
        renamed = true;
        setFile(item);
      }
    }
    window.addEventListener(attachmentRenamed, update);
    void desktop.attachment(workspace, pageId, attachmentId).then((item) => {
      if (active && !renamed) setFile(item);
    }).catch(() => { if (active && !renamed) setFile(null); });
    return () => { active = false; window.removeEventListener(attachmentRenamed, update); };
  }, [workspace, pageId, attachmentId, visible]);

  return <div ref={card} className="file-card" data-drag-handle="">
    <span className="attachment-icon"><Paperclip size={17} aria-hidden /></span>
    <div>
      {file && openFile ?
        <button type="button" className="file-card-open" onClick={() => openFile(file.id)}>
          {file.name} <ArrowUpRight size={14} aria-hidden />
        </button> : <strong>{file?.name ?? (file === null ? '附件不可用' : '附件')}</strong>}
      {file && <small>{sizeText(file.bytes)}</small>}
    </div>
  </div>;
}

function AttachmentView({ extension, node, editor }: NodeViewProps) {
  const context = extension.options as FileContext;
  const target = fileTarget(node.attrs);
  return <NodeViewWrapper className="attachment-block" contentEditable={false}>
    {target && <FileCard key={target.attachmentId} {...context} {...target}
      openFile={(id) => { if (editor.isEditable) context.openFile?.(id); }} />}
  </NodeViewWrapper>;
}

export const AttachmentBlock = Node.create<FileContext>({
  name: 'attachmentFile', group: 'block', atom: true, draggable: true,
  addOptions() { return { workspace: '', pageId: '' }; },
  addAttributes() { return { attachmentId: { default: null, rendered: false } }; },
  parseHTML() {
    return [{ tag: 'div[data-zhiye-file]', getAttrs: (element) => {
      if (element.getAttribute('data-workspace') !== this.options.workspace
        || element.getAttribute('data-page') !== this.options.pageId) return false;
      return fileTarget({ attachmentId: element.getAttribute('data-zhiye-file') }) ?? false;
    } }];
  },
  renderHTML({ node, HTMLAttributes }) {
    return ['div', { ...HTMLAttributes, 'data-zhiye-file': node.attrs.attachmentId,
      'data-workspace': this.options.workspace, 'data-page': this.options.pageId }, '附件'];
  },
  addNodeView() { return ReactNodeViewRenderer(AttachmentView); },
});
