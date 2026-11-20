// SPDX-License-Identifier: AGPL-3.0-or-later

import { ArrowDownToLine, FilePlus2, FileText, Image, ImagePlus, Paperclip, Pencil, Plus, RotateCw } from 'lucide-react';
import { lazy, Suspense, useEffect, useState } from 'react';
import type { Attachment, PdfDocumentStatus } from '../contracts';
import { errorMessage } from '../contracts';
import { desktop } from '../desktop';
import { Modal } from '../dialogs';
import { isImageAttachment, maxImageBytes } from '../editor/rich-content';

const PdfPreview = lazy(() => import('./PdfPreview'));
const previewLimit = maxImageBytes;

export const attachmentRenamed = 'zhiye:attachment-renamed';
export interface AttachmentChange { workspace: string; item: Attachment }

export function sizeText(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}

interface Props {
  workspace: string;
  page: string;
  items: Attachment[];
  notice?: string;
  error?: string;
  preferredId?: string;
  preferredPdfPage?: number;
  pdfIndexRevision?: number;
  busy: boolean;
  onAdd(): void;
  onReload(): void;
  onInsertImage(item: Attachment): void;
  onInsertFile?(item: Attachment): void;
  onRename(item: Attachment, name: string): Promise<boolean>;
  onSave(item: Attachment): void;
  onClose(): void;
}

type Preview = { status: 'loading' } | { status: 'error'; message: string } | { status: 'image'; url: string }
  | { status: 'pdf'; data: ArrayBuffer } | { status: 'unavailable' };

function pdfStatus(status: PdfDocumentStatus | undefined) {
  switch (status?.status) {
    case 'indexed': return `已索引 ${status.indexedPages}/${status.pageCount} 页`;
    case 'partial': return `部分提取 · 已索引 ${status.indexedPages}/${status.pageCount} 页`;
    case 'noText': return '无文本层，未进行 OCR';
    case 'locked': return 'PDF 加密，无法提取';
    case 'unreadable': return 'PDF 无法读取或提取';
    case 'limitReached': return 'PDF 超出提取上限';
    default: return '等待提取 PDF 文本';
  }
}

export function AttachmentDialog({ workspace, page, items, notice, error, preferredId, preferredPdfPage, pdfIndexRevision, busy,
  onAdd, onReload, onInsertImage, onInsertFile, onRename, onSave, onClose }: Props) {
  const [item, setItem] = useState(() => items.find((attachment) => attachment.id === preferredId));
  const [landingPage, setLandingPage] = useState(preferredPdfPage);
  const [renaming, setRenaming] = useState<Attachment>();
  const [name, setName] = useState('');
  const [preview, setPreview] = useState<Preview>({ status: 'loading' });
  const [pdfStatuses, setPdfStatuses] = useState<Record<string, PdfDocumentStatus>>({});
  const [pdfStatusError, setPdfStatusError] = useState(false);

  useEffect(() => {
    if (!items.some((item) => item.mediaType === 'application/pdf')) return;
    let active = true;
    void desktop.pdfDocumentStatus(workspace, page).then((statuses) => {
      if (!active) return;
      setPdfStatusError(false);
      setPdfStatuses(Object.fromEntries(statuses.map((status) => [status.attachmentId, status])));
    }).catch(() => { if (active) setPdfStatusError(true); });
    return () => { active = false; };
  }, [items, page, workspace, pdfIndexRevision]);

  useEffect(() => {
    if (!item) return;
    let active = true;
    let url: string | undefined;
    if (item.bytes > previewLimit || (!isImageAttachment(item) && item.mediaType !== 'application/pdf')) {
      setPreview({ status: 'unavailable' });
      return;
    }
    setPreview({ status: 'loading' });
    void desktop.previewAttachment(workspace, page, item.id).then((bytes) => {
      if (!active) return;
      if (!(bytes instanceof ArrayBuffer)) throw new Error('无法读取附件预览数据。');
      if (item.mediaType === 'application/pdf') {
        setPreview({ status: 'pdf', data: bytes });
      } else {
        url = URL.createObjectURL(new Blob([bytes], { type: item.mediaType }));
        setPreview({ status: 'image', url });
      }
    }).catch((error: unknown) => { if (active) setPreview({ status: 'error', message: errorMessage(error) }); });
    return () => { active = false; if (url) URL.revokeObjectURL(url); };
  }, [item, page, workspace]);

  return <Modal title={item ? item.name : renaming ? '重命名附件' : '页面附件'} busy={busy} onClose={onClose} className={item ? 'attachment-preview-dialog' : 'attachments-dialog'}>
    {renaming ? <form className="dialog-form" onSubmit={(event) => {
      event.preventDefault();
      if (name.trim() && name.trim() !== renaming.name) void onRename(renaming, name).then((saved) => { if (saved) setRenaming(undefined); });
    }}>
      <label htmlFor="attachment-name">附件名称</label>
      <input id="attachment-name" autoFocus maxLength={255} disabled={busy} value={name} onChange={(event) => setName(event.currentTarget.value)} />
      <div className="dialog-actions">
        <button type="button" className="secondary-button" disabled={busy} onClick={() => setRenaming(undefined)}>取消</button>
        <button type="submit" className="primary-button" disabled={busy || !name.trim() || name.trim() === renaming.name}>保存名称</button>
      </div>
    </form> : item ? <>
      <div className="attachment-preview-area">
        {preview.status === 'loading' && <p className="dialog-empty" role="status">正在读取附件…</p>}
        {preview.status === 'unavailable' && <p className="dialog-empty">此文件可另存为原件。</p>}
        {preview.status === 'error' && <p className="dialog-empty" role="alert">{preview.message}</p>}
        {preview.status === 'image' && <img className="attachment-image" alt={item.name} src={preview.url}
          onError={() => setPreview({ status: 'error', message: '无法显示该图片，可以另存原件。' })} />}
        {preview.status === 'pdf' && <Suspense fallback={<p className="dialog-empty" role="status">正在加载 PDF…</p>}>
          <PdfPreview data={preview.data} initialPage={landingPage} />
        </Suspense>}
      </div>
      {item.mediaType === 'application/pdf' && !pdfStatusError &&
        <p className="attachment-notice" role="status">{pdfStatus(pdfStatuses[item.id])}</p>}
      <footer className="dialog-actions attachment-preview-actions">
        <button type="button" className="secondary-button" disabled={busy}
          onClick={() => { setItem(undefined); setLandingPage(undefined); }}>返回列表</button>
        {onInsertFile && <button type="button" className="secondary-button" disabled={busy} onClick={() => onInsertFile(item)}>
          <FilePlus2 size={16} />插入附件块</button>}
        {isImageAttachment(item) && <button type="button" className="secondary-button" disabled={busy} onClick={() => onInsertImage(item)}>
          <ImagePlus size={16} />插入正文</button>}
        <button type="button" className="primary-button" disabled={busy} onClick={() => onSave(item)}><ArrowDownToLine size={16} />另存原件</button>
      </footer>
    </> : <>
      <div className="attachment-toolbar">
        <button type="button" className="primary-button" disabled={busy} onClick={onAdd}><Plus size={16} />添加附件</button>
        <button type="button" className="icon-button" aria-label="刷新附件" disabled={busy} onClick={onReload}><RotateCw size={15} /></button>
        <span>{items.length} 个文件</span>
      </div>
      <div className="attachment-items" aria-label="附件列表">
        {!items.length && <p className="dialog-empty">这个页面还没有附件。</p>}
        {items.map((attachment) => {
          const Icon = attachment.mediaType.startsWith('image/') ? Image : attachment.mediaType === 'application/pdf' ? FileText : Paperclip;
          return <div className="attachment-row" key={attachment.id}>
            <button type="button" className="attachment-file" disabled={busy} onClick={() => {
              setPreview({ status: 'loading' }); setItem(attachment);
            }}>
              <span className="attachment-icon"><Icon size={17} /></span>
              <span><strong>{attachment.name}</strong><small>{sizeText(attachment.bytes)}
                {attachment.mediaType === 'application/pdf' && !pdfStatusError && ` · ${pdfStatus(pdfStatuses[attachment.id])}`}</small></span>
            </button>
            {onInsertFile && <button type="button" className="icon-button" aria-label={`插入附件块 ${attachment.name}`} title="插入附件块"
              disabled={busy} onClick={() => onInsertFile(attachment)}><FilePlus2 size={17} /></button>}
            <button type="button" className="icon-button" aria-label={`重命名 ${attachment.name}`} title="重命名"
              disabled={busy} onClick={() => { setName(attachment.name); setRenaming(attachment); }}><Pencil size={16} /></button>
            <button type="button" className="icon-button" aria-label={`另存 ${attachment.name}`} title="另存原件"
              disabled={busy} onClick={() => onSave(attachment)}><ArrowDownToLine size={17} /></button>
          </div>;
        })}
      </div>
    </>}
    {notice && <p className="attachment-notice" role="status">{notice}</p>}
    {pdfStatusError && <p className="dialog-error" role="alert">无法读取 PDF 索引状态。</p>}
    {error && <p className="dialog-error" role="alert">{error}</p>}
  </Modal>;
}
