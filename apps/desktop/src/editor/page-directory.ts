// SPDX-License-Identifier: AGPL-3.0-or-later

import type { PageInfo } from '../contracts';

/**
 * Current page titles for page references. Like Notion mentions, a reference shows the
 * page's current title when its stored label is one of that page's titles (current, an
 * earlier title seen in this session, or the untitled placeholder). Links whose label is
 * custom text, such as imported Markdown links, keep their own text.
 */
interface Entry { title: string; trashed: boolean; titles: Set<string> }

const untitled = '无标题';
const entries = new Map<string, Entry>();
const listeners = new Set<() => void>();

export function setPageDirectory(pages: readonly PageInfo[]) {
  const seen = new Set<string>();
  for (const page of pages) {
    seen.add(page.id);
    const title = page.title || untitled;
    const entry = entries.get(page.id);
    if (entry) {
      entry.title = title;
      entry.trashed = Boolean(page.trashRootId);
      entry.titles.add(title);
    } else {
      entries.set(page.id, { title, trashed: Boolean(page.trashRootId), titles: new Set([title, untitled, '']) });
    }
  }
  for (const id of entries.keys()) if (!seen.has(id)) entries.delete(id);
  for (const listener of listeners) listener();
}

/** The text a page reference shows: the current title, or its own label when that is custom text. */
export function referenceLabel(pageId: string, label: string): string {
  const entry = entries.get(pageId);
  return entry && entry.titles.has(label) ? entry.title : label;
}

export function referenceTrashed(pageId: string) {
  return entries.get(pageId)?.trashed ?? false;
}

export function onPageDirectory(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

/** A page mentioned inside a comment: `@[标题](page:<id>)`, shown as a link with the page's current title. */
const mentionToken = /@\[([^\]\n]{0,200})\]\(page:([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\)/g;

export function commentMention(page: PageInfo) {
  return `@[${(page.title || untitled).replace(/[\]\n]/g, ' ')}](page:${page.id})`;
}

export type CommentPart = { text: string } | { pageId: string; label: string; trashed: boolean };

/** Splits comment text into plain text and page mentions. */
export function commentParts(text: string): CommentPart[] {
  const parts: CommentPart[] = [];
  let last = 0;
  for (const match of text.matchAll(mentionToken)) {
    if (match.index > last) parts.push({ text: text.slice(last, match.index) });
    parts.push({ pageId: match[2], label: referenceLabel(match[2], match[1]) || untitled, trashed: referenceTrashed(match[2]) });
    last = match.index + match[0].length;
  }
  if (last < text.length) parts.push({ text: text.slice(last) });
  return parts;
}
