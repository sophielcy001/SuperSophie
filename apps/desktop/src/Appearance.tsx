// SPDX-License-Identifier: AGPL-3.0-or-later

import { Check } from 'lucide-react';
import { useLayoutEffect, useRef, useState } from 'react';
import { Modal } from './dialogs';

const preferenceKey = 'zhiye.theme';
export const themeOptions = [
  { value: 'system', name: '跟随系统' },
  { value: 'light', name: '使用浅色主题' },
  { value: 'dark', name: '使用深色主题' },
] as const;
export type Theme = typeof themeOptions[number]['value'];

export function useTheme() {
  const [theme, chooseTheme] = useState<Theme>(() => {
    try {
      const saved = localStorage.getItem(preferenceKey);
      if (saved === 'light' || saved === 'dark') return saved;
    } catch { /* Use the system appearance when WebView storage is unavailable. */ }
    return 'system';
  });
  useLayoutEffect(() => {
    const system = matchMedia('(prefers-color-scheme: dark)');
    const apply = () => {
      document.documentElement.dataset.theme = theme;
      document.documentElement.dataset.colorScheme = theme === 'system' ? (system.matches ? 'dark' : 'light') : theme;
    };
    apply();
    if (theme !== 'system') return;
    system.addEventListener('change', apply);
    return () => system.removeEventListener('change', apply);
  }, [theme]);

  function setTheme(value: Theme) {
    chooseTheme(value);
    try { localStorage.setItem(preferenceKey, value); } catch { /* Apply for the current session. */ }
  }
  return { theme, setTheme };
}

export function AppearanceDialog({ theme, onChoose, onClose }: {
  theme: Theme; onChoose(value: Theme): void; onClose(): void;
}) {
  const selected = useRef<HTMLButtonElement>(null);
  return <Modal title="外观" onClose={onClose} initialFocus={selected}>
    <div className="appearance-options" role="group" aria-label="主题">
      {themeOptions.map(({ value, name }) => <button key={value} type="button" className="secondary-button"
        ref={theme === value ? selected : undefined} aria-pressed={theme === value} onClick={() => onChoose(value)}>
        {name}{theme === value && <Check size={16} aria-hidden />}
      </button>)}
    </div>
  </Modal>;
}
