// SPDX-License-Identifier: AGPL-3.0-or-later

import { Node } from '@tiptap/core';
import { NodeViewWrapper, ReactNodeViewRenderer } from '@tiptap/react';
import type { NodeViewProps } from '@tiptap/react';
import { useEffect, useState, useSyncExternalStore } from 'react';
import { desktop } from '../desktop';
import { bookmarkAttributes } from './marks';

interface BookmarkContext { workspace: string; pageId: string }

/** Bookmark cards whose page is being read, by block ID. */
const reading = new Set<string>();
const readers = new Set<() => void>();
export function markBookmarkReading(id: string, active: boolean) {
  if (active) reading.add(id);
  else reading.delete(id);
  for (const listener of readers) listener();
}
function useReading(id: unknown) {
  return useSyncExternalStore((listener) => { readers.add(listener); return () => { readers.delete(listener); }; },
    () => typeof id === 'string' && reading.has(id));
}

function host(url: string) {
  try { return new URL(url).host; } catch { return url; }
}

/** The preview image, read from the page's own attachment: opening the page stays offline. */
function BookmarkImage({ workspace, pageId, attachmentId, mediaType }: BookmarkContext & { attachmentId: string; mediaType: string }) {
  const [url, setUrl] = useState<string>();
  useEffect(() => {
    let active = true;
    let created: string | undefined;
    void desktop.previewAttachment(workspace, pageId, attachmentId).then((bytes) => {
      if (!active || !(bytes instanceof ArrayBuffer)) return;
      created = URL.createObjectURL(new Blob([bytes], { type: mediaType }));
      setUrl(created);
    }).catch(() => { /* the card still shows its text */ });
    return () => { active = false; if (created) URL.revokeObjectURL(created); };
  }, [workspace, pageId, attachmentId, mediaType]);
  return url ? <img className="bookmark-image" src={url} alt="" /> : <span className="bookmark-image" aria-hidden />;
}

function BookmarkView({ node, extension }: NodeViewProps) {
  const context = extension.options as BookmarkContext;
  const card = bookmarkAttributes(node.attrs);
  const url = card?.url ?? '';
  const pending = useReading(node.attrs.id);
  return <NodeViewWrapper as="a" className="bookmark-card" href={url} data-web-link="true" data-zhiye-bookmark=""
    contentEditable={false} title="按住 Ctrl 或 ⌘ 点击打开链接">
    <span className="bookmark-text">
      <span className="bookmark-title">{card?.title || host(url)}</span>
      {card?.description ? <span className="bookmark-description">{card.description}</span>
        : pending && <span className="bookmark-description" role="status">正在读取网页信息…</span>}
      <span className="bookmark-url">{url}</span>
    </span>
    {card?.attachmentId && card.mediaType && <BookmarkImage {...context} attachmentId={card.attachmentId} mediaType={card.mediaType} />}
  </NodeViewWrapper>;
}

/**
 * A bookmark card for a web page. Its title, description and preview image are read only
 * when the user asks; the image is kept as an attachment of the page.
 */
export const Bookmark = Node.create<BookmarkContext>({
  name: 'bookmark', group: 'block', atom: true, draggable: true,
  addOptions() { return { workspace: '', pageId: '' }; },
  addAttributes() {
    return {
      url: { default: '', rendered: false }, title: { default: null, rendered: false },
      description: { default: null, rendered: false }, attachmentId: { default: null, rendered: false },
      mediaType: { default: null, rendered: false },
    };
  },
  parseHTML() {
    return [{ tag: 'a[data-zhiye-bookmark]', getAttrs: (element) => bookmarkAttributes({
      url: element.getAttribute('href'), title: element.getAttribute('data-title') || undefined,
      description: element.getAttribute('data-description') || undefined,
    }) ?? false }];
  },
  renderHTML({ node, HTMLAttributes }) {
    const card = bookmarkAttributes(node.attrs);
    const url = card?.url ?? '';
    return ['a', {
      ...HTMLAttributes, 'data-zhiye-bookmark': '', 'data-web-link': 'true', 'data-title': card?.title ?? '',
      'data-description': card?.description ?? '', href: url, class: 'bookmark-card',
    }, ['span', { class: 'bookmark-title' }, card?.title || host(url)],
    ...(card?.description ? [['span', { class: 'bookmark-description' }, card.description]] : []),
    ['span', { class: 'bookmark-url' }, url]];
  },
  addNodeView() { return ReactNodeViewRenderer(BookmarkView); },
});
