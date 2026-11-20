// SPDX-License-Identifier: AGPL-3.0-or-later

import { useId, useState } from 'react';
import type { RecordDraft } from '../contracts';
import { Modal } from '../dialogs';
import { mergeRecordDraft } from './recordConflict';
import type { RecordComparison, RecordVersionChoice } from './recordConflict';

export function RecordConflictDialog({ comparison, busy, error, onClose, onSave }: {
  comparison: RecordComparison; busy: boolean; error?: string; onClose(): void; onSave(draft: RecordDraft): void;
}) {
  const scope = useId();
  const [choices, setChoices] = useState<Record<string, RecordVersionChoice>>({});
  const ready = comparison.fields.every(({ field, defaultChoice }) => choices[field.id] ?? defaultChoice);
  return <Modal title="对比记录版本" className="conflict-dialog" busy={busy} onClose={onClose}>
    <form className="dialog-form" onSubmit={(event) => {
      event.preventDefault();
      if (ready && !busy) onSave(mergeRecordDraft(comparison, choices));
    }}>
      <p className="record-compare-note">已保存版本 {comparison.revision}。请检查要保留的属性。</p>
      {!comparison.fields.length && <p className="record-compare-note">表单值与已保存的属性一致。</p>}
      {comparison.fields.map(({ field, savedText, localText, defaultChoice, acceptsLocal }) => {
        const control = `${scope}-${field.id}`;
        const localLabel = field.type === 'relation' ? '应用本机增删后' : '本机草稿';
        return <fieldset key={field.id} className="record-compare-field" disabled={busy}>
          <legend>{field.name}</legend>
          <dl className="record-compare-values">
            <div><dt>已保存版本</dt><dd>{savedText}</dd></div>
            {acceptsLocal && <div><dt>{localLabel}</dt><dd>{localText}</dd></div>}
          </dl>
          {acceptsLocal && <div className="form-field">
            <label htmlFor={control}>保留{field.name}</label>
            <select id={control} value={choices[field.id] ?? defaultChoice ?? ''} onChange={(event) => {
              const next = event.currentTarget.value as RecordVersionChoice;
              setChoices((previous) => ({ ...previous, [field.id]: next }));
            }}>
              <option value="" disabled>选择要保留的值</option>
              <option value="saved">已保存版本</option>
              <option value="local">{localLabel}</option>
            </select>
          </div>}
        </fieldset>;
      })}
      {error && <p className="collection-form-error" role="alert">{error}</p>}
      <footer className="dialog-actions">
        <button type="button" className="secondary-button" disabled={busy} onClick={onClose}>返回草稿</button>
        <button type="submit" className="primary-button" disabled={busy || !ready}>保存所选属性</button>
      </footer>
    </form>
  </Modal>;
}
