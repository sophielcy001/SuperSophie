// SPDX-License-Identifier: AGPL-3.0-or-later

import { mergeAttributes, Node } from '@tiptap/core';
import { NodeViewWrapper, ReactNodeViewRenderer } from '@tiptap/react';
import type { NodeViewProps } from '@tiptap/react';
import { Image, ImageOff, RotateCw } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { errorMessage } from '../contracts';
import { desktop } from '../desktop';
import { imageAttributes } from './rich-content';

interface ImageContext { workspace: string; pageId: string }

function ImageView({ node, extension, selected }: NodeViewProps) {
  const { workspace, pageId } = extension.options as ImageContext;
  return <NodeViewWrapper className="attachment-image-block" contentEditable={false}>
    <AttachmentFigure workspace={workspace} pageId={pageId} attachment={imageAttributes(node.attrs)} selected={selected} draggable />
  </NodeViewWrapper>;
}

export function AttachmentFigure({ workspace, pageId, attachment, selected, draggable, defaultAlt = '正文图片' }: ImageContext & {
  attachment: ReturnType<typeof imageAttributes>;
  selected?: boolean;
  draggable?: boolean;
  defaultAlt?: string;
}) {
  const attachmentId = attachment?.attachmentId;
  const mediaType = attachment?.mediaType;
  const alt = attachment?.alt;
  const figure = useRef<HTMLElement>(null);
  const [visible, setVisible] = useState(false);
  const [retry, setRetry] = useState(0);
  const [preview, setPreview] = useState<{ url?: string; error?: string }>({});
  const shouldLoad = visible || selected;

  useEffect(() => {
    if (!figure.current || visible) return;
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) setVisible(true);
    });
    observer.observe(figure.current);
    return () => observer.disconnect();
  }, [visible]);

  useEffect(() => {
    if (!shouldLoad || !attachmentId || !mediaType) return;
    let active = true;
    let url: string | undefined;
    setPreview({});
    void desktop.previewAttachment(workspace, pageId, attachmentId).then((bytes) => {
      if (!active) return;
      if (!(bytes instanceof ArrayBuffer)) throw new Error('无法读取图片预览数据。');
      url = URL.createObjectURL(new Blob([bytes], { type: mediaType }));
      setPreview({ url });
    }).catch((error: unknown) => {
      if (active) setPreview({ error: errorMessage(error) });
    });
    return () => { active = false; if (url) URL.revokeObjectURL(url); };
  }, [shouldLoad, attachmentId, mediaType, pageId, retry, workspace]);

  const error = preview.error ?? (!attachment ? '图片信息无效。' : undefined);
  return <figure ref={figure} data-drag-handle={draggable ? '' : undefined} className={selected ? 'image-selected' : undefined}>
      {preview.url && !error ? <img src={preview.url} alt={alt || defaultAlt} draggable={false}
        decoding="async" onError={() => setPreview({ error: '图片无法显示。可在附件中另存原件。' })} /> :
        <div className="inline-image-placeholder" role={error ? 'status' : undefined}>
          {error ? <ImageOff size={21} aria-hidden /> : <Image size={21} aria-hidden />}
          <span>{error ?? (visible ? '正在读取图片…' : '图片')}</span>
          {error && attachment && <button type="button" className="icon-button" aria-label="重试读取图片"
            onClick={() => setRetry((count) => count + 1)}><RotateCw size={15} /></button>}
        </div>}
      {alt && <figcaption>{alt}</figcaption>}
    </figure>;
}

export const AttachmentImage = Node.create<ImageContext>({
  name: 'attachmentImage',
  group: 'block',
  atom: true,
  draggable: true,
  addOptions() {
    return { workspace: '', pageId: '' };
  },
  addAttributes() {
    return {
      attachmentId: { default: null, rendered: false },
      mediaType: { default: null, rendered: false },
      alt: { default: '', rendered: false },
    };
  },
  parseHTML() {
    return [{
      tag: 'figure[data-zhiye-image]',
      getAttrs: (figure) => {
        if (figure.getAttribute('data-workspace') !== this.options.workspace
          || figure.getAttribute('data-page') !== this.options.pageId) return false;
        return imageAttributes({
          attachmentId: figure.getAttribute('data-zhiye-image'),
          mediaType: figure.getAttribute('data-media-type'),
          alt: figure.getAttribute('data-alt'),
        }) ?? false;
      },
    }];
  },
  renderHTML({ node, HTMLAttributes }) {
    return ['figure', mergeAttributes(HTMLAttributes, {
      'data-zhiye-image': node.attrs.attachmentId,
      'data-media-type': node.attrs.mediaType,
      'data-alt': node.attrs.alt,
      'data-workspace': this.options.workspace,
      'data-page': this.options.pageId,
    }), ['figcaption', node.attrs.alt || '正文图片']];
  },
  addNodeView() {
    return ReactNodeViewRenderer(ImageView, {
      attrs: ({ node }) => ({ 'data-id': node.attrs.id ?? '' }),
    });
  },
});
