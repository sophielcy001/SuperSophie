// SPDX-License-Identifier: AGPL-3.0-or-later

import { FolderInput, Trash2, X } from 'lucide-react';

export interface BatchSelection {
  ids: ReadonlySet<string>;
  select(ids: readonly string[], checked: boolean): void;
}

export function BatchCheckbox({ batch, id, title, disabled }: {
  batch: BatchSelection; id: string; title: string; disabled: boolean;
}) {
  return <input className="batch-checkbox" type="checkbox" aria-label={`选择「${title}」`}
    disabled={disabled} checked={batch.ids.has(id)}
    onChange={(event) => batch.select([id], event.currentTarget.checked)} />;
}

export function BatchBar({ count, canMove, disabled, onMove, onArchive, onClose }: {
  count: number; canMove: boolean; disabled: boolean;
  onMove(): void; onArchive(): void; onClose(): void;
}) {
  return <div className="batch-bar" role="group" aria-label="批量整理操作">
    <span role="status">已选 {count} 项</span>
    {count > 0 && !canMove && <span className="batch-hint">记录随所属数据库移动</span>}
    <button type="button" className="secondary-button" disabled={disabled || !canMove} onClick={onMove}>
      <FolderInput size={15} />批量移动
    </button>
    <button type="button" className="secondary-button" disabled={disabled || !count} onClick={onArchive}>
      <Trash2 size={15} />批量归档
    </button>
    <button type="button" className="icon-button" aria-label="结束选择" title="结束选择"
      disabled={disabled} onClick={onClose}><X size={17} /></button>
  </div>;
}
