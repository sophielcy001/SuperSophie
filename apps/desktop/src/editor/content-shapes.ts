// SPDX-License-Identifier: AGPL-3.0-or-later

const utf8 = new TextEncoder();

export const calloutLabels = {
  note: '说明',
  tip: '提示',
  warning: '注意',
} as const;

export function calloutTone(value: unknown) {
  return value === 'note' || value === 'tip' || value === 'warning' ? value : undefined;
}

export function toggleTitle(value: unknown): string | undefined {
  return typeof value === 'string' && utf8.encode(value).length <= 512 && !/[\u0000-\u001f\u007f-\u009f]/u.test(value)
    ? value : undefined;
}

export function mathSource(value: unknown): string | undefined {
  return typeof value === 'string' && utf8.encode(value).length <= 4096 && !value.includes('\0') ? value : undefined;
}
