// SPDX-License-Identifier: AGPL-3.0-or-later

import type { Attachment, Collection, CollectionRecord, PropertyDefinition, PropertyValue, RecordDraft, RelatedRecord } from '../contracts';
import { autoField, computedField, fieldText, titleField, valueOf } from './fields';

export type RecordVersionChoice = 'saved' | 'local';

interface ComparedField {
  field: PropertyDefinition;
  savedText: string;
  localText: string;
  localValue?: RecordDraft['properties'][string];
  defaultChoice?: RecordVersionChoice;
  acceptsLocal: boolean;
}

export interface RecordComparison {
  revision: number;
  draft: RecordDraft;
  fields: ComparedField[];
}

function equalValues(left: PropertyValue | undefined, right: PropertyValue | undefined): boolean {
  if (Array.isArray(left)) {
    return Array.isArray(right) && left.length === right.length && left.every((item, index) => item === right[index]);
  }
  return left === right;
}

function compareRelations(field: PropertyDefinition, base: CollectionRecord, latest: CollectionRecord,
  value: RecordDraft['properties'][string] | undefined, selection: RelatedRecord[]): ComparedField | undefined {
  const previous = base.relations?.[field.id];
  const current = latest.relations?.[field.id];
  const previousIds = (previous?.items ?? []).map(({ id }) => id).sort();
  const currentItems = current?.items ?? [];
  const currentIds = new Set(currentItems.map(({ id }) => id));
  const changedRemotely = !equalValues(previousIds, [...currentIds].sort()) || Boolean(previous?.restricted) !== Boolean(current?.restricted);
  const delta = typeof value === 'object' && !Array.isArray(value) ? value : undefined;
  const add = delta?.add.filter((id) => !currentIds.has(id)) ?? [];
  const remove = new Set(delta?.remove.filter((id) => currentIds.has(id)));
  const changedLocally = add.length > 0 || remove.size > 0;
  if (!changedLocally && !changedRemotely) return undefined;

  const addedTitles = new Map(selection.map(({ id, title }) => [id, title]));
  const currentTitles = currentItems.map(({ title }) => title);
  const mergedTitles = [...currentItems.filter(({ id }) => !remove.has(id)).map(({ title }) => title),
    ...add.map((id) => addedTitles.get(id) ?? '所选记录')];
  function describe(titles: string[]) {
    const labels = current?.restricted ? [...titles, '部分关联不可用'] : titles;
    return labels.join('、') || '未填写';
  }
  return {
    field, savedText: describe(currentTitles), localText: describe(mergedTitles), acceptsLocal: changedLocally,
    localValue: changedLocally ? { add, remove: [...remove] } : undefined,
    defaultChoice: changedLocally ? (changedRemotely ? undefined : 'local') : 'saved',
  };
}

export function compareRecordDraft(collection: Collection, base: CollectionRecord, latest: CollectionRecord,
  local: RecordDraft, relations: Record<string, RelatedRecord[]>, attachments: Attachment[]): RecordComparison {
  const names = new Map(attachments.map(({ id, name }) => [id, name]));
  const comparison: RecordComparison = {
    revision: latest.revision,
    draft: { title: latest.title, schemaRevision: collection.schemaRevision, properties: {} },
    fields: [],
  };

  for (const field of [titleField, ...collection.properties]) {
    if (autoField(field.type) || computedField(field.type)) continue;
    if (field.type === 'relation') {
      const review = compareRelations(field, base, latest, local.properties[field.id], relations[field.id] ?? []);
      if (review) comparison.fields.push(review);
      continue;
    }
    const isTitle = field.id === titleField.id;
    const before = valueOf(base, field);
    const saved = valueOf(latest, field);
    const proposed = local.properties[field.id];
    const mine = isTitle ? local.title.trim() || '无标题' :
      (typeof proposed === 'object' && !Array.isArray(proposed) ? undefined : proposed);
    if (!isTitle && saved !== undefined) comparison.draft.properties[field.id] = saved;
    if (equalValues(saved, mine)) continue;
    const changedLocally = !equalValues(before, mine);
    const changedRemotely = !equalValues(before, saved);
    const describe = (value: PropertyValue | undefined) => field.type === 'attachment' && Array.isArray(value)
      ? value.map((id) => names.get(id) ?? '原件不可用').join('、')
      : fieldText(field, value);
    comparison.fields.push({ field, savedText: describe(saved), localText: describe(mine), localValue: mine,
      acceptsLocal: true, defaultChoice: changedLocally ? (changedRemotely ? undefined : 'local') : 'saved' });
  }
  return comparison;
}

export function mergeRecordDraft(comparison: RecordComparison, choices: Record<string, RecordVersionChoice>): RecordDraft {
  const draft = { ...comparison.draft, properties: { ...comparison.draft.properties } };
  for (const { field, localValue, defaultChoice, acceptsLocal } of comparison.fields) {
    const choice = choices[field.id] ?? defaultChoice;
    if (!choice) throw new Error(`请选择要保留的「${field.name}」。`);
    if (choice !== 'local' || !acceptsLocal) continue;
    if (field.id === titleField.id && typeof localValue === 'string') draft.title = localValue;
    else if (localValue === undefined) delete draft.properties[field.id];
    else draft.properties[field.id] = localValue;
  }
  return draft;
}
