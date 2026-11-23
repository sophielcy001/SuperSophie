// SPDX-License-Identifier: AGPL-3.0-or-later

import { CornerDownLeft, Search } from 'lucide-react';
import { useEffect, useId, useRef, useState } from 'react';
import { Modal } from './dialogs';
import { composing } from './keyboard';

export interface Command {
  id: string;
  name: string;
  run(): void;
}

export function CommandPalette({ commands, onChoose, onClose }: {
  commands: Command[]; onChoose(command: Command): void; onClose(): void;
}) {
  const id = useId();
  const input = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState('');
  const [index, setIndex] = useState(0);
  const terms = query.trim().toLowerCase().split(/\s+/);
  const matches = commands.filter(({ name }) => terms.every((term) => name.toLowerCase().includes(term)));
  const selected = Math.min(index, Math.max(matches.length - 1, 0));
  const active = matches[selected];
  const activeId = active ? `${id}-${active.id}` : undefined;

  useEffect(() => {
    if (activeId) document.getElementById(activeId)?.scrollIntoView({ block: 'nearest' });
  }, [activeId]);

  return <Modal title="命令面板" className="picker-dialog" initialFocus={input} onClose={onClose}>
    <div className="picker-input"><Search size={18} aria-hidden /><input ref={input} type="text" role="combobox"
      aria-label="筛选命令" aria-controls={`${id}-commands`} aria-expanded={matches.length > 0}
      aria-autocomplete="list" aria-activedescendant={activeId} placeholder="输入命令名称…" value={query}
      onChange={(event) => { setQuery(event.currentTarget.value); setIndex(0); }}
      onKeyDown={(event) => {
        if (composing(event.nativeEvent) || !active) return;
        if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
          event.preventDefault();
          const step = event.key === 'ArrowDown' ? 1 : -1;
          setIndex(Math.max(0, Math.min(selected + step, matches.length - 1)));
        } else if (event.key === 'Enter') {
          event.preventDefault();
          onChoose(active);
        }
      }} /></div>
    <div id={`${id}-commands`} className="picker-results command-results" role="listbox" aria-label="可执行命令" hidden={!matches.length}>
      {matches.map((command, item) => <div id={`${id}-${command.id}`} key={command.id} role="option" aria-selected={item === selected}
        className={`picker-row command-row${item === selected ? ' highlighted' : ''}`}
        onMouseDown={(event) => event.preventDefault()} onMouseMove={() => setIndex(item)} onClick={() => onChoose(command)}>
        <strong>{command.name}</strong>
        {item === selected && <CornerDownLeft size={15} className="picker-enter" aria-hidden />}
      </div>)}
    </div>
    {!matches.length && <p className="dialog-empty" role="status">没有找到匹配的命令。</p>}
  </Modal>;
}
