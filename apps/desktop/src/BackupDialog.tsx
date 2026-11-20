// SPDX-License-Identifier: AGPL-3.0-or-later

import { ArrowDownToLine, ArrowUpFromLine, DatabaseBackup, FolderInput, FolderOpen } from 'lucide-react';
import { useEffect, useId, useRef, useState } from 'react';
import type { BackupAttempt, BackupPolicy, BackupStatus, MigrationBatch, Workspace } from './contracts';
import { errorMessage } from './contracts';
import { desktop } from './desktop';
import { Modal } from './dialogs';

type Draft = Omit<BackupPolicy, 'keepDaily' | 'keepWeekly'> & { keepDaily: string; keepWeekly: string };

const date = new Intl.DateTimeFormat('zh-CN', { dateStyle: 'medium', timeStyle: 'short' });
const mergeStatus: Record<MigrationBatch['status'], string> = {
  preparing: '正在合并', completed: '合并完成', failed: '合并失败',
  cancelled: '合并已取消', interrupted: '合并中断',
};

function displayBackupPath(path: string): string {
  // The Windows file picker expects shell paths without the extended-length prefix.
  if (!path.startsWith('\\\\?\\')) return path;
  const native = path.slice(4);
  if (native.startsWith('UNC\\')) return '\\\\' + native.slice(4);
  return /^[a-z]:\\/i.test(native) ? native : path;
}

export function BackupDialog({ busy, workspace, migration, actionError, onClose, onRecovery,
  onExport, onImport, onMerge, onCancelMerge, onRun }: {
  busy: boolean;
  workspace?: Workspace;
  migration?: 'pending' | 'cancelling';
  actionError?: string;
  onClose(): void;
  onRecovery(): void;
  onExport(includeHistory: boolean): void;
  onImport(): void;
  onMerge(): void;
  onCancelMerge(): void;
  onRun(workspace: string): Promise<BackupAttempt | null>;
}) {
  const [status, setStatus] = useState<BackupStatus>();
  const [draft, setDraft] = useState<Draft>();
  const [loadingError, setLoadingError] = useState('');
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [pending, setPending] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const [includeHistory, setIncludeHistory] = useState(true);
  const [previousMerge, setPreviousMerge] = useState<MigrationBatch | null>(null);
  const working = useRef(false);
  const headingId = useId();
  const workspaceId = workspace?.id ?? null;
  const locked = pending || busy;

  useEffect(() => {
    let disposed = false;
    let timeout: number;
    async function poll() {
      try {
        const current = await desktop.backupStatus(workspaceId);
        if (!disposed) {
          setStatus(current);
          setLoadingError('');
        }
      } catch (cause) {
        if (!disposed) setLoadingError(errorMessage(cause));
      }
      if (!disposed) timeout = window.setTimeout(poll, 2500);
    }
    void poll();
    return () => { disposed = true; window.clearTimeout(timeout); };
  }, [workspaceId, refresh]);

  useEffect(() => {
    if (!workspaceId || busy) return;
    let disposed = false;
    void desktop.latestMigration(workspaceId, true).then((batch) => {
      if (!disposed) setPreviousMerge(batch);
    }).catch(() => {
      if (!disposed) setPreviousMerge(null);
    });
    return () => { disposed = true; };
  }, [workspaceId, busy]);

  const policy = status?.policy;
  const fields = draft ?? (policy && {
    ...policy, keepDaily: String(policy.keepDaily), keepWeekly: String(policy.keepWeekly),
  });
  const latest = status?.lastAttempt;
  const saved = status?.lastSuccess;

  async function act(job: () => Promise<void>) {
    if (busy || working.current) return;
    working.current = true;
    setPending(true);
    setError('');
    setNotice('');
    try {
      await job();
      setRefresh((value) => value + 1);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      working.current = false;
      setPending(false);
    }
  }

  return <Modal title="备份与恢复" onClose={onClose} busy={locked} className="backup-dialog">
    <div className="backup-options">
      <button type="button" className="backup-option" onClick={onRecovery} disabled={locked || !workspaceId}>
        <span className="backup-icon"><DatabaseBackup size={20} /></span>
        <span><strong>创建整库恢复包</strong><small>保存数据库快照、修订、集成审计和附件原件。</small></span>
      </button>
      <button type="button" className="backup-option" onClick={onImport} disabled={locked}>
        <span className="backup-icon"><ArrowUpFromLine size={20} /></span>
        <span><strong>从备份恢复</strong><small>在新空间打开知页备份。恢复后需要重新授权集成。</small></span>
      </button>
      <button type="button" className="backup-option" onClick={() => onExport(includeHistory)} disabled={locked || !workspaceId}>
        <span className="backup-icon"><ArrowDownToLine size={20} /></span>
        <span><strong>导出工作空间</strong><small>生成可迁移页面、数据库和附件的内容包。</small></span>
      </button>
      <label className="backup-enabled backup-export-history">
        <input type="checkbox" checked={includeHistory} disabled={locked || !workspaceId}
          onChange={(event) => setIncludeHistory(event.currentTarget.checked)} />内容包包含修订历史
      </label>
      <button type="button" className="backup-option" onClick={onMerge} disabled={locked || !workspaceId}>
        <span className="backup-icon"><FolderInput size={20} /></span>
        <span><strong>合并内容包</strong><small>在当前空间建立导入分支，将包内链接指向导入的副本。</small></span>
      </button>
      {migration && <div className="backup-result">
        <p role="status">{migration === 'cancelling' ? '正在取消合并…' : '选择文件后校验内容和附件原件。'}</p>
        <div className="backup-actions"><button type="button" className="secondary-button"
          disabled={migration === 'cancelling'} onClick={onCancelMerge}>取消合并</button></div>
      </div>}
      {actionError && <p className="dialog-error" role="alert">{actionError}</p>}
      {!migration && !actionError && previousMerge && <details className="backup-result">
        <summary>最近一次{mergeStatus[previousMerge.status]} · {previousMerge.source}</summary>
        <p>{previousMerge.detail ?? '可以重新选择来源。'}</p>
      </details>}
    </div>
    <section className="backup-settings" aria-labelledby={headingId}>
      <h3 id={headingId}>自动备份</h3>
      {loadingError && <p role="alert" className="dialog-error">{loadingError}</p>}
      {!status && !loadingError && <p className="library-hint">正在读取备份状态…</p>}
      {status && fields && <>
        {workspace && <div className="backup-result">
          <h4>{workspace.name}</h4>
          {saved
            ? <details><summary>最近成功于 {date.format(saved.createdAt)}</summary><code>{displayBackupPath(saved.path)}</code></details>
            : <p>还没有成功的自动备份。</p>}
          {latest?.state === 'running' && <p role="status">正在备份…</p>}
          {status.checkError && !loadingError && <p className="dialog-error" role="alert">自动备份状态无法更新。{status.checkError}</p>}
          {latest?.state === 'failed' && <p className="dialog-error" role="alert">最近备份失败。{latest.error}<br />{displayBackupPath(latest.directory)}</p>}
          {latest?.state === 'interrupted' && <p className="dialog-error" role="alert">上一次备份中断。可以立即重试。</p>}
          {status.retentionError && <p className="dialog-error" role="alert">清理旧备份遇到问题。{status.retentionError}</p>}
        </div>}
        <form className="backup-form" onSubmit={(event) => {
          event.preventDefault();
          void act(async () => {
            const savedPolicy = await desktop.setBackupPolicy({
              ...fields, keepDaily: Number(fields.keepDaily), keepWeekly: Number(fields.keepWeekly),
            });
            setStatus((previous) => previous && {
              ...previous, policy: savedPolicy, directory: savedPolicy.directory ?? previous.defaultDirectory,
            });
            setDraft(undefined);
            setNotice('备份设置已保存。');
          });
        }}>
          <label className="backup-enabled"><input type="checkbox" checked={fields.enabled} disabled={locked}
            onChange={(event) => setDraft({ ...fields, enabled: event.currentTarget.checked })} />启用自动备份</label>
          <p className="library-hint">知页运行时按 UTC 日期检查，每个工作空间每天保存一份。</p>
          <div className="backup-destination">
            <p>保存目录 <code>{displayBackupPath(fields.directory ?? status.defaultDirectory)}</code></p>
            <div className="backup-actions">
              <button type="button" className="secondary-button" disabled={locked} onClick={() => { void act(async () => {
                const directory = await desktop.chooseBackupDirectory();
                if (directory) setDraft({ ...fields, directory });
              }); }}><FolderOpen size={15} />选择目录</button>
              <button type="button" className="secondary-button" disabled={locked || fields.directory === null}
                onClick={() => setDraft({ ...fields, directory: null })}>使用本机默认目录</button>
            </div>
          </div>
          {fields.directory === null && <p className="library-hint">默认目录与原数据共用磁盘。整盘损坏会同时影响两份数据。</p>}
          <div className="backup-retention">
            <label>每日保留份数<input type="number" min="1" max="365" step="1" required value={fields.keepDaily} disabled={locked}
              onChange={(event) => setDraft({ ...fields, keepDaily: event.currentTarget.value })} /></label>
            <label>每周保留份数<input type="number" min="0" max="52" step="1" required value={fields.keepWeekly} disabled={locked}
              onChange={(event) => setDraft({ ...fields, keepWeekly: event.currentTarget.value })} /></label>
          </div>
          <div className="backup-actions backup-submit">
            <button type="button" className="secondary-button" disabled={locked || !workspaceId || latest?.state === 'running'}
              onClick={() => { if (workspaceId) void act(async () => {
                const attempt = await onRun(workspaceId);
                if (!attempt) setNotice('今天已经有一份自动备份。');
                else if (attempt.state === 'completed') setNotice('自动备份已保存。');
              }); }}>立即运行备份</button>
            <button type="submit" className="primary-button" disabled={locked || !draft}>保存备份设置</button>
          </div>
          {error && <p className="dialog-error" role="alert">{error}</p>}
          {notice && <p className="library-hint" role="status">{notice}</p>}
        </form>
        <details className="backup-upgrade"><summary>升级前备份目录</summary><code>{displayBackupPath(status.upgradeDirectory)}</code></details>
      </>}
    </section>
  </Modal>;
}
