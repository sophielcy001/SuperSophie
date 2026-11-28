// SPDX-License-Identifier: AGPL-3.0-or-later

import { ArrowUpRight, FileText, ImageIcon, Pencil, Plus } from 'lucide-react';
import { createContext, useContext, useRef, useState } from 'react';
import type { KeyboardEvent, PointerEvent as ReactPointerEvent } from 'react';
import type { CollectionRecord, PropertyDefinition, PropertyValue, ViewKind } from '../contracts';
import { AttachmentFigure } from '../editor/AttachmentImage';
import { BatchCheckbox } from '../organize/BatchControls';
import type { BatchSelection } from '../organize/BatchControls';
import { RecordFieldValue, fieldText, valueOf } from './fields';
import { InlineValue, inlineKinds } from './RecordPanel';
import { holdNativeDrag } from './ScheduleViews';

/**
 * Typing a record's title in place, as in Notion: a new record waits for its title where it appears,
 * and a table's title cell is edited with a click.
 */
export interface TitleEditing {
  id?: string;
  start(id: string): void;
  finish(): void;
  rename(record: CollectionRecord, title: string): void;
}
export const RecordTitleEditing = createContext<TitleEditing | undefined>(undefined);

/** Saves one property typed in a table cell. */
export type SaveField = (record: CollectionRecord, field: PropertyDefinition, value: PropertyValue | undefined) => void;

interface RecordActions {
  busy: boolean;
  batch?: BatchSelection;
  onOpen(id: string): void;
  onEdit?(id: string): void;
  onSaveField?: SaveField;
}

export function CollectionRecords({ workspace, view, grouping, fields, rows, onCreate, ...actions }: RecordActions & {
  workspace: string; view: ViewKind; grouping?: PropertyDefinition; fields: PropertyDefinition[]; rows: CollectionRecord[];
  /** Adds a record; on a board the column's value is passed as `preset`. */
  onCreate?(preset?: { field: string; value: PropertyValue }): void;
}) {
  const groups = groupsFor(grouping, rows);
  const lanes = useLaneDrag(grouping, actions);
  if (view === 'board') {
    const cardFields = fields.filter((field) => field.id !== grouping?.id);
    return <div className="collection-board" aria-label="看板分组">
      {groups.map((group, index) => <section key={group.id} className="board-lane" aria-label={group.name}
        data-lane={group.id} data-drop-active={lanes.over === group.id || undefined}>
        <h3><span className="property-pill" data-color={group.id ? index % 6 : undefined}>{group.name}</span>
          <span className="board-count" aria-label={`${group.rows.length} 条`}>{group.rows.length}</span></h3>
        {group.rows.map((record) => <RecordCard key={record.id} record={record} fields={cardFields} {...actions}
          dragging={lanes.dragging === record.id} onPointerDown={lanes.begin && ((event) => lanes.begin?.(event, record, group.id))} />)}
        {onCreate && <button type="button" className="board-add" disabled={actions.busy}
          aria-label={`在「${group.name}」中新建记录`}
          onClick={() => onCreate(grouping && group.id ? { field: grouping.id, value: groupValue(grouping, group.id) } : undefined)}>
          <Plus size={15} aria-hidden />新建</button>}
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
        </li>)}
          {onCreate && <li className="gallery-add"><button type="button" disabled={actions.busy} onClick={() => onCreate()}>
            <Plus size={16} aria-hidden />新建</button></li>}
        </ul> :
        <ul className="collection-list">{group.rows.map((record) => <li key={record.id}><RecordCard record={record} fields={fields} {...actions} /></li>)}</ul>}
    </section>)}
  </div>;
}

/** Board column ids are option ids, or `true`/`false` for a checkbox grouping. */
function groupValue(field: PropertyDefinition, id: string): PropertyValue {
  return field.type === 'checkbox' ? id === 'true' : id;
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
        {actions.onEdit && <th scope="col" className="record-action-cell"><span className="visually-hidden">操作</span></th>}
      </tr></thead>
      <tbody>{records.map((record) => <tr key={record.id} className={actions.batch?.ids.has(record.id) ? 'batch-selected' : undefined}>
        <th scope="row"><div className="record-selection">
          {actions.batch && <BatchCheckbox batch={actions.batch} id={record.id} title={record.title} disabled={actions.busy} />}
          <TitleCell record={record} {...actions} />
        </div></th>
        {fields.map((field) => {
          const save = actions.onSaveField;
          const inline = save && !actions.batch && inlineKinds.has(field.type);
          return <td key={field.id} className={inline ? 'record-cell-editable' : undefined} title={fieldText(field, valueOf(record, field))}>
            {inline ? <InlineValue field={field} record={record} busy={actions.busy} label={`修改「${record.title}」的${field.name}`}
              onSave={(changed, value) => save(record, changed, value)} />
              : <RecordFieldValue field={field} record={record} busy={actions.busy} onOpen={actions.onOpen} />}
          </td>;
        })}
        {actions.onEdit && <td className="record-action-cell"><EditRecord record={record} {...actions} /></td>}
      </tr>)}</tbody>
    </table>
  </div>;
}

function RecordCard({ record, fields, dragging, onPointerDown, ...actions }: RecordActions & {
  record: CollectionRecord; fields: PropertyDefinition[]; dragging?: boolean; onPointerDown?(event: ReactPointerEvent): void;
}) {
  const preview = fields.filter((field) => valueOf(record, field) !== undefined || record.computed?.[field.id]?.status === 'error').slice(0, 3);
  return <div className={`collection-card${actions.batch?.ids.has(record.id) ? ' batch-selected' : ''}`}
    data-dragging={dragging || undefined} data-movable={onPointerDown ? true : undefined} onPointerDown={onPointerDown}
    onMouseDown={onPointerDown && holdNativeDrag} onDragStart={onPointerDown && holdNativeDrag}>
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
  const editing = useContext(RecordTitleEditing);
  if (editing?.id === record.id) return <TitleInput record={record} editing={editing} />;
  return <button type="button" className="record-link" title={record.title} disabled={busy} onClick={() => onOpen(record.id)}>
    <FileText size={15} /><span>{record.title}</span></button>;
}

export function EditRecord({ busy, record, onEdit }: Pick<RecordActions, 'busy' | 'onEdit'> & { record: CollectionRecord }) {
  return onEdit && <button type="button" className="icon-button record-edit" title="编辑属性" aria-label={`编辑「${record.title}」的属性`}
    disabled={busy} onClick={() => onEdit(record.id)}><Pencil size={14} /></button>;
}

const untitled = '无标题';

function TitleInput({ record, editing }: { record: CollectionRecord; editing: TitleEditing }) {
  const done = useRef(false);
  const commit = (value: string) => {
    if (done.current) return;
    done.current = true;
    editing.finish();
    const title = value.trim();
    if (title && title !== record.title) editing.rename(record, title);
  };
  const keys = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing) return;
    event.stopPropagation();
    if (event.key === 'Enter') {
      event.preventDefault();
      commit(event.currentTarget.value);
    } else if (event.key === 'Escape') {
      event.preventDefault();
      done.current = true;
      editing.finish();
    }
  };
  return <input className="record-title-input" autoFocus aria-label="记录标题" placeholder={untitled} maxLength={200}
    defaultValue={record.title === untitled ? '' : record.title} autoCapitalize="off" autoCorrect="off" spellCheck={false}
    onPointerDown={(event) => event.stopPropagation()} onKeyDown={keys} onBlur={(event) => commit(event.currentTarget.value)} />;
}

/** A table's title cell: a click edits the title and the button beside it opens the page, as in Notion. */
function TitleCell({ record, ...actions }: RecordActions & { record: CollectionRecord }) {
  const editing = useContext(RecordTitleEditing);
  if (!editing || actions.batch) return <RecordLink record={record} {...actions} />;
  if (editing.id === record.id) return <TitleInput record={record} editing={editing} />;
  return <span className="record-title-cell">
    <button type="button" className="record-title-text" title="编辑标题" aria-label={`编辑「${record.title}」的标题`}
      disabled={actions.busy} onClick={() => editing.start(record.id)}><FileText size={15} aria-hidden /><span>{record.title}</span></button>
    <button type="button" className="record-open" title="打开页面" aria-label={`打开「${record.title}」`} disabled={actions.busy}
      onClick={() => actions.onOpen(record.id)}><ArrowUpRight size={13} aria-hidden />打开</button>
  </span>;
}

/** Dragging a board card into another column sets the grouping property, as in Notion. */
function useLaneDrag(grouping: PropertyDefinition | undefined, actions: RecordActions) {
  const [state, setState] = useState<{ id: string; over?: string }>();
  const save = actions.onSaveField;
  if (!grouping || !save || actions.batch || actions.busy) return { dragging: state?.id, over: state?.over };
  const begin = (event: ReactPointerEvent, record: CollectionRecord, lane: string) => {
    if (event.button !== 0 || (event.target as Element).closest('input, select, textarea')) return;
    const pointer = event.pointerId;
    const doc = event.currentTarget.ownerDocument;
    const origin = { x: event.clientX, y: event.clientY };
    let started = false;
    let over = lane;
    doc.getSelection()?.removeAllRanges();
    const board = event.currentTarget.closest('.collection-board');
    // Like Notion, a card dropped anywhere within a column's width goes to that column.
    const laneAt = (x: number, y: number) => {
      for (const element of doc.elementsFromPoint(x, y)) {
        const found = element.closest('[data-lane]')?.getAttribute('data-lane');
        if (found !== null && found !== undefined) return found;
      }
      const area = board?.getBoundingClientRect();
      if (!board || !area || y < area.top - 40 || y > area.bottom + 400) return undefined;
      for (const column of board.querySelectorAll<HTMLElement>('[data-lane]')) {
        const box = column.getBoundingClientRect();
        if (x >= box.left && x <= box.right) return column.dataset.lane;
      }
      return undefined;
    };
    const follow = (moved: PointerEvent) => {
      if (moved.pointerId !== pointer) return;
      if (!started && Math.hypot(moved.clientX - origin.x, moved.clientY - origin.y) < 4) return;
      started = true;
      moved.preventDefault();
      over = laneAt(moved.clientX, moved.clientY) ?? over;
      setState({ id: record.id, over });
    };
    const finish = (released: PointerEvent) => {
      if (released.pointerId !== pointer) return;
      doc.removeEventListener('pointermove', follow, true);
      doc.removeEventListener('pointerup', finish, true);
      doc.removeEventListener('pointercancel', finish, true);
      setState(undefined);
      if (!started || released.type === 'pointercancel') return;
      const swallow = (click: MouseEvent) => { click.stopPropagation(); click.preventDefault(); };
      doc.addEventListener('click', swallow, { capture: true, once: true });
      setTimeout(() => doc.removeEventListener('click', swallow, { capture: true }), 0);
      const target = laneAt(released.clientX, released.clientY) ?? over;
      if (target !== lane) save(record, grouping, target ? groupValue(grouping, target) : undefined);
    };
    doc.addEventListener('pointermove', follow, true);
    doc.addEventListener('pointerup', finish, true);
    doc.addEventListener('pointercancel', finish, true);
  };
  return { dragging: state?.id, over: state?.over, begin };
}
