// SPDX-License-Identifier: AGPL-3.0-or-later

import { useMemo, useState } from 'react';
import type { CollectionView, PropertyDefinition, TimelineZoom, ViewKind, ViewRange } from '../contracts';

export const viewLabels: Record<ViewKind, string> = {
  table: '表格', board: '看板', list: '列表', calendar: '日历', gallery: '画廊', timeline: '时间线',
};

export const zoomOptions: Record<TimelineZoom, { days: number; label: string }> = {
  week: { days: 7, label: '一周' }, fortnight: { days: 14, label: '两周' }, month: { days: 31, label: '一个月' },
};

const dayMs = 86_400_000;
const minDay = '0001-01-01';
const maxDay = '9999-12-31';
const civil = (day: string) => new Date(day + 'T00:00:00Z');
const asDay = (date: Date) => date.getUTCFullYear() < 1 ? minDay
  : date.getUTCFullYear() > 9999 ? maxDay : date.toISOString().slice(0, 10);

export function addDays(day: string, days: number) {
  return asDay(new Date(civil(day).getTime() + days * dayMs));
}

export function distance(from: string, to: string) {
  return Math.round((civil(to).getTime() - civil(from).getTime()) / dayMs);
}

export function mondayIndex(day: string) {
  return (civil(day).getUTCDay() + 6) % 7;
}

function monthStart(day: string, step: number) {
  const date = civil(day.slice(0, 7) + '-01');
  date.setUTCMonth(date.getUTCMonth() + step);
  return asDay(date);
}

function today(utc: boolean) {
  const now = new Date();
  if (utc) return asDay(now);
  return [now.getFullYear().toString().padStart(4, '0'), (now.getMonth() + 1).toString().padStart(2, '0'),
    now.getDate().toString().padStart(2, '0')].join('-');
}

export const dateField = (field: PropertyDefinition) => field.type === 'date' || field.type === 'dateTime';
export const scheduleKind = (kind: ViewKind | undefined) => kind === 'calendar' || kind === 'timeline';

export function scheduleFields(view: CollectionView | undefined, properties: PropertyDefinition[]) {
  if (view?.layout?.type === 'calendar') {
    const { dateField: id, endField } = view.layout;
    return { start: properties.find((field) => field.id === id),
      end: endField ? properties.find((field) => field.id === endField) : undefined };
  }
  if (view?.layout?.type === 'timeline') {
    const { startField, endField } = view.layout;
    return { start: properties.find((field) => field.id === startField),
      end: properties.find((field) => field.id === endField) };
  }
  const fields = properties.filter(dateField);
  if (view?.type === 'calendar') return { start: fields[0] };
  if (view?.type === 'timeline') {
    for (const [index, start] of fields.entries()) {
      const end = fields.slice(index + 1).find((field) => field.type === start.type);
      if (end) return { start, end };
    }
  }
  return {};
}

export function scheduledDay(value: unknown, field: PropertyDefinition): string | undefined {
  if (typeof value !== 'string') return undefined;
  if (field.type === 'date') return value;
  const instant = new Date(value);
  return Number.isFinite(instant.getTime()) ? asDay(instant) : undefined;
}

interface Navigation {
  key: string;
  day: string;
  zoom: TimelineZoom;
  unscheduled: boolean;
}

export function useSchedule(view: CollectionView | undefined, properties: PropertyDefinition[]) {
  const [state, setState] = useState<Navigation>();
  const fields = scheduleFields(view, properties);
  const timed = fields.start?.type === 'dateTime';
  const currentDay = today(timed);
  const key = (view?.id ?? '') + '/' + (view?.revision ?? '');
  const zoom = view?.layout?.type === 'timeline' ? view.layout.zoom : 'fortnight' as const;
  // A saved change to the same view, such as the end property a calendar adds when an edge is
  // first dragged, keeps the month or range in sight.
  const sameView = state?.key.split('/')[0] === (view?.id ?? '');
  const nav = state?.key === key ? state : sameView && state ? { ...state, key, zoom } : {
    key, day: currentDay, zoom, unscheduled: false,
  };
  const move = (update: Partial<Navigation>) => setState({ ...nav, ...update });
  const calendar = view?.type === 'calendar';
  const month = monthStart(nav.day, 0);
  const from = calendar ? addDays(month, -mondayIndex(month)) : nav.day;
  const length = calendar ? 42 : zoomOptions[nav.zoom].days;
  const through = addDays(from, length - 1);
  const active = scheduleKind(view?.type);
  const unscheduled = nav.unscheduled || !fields.start;
  const range: ViewRange | undefined = useMemo(() => {
    if (!active) return undefined;
    return unscheduled ? { mode: 'unscheduled' } : { mode: 'between', from, through };
  }, [active, unscheduled, from, through]);
  return {
    ...fields, timed, range, calendar, nav, currentDay, from, through,
    setDay: (day: string) => { if (day) move({ day, unscheduled: false }); },
    setZoom: (zoom: TimelineZoom) => move({ zoom }),
    toggleUnscheduled: () => move({ unscheduled: !nav.unscheduled }),
    step: (direction: number) => move({
      day: calendar ? monthStart(nav.day, direction) : addDays(nav.day, direction * length),
    }),
  };
}

export type ViewSchedule = ReturnType<typeof useSchedule>;
