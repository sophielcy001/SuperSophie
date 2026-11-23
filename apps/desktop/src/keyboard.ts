// SPDX-License-Identifier: AGPL-3.0-or-later

import type { FocusEvent, KeyboardEvent as ReactKeyboardEvent, MouseEvent } from 'react';

export function composing(event: KeyboardEvent) {
  return event.isComposing || event.keyCode === 229;
}

export function disclosureEvents(close: () => void) {
  const trigger = (anchor: HTMLElement) => anchor.querySelector<HTMLButtonElement>(':scope > button[aria-expanded="true"]');
  return {
    onKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
      if (event.key !== 'Escape' || composing(event.nativeEvent) || event.defaultPrevented) return;
      const button = trigger(event.currentTarget);
      if (!button) return;
      event.preventDefault();
      button.focus();
      close();
    },
    onBlur(event: FocusEvent<HTMLDivElement>) {
      if (event.relatedTarget instanceof Node && !event.currentTarget.contains(event.relatedTarget)) close();
    },
    onClickCapture(event: MouseEvent<HTMLDivElement>) {
      if (event.target instanceof Element && event.target.closest('.menu-item')) {
        trigger(event.currentTarget)?.focus({ preventScroll: true });
      }
    },
  };
}
