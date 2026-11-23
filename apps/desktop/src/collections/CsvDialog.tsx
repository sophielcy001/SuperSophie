// SPDX-License-Identifier: AGPL-3.0-or-later

import { useRef, useState } from 'react';
import type { CollectionView, CsvExport } from '../contracts';
import { Modal } from '../dialogs';
import { viewLabels } from './viewDates';

export function CsvDialog({ view, busy, result, onExport, onClose }: {
  view: CollectionView; busy: boolean; result?: CsvExport;
  onExport(viewId: string | null): void; onClose(): void;
}) {
  const [scope, setScope] = useState('database');
  const select = useRef<HTMLSelectElement>(null);
  return <Modal title={result ? 'CSV 已导出' : '导出 CSV'} initialFocus={result ? undefined : select} onClose={onClose} busy={busy}>
    {result ? <>
      <div className="dialog-copy csv-summary" role="status">
        <p>已导出 {result.rows} 条记录{result.attachments ? `和 ${result.attachments} 份附件原件` : ''}。</p>
        <p>导出报告位于同目录的「{result.companionFolder}」文件夹。</p>
        <ul>{result.limitations.map((note) => <li key={note}>{note}</li>)}</ul>
      </div>
      <footer className="dialog-actions dialog-pad"><button type="button" className="primary-button" disabled={busy} onClick={onClose}>完成</button></footer>
    </> : <form className="dialog-form" onSubmit={(event) => {
      event.preventDefault();
      if (!busy) onExport(scope === 'view' ? view.id : null);
    }}>
      <label htmlFor="csv-export-scope">导出范围</label>
      <select id="csv-export-scope" ref={select} value={scope} disabled={busy} onChange={(event) => setScope(event.currentTarget.value)}>
        <option value="database">整个数据库</option>
        <option value="view">当前{view.name ?? viewLabels[view.type]}视图</option>
      </select>
      <p className="library-hint">{scope === 'view'
        ? '按当前视图的筛选和排序导出全部匹配记录。'
        : '导出数据库中的全部有效记录。'}</p>
      <footer className="dialog-actions">
        <button type="button" className="secondary-button" disabled={busy} onClick={onClose}>取消</button>
        <button type="submit" className="primary-button" disabled={busy}>选择保存位置</button>
      </footer>
    </form>}
  </Modal>;
}
