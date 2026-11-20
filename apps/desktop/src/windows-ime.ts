// SPDX-License-Identifier: AGPL-3.0-or-later

const imeFocusMs = 300;

export function prepareWindowsIme(): Promise<void> {
  if (!navigator.userAgent.includes('Windows NT')) return Promise.resolve();

  return new Promise((resolve) => {
    let timer: number | undefined;
    let input: HTMLTextAreaElement | undefined;

    function finish() {
      window.clearTimeout(timer);
      window.removeEventListener('focus', prepare);
      window.removeEventListener('blur', finish);
      window.removeEventListener('pagehide', finish);
      input?.removeEventListener('blur', finish);
      input?.remove();
      resolve();
    }

    function prepare() {
      if (!document.hasFocus()) return;
      window.removeEventListener('focus', prepare);
      if (document.activeElement !== document.body) {
        finish();
        return;
      }

      // WebView2 can miss the first Microsoft Pinyin composition in a populated editor.
      // Briefly focus an empty input before making the workspace interactive.
      input = document.createElement('textarea');
      input.tabIndex = -1;
      input.setAttribute('aria-hidden', 'true');
      Object.assign(input.style, {
        position: 'fixed', top: '0', left: '0', width: '1px', height: '1px', opacity: '0', pointerEvents: 'none',
      });
      document.body.append(input);
      input.addEventListener('blur', finish);
      window.clearTimeout(timer);
      timer = window.setTimeout(finish, imeFocusMs);
      input.focus({ preventScroll: true });
      if (document.activeElement !== input) finish();
    }

    window.addEventListener('focus', prepare);
    window.addEventListener('blur', finish);
    window.addEventListener('pagehide', finish);
    timer = window.setTimeout(finish, imeFocusMs);
    prepare();
  });
}
