// SPDX-License-Identifier: AGPL-3.0-or-later

import { Extension, Mark } from '@tiptap/core';
import { imageAttributes, webAddress } from './rich-content';

/** Notion's nine colors; each has a text and a background shade in theme.css. */
export const inlineColors = [
  { id: 'gray', label: '灰色' },
  { id: 'brown', label: '棕色' },
  { id: 'orange', label: '橙色' },
  { id: 'yellow', label: '黄色' },
  { id: 'green', label: '绿色' },
  { id: 'blue', label: '蓝色' },
  { id: 'purple', label: '紫色' },
  { id: 'pink', label: '粉色' },
  { id: 'red', label: '红色' },
] as const;

export type InlineColor = typeof inlineColors[number]['id'];

export function inlineColor(value: unknown): InlineColor | undefined {
  return inlineColors.find((color) => color.id === value)?.id;
}

/** A whole block's color, named as in Notion's API: "red" for text, "red_background" for background. */
export function blockColor(value: unknown): string | undefined {
  if (typeof value !== 'string') return;
  const [color, background, extra] = value.split('_');
  return inlineColor(color) && extra === undefined && (background === undefined || background === 'background') ? value : undefined;
}

export const BlockColor = Extension.create({
  name: 'blockColor',
  addGlobalAttributes() {
    return [{
      types: ['paragraph', 'heading'],
      attributes: {
        blockColor: {
          default: null,
          // A new line typed after a colored one starts uncolored, as in Notion.
          keepOnSplit: false,
          parseHTML: (element) => blockColor(element.getAttribute('data-block-color')) ?? null,
          renderHTML: (attributes) => blockColor(attributes.blockColor) ? { 'data-block-color': attributes.blockColor } : {},
        },
      },
    }];
  },
});

function colorMark(name: 'textColor' | 'backgroundColor', attribute: string) {
  return Mark.create({
    name,
    // Text and background colors combine; a new color of the same kind replaces the old one.
    excludes: name,
    addAttributes() {
      return { color: { default: 'gray', parseHTML: (element) => inlineColor(element.getAttribute(attribute)) ?? 'gray', rendered: false } };
    },
    parseHTML() {
      return [{ tag: `span[${attribute}]`, getAttrs: (element) => inlineColor(element.getAttribute(attribute)) ? null : false }];
    },
    renderHTML({ mark }) {
      return ['span', { [attribute]: inlineColor(mark.attrs.color) ?? 'gray' }, 0];
    },
  });
}

export const TextColor = colorMark('textColor', 'data-text-color');
export const BackgroundColor = colorMark('backgroundColor', 'data-background-color');

/** A personal comment on text: 1 to 2,000 characters. */
export function commentText(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() && [...value].length <= 2000 ? value : undefined;
}

/** Comments are marks on the commented text, shown highlighted with the comment on hover and click. */
export const Comment = Mark.create({
  name: 'comment',
  inclusive: false,
  addAttributes() {
    return { text: { default: '', rendered: false }, replies: { default: null, rendered: false } };
  },
  parseHTML() {
    return [{ tag: 'span[data-comment]', getAttrs: (element) => {
      const text = commentText(element.getAttribute('data-comment'));
      return text ? { text } : false;
    } }];
  },
  renderHTML({ mark }) {
    return ['span', { class: 'comment-mark', 'data-comment': mark.attrs.text, title: `评论：${mark.attrs.text}` }, 0];
  },
});

/** A comment's replies: 1 to 50 texts, or undefined when there are none. */
export function commentReplies(value: unknown): string[] | undefined {
  if (!Array.isArray(value) || !value.length || value.length > 50) return;
  return value.every((reply) => commentText(reply) !== undefined) ? value as string[] : undefined;
}

/** A bookmark card's data: a web address and an optional title the user typed (nothing is fetched). */
export type BookmarkData = { url: string; title?: string; description?: string; attachmentId?: string; mediaType?: string };

export function bookmarkAttributes(value: Record<string, unknown>): BookmarkData | undefined {
  const url = webAddress(value.url);
  const text = (raw: unknown, limit: number) => raw === undefined || raw === null || raw === '' ? ''
    // eslint-disable-next-line no-control-regex
    : typeof raw === 'string' && [...raw].length <= limit && !/[\u0000-\u001f\u007f]/.test(raw) ? raw : undefined;
  const title = text(value.title, 500);
  const description = text(value.description, 1000);
  if (!url || title === undefined || description === undefined) return;
  // The preview image is an attachment of the page; both fields or neither.
  const hasImage = (value.attachmentId ?? null) !== null || (value.mediaType ?? null) !== null;
  const image = hasImage ? imageAttributes({ attachmentId: value.attachmentId, mediaType: value.mediaType, alt: '' }) : undefined;
  if (hasImage && !image) return;
  return {
    url, ...(title ? { title } : {}), ...(description ? { description } : {}),
    ...(image ? { attachmentId: image.attachmentId, mediaType: image.mediaType } : {}),
  };
}
