// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useRef, useState } from 'react';
import type { Collection, PropertyDefinition, RollupCalculation, RollupFunction } from '../contracts';
import { errorMessage } from '../contracts';
import { desktop } from '../desktop';
import { computedField, titleField } from './fields';

const functions: Record<RollupFunction, string> = {
  count: '计数', sum: '求和', min: '最小值', max: '最大值', average: '平均值',
};

function propertyReference(field: PropertyDefinition, fields: PropertyDefinition[]): string {
  const ambiguous = fields.some((item) => item.id !== field.id && item.id === field.name);
  return 'prop(' + JSON.stringify(ambiguous ? field.id : field.name) + ')';
}

export function displayFormula(expression: string, properties: PropertyDefinition[]): string {
  const fields = [titleField, ...properties];
  // Consume quoted strings as units so a literal containing prop(...) is preserved.
  return expression.replace(/\bprop\s*\(\s*("(?:[^"\\]|\\.)*")\s*\)|"(?:[^"\\]|\\.)*"/g, (token, reference?: string) => {
    if (!reference) return token;
    const field = fields.find((item) => item.id === JSON.parse(reference));
    return field ? propertyReference(field, fields) : token;
  });
}

export function FormulaFields({ scope, properties, expression, onChange }: {
  scope: string; properties: PropertyDefinition[]; expression: string; onChange(value: string): void;
}) {
  const input = useRef<HTMLTextAreaElement>(null);
  const fields = [titleField, ...properties];
  return <>
    <div className="form-field"><label htmlFor={scope + '-expression'}>公式表达式</label>
      <textarea id={scope + '-expression'} ref={input} value={expression} rows={3} maxLength={8192}
        required spellCheck={false} placeholder={'prop("数量") * 2'}
        onChange={(event) => onChange(event.currentTarget.value)} />
    </div>
    <div className="form-field"><label htmlFor={scope + '-input'}>插入属性</label>
      <select id={scope + '-input'} value="" onChange={(event) => {
        const field = fields.find((item) => item.id === event.currentTarget.value);
        const editor = input.current;
        if (!field || !editor) return;
        const text = propertyReference(field, fields);
        const start = editor.selectionStart;
        onChange(expression.slice(0, start) + text + expression.slice(editor.selectionEnd));
        requestAnimationFrame(() => {
          editor.focus();
          editor.setSelectionRange(start + text.length, start + text.length);
        });
      }}>
        <option value="">选择公式输入</option>
        {fields.map((field) => <option key={field.id} value={field.id}>{field.name}</option>)}
      </select>
    </div>
  </>;
}

export function RollupFields({ scope, workspace, collection, rule, onChange }: {
  scope: string; workspace: string; collection: Collection; rule: RollupCalculation;
  onChange(value: RollupCalculation): void;
}) {
  const relations = collection.properties.filter((field) => field.relation);
  const targetId = relations.find((field) => field.id === rule.relationPropertyId)?.relation?.collectionId;
  const needsValue = rule.function !== 'count';
  const lookup = needsValue && targetId && collection.id !== targetId ? targetId : undefined;
  const [target, setTarget] = useState<{ id: string; fields?: PropertyDefinition[]; error?: string }>();
  useEffect(() => {
    if (!lookup) return;
    let active = true;
    void desktop.collection(workspace, lookup).then((result) => {
      if (active) setTarget({ id: lookup, fields: result.properties });
    }).catch((error: unknown) => {
      if (active) setTarget({ id: lookup, error: errorMessage(error) });
    });
    return () => { active = false; };
  }, [workspace, lookup]);
  const hasTarget = target?.id === targetId ? target : undefined;
  const fields = targetId === collection.id ? collection.properties : hasTarget?.fields;
  const numeric = fields?.filter((item) => item.type === 'number' || computedField(item.type));
  return <>
    <div className="form-field"><label htmlFor={scope + '-relation'}>关联属性</label>
      <select id={scope + '-relation'} value={rule.relationPropertyId} required onChange={(event) => {
        onChange({ ...rule, relationPropertyId: event.currentTarget.value, targetPropertyId: null });
      }}>
        <option value="">{relations.length ? '选择关联属性' : '请先创建关联属性'}</option>
        {relations.map((field) => <option key={field.id} value={field.id}>{field.name}</option>)}
      </select>
    </div>
    <div className="form-field"><label htmlFor={scope + '-function'}>汇总方式</label>
      <select id={scope + '-function'} value={rule.function} onChange={(event) => {
        const method = event.currentTarget.value as RollupFunction;
        onChange({ ...rule, function: method, targetPropertyId: method === 'count' ? null : rule.targetPropertyId });
      }}>
        {Object.entries(functions).map(([key, label]) => <option key={key} value={key}>{label}</option>)}
      </select>
    </div>
    {needsValue && <div className="form-field"><label htmlFor={scope + '-target-property'}>目标数值属性</label>
      <select id={scope + '-target-property'} value={rule.targetPropertyId ?? ''} required
        onChange={(event) => onChange({ ...rule, targetPropertyId: event.currentTarget.value || null })}>
        <option value="">{numeric?.length ? '选择数值属性' : '暂无可选数值属性'}</option>
        {rule.targetPropertyId && !numeric?.some((item) => item.id === rule.targetPropertyId) &&
          <option value={rule.targetPropertyId}>原目标属性</option>}
        {numeric?.map((field) => <option key={field.id} value={field.id}>{field.name}</option>)}
      </select>
      {hasTarget?.error && <p className="collection-form-error" role="alert">{hasTarget.error}</p>}
    </div>}
  </>;
}
