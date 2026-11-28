// SPDX-License-Identifier: AGPL-3.0-or-later

import { createRoot } from 'react-dom/client';
import { App } from './App';
import './theme.css';
import './styles.css';
import './collections/collections.css';

// Writing surfaces show a focus ring only when reached by keyboard navigation;
// while typing or after a click the caret marks the focus, as in Notion.
const root = document.documentElement;
const navigationKeys = new Set(['Tab', 'F6']);
document.addEventListener('keydown', (event) => {
  if (navigationKeys.has(event.key) || (event.altKey && event.key === 'F10')) root.dataset.focusSource = 'keyboard';
  else if (!['Shift', 'Alt', 'Control', 'Meta', 'CapsLock'].includes(event.key)) delete root.dataset.focusSource;
}, true);
document.addEventListener('pointerdown', () => { delete root.dataset.focusSource; }, true);

createRoot(document.getElementById('root')!).render(<App />);
