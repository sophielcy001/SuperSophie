// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from 'react';
import { FieldValue } from '../collections/fields';
import type { ProposalAction, ProposalReview, ProposalSnapshot, ProposalStatus, ProposalSummary } from '../contracts';
import { errorMessage } from '../contracts';
import { desktop } from '../desktop';
import { Modal } from '../dialogs';
import './integrations.css';

const dateFormat = new Intl.DateTimeFormat('zh-CN', { dateStyle: 'medium', timeStyle: 'short' });
const statusNames: Record<ProposalStatus, string> = {
  pending: '待审阅', conflict: '需重新对比', accepted: '已接受', rejected: '已拒绝', expired: '已过期',
};
const conflictMessages = {
  revision_changed: '提交建议后，原页或其授权表示已有更新。请按当前版本重新预览。',
  authorization_changed: '提交者已失去必要的授权。',
  candidate_changed: '候选操作使用的资源或属性已变化。请重新预览并核对。',
};

export function ProposalDialog({ workspace, pageId, onClose, onAction }: {
  workspace: string;
  pageId: string | null;
  onClose(): void;
  onAction(review: ProposalReview, action: ProposalAction): Promise<ProposalReview>;
}) {
  const [items, setItems] = useState<ProposalSummary[]>();
  const [review, setReview] = useState<ProposalReview>();
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let active = true;
    void desktop.proposals(workspace, pageId).then((result) => {
      if (active) setItems(result);
    }).catch((reason: unknown) => {
      if (active) setError(errorMessage(reason));
    });
    return () => { active = false; };
  }, [workspace, pageId, attempt]);

  async function open(id: string) {
    setError('');
    setBusy(true);
    try {
      setReview(await desktop.proposal(workspace, id));
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  async function resolve(action: ProposalAction) {
    if (!review) return;
    setError('');
    setBusy(true);
    try {
      const result = await onAction(review, action);
      setReview(result);
      setItems((previous) => previous?.map((item) => item.id === result.summary.id ? result.summary : item));
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  if (!review) return <Modal title="修改建议" onClose={onClose} busy={busy} className="integrations-dialog">
    {error && <p className="dialog-error" role="alert">{error}</p>}
    <div className="proposal-list-header">
      <span>{pageId ? '当前页面' : '当前工作空间'}</span>
      <button type="button" className="secondary-button" disabled={busy} onClick={() => {
        setError('');
        setAttempt((previous) => previous + 1);
      }}>刷新列表</button>
    </div>
    {!items && !error && <p className="dialog-copy" role="status">正在读取建议…</p>}
    {items?.length === 0 && <p className="dialog-empty">暂无修改建议。</p>}
    {items && items.length > 0 && <ul className="dialog-list proposal-list">
      {items.map((item) => <li className="dialog-list-row" key={item.id}>
        <span><strong>{item.pageTitle}</strong>
          <small>{item.integrationName} · {dateFormat.format(item.createdAt)} · {statusNames[item.status]}</small>
        </span>
        <button type="button" className="secondary-button" disabled={busy} onClick={() => { void open(item.id); }}>
          查看建议
        </button>
      </li>)}
    </ul>}
  </Modal>;

  const { summary, base, candidate, current, currentRevision, fields } = review;
  const reviewable = summary.status === 'pending' || summary.status === 'conflict';
  const result = summary.acceptedRevision ? ` · 已写入修订 ${summary.acceptedRevision}` : '';
  return <Modal title="审阅修改建议" onClose={onClose} busy={busy} className="proposal-dialog">
    <section className="proposal-review">
      <div className="integration-heading">
        <h3>{summary.pageTitle}</h3>
        <button type="button" className="secondary-button" disabled={busy} onClick={() => {
          setReview(undefined);
          setError('');
        }}>返回列表</button>
      </div>
      <p>{summary.integrationName} · {dateFormat.format(summary.createdAt)}</p>
      <p role="status">{statusNames[summary.status]}{result}</p>
      {error && <p className="dialog-error" role="alert">{error}</p>}
      {summary.conflict && reviewable && <p className="dialog-error" role="alert">{conflictMessages[summary.conflict]}</p>}
      {current && reviewable && <details>
        <summary>当前页面 · 修订 {currentRevision}</summary>
        <pre className="proposal-body">{current.markdown}</pre>
      </details>}
      <div className="proposal-comparison">
        <Snapshot label={`原文 · 修订 ${summary.baseRevision}`} value={base} />
        <Snapshot label="候选内容" value={candidate} />
      </div>
      {base.icon !== candidate.icon && <p>图标 <code>{base.icon}</code> → <code>{candidate.icon}</code></p>}
      <PropertyChanges base={base} candidate={candidate} fields={fields} />
      {reviewable && <footer className="dialog-actions proposal-actions">
        <button type="button" className="secondary-button" disabled={busy} onClick={() => { void resolve('reject'); }}>拒绝建议</button>
        {summary.status === 'pending' && <button type="button" className="primary-button" disabled={busy}
          onClick={() => { void resolve('accept'); }}>接受建议</button>}
        {summary.status === 'conflict' && summary.conflict !== 'authorization_changed' &&
          <button type="button" className="primary-button" disabled={busy} onClick={() => { void resolve('rebase'); }}>按当前版本重新预览</button>}
      </footer>}
    </section>
  </Modal>;
}

function Snapshot({ label, value }: { label: string; value: ProposalSnapshot }) {
  const [expanded, setExpanded] = useState(false);
  return <section className="proposal-snapshot" aria-label={label}>
    <h4>{label}</h4>
    <pre className="proposal-body">{value.markdown}</pre>
    <details onToggle={(event) => setExpanded(event.currentTarget.open)}>
      <summary>结构化正文</summary>
      {expanded && <pre className="proposal-body">{JSON.stringify(value.blocks, null, 2)}</pre>}
    </details>
  </section>;
}

function PropertyChanges({ base, candidate, fields }: {
  base: ProposalSnapshot; candidate: ProposalSnapshot; fields: ProposalReview['fields'];
}) {
  const keys = new Set([...Object.keys(base.properties ?? {}), ...Object.keys(candidate.properties ?? {})]);
  const changes = [...keys].flatMap((key) => {
    const original = base.properties?.[key];
    const proposed = candidate.properties?.[key];
    if (JSON.stringify(original) === JSON.stringify(proposed)) return [];
    return [{ key, original, proposed, field: fields.find((field) => field.id === key) }];
  });
  return changes.length > 0 && <table className="proposal-properties">
    <caption>属性差异</caption>
    <thead><tr><th scope="col">属性</th><th scope="col">原值</th><th scope="col">候选值</th></tr></thead>
    <tbody>{changes.map(({ key, original, proposed, field }) => <tr key={key}>
      <th scope="row">{field?.name ?? key}</th>
      <td>{field ? <FieldValue field={field} value={original} /> : JSON.stringify(original) ?? '—'}</td>
      <td>{field ? <FieldValue field={field} value={proposed} /> : JSON.stringify(proposed) ?? '—'}</td>
    </tr>)}</tbody>
  </table>;
}
