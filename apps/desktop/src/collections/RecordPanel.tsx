// SPDX-License-Identifier: AGPL-3.0-or-later

import { Pencil, RefreshCw } from 'lucide-react';
import { useContext, useEffect, useState } from 'react';
import type { CollectionRecord, PropertyDefinition } from '../contracts';
import { errorMessage } from '../contracts';
import { desktop } from '../desktop';
import { RecordFieldValue, computedField } from './fields';
import { ExternalRecordUpdates } from './recordUpdates';

export function RecordPanel({ workspace, collectionId, pageId, updatedAt, recordUpdate, busy, onEdit, onOpen }: {
  workspace: string; collectionId: string; pageId: string; updatedAt: number; recordUpdate: number;
  busy: boolean; onEdit(): void; onOpen(id: string): void;
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
    <div className="record-panel-heading"><h2>记录属性</h2>
      {data?.fields.some((field) => computedField(field.type)) &&
        <button type="button" className="icon-button" disabled={busy} aria-label="刷新计算结果" title="刷新计算结果"
          onClick={() => { setError(''); setAttempt((value) => value + 1); }}><RefreshCw size={14} /></button>}
      <button type="button" className="secondary-button" disabled={busy} onClick={onEdit}><Pencil size={14} />编辑属性</button>
    </div>
    {error && <div className="save-error" role="alert">{error}
      <button type="button" className="text-button" onClick={() => { setError(''); setAttempt((value) => value + 1); }}>重试</button>
    </div>}
    {!data && !error && <p role="status">正在读取属性…</p>}
    {data && record && data.fields.length > 0 && <dl className="record-panel-values">{data.fields.map((field) => <div key={field.id}>
      <dt>{field.name}</dt><dd><RecordFieldValue field={field} record={record} busy={busy} onOpen={onOpen} /></dd>
    </div>)}</dl>}
  </section>;
}
