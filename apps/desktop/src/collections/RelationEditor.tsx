// SPDX-License-Identifier: AGPL-3.0-or-later

import { ChevronLeft, ChevronRight, Search, X } from 'lucide-react';
import { useEffect, useId, useState } from 'react';
import type { PropertyDefinition, RelatedRecord, RelationTargetPage } from '../contracts';
import { errorMessage } from '../contracts';
import { desktop } from '../desktop';

export function RelationEditor({ workspace, field, selected, restricted, onChange }: {
  workspace: string; field: PropertyDefinition; selected: RelatedRecord[];
  restricted: boolean; onChange(items: RelatedRecord[]): void;
}) {
  const inputId = useId();
  const collection = field.relation?.collectionId;
  const [opened, setOpened] = useState(false);
  const [query, setQuery] = useState('');
  const [offset, setOffset] = useState(0);
  const [response, setResponse] = useState<{ key: string; data?: RelationTargetPage; error?: string }>();
  const term = query.trim();
  const key = JSON.stringify([collection, term, offset]);
  const current = response?.key === key ? response : undefined;

  useEffect(() => {
    if (!opened || !collection) return;
    let active = true;
    const timer = setTimeout(() => {
      void desktop.relationTargets(workspace, collection, term, offset).then((data) => {
        if (active) setResponse({ key, data });
      }).catch((error: unknown) => {
        if (active) setResponse({ key, error: errorMessage(error) });
      });
    }, 120);
    return () => { active = false; clearTimeout(timer); };
  }, [opened, workspace, collection, term, offset, key]);

  return <fieldset className="field-choices relation-editor">
    <legend>{field.name}</legend>
    {selected.length > 0 && <div className="relation-items">
      {selected.map((item) => <button type="button" className="relation-record" key={item.id}
        aria-label={`移除关联「${item.title}」`} onClick={() => onChange(selected.filter((other) => other.id !== item.id))}>
        {item.title}<X size={14} aria-hidden />
      </button>)}
    </div>}
    {restricted && <p className="field-empty">部分关联不可用</p>}
    <details onToggle={(event) => setOpened(event.currentTarget.open)}>
      <summary>添加关联</summary>
      {opened && <div className="relation-picker">
        <label className="visually-hidden" htmlFor={inputId}>查找「{field.name}」的关联记录</label>
        <div className="picker-input"><Search size={15} aria-hidden />
          <input id={inputId} placeholder="按记录标题查找…" value={query} maxLength={200}
            onChange={(event) => { setQuery(event.currentTarget.value); setOffset(0); }} />
        </div>
        {!current && <p className="field-empty" role="status">正在查找记录…</p>}
        {current?.error && <p className="collection-form-error" role="alert">{current.error}</p>}
        {current?.data?.items.length === 0 && <p className="field-empty">没有匹配的记录。</p>}
        <div className="relation-picker-options">
          {current?.data?.items.map((item) => <label key={item.id}>
            <input type="checkbox" checked={selected.some((other) => other.id === item.id)}
              onChange={(event) => onChange(event.currentTarget.checked
                ? [...selected, item] : selected.filter((other) => other.id !== item.id))} />
            <span>{item.title}</span>
          </label>)}
        </div>
        {(offset > 0 || current?.data?.nextOffset != null) && <div className="picker-pages">
          <button type="button" className="icon-button" aria-label="上一页关联记录" disabled={offset === 0}
            onClick={() => setOffset(Math.max(0, offset - 25))}><ChevronLeft size={16} /></button>
          <button type="button" className="icon-button" aria-label="下一页关联记录" disabled={current?.data?.nextOffset == null}
            onClick={() => { if (current?.data?.nextOffset != null) setOffset(current.data.nextOffset); }}><ChevronRight size={16} /></button>
        </div>}
      </div>}
    </details>
  </fieldset>;
}
