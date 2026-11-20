// SPDX-License-Identifier: AGPL-3.0-or-later

import { useId } from 'react';
import type { FilterOperator, PropertyDefinition, ViewSettings } from '../contracts';
import { ScalarField, autoField, computedField, operatorNames, operators, writeInput } from './fields';

export interface FilterDraft { fieldId: string; operator: FilterOperator; raw: string }

export function filterDraft(filter?: ViewSettings['filter']): FilterDraft {
  return {
    fieldId: filter?.fieldId ?? '',
    operator: filter?.operator ?? 'contains',
    raw: filter?.value == null ? '' : String(filter.value),
  };
}

export function readFilter(draft: FilterDraft, fields: PropertyDefinition[]): ViewSettings['filter'] {
  if (!draft.fieldId) return null;
  const field = fields.find((item) => item.id === draft.fieldId);
  if (!field) throw new Error('请重新选择筛选字段。');
  const value = needsValue(draft.operator) ? writeInput(field, draft.raw) : null;
  if (value === undefined || Array.isArray(value)) throw new Error('请填写筛选值。');
  return { fieldId: field.id, operator: draft.operator, value };
}

function needsValue(operator: FilterOperator) {
  return operator !== 'empty' && operator !== 'notEmpty';
}

export function FilterFields({ fields, draft, onChange }: {
  fields: PropertyDefinition[]; draft: FilterDraft; onChange(draft: FilterDraft): void;
}) {
  const id = useId();
  const field = fields.find((item) => item.id === draft.fieldId);
  return <>
    <div className="form-field"><label htmlFor={`${id}-filter`}>筛选字段</label>
      <select id={`${id}-filter`} value={draft.fieldId} onChange={(event) => {
        const next = fields.find((item) => item.id === event.currentTarget.value);
        onChange({ fieldId: next?.id ?? '', operator: next ? operators(next)[0] : 'contains', raw: '' });
      }}>
        <option value="">无筛选</option>
        {fields.filter((item) => item.type !== 'relation' && !computedField(item.type)).map((item) => <option key={item.id} value={item.id}>{item.name}</option>)}
      </select></div>
    {field && <div className="form-field"><label htmlFor={`${id}-operator`}>筛选条件</label>
      <select id={`${id}-operator`} value={draft.operator} onChange={(event) =>
        onChange({ ...draft, operator: event.currentTarget.value as FilterOperator })}>
        {operators(field).map((item) => <option key={item} value={item}>{operatorNames[item]}</option>)}
      </select></div>}
    {field && needsValue(draft.operator) && <div className="form-field">
      <label htmlFor={`${id}-value`}>{autoField(field.type) ? '筛选日期（UTC）' : '筛选值'}</label>
      <ScalarField id={`${id}-value`} field={field} value={draft.raw} onChange={(raw) => onChange({ ...draft, raw })} singleChoice />
    </div>}
  </>;
}
