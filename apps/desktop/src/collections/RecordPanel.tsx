// SPDX-License-Identifier: AGPL-3.0-or-later

import {
  ArrowUpRight, Calendar, CircleChevronDown, CircleDot, Clock, Hash, Link, List, Paperclip, Pencil,
  RefreshCw, Sigma, SquareCheck, SquareFunction, Type,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { useContext, useEffect, useRef, useState } from 'react';
import type { KeyboardEvent } from 'react';
import type { CollectionRecord, PropertyDefinition, PropertyKind, PropertyValue } from '../contracts';
import { errorMessage } from '../contracts';
import { desktop } from '../desktop';
import { FieldValue, RecordFieldValue, computedField, readInput, valueOf, writeInput } from './fields';
import { ExternalRecordUpdates } from './recordUpdates';

const kindIcons: Record<PropertyKind, LucideIcon> = {
  text: Type, number: Hash, checkbox: SquareCheck, date: Calendar, dateTime: Clock,
  status: CircleDot, select: CircleChevronDown, multiSelect: List, url: Link,
  attachment: Paperclip, relation: ArrowUpRight, rollup: Sigma, formula: SquareFunction,
  createdAt: Clock, updatedAt: Clock,
};

/** Property kinds edited in place, as in Notion. Others open the full form. */
export const inlineKinds = new Set<PropertyKind>(['text', 'number', 'url', 'date', 'checkbox', 'status', 'select']);

function sameValue(left: PropertyValue | undefined, right: PropertyValue | undefined) {
  return JSON.stringify(left ?? null) === JSON.stringify(right ?? null);
}

export function InlineValue({ field, record, busy, onSave, label }: {
  field: PropertyDefinition; record: CollectionRecord; busy: boolean; label?: string;
  onSave(field: PropertyDefinition, value: PropertyValue | undefined): void;
}) {
  const value = valueOf(record, field);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');
  const [invalid, setInvalid] = useState('');
  const done = useRef(false);

  if (field.type === 'checkbox') {
    return <input type="checkbox" className="record-checkbox" aria-label={label ?? field.name} checked={value === true}
      disabled={busy} onChange={(event) => onSave(field, event.currentTarget.checked)} />;
  }

  function start() {
    const raw = readInput(field, value);
    setDraft(Array.isArray(raw) ? '' : raw);
    setInvalid('');
    done.current = false;
    setEditing(true);
  }

  function commit(raw: string) {
    if (done.current) return;
    done.current = true;
    try {
      const next = writeInput(field, raw);
      setEditing(false);
      if (!sameValue(next, value)) onSave(field, next);
    } catch (error) {
      done.current = false;
      setInvalid(errorMessage(error));
    }
  }

  function keys(event: KeyboardEvent<HTMLInputElement | HTMLSelectElement>) {
    if (event.nativeEvent.isComposing) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      done.current = true;
      setEditing(false);
    } else if (event.key === 'Enter' && event.currentTarget instanceof HTMLInputElement) {
      event.preventDefault();
      commit(event.currentTarget.value);
    }
  }

  if (!editing) {
    return <button type="button" className="record-value" disabled={busy} aria-label={label ?? `修改「${field.name}」`}
      onClick={start}>
      {value === undefined ? <span className="record-value-empty">空</span> : <FieldValue field={field} value={value} />}
    </button>;
  }
  if (field.type === 'status' || field.type === 'select') {
    return <select className="record-input" autoFocus aria-label={field.name} value={draft}
      onChange={(event) => commit(event.currentTarget.value)}
      onBlur={() => { done.current = true; setEditing(false); }} onKeyDown={keys}>
      <option value="">空</option>
      {field.options.map((option) => <option key={option.id} value={option.id}>{option.name}</option>)}
    </select>;
  }
  const type = field.type === 'date' ? 'date' : field.type === 'number' ? 'number' : field.type === 'url' ? 'url' : 'text';
  return <span className="record-input-wrap">
    <input className="record-input" autoFocus aria-label={field.name} type={type} value={draft}
      step={type === 'number' ? 'any' : undefined} maxLength={type === 'text' ? 2000 : 2048}
      aria-invalid={Boolean(invalid)} onChange={(event) => setDraft(event.currentTarget.value)}
      onBlur={(event) => commit(event.currentTarget.value)} onKeyDown={keys} />
    {invalid && <span className="record-input-error" role="alert">{invalid}</span>}
  </span>;
}

export function RecordPanel({ workspace, collectionId, pageId, updatedAt, recordUpdate, busy, onEdit, onOpen, onSaveField }: {
  workspace: string; collectionId: string; pageId: string; updatedAt: number; recordUpdate: number;
  busy: boolean; onEdit(): void; onOpen(id: string): void;
  onSaveField(field: PropertyDefinition, value: PropertyValue | undefined): void;
}) {
  const [data, setData] = useState<{ record: CollectionRecord; fields: PropertyDefinition[] }>();
  const [error, setError] = useState('');
  const [attempt, setAttempt] = useState(0);
  const externalUpdate = useContext(ExternalRecordUpdates);
  useEffect(() => {
    let active = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function read(fields?: PropertyDefinition[]) {
      try {
        const [record, schemaFields] = await Promise.all([
          desktop.record(workspace, collectionId, pageId),
          fields ?? desktop.collection(workspace, collectionId).then((collection) => collection.properties),
        ]);
        if (!active) return;
        setData({ record, fields: schemaFields });
        setError('');
        if (record.computedPending) timer = setTimeout(() => { void read(schemaFields); }, 650);
      } catch (error) {
        if (active) setError(errorMessage(error));
      }
    }
    void read();
    return () => { active = false; clearTimeout(timer); };
  }, [workspace, collectionId, pageId, recordUpdate, attempt, externalUpdate]);

  const record = data && { ...data.record, updatedAt: Math.max(data.record.updatedAt, updatedAt) };
  return <section className="record-panel" aria-label="记录属性">
    <div className="record-panel-heading"><h2 className="visually-hidden">记录属性</h2>
      {data?.fields.some((field) => computedField(field.type)) &&
        <button type="button" className="icon-button" disabled={busy} aria-label="刷新计算结果" title="刷新计算结果"
          onClick={() => { setError(''); setAttempt((value) => value + 1); }}><RefreshCw size={14} /></button>}
      <button type="button" className="icon-button" disabled={busy} onClick={onEdit} aria-label="编辑属性"
        title="在表单中编辑全部属性"><Pencil size={14} /></button>
    </div>
    {error && <div className="save-error" role="alert">{error}
      <button type="button" className="text-button" onClick={() => { setError(''); setAttempt((value) => value + 1); }}>重试</button>
    </div>}
    {!data && !error && <p role="status">正在读取属性…</p>}
    {data && record && data.fields.length > 0 && <dl className="record-panel-values">{data.fields.map((field) => {
      const Icon = kindIcons[field.type];
      return <div key={field.id}>
        <dt><Icon size={15} aria-hidden /><span>{field.name}</span></dt>
        <dd>{inlineKinds.has(field.type)
          ? <InlineValue field={field} record={record} busy={busy} onSave={onSaveField} />
          : <span className="record-value-static">
            <RecordFieldValue field={field} record={record} busy={busy} onOpen={onOpen} />
          </span>}</dd>
      </div>;
    })}</dl>}
  </section>;
}
