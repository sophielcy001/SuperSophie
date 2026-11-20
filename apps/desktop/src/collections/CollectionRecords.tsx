// SPDX-License-Identifier: AGPL-3.0-or-later

import { FileText, ImageIcon, Pencil } from 'lucide-react';
import type { CollectionRecord, PropertyDefinition, ViewKind } from '../contracts';
import { AttachmentFigure } from '../editor/AttachmentImage';
import { BatchCheckbox } from '../organize/BatchControls';
import type { BatchSelection } from '../organize/BatchControls';
import { RecordFieldValue, fieldText, valueOf } from './fields';

interface RecordActions {
  busy: boolean;
  batch?: BatchSelection;
  onOpen(id: string): void;
  onEdit?(id: string): void;
}

export function CollectionRecords({ workspace, view, grouping, fields, rows, ...actions }: RecordActions & {
  workspace: string; view: ViewKind; grouping?: PropertyDefinition; fields: PropertyDefinition[]; rows: CollectionRecord[];
}) {
  const groups = groupsFor(grouping, rows);
  if (view === 'board') {
    const cardFields = fields.filter((field) => field.id !== grouping?.id);
    return <div className="collection-board" aria-label="看板分组">
      {groups.map((group, index) => <section key={group.id} className="board-lane" aria-label={group.name}>
        <h3><span className="property-pill" data-color={group.id ? index % 6 : undefined}>{group.name}</span></h3>
        {group.rows.map((record) => <RecordCard key={record.id} record={record} fields={cardFields} {...actions} />)}
        {!group.rows.length && <p className="board-empty">本页无记录</p>}
      </section>)}
    </div>;
  }
  return <div className={grouping ? 'collection-groups' : undefined}>
    {groups.filter((group) => group.rows.length > 0).map((group) => <section key={group.id}>
      {grouping && <h3 className="collection-group-title">{group.name}</h3>}
      {view === 'table' ? <RecordTable fields={fields} records={group.rows} {...actions} /> :
        view === 'gallery' ? <ul className="collection-gallery" aria-label="画廊">{group.rows.map((record) => <li key={record.id}>
          <div className="gallery-cover">
            {record.cover ? <AttachmentFigure workspace={workspace} pageId={record.id} defaultAlt={record.cover.name} attachment={{
              attachmentId: record.cover.attachmentId, mediaType: record.cover.mediaType, alt: '',
            }} /> : <ImageIcon size={26} aria-label="无封面图片" />}
          </div>
          <RecordCard record={record} fields={fields} {...actions} />
        </li>)}</ul> :
        <ul className="collection-list">{group.rows.map((record) => <li key={record.id}><RecordCard record={record} fields={fields} {...actions} /></li>)}</ul>}
    </section>)}
  </div>;
}

function groupsFor(field: PropertyDefinition | undefined, rows: CollectionRecord[]) {
  if (!field) return [{ id: 'all', name: '全部记录', rows }];
  const options = field.type === 'checkbox' ? [{ id: 'true', name: '是' }, { id: 'false', name: '否' }] : field.options;
  const groups = new Map([...options, { id: '', name: '未填写' }].map((option) => [option.id, {
    ...option, rows: [] as CollectionRecord[],
  }]));
  for (const record of rows) groups.get(String(record.properties[field.id] ?? ''))?.rows.push(record);
  return [...groups.values()];
}

function RecordTable({ fields, records, ...actions }: RecordActions & { fields: PropertyDefinition[]; records: CollectionRecord[] }) {
  return <div className="collection-table-scroll" role="region" aria-label="记录表格" tabIndex={0}>
    <table className="collection-table">
      <thead><tr><th scope="col">标题</th>
        {fields.map((field) => <th key={field.id} scope="col" title={field.name}>{field.name}</th>)}
        {actions.onEdit && <th scope="col" className="record-action-cell">操作</th>}
      </tr></thead>
      <tbody>{records.map((record) => <tr key={record.id} className={actions.batch?.ids.has(record.id) ? 'batch-selected' : undefined}>
        <th scope="row"><div className="record-selection">
          {actions.batch && <BatchCheckbox batch={actions.batch} id={record.id} title={record.title} disabled={actions.busy} />}
          <RecordLink record={record} {...actions} />
        </div></th>
        {fields.map((field) => <td key={field.id} title={fieldText(field, valueOf(record, field))}>
          <RecordFieldValue field={field} record={record} busy={actions.busy} onOpen={actions.onOpen} />
        </td>)}
        {actions.onEdit && <td className="record-action-cell"><EditRecord record={record} {...actions} /></td>}
      </tr>)}</tbody>
    </table>
  </div>;
}

function RecordCard({ record, fields, ...actions }: RecordActions & { record: CollectionRecord; fields: PropertyDefinition[] }) {
  const preview = fields.filter((field) => valueOf(record, field) !== undefined || record.computed?.[field.id]?.status === 'error').slice(0, 3);
  return <div className={`collection-card${actions.batch?.ids.has(record.id) ? ' batch-selected' : ''}`}>
    <div className="collection-card-heading">
      {actions.batch && <BatchCheckbox batch={actions.batch} id={record.id} title={record.title} disabled={actions.busy} />}
      <RecordLink record={record} {...actions} /><EditRecord record={record} {...actions} />
    </div>
    {preview.length > 0 && <dl className="collection-card-values">{preview.map((field) => <div key={field.id}>
      <dt>{field.name}</dt><dd title={fieldText(field, valueOf(record, field))}>
        <RecordFieldValue field={field} record={record} busy={actions.busy} onOpen={actions.onOpen} />
      </dd>
    </div>)}</dl>}
  </div>;
}

export function RecordLink({ busy, record, onOpen }: Pick<RecordActions, 'busy' | 'onOpen'> & { record: CollectionRecord }) {
  return <button type="button" className="record-link" title={record.title} disabled={busy} onClick={() => onOpen(record.id)}>
    <FileText size={15} /><span>{record.title}</span></button>;
}

export function EditRecord({ busy, record, onEdit }: Pick<RecordActions, 'busy' | 'onEdit'> & { record: CollectionRecord }) {
  return onEdit && <button type="button" className="icon-button record-edit" title="编辑属性" aria-label={`编辑「${record.title}」的属性`}
    disabled={busy} onClick={() => onEdit(record.id)}><Pencil size={14} /></button>;
}
