// SPDX-License-Identifier: AGPL-3.0-or-later

import type { JSONContent } from '@tiptap/core';
import type { Node as DocumentNode } from '@tiptap/pm/model';
import type { Attachment } from '../contracts';

export const maxImageBytes = 24 * 1024 * 1024;
export const imageMediaTypes = ['image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/bmp'];
const validId = /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/u;
const forbiddenInText = /[\u0000-\u001f\u007f-\u009f]/u;

export function inlineText(node: DocumentNode) {
  return node.textBetween(0, node.content.size, ' ', (leaf) =>
    leaf.type.name === 'pageReference' ? String(leaf.attrs.label ?? '') : ' ').trim();
}

export function webAddress(value: unknown) {
  if (typeof value !== 'string' || value.length > 4096 || /\s/u.test(value) || forbiddenInText.test(value)) return;
  try {
    const url = new URL(value);
    if (url.hostname && (url.protocol === 'http:' || url.protocol === 'https:')) return url.href;
  } catch {
    // Unrecognizable text remains ordinary text.
  }
}

export function isImageAttachment(attachment: Attachment) {
  return imageMediaTypes.includes(attachment.mediaType) && attachment.bytes <= maxImageBytes;
}

export function imageAttributes(values: Record<string, unknown>) {
  const { attachmentId, mediaType, alt } = values;
  if (typeof attachmentId !== 'string' || !validId.test(attachmentId) || typeof mediaType !== 'string'
    || !imageMediaTypes.includes(mediaType) || typeof alt !== 'string'
    || Array.from(alt).length > 500 || forbiddenInText.test(alt)) return;
  return { attachmentId, mediaType, alt };
}

export function imageNode(attachment: Attachment): JSONContent {
  const name = attachment.name.replace(/\.[^.]+$/u, '') || attachment.name;
  return {
    type: 'attachmentImage',
    attrs: { attachmentId: attachment.id, mediaType: attachment.mediaType, alt: name },
  };
}

export function tableAlignment(value: unknown): 'left' | 'center' | 'right' | undefined {
  if (value === 'left' || value === 'center' || value === 'right') return value;
}

export function spreadsheetTable(text: string): JSONContent | undefined {
  const rows = text.replace(/\r\n?/gu, '\n').replace(/\n$/u, '').split('\n').map((line) => line.split('\t'));
  const width = rows.reduce((max, row) => Math.max(max, row.length), 0);
  if (width < 2 || width * rows.length > 5000) return;
  return {
    type: 'table',
    content: rows.map((row) => ({
      type: 'tableRow',
      content: Array.from({ length: width }, (_, column) => {
        const text = row[column];
        return {
          type: 'tableCell',
          content: [{ type: 'paragraph', content: text ? [{ type: 'text', text }] : [] }],
        };
      }),
    })),
  };
}
