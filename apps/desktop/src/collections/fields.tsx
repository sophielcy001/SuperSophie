// SPDX-License-Identifier: AGPL-3.0-or-later

import type { CollectionRecord, ComputedError, FilterOperator, PropertyDefinition, PropertyKind, PropertyOption, PropertyValue } from '../contracts';

export const propertyNames: Record<PropertyKind, string> = {
  text: '文本', number: '数字', checkbox: '复选框', date: '日期', dateTime: '日期时间（带时区）',
  status: '状态', select: '单选', multiSelect: '多选', url: '网址',
  attachment: '附件', relation: '关联', rollup: '汇总', formula: '公式', createdAt: '创建时间', updatedAt: '修改时间',
};

export const titleField: PropertyDefinition = { id: 'title', name: '标题', type: 'text', options: [] };
export const autoField = (kind: PropertyKind) => kind === 'createdAt' || kind === 'updatedAt';
export const computedField = (kind: PropertyKind) => kind === 'formula' || kind === 'rollup';
export const choiceField = (kind: PropertyKind) => ['status', 'select', 'multiSelect'].includes(kind);
export const sortable = (kind: PropertyKind) => !computedField(kind) && !['multiSelect', 'attachment', 'relation'].includes(kind);
export const groupable = (kind: PropertyKind) => ['status', 'select', 'checkbox'].includes(kind);
export const operatorNames: Record<FilterOperator, string> = {
  equal: '等于', contains: '包含', empty: '未填写', notEmpty: '已填写',
};

export function operators(field: PropertyDefinition): FilterOperator[] {
  if (field.id === titleField.id) return ['contains', 'equal'];
  if (field.type === 'attachment') return ['empty', 'notEmpty'];
  if (autoField(field.type)) return ['equal'];
  if (field.type === 'multiSelect') return ['contains', 'empty', 'notEmpty'];
  if (field.type === 'text' || field.type === 'url') return ['contains', 'equal', 'empty', 'notEmpty'];
  return ['equal', 'empty', 'notEmpty'];
}

const utcTime = new Intl.DateTimeFormat('zh-CN', { dateStyle: 'medium', timeStyle: 'short', timeZone: 'UTC' });

export function valueOf(record: CollectionRecord, field: PropertyDefinition): PropertyValue | undefined {
  if (field.id === titleField.id) return record.title;
  if (field.type === 'createdAt') return record.createdAt;
  if (field.type === 'updatedAt') return record.updatedAt;
  if (computedField(field.type)) {
    const result = record.computed?.[field.id];
    return result?.status === 'value' ? result.value ?? undefined : undefined;
  }
  return record.properties[field.id];
}

export function fieldText(field: PropertyDefinition, value: PropertyValue | undefined): string {
  if (value === undefined) return '未填写';
  if (autoField(field.type) && typeof value === 'number') {
    return Math.abs(value) <= 8.64e15 ? `${utcTime.format(value)} UTC` : '时间超出显示范围';
  }
  if (field.type === 'attachment' && Array.isArray(value)) return `${value.length} 个附件`;
  if (field.type === 'relation' && Array.isArray(value)) return value.length ? '有关联记录' : '关联不可用';
  if (value === true) return '是';
  if (value === false) return '否';
  if (Array.isArray(value)) return value.map((id) => field.options.find((option) => option.id === id)?.name ?? id).join('、');
  return field.options.find((option) => option.id === value)?.name ?? String(value);
}

export function RecordFieldValue({ record, field, onOpen, busy }: {
  record: CollectionRecord; field: PropertyDefinition; onOpen(id: string): void; busy: boolean;
}) {
  if (computedField(field.type)) return <ComputedFieldValue record={record} field={field} />;
  const relation = record.relations?.[field.id];
  if (field.type !== 'relation' || !relation) return <FieldValue field={field} value={valueOf(record, field)} />;
  return <span className="relation-items">
    {relation.items.map((item) => <button key={item.id} type="button" className="relation-record" disabled={busy}
      onClick={() => onOpen(item.id)}>{item.title}</button>)}
    {relation.restricted && <span className="field-empty">部分关联不可用</span>}
  </span>;
}

const computedErrors: Record<ComputedError, string> = {
  CYCLE: '计算依赖存在循环', LIMIT_EXCEEDED: '超过计算上限', RESTRICTED: '计算依赖不可读',
  TYPE_ERROR: '计算类型不匹配', DIVISION_BY_ZERO: '除数为零',
  OUT_OF_RANGE: '数值超出范围', INVALID_DATE: '日期无效',
};

export function ComputedFieldValue({ record, field }: { record: CollectionRecord; field: PropertyDefinition }) {
  if (record.computedPending) return <span className="field-empty">计算中…</span>;
  const result = record.computed?.[field.id];
  if (result?.status === 'error') return <span className="computed-error" title={result.code}>{computedErrors[result.code]}</span>;
  return <FieldValue field={field} value={valueOf(record, field)} />;
}

export function FieldValue({ field, value }: {
  field: PropertyDefinition; value: PropertyValue | undefined;
}) {
  if (value === undefined) return <span className="field-empty" aria-label="未填写">—</span>;
  if (!choiceField(field.type)) return <span>{fieldText(field, value)}</span>;
  const selected = Array.isArray(value) ? value : [value];
  return <span className="property-pills">{field.options.map((option, index) => selected.includes(option.id) &&
    <span key={option.id} className="property-pill" data-color={index % 6}>{option.name}</span>)}</span>;
}

export function readInput(field: PropertyDefinition, value: PropertyValue | undefined): string | string[] {
  if (Array.isArray(value)) return value;
  if (field.type === 'attachment' || field.type === 'multiSelect') return [];
  return value === undefined ? '' : String(value);
}

export function writeInput(field: PropertyDefinition, raw: string | string[]): PropertyValue | undefined {
  if (Array.isArray(raw)) return raw.length ? raw : undefined;
  if (!raw.trim()) return undefined;
  if (field.type === 'number') {
    const number = Number(raw);
    if (!Number.isFinite(number)) throw new Error(`「${field.name}」需要有效数字。`);
    return number;
  }
  if (field.type === 'checkbox') return raw === 'true';
  return field.type === 'url' || field.type === 'dateTime' ? raw.trim() : raw;
}

export function ScalarField({ id, field, value, onChange, singleChoice = false }: {
  id: string; field: PropertyDefinition; value: string; onChange(value: string): void; singleChoice?: boolean;
}) {
  if (choiceField(field.type) || field.type === 'checkbox') {
    const options = field.type === 'checkbox' ? [{ id: 'true', name: '是' }, { id: 'false', name: '否' }] : field.options;
    return <select id={id} value={value} onChange={(event) => onChange(event.currentTarget.value)}>
      <option value="">未填写</option>
      {options.map((option) => <option key={option.id} value={option.id}>{option.name}</option>)}
    </select>;
  }
  if (field.type === 'text' && !singleChoice) {
    return <textarea id={id} value={value} rows={2} maxLength={2000} onChange={(event) => onChange(event.currentTarget.value)} />;
  }
  const type = field.type === 'date' || autoField(field.type) ? 'date' : field.type === 'number' ? 'number' : 'text';
  return <input id={id} type={type} value={value} step={type === 'number' ? 'any' : undefined}
    placeholder={field.type === 'dateTime' ? '2026-11-16T09:30:00+08:00' : undefined}
    maxLength={singleChoice ? 256 : 2048} onChange={(event) => onChange(event.currentTarget.value)} />;
}

export function MultipleField({ label, choices, value, onChange, empty }: {
  label: string; choices: PropertyOption[]; value: string[]; onChange(value: string[]): void; empty: string;
}) {
  return <fieldset className="field-choices">
    <legend>{label}</legend>
    {choices.length ? choices.map((choice) => <label key={choice.id}>
      <input type="checkbox" checked={value.includes(choice.id)} onChange={(event) => {
        onChange(event.currentTarget.checked ? [...value, choice.id] : value.filter((id) => id !== choice.id));
      }} />
      <span>{choice.name}</span>
    </label>) : <p>{empty}</p>}
  </fieldset>;
}
