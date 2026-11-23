// SPDX-License-Identifier: AGPL-3.0-or-later

import { ChevronLeft, ChevronRight } from 'lucide-react';
import { useId, useRef, useState } from 'react';
import type { CSSProperties, DragEvent, ReactNode } from 'react';
import type { CollectionRecord, PropertyDefinition, TimelineZoom } from '../contracts';
import { addDays, distance, mondayIndex, scheduledDay, zoomOptions } from './viewDates';
import type { ViewSchedule } from './viewDates';

export function ScheduleToolbar({ schedule, busy, onNavigate }: {
  schedule: ViewSchedule; busy: boolean; onNavigate(): void;
}) {
  const id = useId();
  const { nav, calendar, timed, currentDay, from, through, start, end } = schedule;
  if (!schedule.range) return null;
  const move = (action: () => void) => { onNavigate(); action(); };
  const dates = calendar ? nav.day.slice(0, 4) + ' 年 ' + Number(nav.day.slice(5, 7)) + ' 月'
    : from + ' 至 ' + through;
  if (!start) return <p className="collection-empty">
    {calendar ? '请先添加日期或带时区的日期时间属性。' : '请添加两项同类型的日期属性，用于起止日期。'}
  </p>;
  return <div className="schedule-toolbar">
    <div className="schedule-period" aria-label="日期导航">
      <button type="button" className="icon-button" aria-label="上个日期区间"
        disabled={busy || nav.unscheduled || nav.day <= '0001-01-31'} onClick={() => move(() => schedule.step(-1))}><ChevronLeft size={15} /></button>
      <strong aria-live="polite">{dates}</strong>
      <button type="button" className="icon-button" aria-label="下个日期区间"
        disabled={busy || nav.unscheduled || nav.day >= '9999-12-01'} onClick={() => move(() => schedule.step(1))}><ChevronRight size={15} /></button>
      <button type="button" className="secondary-button" disabled={busy} onClick={() => move(() => schedule.setDay(currentDay))}>今天</button>
    </div>
    <div className="schedule-options">
      <label htmlFor={id + '-go'} className="visually-hidden">跳转日期</label>
      <input id={id + '-go'} type="date" min="0001-01-01" max="9999-12-31" value={nav.day}
        disabled={busy} onChange={(event) => {
          const day = event.currentTarget.value;
          if (day >= '0001-01-01' && day <= '9999-12-31' && day.length === 10) move(() => schedule.setDay(day));
        }} />
      {!calendar && <select aria-label="时间线缩放" value={nav.zoom} disabled={busy || nav.unscheduled}
        onChange={(event) => { const zoom = event.currentTarget.value as TimelineZoom; move(() => schedule.setZoom(zoom)); }}>
        {Object.entries(zoomOptions).map(([value, { label }]) => <option key={value} value={value}>{label}</option>)}
      </select>}
      <button type="button" className="collection-tab" aria-pressed={nav.unscheduled} disabled={busy}
        onClick={() => move(schedule.toggleUnscheduled)}>未排期</button>
    </div>
    <p className="schedule-field-label">{end ? start.name + ' → ' + end.name : start.name}{timed && ' · UTC'}</p>
  </div>;
}

interface ScheduleProps {
  schedule: ViewSchedule;
  records: CollectionRecord[];
  busy: boolean;
  render(record: CollectionRecord): ReactNode;
  onMove?(record: CollectionRecord, day: string): void;
}

export function ScheduleRecords({ schedule, records, busy, render, onMove }: ScheduleProps) {
  const { start, end, from, through, calendar, nav } = schedule;
  const drag = useRef<{ record: CollectionRecord; grab: number }>(null);
  const [over, setOver] = useState<string>();
  if (!start || schedule.range?.mode !== 'between') return null;
  const days = Array.from({ length: distance(from, through) + 1 }, (_, index) => addDays(from, index));
  const clear = () => { drag.current = null; setOver(undefined); };
  const startDrag = (event: DragEvent, record: CollectionRecord, grab: number) => {
    if (!onMove || busy) return;
    drag.current = { record, grab };
    event.dataTransfer.effectAllowed = 'move';
    event.dataTransfer.setData('text/plain', record.title);
    event.stopPropagation();
  };
  const dragOver = (event: DragEvent, day: string) => {
    if (!drag.current || busy) return;
    event.preventDefault();
    event.stopPropagation();
    event.dataTransfer.dropEffect = 'move';
    setOver(day);
  };
  const drop = (event: DragEvent, day: string) => {
    if (!drag.current || !onMove || busy) return;
    event.preventDefault();
    event.stopPropagation();
    const { record, grab } = drag.current;
    clear();
    onMove(record, addDays(day, -grab));
  };
  if (calendar) {
    const byDay = new Map<string, CollectionRecord[]>();
    for (const record of records) {
      const day = scheduledDay(record.properties[start.id], start);
      if (!day) continue;
      const dated = byDay.get(day) ?? [];
      dated.push(record);
      byDay.set(day, dated);
    }
    return <div className="schedule-scroll" role="region" aria-label="日历" tabIndex={0}>
      <div className="month-grid">
        {['一', '二', '三', '四', '五', '六', '日'].map((day) => <div key={day} className="month-weekday">周{day}</div>)}
        {Array.from({ length: mondayIndex(from) }, (_, index) => <div key={index} className="month-day outside" aria-hidden />)}
        {days.map((day) => <section key={day} aria-label={day}
          className={'month-day' + (day.slice(0, 7) !== nav.day.slice(0, 7) ? ' outside' : '')}
          data-drop-active={over === day || undefined}
          onDragOver={(event) => dragOver(event, day)} onDrop={(event) => drop(event, day)}>
          <time dateTime={day} data-today={day === schedule.currentDay || undefined}>{Number(day.slice(8, 10))}</time>
          {byDay.get(day)?.map((record) => <div className="calendar-event" key={record.id} draggable={Boolean(onMove) && !busy}
            onDragStart={(event) => startDrag(event, record, 0)} onDragEnd={clear}>
            {start.type === 'dateTime' && <span className="calendar-event-time">{utcTime(record, start)}</span>}
            {render(record)}
          </div>)}
        </section>)}
      </div>
    </div>;
  }
  if (!end) return null;
  const position = (event: DragEvent, track: HTMLElement) => {
    const rect = track.getBoundingClientRect();
    return Math.min(days.length - 1, Math.max(0, Math.floor((event.clientX - rect.left) * days.length / rect.width)));
  };
  return <div className="schedule-scroll" role="region" aria-label="时间线" tabIndex={0}>
    <div className="timeline-grid" style={{ '--days': days.length } as CSSProperties}>
      <div className="timeline-heading"><span>记录</span><div className="timeline-track">
        {days.map((day) => <time dateTime={day} title={day} key={day}>{day.slice(5).replace('-', '/')}</time>)}
      </div></div>
      {records.map((record) => {
        const first = scheduledDay(record.properties[start.id], start);
        const last = scheduledDay(record.properties[end.id], end);
        if (!first || !last) return null;
        const offset = distance(from, first);
        const left = Math.max(0, offset);
        const width = Math.min(days.length - 1, distance(from, last)) - left + 1;
        if (width < 1) return null;
        const periodLabel = String(record.properties[start.id]) + ' 至 ' + String(record.properties[end.id]);
        return <div className="timeline-row" key={record.id}>
          <div className="timeline-record">{render(record)}</div>
          <div className="timeline-track" data-drop-active={over?.startsWith(record.id + '/') || undefined}
            onDragOver={(event) => dragOver(event, record.id + '/' + days[position(event, event.currentTarget)])}
            onDrop={(event) => drop(event, days[position(event, event.currentTarget)])}>
            <div className="timeline-bar" role="img" aria-label={periodLabel} title={periodLabel}
              style={{ gridColumn: (left + 1) + ' / span ' + width }}
              draggable={Boolean(onMove) && !busy} onDragEnd={clear}
              onDragStart={(event) => {
                const track = event.currentTarget.parentElement;
                if (track) startDrag(event, record, position(event, track) - offset);
              }} />
          </div>
        </div>;
      })}
    </div>
  </div>;
}

function utcTime(record: CollectionRecord, field: PropertyDefinition) {
  return new Date(String(record.properties[field.id])).toISOString().slice(11, 16);
}
