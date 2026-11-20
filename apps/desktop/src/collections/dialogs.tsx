// SPDX-License-Identifier: AGPL-3.0-or-later

import { Plus, X } from 'lucide-react';
import { useEffect, useId, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import type {
  Attachment, Collection, CollectionRecord, CollectionView, ComputedDraft,
  PropertyConversionPreview, PropertyDefinition, PropertyDraft, PropertyKind, RecordDraft, RelatedRecord, RelationDraft, RollupCalculation,
  TimelineZoom, ViewDraft, ViewKind, ViewLayout,
} from '../contracts';
import { errorMessage } from '../contracts';
import { desktop } from '../desktop';
import { Modal } from '../dialogs';
import {
  ComputedFieldValue, FieldValue, MultipleField, ScalarField, autoField, choiceField, computedField, groupable,
  fieldText, propertyNames, readInput, sortable, titleField, valueOf, writeInput,
} from './fields';
import { FilterFields, filterDraft, readFilter } from './FilterFields';
import { RelationEditor } from './RelationEditor';
import { RecordConflictDialog } from './RecordConflictDialog';
import { compareRecordDraft } from './recordConflict';
import type { RecordComparison } from './recordConflict';
import { FormulaFields, RollupFields, displayFormula } from './CalculationFields';
import { dateField, scheduleKind, viewLabels, zoomOptions } from './viewDates';

interface DialogActions {
  busy: boolean;
  error?: string;
  onClose(): void;
}

function SaveActions({ busy, onClose, children }: DialogActions & { children?: ReactNode }) {
  return <footer className="dialog-actions">
    <button type="button" className="secondary-button" disabled={busy} onClick={onClose}>取消</button>
    <button type="submit" className="primary-button" disabled={busy}>{children ?? '保存'}</button>
  </footer>;
}

export function SchemaDialog({ workspace, collection, targets, busy, error, onClose, onSave, onPreview, onConvert, onCreateRelation, onSaveComputed }: DialogActions & {
  workspace: string; collection: Collection; targets: RelatedRecord[];
  onSave(draft: PropertyDraft): void; onConvert(draft: PropertyDraft, fingerprint: string): void;
  onPreview(draft: PropertyDraft): Promise<PropertyConversionPreview>;
  onCreateRelation(draft: RelationDraft): void; onSaveComputed(draft: ComputedDraft): void;
}) {
  const [selected, setSelected] = useState('');
  const fieldId = useId();
  return <Modal title="管理属性" className="collection-dialog" busy={busy} onClose={onClose}>
    <div className="collection-dialog-selection">
      <label htmlFor={fieldId}>要编辑的属性</label>
      <select id={fieldId} value={selected} disabled={busy} onChange={(event) => setSelected(event.currentTarget.value)}>
        <option value="">新建属性</option>
        {collection.properties.map((field) => <option key={field.id} value={field.id}>{field.name}</option>)}
      </select>
    </div>
    <PropertyForm key={selected} workspace={workspace} collection={collection} field={collection.properties.find((field) => field.id === selected)}
      targets={targets} busy={busy} error={error} onClose={onClose} onSave={onSave} onPreview={onPreview} onConvert={onConvert}
      onCreateRelation={onCreateRelation} onSaveComputed={onSaveComputed} />
  </Modal>;
}

function convertible(kind: PropertyKind) {
  return !autoField(kind) && !computedField(kind) && kind !== 'relation';
}

function PropertyForm({ workspace, collection, field, targets, busy, error, onClose, onSave, onPreview, onConvert, onCreateRelation, onSaveComputed }: DialogActions & {
  workspace: string; collection: Collection; field?: PropertyDefinition; targets: RelatedRecord[];
  onSave(draft: PropertyDraft): void; onConvert(draft: PropertyDraft, fingerprint: string): void;
  onPreview(draft: PropertyDraft): Promise<PropertyConversionPreview>;
  onCreateRelation(draft: RelationDraft): void; onSaveComputed(draft: ComputedDraft): void;
}) {
  const scope = useId();
  const optionKey = useRef(0);
  const [name, setName] = useState(field?.name ?? '');
  const [kind, setKind] = useState<PropertyKind>(field?.type ?? 'text');
  const [collectionId, setCollectionId] = useState('');
  const [withInverse, setWithInverse] = useState(false);
  const [inverseName, setInverseName] = useState('');
  const [expression, setExpression] = useState(() => field?.calculation?.type === 'formula'
    ? displayFormula(field.calculation.expression, collection.properties) : '');
  const [rollup, setRollup] = useState<RollupCalculation>(() => field?.calculation?.type === 'rollup'
    ? field.calculation : { type: 'rollup', relationPropertyId: '', function: 'count', targetPropertyId: null });
  const [options, setOptions] = useState(() => (field?.options ?? []).map((option) => ({ ...option, key: option.id })));
  const [previewing, setPreviewing] = useState(false);
  const [previewError, setPreviewError] = useState('');
  const [conversion, setConversion] = useState<{ draft: PropertyDraft; preview: PropertyConversionPreview }>();
  const converting = Boolean(field && field.type !== kind);
  const addOption = () => {
    const key = String(++optionKey.current);
    setOptions((old) => [...old, { key, id: '', name: '' }]);
  };

  async function previewConversion(draft: PropertyDraft) {
    setPreviewError('');
    setPreviewing(true);
    try {
      const preview = await onPreview(draft);
      setConversion({ draft, preview });
    } catch (error) {
      setPreviewError(errorMessage(error));
    } finally {
      setPreviewing(false);
    }
  }

  if (conversion) return <ConversionReview preview={conversion.preview} busy={busy} error={error}
    onBack={() => setConversion(undefined)} onClose={onClose} onConfirm={() => onConvert(conversion.draft, conversion.preview.fingerprint)} />;

  return <form className="dialog-form collection-form" onSubmit={(event) => {
    event.preventDefault();
    if (!name.trim() || previewing) return;
    if (kind === 'formula' || kind === 'rollup') {
      onSaveComputed({ id: field?.id ?? null, name, calculation: kind === 'formula' ? { type: 'formula', expression } : rollup });
      return;
    }
    if (kind === 'relation' && !field) {
      if (collectionId && (!withInverse || inverseName.trim())) {
        onCreateRelation({ name, collectionId, inverseName: withInverse ? inverseName : null });
      }
      return;
    }
    const draft = { id: field?.id ?? null, name, type: kind, options: choiceField(kind)
      ? options.map((option) => ({ id: option.id || null, name: option.name })) : [] };
    if (converting) void previewConversion(draft);
    else onSave(draft);
  }}>
    <fieldset disabled={busy || previewing} className="collection-form-fields">
      <div className="form-field"><label htmlFor={`${scope}-name`}>属性名称</label>
        <input id={`${scope}-name`} value={name} maxLength={64} required onChange={(event) => setName(event.currentTarget.value)} /></div>
      <div className="form-field"><label htmlFor={`${scope}-type`}>属性类型</label>
        <select id={`${scope}-type`} value={kind} disabled={Boolean(field && !convertible(field.type))} onChange={(event) => {
          const type = event.currentTarget.value as PropertyKind;
          setKind(type);
          setPreviewError('');
          if (type === field?.type && converting) setOptions(field.options.map((option) => ({ ...option, key: option.id })));
          else if (choiceField(type) && !options.length) addOption();
        }}>
          {Object.entries(propertyNames).filter(([value]) => !field || value === field.type || (convertible(field.type) && convertible(value as PropertyKind)))
            .map(([value, label]) => <option key={value} value={value}>{label}</option>)}
        </select></div>
      {kind === 'formula' && <FormulaFields scope={scope} properties={collection.properties} expression={expression} onChange={setExpression} />}
      {kind === 'rollup' && <RollupFields scope={scope} workspace={workspace} collection={collection} rule={rollup} onChange={setRollup} />}
      {kind === 'relation' && (field ?
        <p className="field-empty">{targets.find((target) => target.id === field.relation?.collectionId)?.title ?? '目标数据库不可用'}</p> :
        <>
          <div className="form-field"><label htmlFor={`${scope}-target`}>目标数据库</label>
            <select id={`${scope}-target`} value={collectionId} required onChange={(event) => setCollectionId(event.currentTarget.value)}>
              <option value="">选择数据库</option>
              {targets.map((target) => <option key={target.id} value={target.id}>{target.title}</option>)}
            </select>
          </div>
          <label className="relation-pair"><input type="checkbox" checked={withInverse}
            onChange={(event) => setWithInverse(event.currentTarget.checked)} />同时创建反向关联</label>
          {withInverse && <div className="form-field"><label htmlFor={`${scope}-inverse`}>反向属性名称</label>
            <input id={`${scope}-inverse`} value={inverseName} maxLength={64} required onChange={(event) => setInverseName(event.currentTarget.value)} />
          </div>}
        </>)}
      {choiceField(kind) && <fieldset className="option-fields">
        <legend>选项</legend>
        {options.map((option, index) => <div className="option-row" key={option.key}>
          <input aria-label={`选项 ${index + 1}`} value={option.name} maxLength={64} required onChange={(event) => {
            const name = event.currentTarget.value;
            setOptions((old) => old.map((item) => item.key === option.key ? { ...item, name } : item));
          }} />
          {(!option.id || converting) && options.length > 1 && <button type="button" className="icon-button" aria-label={`移除选项 ${index + 1}`}
            onClick={() => setOptions((old) => old.filter((item) => item.key !== option.key))}><X size={15} /></button>}
        </div>)}
        <button type="button" className="secondary-button" disabled={options.length >= 64} onClick={addOption}><Plus size={15} />添加选项</button>
      </fieldset>}
    </fieldset>
    {(previewError || error) && <p className="collection-form-error" role="alert">{previewError || error}</p>}
    <SaveActions busy={busy || previewing} onClose={onClose}>
      {previewing ? '正在预览…' : converting ? '预览转换' : field ? '保存属性' : '创建属性'}
    </SaveActions>
  </form>;
}

function ConversionReview({ preview, busy, error, onBack, onClose, onConfirm }: DialogActions & {
  preview: PropertyConversionPreview; onBack(): void; onConfirm(): void;
}) {
  const heading = useRef<HTMLHeadingElement>(null);
  const label = useId();
  useEffect(() => { heading.current?.focus(); }, []);
  const { previous, target, convertedRecords, clearedRecords, convertedTemplates, clearedTemplates, samples, viewChanges } = preview;
  return <section className="dialog-form conversion-review" aria-labelledby={label}>
    <h3 id={label} tabIndex={-1} ref={heading}>{previous.name} · {propertyNames[previous.type]} → {propertyNames[target.type]}</h3>
    <p>可转换 {convertedRecords} 条记录，{clearedRecords} 条记录将留空。原值保留在页面历史中。</p>
    {convertedTemplates.length > 0 && <p>将更新模板「{convertedTemplates.join('、')}」的属性值。</p>}
    {clearedTemplates.length > 0 && <p>模板「{clearedTemplates.join('、')}」中此属性的值将留空。</p>}
    {viewChanges.length > 0 && <ul>{viewChanges.map((change) => <li key={change}>{change}</li>)}</ul>}
    {samples.length > 0 && <table className="conversion-values">
      <caption>受影响记录示例</caption>
      <thead><tr><th scope="col">记录</th><th scope="col">原值</th><th scope="col">转换后</th></tr></thead>
      <tbody>{samples.map((sample, index) => <tr key={index}>
        <th scope="row">{sample.title}</th>
        <td>{fieldText(previous, sample.previous)}</td>
        <td>{sample.converted === null ? '留空' : fieldText(target, sample.converted)}</td>
      </tr>)}</tbody>
    </table>}
    {error && <p className="collection-form-error" role="alert">{error}</p>}
    <footer className="dialog-actions">
      <button type="button" className="secondary-button" disabled={busy} onClick={onBack}>返回修改</button>
      <button type="button" className="secondary-button" disabled={busy} onClick={onClose}>取消</button>
      <button type="button" className="primary-button" disabled={busy} onClick={onConfirm}>确认转换</button>
    </footer>
  </section>;
}

export function RecordDialog({ workspace, collection, record, attachments, conflicts, busy, error, onClose, onSave }: DialogActions & {
  workspace: string; collection: Collection; record: CollectionRecord; attachments: Attachment[];
  conflicts: number; onSave(draft: RecordDraft, revision: number): void;
}) {
  const scope = useId();
  const [title, setTitle] = useState(record.title);
  const [input, setInput] = useState<Record<string, string | string[]>>(() => Object.fromEntries(
    collection.properties.filter((field) => field.type !== 'relation' && !autoField(field.type) && !computedField(field.type))
      .map((field) => [field.id, readInput(field, valueOf(record, field))]),
  ));
  const [relations, setRelations] = useState<Record<string, RelatedRecord[]>>({});
  const [invalid, setInvalid] = useState('');
  const [comparison, setComparison] = useState<{ conflicts: number; result: RecordComparison }>();
  const [task, setTask] = useState<'compare' | 'export'>();
  const [exported, setExported] = useState(false);
  const blocked = busy || Boolean(task);

  function readDraft(): RecordDraft {
    const properties: RecordDraft['properties'] = {};
    for (const field of collection.properties) {
      const selected = relations[field.id];
      if (field.type === 'relation' && selected) {
        const before = new Set(record.relations?.[field.id]?.items.map((item) => item.id));
        const after = new Set(selected.map((item) => item.id));
        const add = [...after].filter((id) => !before.has(id));
        const remove = [...before].filter((id) => !after.has(id));
        if (add.length || remove.length) properties[field.id] = { add, remove };
        continue;
      }
      const raw = input[field.id];
      if (raw === undefined) continue;
      const value = writeInput(field, raw);
      if (value !== undefined) properties[field.id] = value;
    }
    return { title, properties, schemaRevision: collection.schemaRevision };
  }

  async function compare(draft: RecordDraft) {
    setTask('compare');
    try {
      const [schema, saved, originals] = await Promise.all([
        desktop.collection(workspace, collection.id), desktop.record(workspace, collection.id, record.id),
        desktop.attachments(workspace, record.id),
      ]);
      if (schema.schemaRevision !== draft.schemaRevision) {
        throw new Error('数据库的属性定义已更新。请先导出草稿，再重新打开记录。');
      }
      setComparison({ conflicts, result: compareRecordDraft(schema, record, saved, draft, relations, [...attachments, ...originals]) });
    } catch (error) {
      setInvalid(errorMessage(error));
    } finally {
      setTask(undefined);
    }
  }

  async function exportDraft() {
    setTask('export');
    setInvalid('');
    setExported(false);
    try {
      setExported(await desktop.exportRecordDraft(workspace, collection.id, record.id, record.revision, readDraft()));
    } catch (error) {
      setInvalid(errorMessage(error));
    } finally {
      setTask(undefined);
    }
  }

  if (comparison && comparison.conflicts === conflicts) return <RecordConflictDialog
    comparison={comparison.result} busy={blocked} error={error} onClose={() => setComparison(undefined)}
    onSave={(draft) => onSave(draft, comparison.result.revision)} />;

  const message = invalid || error || (conflicts > 0 ? '记录已有更新。已保留表单输入，请对比最新版本。' : '');
  return <Modal title="编辑记录属性" className="collection-dialog" busy={blocked} onClose={onClose}>
    <form className="dialog-form collection-form" onSubmit={(event) => {
      event.preventDefault();
      if (blocked) return;
      setInvalid('');
      setExported(false);
      try {
        const draft = readDraft();
        if (conflicts > 0) void compare(draft);
        else onSave(draft, record.revision);
      } catch (error) {
        setInvalid(errorMessage(error));
      }
    }}>
      <fieldset disabled={blocked} className="collection-form-fields">
        <div className="form-field"><label htmlFor={`${scope}-title`}>标题</label>
          <input id={`${scope}-title`} value={title} maxLength={200} onChange={(event) => setTitle(event.currentTarget.value)} /></div>
        {collection.properties.map((field) => {
          if (field.type === 'relation') {
            const original = record.relations?.[field.id];
            return <RelationEditor key={field.id} workspace={workspace} field={field}
              selected={relations[field.id] ?? original?.items ?? []} restricted={original?.restricted ?? false}
              onChange={(items) => setRelations((old) => ({ ...old, [field.id]: items }))} />;
          }
          const id = `${scope}-${field.id}`;
          const value = input[field.id];
          const onChange = (next: string | string[]) => setInput((old) => ({ ...old, [field.id]: next }));
          if (Array.isArray(value)) {
            const choices = field.type === 'attachment' ? attachments : field.options;
            return <MultipleField key={field.id} label={field.name} value={value} choices={choices} onChange={onChange}
              empty={field.type === 'attachment' ? '在记录页的「附件」菜单中添加文件。' : '暂无选项。'} />;
          }
          return <div key={field.id} className="form-field"><label htmlFor={id}>{field.name}</label>
            {computedField(field.type) ? <output id={id}><ComputedFieldValue record={record} field={field} /></output>
              : autoField(field.type) ? <output id={id}><FieldValue field={field} value={valueOf(record, field)} /></output>
              : <ScalarField id={id} field={field} value={value ?? ''} onChange={onChange} />}</div>;
        })}
      </fieldset>
      {message && <p className="collection-form-error" role="alert">{message}</p>}
      {exported && <p className="record-compare-note" role="status">记录草稿已导出。</p>}
      <footer className="dialog-actions">
        {(conflicts > 0 || error) && <button type="button" className="text-button" disabled={blocked}
          onClick={() => { void exportDraft(); }}>导出草稿</button>}
        <button type="button" className="secondary-button" disabled={blocked} onClick={onClose}>取消</button>
        <button type="submit" className="primary-button" disabled={blocked}>
          {task === 'compare' ? '正在读取版本…' : conflicts > 0 ? '对比版本' : '保存'}
        </button>
      </footer>
    </form>
  </Modal>;
}

export function ViewDialog({ collection, view, busy, error, onClose, onSave, onDelete }: DialogActions & {
  collection: Collection; view?: CollectionView; onSave(draft: ViewDraft): void; onDelete?(): void;
}) {
  const id = useId();
  const [kind, setKind] = useState<ViewKind>(view?.type ?? 'table');
  const [name, setName] = useState(view ? view.name ?? viewLabels[view.type] : '');
  const [draft, setDraft] = useState(() => filterDraft(view?.settings.filter ?? null));
  const [sortId, setSortId] = useState(view?.settings.sort?.fieldId ?? '');
  const [direction, setDirection] = useState(view?.settings.sort?.direction ?? 'asc');
  const [group, setGroup] = useState(view?.settings.groupBy ?? '');
  const [start, setStart] = useState(view?.layout?.type === 'calendar' ? view.layout.dateField
    : view?.layout?.type === 'timeline' ? view.layout.startField : '');
  const [end, setEnd] = useState(view?.layout?.type === 'timeline' ? view.layout.endField : '');
  const [cover, setCover] = useState(view?.layout?.type === 'gallery' ? view.layout.coverField ?? '' : '');
  const [zoom, setZoom] = useState<TimelineZoom>(view?.layout?.type === 'timeline' ? view.layout.zoom : 'fortnight');
  const [invalid, setInvalid] = useState('');
  const fields = [titleField, ...collection.properties];
  const dates = collection.properties.filter(dateField);
  const startField = dates.find((field) => field.id === start);
  const endChoices = dates.filter((field) => field.id !== start && (!startField || field.type === startField.type));

  return <Modal title={view ? '视图设置' : '新建视图'} className="collection-dialog" busy={busy} onClose={onClose}>
    <form className="dialog-form collection-form" onSubmit={(event) => {
      event.preventDefault();
      setInvalid('');
      try {
        const filter = readFilter(draft, fields);
        let layout: ViewLayout | null = null;
        if (kind === 'calendar' && start) layout = { type: kind, dateField: start };
        if (kind === 'gallery') layout = { type: kind, coverField: cover || null };
        if (kind === 'timeline' && (start || end)) {
          if (!start || !end) throw new Error('请选择时间线的起始日期和结束日期。');
          layout = { type: kind, startField: start, endField: end, zoom };
        }
        onSave({ id: view?.id ?? null, revision: view?.revision ?? null, name, type: kind, layout,
          settings: { filter, sort: sortId ? { fieldId: sortId, direction } : null,
            groupBy: scheduleKind(kind) ? null : group || null } });
      } catch (error) {
        setInvalid(errorMessage(error));
      }
    }}>
      <fieldset disabled={busy} className="collection-form-fields">
        <div className="form-field"><label htmlFor={id + '-name'}>视图名称</label>
          <input id={id + '-name'} value={name} maxLength={48} required onChange={(event) => setName(event.currentTarget.value)} />
        </div>
        {!view && <div className="form-field"><label htmlFor={id + '-type'}>视图类型</label>
          <select id={id + '-type'} value={kind} onChange={(event) => {
            setKind(event.currentTarget.value as ViewKind);
            setStart('');
            setEnd('');
          }}>
            {Object.entries(viewLabels).map(([type, label]) => <option key={type} value={type}>{label}</option>)}
          </select>
        </div>}
        {scheduleKind(kind) && <div className="form-field"><label htmlFor={id + '-start'}>
          {kind === 'calendar' ? '日历日期属性' : '起始日期属性'}
        </label>
          <select id={id + '-start'} value={start} onChange={(event) => {
            const field = dates.find((item) => item.id === event.currentTarget.value);
            setStart(field?.id ?? '');
            if (!field || field.id === end || dates.find((item) => item.id === end)?.type !== field.type) setEnd('');
          }}>
            <option value="">自动选择</option>
            {dates.map((field) => <option key={field.id} value={field.id}>{field.name}</option>)}
          </select>
        </div>}
        {kind === 'timeline' && <>
          <div className="form-field"><label htmlFor={id + '-end'}>结束日期属性</label>
            <select id={id + '-end'} value={end} onChange={(event) => setEnd(event.currentTarget.value)}>
              <option value="">自动选择</option>
              {endChoices.map((field) => <option key={field.id} value={field.id}>{field.name}</option>)}
            </select>
          </div>
          {start && end && <div className="form-field"><label htmlFor={id + '-zoom'}>默认时间范围</label>
            <select id={id + '-zoom'} value={zoom} onChange={(event) => setZoom(event.currentTarget.value as TimelineZoom)}>
              {Object.entries(zoomOptions).map(([value, { label }]) => <option key={value} value={value}>{label}</option>)}
            </select>
          </div>}
        </>}
        {kind === 'gallery' && <div className="form-field"><label htmlFor={id + '-cover'}>封面属性</label>
          <select id={id + '-cover'} value={cover} onChange={(event) => setCover(event.currentTarget.value)}>
            <option value="">页面封面或首张图片附件</option>
            {collection.properties.filter((field) => field.type === 'attachment')
              .map((field) => <option key={field.id} value={field.id}>{field.name}</option>)}
          </select>
        </div>}
        <FilterFields fields={fields} draft={draft} onChange={setDraft} />
        <div className="form-field"><label htmlFor={`${id}-sort`}>排序字段</label>
          <select id={`${id}-sort`} value={sortId} onChange={(event) => setSortId(event.currentTarget.value)}>
            <option value="">默认排序</option>
            {fields.filter((item) => sortable(item.type)).map((item) => <option key={item.id} value={item.id}>{item.name}</option>)}
          </select></div>
        {sortId && <div className="form-field"><label htmlFor={`${id}-direction`}>排序顺序</label>
          <select id={`${id}-direction`} value={direction} onChange={(event) => setDirection(event.currentTarget.value as 'asc' | 'desc')}>
            <option value="asc">升序</option><option value="desc">降序</option>
          </select></div>}
        {!scheduleKind(kind) && <div className="form-field"><label htmlFor={`${id}-group`}>分组字段</label>
          <select id={`${id}-group`} value={group} onChange={(event) => setGroup(event.currentTarget.value)}>
            <option value="">无分组</option>
            {collection.properties.filter((item) => groupable(item.type)).map((item) => <option key={item.id} value={item.id}>{item.name}</option>)}
          </select></div>}
      </fieldset>
      {(invalid || error) && <p className="collection-form-error" role="alert">{invalid || error}</p>}
      {onDelete && <button className="text-button danger" type="button" disabled={busy} onClick={onDelete}>删除视图</button>}
      <SaveActions busy={busy} onClose={onClose} />
    </form>
  </Modal>;
}
