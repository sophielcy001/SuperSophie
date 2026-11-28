// SPDX-License-Identifier: AGPL-3.0-or-later

import { ArrowUpRight, CalendarDays, ChevronLeft, ChevronRight, Columns3, Download, GalleryHorizontalEnd, GanttChart, List, ListChecks, Plus, SlidersHorizontal, Table2, Wrench } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import type {
  Collection, CollectionRecord, CollectionView, ComputedDraft, PropertyDefinition, PropertyDraft, PropertyValue, RelatedRecord, RelationDraft,
  ViewDraft, ViewSettings,
} from '../contracts';
import { errorMessage } from '../contracts';
import { desktop } from '../desktop';
import { BatchCheckbox } from '../organize/BatchControls';
import type { BatchSelection } from '../organize/BatchControls';
import { SchemaDialog, ViewDialog } from './dialogs';
import { fieldText, operatorNames, titleField } from './fields';
import { CollectionRecords, EditRecord, RecordLink, RecordTitleEditing } from './CollectionRecords';
import type { TitleEditing } from './CollectionRecords';
import { ScheduleRecords, ScheduleToolbar } from './ScheduleViews';
import { useSchedule, viewLabels } from './viewDates';
import { useViewRecords } from './useViewRecords';

const viewIcons = {
  table: Table2, board: Columns3, list: List, calendar: CalendarDays, gallery: GalleryHorizontalEnd, timeline: GanttChart,
};
const noFields: PropertyDefinition[] = [];

type Operation = (job: () => Promise<void>) => void;

type Preset = { field: string; value: PropertyValue };

interface Props {
  workspace: string;
  collectionId: string;
  relationTargets: RelatedRecord[];
  selectedView?: string;
  recordUpdate: number;
  busy: boolean;
  batch?: BatchSelection;
  /** Shown above the views when the database sits inside another page, as Notion's inline databases do. */
  inline?: { title: string; onOpenDatabase(): void; onRename(title: string): void };
  onSwitch(view: string): void;
  /** Creates a record and opens its page. `preset` fills one property, e.g. the board column it was added in. */
  onCreate(preset?: Preset): void;
  /** Creates a record without leaving the view and resolves to its id, so its title is typed in place. */
  onAdd(preset?: Preset): Promise<string | undefined>;
  onRename(id: string, title: string): void;
  onSaveField(id: string, field: PropertyDefinition, value: PropertyValue | undefined): void;
  onOpen(id: string): void;
  onEdit(id: string): void;
  onBeginBatch?(): void;
  onExportCsv?(view: CollectionView): void;
  onRecordMoved(record: Awaited<ReturnType<typeof desktop.moveScheduledRecord>>): void;
  onRecordsConverted(): Promise<void>;
  mutate: Operation;
}

export function CollectionPane({ workspace, collectionId, relationTargets, selectedView, recordUpdate, busy, batch, inline, onSwitch, onCreate, onAdd,
  onRename, onSaveField, onOpen, onEdit, onBeginBatch, onExportCsv, onRecordMoved, onRecordsConverted, mutate }: Props) {
  const [loaded, setLoaded] = useState<{ collection?: Collection; error?: string }>({});
  const [offset, setOffset] = useState(0);
  const [attempt, setAttempt] = useState(0);
  const [dialog, setDialog] = useState<'schema' | 'view' | 'newView' | 'dateProperty'>();
  const [dialogError, setDialogError] = useState('');
  const [moveError, setMoveError] = useState('');
  const [titleId, setTitleId] = useState<string>();
  const titles = useMemo<TitleEditing>(() => ({
    id: titleId, start: setTitleId, finish: () => setTitleId(undefined),
    rename: (record, title) => onRename(record.id, title),
  }), [titleId, onRename]);
  const collection = loaded.collection;
  const view = collection?.views.find((item) => item.id === selectedView) ?? collection?.views[0];
  const schedule = useSchedule(view, collection?.properties ?? noFields);
  const current = useViewRecords(workspace, collection, view, offset, schedule.range, recordUpdate + '/' + attempt);

  useEffect(() => {
    let active = true;
    void desktop.collection(workspace, collectionId).then((collection) => {
      if (active) setLoaded({ collection });
    }).catch((error: unknown) => {
      if (active) setLoaded({ error: errorMessage(error) });
    });
    return () => { active = false; };
  }, [workspace, collectionId, attempt]);

  function retry() {
    if (!collection) setLoaded({});
    setAttempt((value) => value + 1);
  }

  function showDialog(kind: 'schema' | 'view' | 'newView' | 'dateProperty') {
    setDialogError('');
    setDialog(kind);
  }

  function save(job: () => Promise<void>) {
    setDialogError('');
    mutate(async () => {
      try {
        await job();
        setDialog(undefined);
      } catch (error) {
        setDialogError(errorMessage(error));
      }
    });
  }

  if (!collection || !view) {
    return <section className="collection-pane" aria-label="数据库">
      {loaded.error ? <CollectionError error={loaded.error} onRetry={retry} /> : <p role="status">正在打开数据库…</p>}
    </section>;
  }

  const saveProperty = (draft: PropertyDraft) => {
    const revision = collection.schemaRevision;
    save(async () => {
      const updated = await desktop.saveProperty(workspace, collectionId, revision, draft);
      setLoaded({ collection: updated });
    });
  };
  const convertProperty = (draft: PropertyDraft, fingerprint: string) => {
    save(async () => {
      const updated = await desktop.applyPropertyConversion(workspace, collectionId, collection.schemaRevision, draft, fingerprint);
      setLoaded({ collection: updated });
      setOffset(0);
      await onRecordsConverted();
    });
  };
  const previewConversion = (draft: PropertyDraft) => {
    setDialogError('');
    return desktop.previewPropertyConversion(workspace, collectionId, collection.schemaRevision, draft);
  };
  const createRelation = (draft: RelationDraft) => {
    save(async () => {
      const updated = await desktop.createRelation(workspace, collectionId, collection.schemaRevision, draft);
      setLoaded({ collection: updated });
    });
  };
  const saveComputed = (draft: ComputedDraft) => {
    save(async () => {
      const updated = await desktop.saveComputedProperty(workspace, collectionId, collection.schemaRevision, draft);
      setLoaded({ collection: updated });
    });
  };

  const saveView = (draft: ViewDraft) => {
    save(async () => {
      const updated = await desktop.saveView(workspace, collectionId, draft);
      setLoaded({ collection: updated });
      const target = draft.id ?? updated.views.find((item) => !collection.views.some(({ id }) => item.id === id))?.id;
      if (target) onSwitch(target);
      setOffset(0);
    });
  };
  const deleteView = () => {
    save(async () => {
      const updated = await desktop.deleteView(workspace, collectionId, view.id, view.revision);
      setLoaded({ collection: updated });
      onSwitch(updated.views[0].id);
      setOffset(0);
    });
  };
  const moveRecord = (record: CollectionRecord, day: string) => {
    setMoveError('');
    mutate(async () => {
      try {
        const page = await desktop.moveScheduledRecord(workspace, collectionId, {
          viewId: view.id, viewRevision: view.revision, pageId: record.id, pageRevision: record.revision, day,
        });
        onRecordMoved(page);
      } catch (error) {
        setMoveError(errorMessage(error));
        retry();
      }
    });
  };

  const resizeRecord = (record: CollectionRecord, edge: 'start' | 'end', day: string) => {
    setMoveError('');
    mutate(async () => {
      try {
        let target = view;
        const start = schedule.start;
        if (!schedule.end && view.type === 'calendar' && start) {
          // Notion keeps a period in one date property; here a calendar reads the end from a second
          // property of the same type, added the first time an edge is dragged.
          let schema = collection;
          let end = schema.properties.find((field) => field.id !== start.id && field.type === start.type && field.name === '结束日期');
          if (!end) {
            const taken = new Set(schema.properties.map((field) => field.name));
            let name = '结束日期';
            for (let index = 2; taken.has(name); index++) name = `结束日期 ${index}`;
            schema = await desktop.saveProperty(workspace, collectionId, schema.schemaRevision, { id: null, name, type: start.type, options: [] });
            end = schema.properties.find((field) => field.name === name);
          }
          if (!end) throw new Error('无法添加结束日期属性。');
          schema = await desktop.saveView(workspace, collectionId, {
            id: view.id, revision: view.revision, type: view.type, name: view.name ?? viewLabels[view.type], settings: view.settings,
            layout: { type: 'calendar', dateField: start.id, endField: end.id },
          });
          setLoaded({ collection: schema });
          target = schema.views.find((item) => item.id === view.id) ?? view;
        }
        const page = await desktop.resizeScheduledRecord(workspace, collectionId, {
          viewId: target.id, viewRevision: target.revision, pageId: record.id, pageRevision: record.revision, edge, day,
        });
        onRecordMoved(page);
      } catch (error) {
        setMoveError(errorMessage(error));
        retry();
      }
    });
  };
  const add = (preset?: Preset) => {
    void onAdd(preset).then((id) => { if (id) setTitleId(id); });
  };
  const saveField = (record: CollectionRecord, field: PropertyDefinition, value: PropertyValue | undefined) => onSaveField(record.id, field, value);

  const data = current?.data;
  const fields = collection.properties;
  const grouping = fields.find((field) => field.id === view.settings.groupBy);
  const callbacks = { busy, batch, onOpen, onEdit, onSaveField: saveField };
  const allSelected = Boolean(batch && data?.records.length && data.records.every((record) => batch.ids.has(record.id)));
  return <RecordTitleEditing.Provider value={titles}><section className={'collection-pane' + (inline ? ' inline' : '')} aria-label={inline ? `数据库「${inline.title}」` : '数据库记录'}>
    {inline && <InlineTitle title={inline.title} busy={busy} onRename={inline.onRename} onOpen={inline.onOpenDatabase} />}
    <div className="collection-toolbar">
      <div className="collection-views" role="group" aria-label="数据库视图">
        {collection.views.map(({ id, type, name }) => {
          const Icon = viewIcons[type];
          return <button key={id} className="collection-tab" type="button" aria-pressed={view.id === id} disabled={busy}
            onClick={() => { setOffset(0); setMoveError(''); onSwitch(id); }}><Icon size={16} />{name ?? viewLabels[type]}</button>;
        })}
        <button className="icon-button" type="button" title="新建视图" aria-label="新建视图" disabled={busy || collection.views.length >= 24}
          onClick={() => showDialog('newView')}><Plus size={16} /></button>
      </div>
      <div className="collection-actions">
        {onExportCsv && <button type="button" className="icon-button" aria-label="导出 CSV" title="导出 CSV"
          onClick={() => onExportCsv(view)} disabled={busy}><Download size={16} /></button>}
        {batch ? <button type="button" className="secondary-button" disabled={busy || !data?.records.length}
          onClick={() => batch.select(data?.records.map((record) => record.id) ?? [], !allSelected)}>
          <ListChecks size={15} />{allSelected ? '取消全选本页' : '全选本页'}
        </button> : onBeginBatch && <button type="button" className="icon-button" aria-label="批量选择" title="批量选择"
          onClick={onBeginBatch} disabled={busy}><ListChecks size={16} /></button>}
        <button type="button" className="icon-button" aria-label="视图设置" title="视图设置（筛选、排序、分组）"
          onClick={() => showDialog('view')} disabled={busy}><SlidersHorizontal size={16} /></button>
        <button type="button" className="icon-button" aria-label="管理属性" title="管理属性"
          onClick={() => showDialog('schema')} disabled={busy}><Wrench size={16} /></button>
        <button type="button" className="primary-button" aria-label="新建记录" onClick={() => inline ? add() : onCreate()} disabled={busy}>新建</button>
      </div>
    </div>
    <ViewSummary fields={fields} settings={view.settings} />
    <ScheduleToolbar schedule={schedule} busy={busy} onNavigate={() => { setOffset(0); setMoveError(''); }}
      onAddDate={() => showDialog('dateProperty')} />
    {moveError && <p className="save-error" role="alert">{moveError}</p>}
    {current?.error && <CollectionError error={current.error} onRetry={retry} />}
    {!current && <p className="collection-empty" role="status">正在读取记录…</p>}
    {data && <>
      {!data.total && !(schedule.calendar && schedule.range?.mode === 'between') && <p className="collection-empty">{schedule.range?.mode === 'between' ? '本区间暂无记录。'
        : view.settings.filter ? '没有符合筛选条件的记录。' : '暂无记录'}</p>}
      {schedule.range?.mode === 'between' ? <ScheduleRecords key={view.id} schedule={schedule} records={data.records} busy={busy}
        onCreate={batch || !schedule.start ? undefined : (day) => schedule.start && add({
          field: schedule.start.id, value: schedule.start.type === 'dateTime' ? day + 'T00:00:00+00:00' : day,
        })}
        onMove={batch ? undefined : moveRecord} onResize={batch ? undefined : resizeRecord} render={(record) => <div className="scheduled-record-actions">
          {batch && <BatchCheckbox batch={batch} id={record.id} title={record.title} disabled={busy} />}
          <RecordLink record={record} {...callbacks} /><EditRecord record={record} {...callbacks} />
        </div>} /> :
        (data.total > 0 || view.type === 'board') && <CollectionRecords workspace={workspace} view={schedule.range ? 'list' : view.type}
          grouping={grouping} fields={fields} rows={data.records} {...callbacks}
          onCreate={batch ? undefined : add} />}
      {!batch && !schedule.range && view.type !== 'board' && view.type !== 'gallery' &&
        <button type="button" className="collection-add-row" onClick={() => add()} disabled={busy}><Plus size={15} />新建一条</button>}
      <div className="collection-footer">
        <span role="status">共 {data.total} 条记录</span>
        {data.records.some((record) => record.computedPending) && <span role="status">正在重新计算属性…</span>}
        {data.total > data.pageSize && <div className="collection-pagination">
          <button type="button" className="icon-button" aria-label="上一页" title="上一页" disabled={busy || data.offset === 0}
            onClick={() => setOffset(data.offset - data.pageSize)}><ChevronLeft size={16} /></button>
          <span>第 {data.offset / data.pageSize + 1} / {Math.ceil(data.total / data.pageSize)} 页</span>
          <button type="button" className="icon-button" aria-label="下一页" title="下一页" disabled={busy || data.offset + data.pageSize >= data.total}
            onClick={() => setOffset(data.offset + data.pageSize)}><ChevronRight size={16} /></button>
        </div>}
      </div>
    </>}
    {(dialog === 'schema' || dialog === 'dateProperty') && <SchemaDialog workspace={workspace} collection={collection} targets={relationTargets} busy={busy} error={dialogError}
      initial={dialog === 'dateProperty' ? {
        name: collection.properties.some((field) => field.type === 'date') ? '结束日期' : '日期', type: 'date',
      } : undefined}
      onClose={() => setDialog(undefined)} onSave={saveProperty} onPreview={previewConversion} onConvert={convertProperty}
      onCreateRelation={createRelation} onSaveComputed={saveComputed} />}
    {(dialog === 'view' || dialog === 'newView') && <ViewDialog collection={collection} view={dialog === 'view' ? view : undefined}
      busy={busy} error={dialogError} onClose={() => setDialog(undefined)} onSave={saveView}
      onDelete={dialog === 'view' && collection.views.length > 1 ? deleteView : undefined} />}
  </section></RecordTitleEditing.Provider>;
}

/** An inline database's name, typed in place as in Notion, with a button that opens the database's own page. */
function InlineTitle({ title, busy, onRename, onOpen }: { title: string; busy: boolean; onRename(title: string): void; onOpen(): void }) {
  const [draft, setDraft] = useState<string>();
  const untitled = title === '无标题数据库';
  const commit = () => {
    const next = draft?.trim();
    setDraft(undefined);
    if (next && next !== title) onRename(next);
  };
  return <div className="inline-database-title">
    {draft === undefined ? <button type="button" className="inline-database-name" data-untitled={untitled || undefined}
      title="编辑数据库名称" aria-label={`编辑数据库名称「${title}」`} disabled={busy} onClick={() => setDraft(untitled ? '' : title)}>{title}</button>
      : <input className="inline-database-input" autoFocus aria-label="数据库名称" placeholder="无标题数据库" value={draft} maxLength={200}
        autoCapitalize="off" autoCorrect="off" spellCheck={false}
        onChange={(event) => setDraft(event.currentTarget.value)} onBlur={commit}
        onKeyDown={(event) => {
          if (event.nativeEvent.isComposing) return;
          if (event.key === 'Enter') { event.preventDefault(); commit(); }
          if (event.key === 'Escape') { event.preventDefault(); setDraft(undefined); }
        }} />}
    <button type="button" className="secondary-button inline-database-open" onClick={onOpen}><ArrowUpRight size={14} aria-hidden />打开数据库</button>
  </div>;
}

function CollectionError({ error, onRetry }: { error: string; onRetry(): void }) {
  return <div className="save-error" role="alert"><span>{error}</span>
    <button type="button" className="text-button" onClick={onRetry}>重试</button></div>;
}

function ViewSummary({ fields, settings }: { fields: PropertyDefinition[]; settings: ViewSettings }) {
  const titleAndFields = [titleField, ...fields];
  const filter = settings.filter;
  const filterField = titleAndFields.find((field) => field.id === filter?.fieldId);
  const sortField = titleAndFields.find((field) => field.id === settings.sort?.fieldId);
  const group = fields.find((field) => field.id === settings.groupBy);
  if (!filterField && !sortField && !group) return null;
  return <div className="collection-view-summary" aria-label="当前视图规则">
    {filter && filterField && <span>{filterField.name} {operatorNames[filter.operator]}
      {filter.value !== null && ` ${fieldText(filterField, filter.value)}`}</span>}
    {sortField && <span>{sortField.name} {settings.sort?.direction === 'desc' ? '降序' : '升序'}</span>}
    {group && <span>按{group.name}分组</span>}
  </div>;
}
