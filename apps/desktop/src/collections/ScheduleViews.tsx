// SPDX-License-Identifier: AGPL-3.0-or-later

import { ChevronLeft, ChevronRight, Plus } from 'lucide-react';
import { useEffect, useId, useRef, useState } from 'react';
import type { CSSProperties, MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent, ReactNode } from 'react';
import type { CollectionRecord, PropertyDefinition, TimelineZoom } from '../contracts';
import { addDays, distance, mondayIndex, scheduledDay, zoomOptions } from './viewDates';
import type { ViewSchedule } from './viewDates';

export function ScheduleToolbar({ schedule, busy, onNavigate, onAddDate }: {
  schedule: ViewSchedule; busy: boolean; onNavigate(): void; onAddDate?(): void;
}) {
  const id = useId();
  const { nav, calendar, timed, currentDay, from, through, start, end } = schedule;
  if (!schedule.range) return null;
  const move = (action: () => void) => { onNavigate(); action(); };
  const dates = calendar ? nav.day.slice(0, 4) + ' 年 ' + Number(nav.day.slice(5, 7)) + ' 月'
    : from + ' 至 ' + through;
  if (!start) return <div className="collection-empty">
    <p>{calendar ? '请先添加日期或带时区的日期时间属性。' : '请添加两项同类型的日期属性，用于起止日期。'}</p>
    {onAddDate && <button type="button" className="secondary-button" disabled={busy} onClick={onAddDate}>添加日期属性</button>}
  </div>;
  return <div className="schedule-toolbar">
    <div className="schedule-title">
      <strong aria-live="polite">{dates}</strong>
      <span className="schedule-field-label">按「{end ? start.name + ' → ' + end.name : start.name}」{timed && ' · UTC'}</span>
    </div>
    <div className="schedule-nav" aria-label="日期导航" role="group">
      {!calendar && <select aria-label="时间线缩放" value={nav.zoom} disabled={busy || nav.unscheduled}
        onChange={(event) => { const zoom = event.currentTarget.value as TimelineZoom; move(() => schedule.setZoom(zoom)); }}>
        {Object.entries(zoomOptions).map(([value, { label }]) => <option key={value} value={value}>{label}</option>)}
      </select>}
      <button type="button" className="schedule-text-button" aria-pressed={nav.unscheduled} disabled={busy}
        onClick={() => move(schedule.toggleUnscheduled)}>未排期</button>
      <label htmlFor={id + '-go'} className="visually-hidden">跳转日期</label>
      <input id={id + '-go'} className="schedule-jump" type="date" min="0001-01-01" max="9999-12-31" value={nav.day}
        title="跳转到日期" disabled={busy} onChange={(event) => {
          const day = event.currentTarget.value;
          if (day >= '0001-01-01' && day <= '9999-12-31' && day.length === 10) move(() => schedule.setDay(day));
        }} />
      <button type="button" className="icon-button" aria-label="上个日期区间" title={calendar ? '上个月' : '上个区间'}
        disabled={busy || nav.unscheduled || nav.day <= '0001-01-31'} onClick={() => move(() => schedule.step(-1))}><ChevronLeft size={16} /></button>
      <button type="button" className="schedule-text-button" disabled={busy} onClick={() => move(() => schedule.setDay(currentDay))}>今天</button>
      <button type="button" className="icon-button" aria-label="下个日期区间" title={calendar ? '下个月' : '下个区间'}
        disabled={busy || nav.unscheduled || nav.day >= '9999-12-01'} onClick={() => move(() => schedule.step(1))}><ChevronRight size={16} /></button>
    </div>
  </div>;
}

interface ScheduleProps {
  schedule: ViewSchedule;
  records: CollectionRecord[];
  busy: boolean;
  render(record: CollectionRecord): ReactNode;
  /** Moves the record so it starts on `day`, keeping its length. */
  onMove?(record: CollectionRecord, day: string): void;
  /** Moves one end of the record's period to `day`, as dragging an edge of its bar does. */
  onResize?(record: CollectionRecord, edge: 'start' | 'end', day: string): void;
  /** Creates a record on the given day (calendar only). */
  onCreate?(day: string): void;
}

interface Span { first: string; last: string }
type Grip = 'move' | 'start' | 'end';

/** A record's first and last day; one without an end value lasts its start day, as in Notion. */
function spanOf(record: CollectionRecord, start: PropertyDefinition, end: PropertyDefinition | undefined): Span | undefined {
  const first = scheduledDay(record.properties[start.id], start);
  if (!first) return undefined;
  const last = end ? scheduledDay(record.properties[end.id], end) : undefined;
  return { first, last: last && last > first ? last : first };
}

/**
 * Pressing on a bar must not start a text selection or the browser's own drag of selected text,
 * which would cancel the pointer drag. Fields inside (a new record's title) keep their clicks.
 */
export function holdNativeDrag(event: ReactMouseEvent) {
  if (!(event.target as Element).closest('input, textarea, select')) event.preventDefault();
}

/** The calendar day under a point, read from the day cells' `data-day`. */
function calendarDay(x: number, y: number) {
  for (const element of document.elementsFromPoint(x, y)) {
    // A bar sits inside the cell of its first day, so the cell under the bar is what counts.
    if (element.closest('.calendar-event')) continue;
    const day = element.closest('[data-day]')?.getAttribute('data-day');
    if (day) return day;
  }
  return undefined;
}

/**
 * Dragging a bar or one of its edges with the pointer, as in Notion's calendar and timeline.
 * The bar follows the pointer; the change is saved when the button is released.
 */
function useSpanDrag(busy: boolean, records: CollectionRecord[], onMove: ScheduleProps['onMove'], onResize: ScheduleProps['onResize']) {
  const [preview, setPreview] = useState<{ id: string } & Span>();
  const active = useRef(false);
  // Saved records come back with their new dates; until then the bar stays where it was dropped.
  useEffect(() => { if (!active.current) setPreview(undefined); }, [records]);

  function begin(event: ReactPointerEvent, record: CollectionRecord, span: Span, grip: Grip, locate: (x: number, y: number) => string | undefined) {
    if (busy || event.button !== 0 || active.current || !(grip === 'move' ? onMove : onResize)) return;
    const grab = locate(event.clientX, event.clientY);
    if (!grab) return;
    if (grip !== 'move') event.preventDefault();
    event.stopPropagation();
    const pointer = event.pointerId;
    const doc = event.currentTarget.ownerDocument;
    const origin = { x: event.clientX, y: event.clientY };
    let started = grip !== 'move';
    let current = span;
    active.current = true;
    doc.getSelection()?.removeAllRanges();
    const follow = (moved: PointerEvent) => {
      if (moved.pointerId !== pointer) return;
      if (!started && Math.hypot(moved.clientX - origin.x, moved.clientY - origin.y) < 4) return;
      started = true;
      moved.preventDefault();
      const day = locate(moved.clientX, moved.clientY);
      if (!day) return;
      const shift = distance(grab, day);
      current = grip === 'move' ? { first: addDays(span.first, shift), last: addDays(span.last, shift) }
        : grip === 'start' ? { first: day < span.last ? day : span.last, last: span.last }
          : { first: span.first, last: day > span.first ? day : span.first };
      setPreview({ id: record.id, ...current });
    };
    const finish = (released: PointerEvent) => {
      if (released.pointerId !== pointer) return;
      doc.removeEventListener('pointermove', follow, true);
      doc.removeEventListener('pointerup', finish, true);
      doc.removeEventListener('pointercancel', finish, true);
      active.current = false;
      if (released.type === 'pointerup') follow(released);
      if (!started || released.type === 'pointercancel') {
        setPreview(undefined);
        return;
      }
      // The click that ends a drag must not open the record.
      const swallow = (click: MouseEvent) => { click.stopPropagation(); click.preventDefault(); };
      doc.addEventListener('click', swallow, { capture: true, once: true });
      setTimeout(() => doc.removeEventListener('click', swallow, { capture: true }), 0);
      if (grip === 'move' && current.first !== span.first) onMove?.(record, current.first);
      else if (grip === 'start' && current.first !== span.first) onResize?.(record, 'start', current.first);
      else if (grip === 'end' && current.last !== span.last) onResize?.(record, 'end', current.last);
      else setPreview(undefined);
    };
    doc.addEventListener('pointermove', follow, true);
    doc.addEventListener('pointerup', finish, true);
    doc.addEventListener('pointercancel', finish, true);
  }
  return { preview, begin };
}

export function ScheduleRecords({ schedule, records, busy, render, onMove, onResize, onCreate }: ScheduleProps) {
  const { start, end, from, through, calendar, nav } = schedule;
  const { preview, begin } = useSpanDrag(busy, records, onMove, onResize);
  if (!start || schedule.range?.mode !== 'between') return null;
  const days = Array.from({ length: distance(from, through) + 1 }, (_, index) => addDays(from, index));
  const spans = new Map<string, Span>();
  for (const record of records) {
    const span = preview?.id === record.id ? preview : spanOf(record, start, end);
    if (span) spans.set(record.id, span);
  }
  // Without an end property, dragging an edge first adds one (see CollectionPane), so edges show anyway.
  const resizable = Boolean(onResize) && !busy;
  const edges = (record: CollectionRecord, span: Span, locate: (x: number, y: number) => string | undefined, shown: { start: boolean; end: boolean }) => resizable && <>
    {shown.start && <span className="schedule-edge start" aria-hidden title="拖动调整开始日期"
      onPointerDown={(event) => begin(event, record, span, 'start', locate)} />}
    {shown.end && <span className="schedule-edge end" aria-hidden title="拖动调整结束日期"
      onPointerDown={(event) => begin(event, record, span, 'end', locate)} />}
  </>;
  if (calendar) {
    const weeks = Array.from({ length: Math.ceil(days.length / 7) }, (_, index) => days.slice(index * 7, index * 7 + 7));
    return <div className="schedule-scroll" role="region" aria-label="日历" tabIndex={0}>
      <div className="month-grid">
        {['一', '二', '三', '四', '五', '六', '日'].map((day) => <div key={day} className="month-weekday">周{day}</div>)}
        {Array.from({ length: mondayIndex(from) }, (_, index) => <div key={index} className="month-day outside" aria-hidden />)}
        {weeks.flatMap((week) => {
          const monday = week[0];
          const sunday = week[week.length - 1];
          // Each record shows once a week, as a bar from its first to its last day in that week,
          // in the first free lane, longer bars first.
          const segments = records.flatMap((record) => {
            const span = spans.get(record.id);
            if (!span || span.last < monday || span.first > sunday) return [];
            const first = span.first < monday ? monday : span.first;
            const last = span.last > sunday ? sunday : span.last;
            return [{ record, span, first, length: distance(first, last) + 1, column: distance(monday, first) }];
          }).sort((left, right) => left.first.localeCompare(right.first) || right.length - left.length);
          const lanes: number[] = [];
          const placed = segments.map((segment) => {
            let lane = lanes.findIndex((free) => free <= segment.column);
            if (lane < 0) lane = lanes.push(0) - 1;
            lanes[lane] = segment.column + segment.length;
            return { ...segment, lane };
          });
          return week.map((day) => <section key={day} aria-label={day} data-day={day}
            className={'month-day' + (day.slice(0, 7) !== nav.day.slice(0, 7) ? ' outside' : '')}
            style={{ '--lanes': lanes.length } as CSSProperties}>
            <div className="month-day-head">
              {onCreate && <button type="button" className="calendar-add" disabled={busy} aria-label={`在 ${day} 新建记录`}
                title="新建记录" onClick={() => onCreate(day)}><Plus size={14} aria-hidden /></button>}
              <time dateTime={day} data-today={day === schedule.currentDay || undefined}>
                {day.endsWith('-01') ? Number(day.slice(5, 7)) + ' 月 1 日' : Number(day.slice(8, 10))}</time>
            </div>
            {placed.filter((segment) => segment.first === day).map(({ record, span, length, lane }) => <div
              className="calendar-event" key={record.id} data-dragging={preview?.id === record.id || undefined}
              data-continues={[span.first < monday && 'before', span.last > sunday && 'after'].filter(Boolean).join(' ') || undefined}
              style={{ '--span': length, '--lane': lane } as CSSProperties} onMouseDown={holdNativeDrag} onDragStart={holdNativeDrag}
              onPointerDown={(event) => begin(event, record, span, 'move', calendarDay)}>
              {start.type === 'dateTime' && span.first >= monday && <span className="calendar-event-time">{utcTime(record, start)}</span>}
              {render(record)}
              {edges(record, span, calendarDay, { start: span.first >= monday, end: span.last <= sunday })}
            </div>)}
          </section>);
        })}
      </div>
    </div>;
  }
  if (!end) return null;
  const trackDay = (track: Element) => (x: number) => {
    const rect = track.getBoundingClientRect();
    return days[Math.min(days.length - 1, Math.max(0, Math.floor((x - rect.left) * days.length / rect.width)))];
  };
  return <div className="schedule-scroll" role="region" aria-label="时间线" tabIndex={0}>
    <div className="timeline-grid" style={{ '--days': days.length } as CSSProperties}>
      <div className="timeline-heading"><span>记录</span><div className="timeline-track">
        {days.map((day) => <time dateTime={day} title={day} key={day}>{day.slice(5).replace('-', '/')}</time>)}
      </div></div>
      {records.map((record) => {
        const span = spans.get(record.id);
        if (!span || span.last < from || span.first > through) return null;
        const left = Math.max(0, distance(from, span.first));
        const width = Math.min(days.length - 1, distance(from, span.last)) - left + 1;
        const periodLabel = span.first + ' 至 ' + span.last;
        const locate = (event: ReactPointerEvent) => {
          const track = event.currentTarget.closest('.timeline-track');
          return track ? trackDay(track) : () => undefined;
        };
        return <div className="timeline-row" key={record.id}>
          <div className="timeline-record">{render(record)}</div>
          <div className="timeline-track">
            <div className="timeline-bar" role="img" aria-label={periodLabel} title={periodLabel}
              data-dragging={preview?.id === record.id || undefined} data-movable={Boolean(onMove) && !busy || undefined}
              style={{ gridColumn: (left + 1) + ' / span ' + width }} onMouseDown={holdNativeDrag} onDragStart={holdNativeDrag}
              onPointerDown={(event) => begin(event, record, span, 'move', locate(event))}>
              {resizable && <>
                {span.first >= from && <span className="schedule-edge start" aria-hidden title="拖动调整开始日期"
                  onPointerDown={(event) => begin(event, record, span, 'start', locate(event))} />}
                {span.last <= through && <span className="schedule-edge end" aria-hidden title="拖动调整结束日期"
                  onPointerDown={(event) => begin(event, record, span, 'end', locate(event))} />}
              </>}
            </div>
          </div>
        </div>;
      })}
    </div>
  </div>;
}

function utcTime(record: CollectionRecord, field: PropertyDefinition) {
  return new Date(String(record.properties[field.id])).toISOString().slice(11, 16);
}
