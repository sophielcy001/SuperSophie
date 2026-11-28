// SPDX-License-Identifier: AGPL-3.0-or-later

import { isTauri } from '@tauri-apps/api/core';
import { useEffect, useId, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import type {
  PageInfo, RemoteWorkspace, SyncConflictKind, SyncOverview, SyncReport, SyncSkipReason, Workspace,
} from '../contracts';
import { errorMessage } from '../contracts';
import { desktop } from '../desktop';
import { Modal } from '../dialogs';
import './sync.css';

const date = new Intl.DateTimeFormat('zh-CN', { dateStyle: 'medium', timeStyle: 'short' });

export const skipReasons: Record<SyncSkipReason, string> = {
  ORIGINAL_MISSING: '本机缺少附件原件',
  PARENT_SKIPPED: '上级页面暂不同步',
};

export const conflictKinds: Record<SyncConflictKind, string> = {
  EDIT_EDIT: '两台设备都修改了此页。已采用服务中的版本，本机版本另存为冲突副本。',
  DELETE_EDIT: '另一台设备删除了此页，本机的修改已保留并重新上传。',
  EDIT_DELETE: '本机删除的页面在另一台设备上被修改，已恢复修改后的页面。',
  PARENT_CYCLE: '两台设备交叉移动了页面，此页已移到顶层。',
  SCHEMA_MERGED: '两台设备都修改了这个数据库的属性或视图，已合并双方的修改。',
  REJECTED: '服务中已有更新的版本，下次同步时会合并。',
};

function defaultDeviceName() {
  const agent = navigator.userAgent;
  if (agent.includes('Windows')) return 'Windows 电脑';
  if (agent.includes('Mac')) return 'Mac';
  return '我的电脑';
}

function summary(report: SyncReport) {
  const parts = [`上传 ${report.accepted} 项`, `写入 ${report.applied} 项`];
  if (report.conflicts.length) parts.push(`${report.conflicts.length} 项需要留意`);
  return `同步完成：${parts.join('，')}。`;
}

export function SyncDialog({ workspace, pages, onClose, onOpenPage, onJoined, onBeforeSync }: {
  workspace?: Workspace;
  pages: ReadonlyMap<string, PageInfo>;
  onClose(): void;
  onOpenPage(id: string): void;
  onJoined(workspace: Workspace): void;
  onBeforeSync(): Promise<void>;
}) {
  const [overview, setOverview] = useState<SyncOverview>();
  const [loadError, setLoadError] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [report, setReport] = useState<SyncReport>();
  const [server, setServer] = useState('https://');
  const [adminToken, setAdminToken] = useState('');
  const [name, setName] = useState(defaultDeviceName);
  const [confirmForget, setConfirmForget] = useState(false);
  const [remote, setRemote] = useState<RemoteWorkspace[]>();
  const serverField = useRef<HTMLInputElement>(null);
  const deviceHeading = useId();
  const workspaceHeading = useId();
  const remoteHeading = useId();
  const workspaceId = workspace?.id;

  async function load() {
    if (!workspaceId) {
      setOverview(undefined);
      return;
    }
    try {
      setOverview(await desktop.syncOverview(workspaceId));
      setLoadError('');
    } catch (cause) {
      setLoadError(errorMessage(cause));
    }
  }

  useEffect(() => {
    void load();
    if (!isTauri() || !workspaceId) return;
    let alive = true;
    const subscription = desktop.onSyncFinished((id) => {
      if (alive && id === workspaceId) void load();
    }).catch(() => undefined);
    return () => {
      alive = false;
      void subscription.then((stop) => stop?.());
    };
  }, [workspaceId]);

  async function act(job: () => Promise<void>) {
    if (busy) return;
    setBusy(true);
    setError('');
    try {
      await job();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
      await load();
    }
  }

  function register(event: FormEvent) {
    event.preventDefault();
    void act(async () => {
      await desktop.registerSyncDevice(server, adminToken, name);
      setAdminToken('');
      setRemote(undefined);
    });
  }

  function syncNow(enable: boolean) {
    if (!workspaceId) return;
    void act(async () => {
      setReport(undefined);
      await onBeforeSync();
      setReport(enable
        ? await desktop.enableWorkspaceSync(workspaceId)
        : await desktop.syncWorkspaceNow(workspaceId));
    });
  }

  const device = overview?.device ?? null;
  const status = overview?.status;
  const otherService = Boolean(device && status?.serverUrl && status.serverUrl !== device.server);

  return <Modal title="同步" onClose={onClose} busy={busy} className="sync-dialog"
    initialFocus={device ? undefined : serverField}>
    {error && <p className="dialog-error" role="alert">{error}</p>}
    {loadError && <p className="dialog-error" role="alert">{loadError}</p>}

    <section className="sync-section" aria-labelledby={deviceHeading}>
      <h3 id={deviceHeading}>本机设备</h3>
      {!overview && !loadError && workspaceId && <p>正在读取同步状态…</p>}
      {overview && !device && <form className="sync-form" onSubmit={register}>
        <p>连接自己部署的知页同步服务。管理令牌只在登记时使用，不会保存；本机收到的设备令牌保存在系统凭据中。</p>
        <label htmlFor="sync-server">服务地址</label>
        <input id="sync-server" ref={serverField} value={server} disabled={busy} inputMode="url"
          autoComplete="url" spellCheck={false} placeholder="https://sync.example.com"
          onChange={(event) => setServer(event.currentTarget.value)} />
        <label htmlFor="sync-admin">管理令牌</label>
        <input id="sync-admin" type="password" value={adminToken} disabled={busy} autoComplete="off"
          spellCheck={false} onChange={(event) => setAdminToken(event.currentTarget.value)} />
        <label htmlFor="sync-name">本机名称</label>
        <input id="sync-name" value={name} maxLength={64} disabled={busy}
          onChange={(event) => setName(event.currentTarget.value)} />
        <div className="sync-actions">
          <button type="submit" className="primary-button"
            disabled={busy || !server.trim() || !adminToken.trim() || !name.trim()}>
            {busy ? '正在登记…' : '登记本机'}
          </button>
        </div>
      </form>}
      {device && <>
        <dl className="sync-facts">
          <dt>服务</dt><dd><code>{device.server}</code></dd>
          <dt>本机名称</dt><dd>{device.deviceName}</dd>
          <dt>登记时间</dt><dd>{date.format(device.registeredAt)}</dd>
        </dl>
        {!confirmForget
          ? <div className="sync-actions"><button type="button" className="secondary-button" disabled={busy}
            onClick={() => setConfirmForget(true)}>移除本机凭据</button></div>
          : <div className="sync-confirm" role="group" aria-label="确认移除本机凭据">
            <p>移除后本机停止同步，本地资料保留。如需彻底停用旧令牌，请在同步服务中撤销此设备。</p>
            <div className="sync-actions">
              <button type="button" className="secondary-button" disabled={busy}
                onClick={() => setConfirmForget(false)}>取消</button>
              <button type="button" className="danger-button" disabled={busy} onClick={() => void act(async () => {
                await desktop.forgetSyncDevice();
                setConfirmForget(false);
                setRemote(undefined);
                setReport(undefined);
              })}>确认移除</button>
            </div>
          </div>}
      </>}
    </section>

    {workspace && status && <section className="sync-section" aria-labelledby={workspaceHeading}>
      <h3 id={workspaceHeading}>{workspace.name}</h3>
      {!device && <p>登记本机后即可同步此工作空间。</p>}
      {device && <>
        <p role="status">
          {status.enabled && !otherService ? '已启用同步。每 5 分钟自动同步一次，也可以立即同步。' : '尚未启用同步。'}
        </p>
        {otherService && <p className="sync-warning">
          此工作空间曾同步到 <code>{status.serverUrl}</code>。启用后会改为当前服务，并重新上传本机的全部页面。
        </p>}
        <dl className="sync-facts">
          <dt>上次成功</dt><dd>{status.lastSuccessAt ? date.format(status.lastSuccessAt) : '尚无'}</dd>
          <dt>待上传</dt><dd>{status.unsent} 项</dd>
          <dt>已同步页面</dt><dd>{status.tracked} 项</dd>
        </dl>
        {status.lastError && <p className="dialog-error" role="alert">
          上次同步失败{status.lastAttemptAt ? `（${date.format(status.lastAttemptAt)}）` : ''}：{status.lastError}
        </p>}
        <div className="sync-actions">
          {status.enabled && !otherService
            ? <>
              <button type="button" className="secondary-button" disabled={busy}
                onClick={() => void act(async () => { await desktop.disableWorkspaceSync(workspace.id); })}>停用同步</button>
              <button type="button" className="primary-button" disabled={busy} onClick={() => syncNow(false)}>
                {busy ? '正在同步…' : '立即同步'}</button>
            </>
            : <button type="button" className="primary-button" disabled={busy} onClick={() => syncNow(true)}>
              {busy ? '正在同步…' : '启用同步'}</button>}
        </div>
        {report && <div className="sync-report" role="status">
          <p>{summary(report)}</p>
          {report.conflicts.length > 0 && <ul className="sync-list">
            {report.conflicts.map((conflict) => <li key={`${conflict.kind}/${conflict.pageId}`}>
              <span><strong>{pages.get(conflict.pageId)?.title ?? '页面'}</strong>{conflictKinds[conflict.kind]}</span>
              <span className="sync-row-actions">
                {pages.has(conflict.pageId) && <button type="button" className="secondary-button"
                  onClick={() => onOpenPage(conflict.pageId)}>打开</button>}
                {conflict.copyPageId && pages.has(conflict.copyPageId) && <button type="button"
                  className="secondary-button" onClick={() => onOpenPage(conflict.copyPageId!)}>打开副本</button>}
              </span>
            </li>)}
          </ul>}
        </div>}
        {status.pending.length > 0 && <details className="sync-details">
          <summary>{status.pending.length} 项远端变化等待写入</summary>
          <ul className="sync-list">{status.pending.map((item) =>
            <li key={item.objectId}><span>{pages.get(item.objectId)?.title ?? item.objectId}：{item.reason}</span></li>)}</ul>
        </details>}
        {status.skipped.length > 0 && <details className="sync-details">
          <summary>{status.skipped.length} 个页面暂不同步</summary>
          <p>缺少附件原件的页面及其子页在原件恢复后同步。</p>
          <ul className="sync-list">{status.skipped.map((item) =>
            <li key={item.pageId}><span>{item.title}</span><small>{skipReasons[item.reason]}</small></li>)}</ul>
        </details>}
      </>}
    </section>}

    {device && <section className="sync-section" aria-labelledby={remoteHeading}>
      <div className="sync-heading">
        <h3 id={remoteHeading}>服务中的工作空间</h3>
        <button type="button" className="secondary-button" disabled={busy} onClick={() => void act(async () => {
          setRemote(await desktop.remoteWorkspaces());
        })}>{remote ? '刷新' : '查看'}</button>
      </div>
      {!remote && <p>从其他设备同步上来的工作空间可以在这里加入本机。</p>}
      {remote && remote.length === 0 && <p>服务中还没有工作空间。</p>}
      {remote && remote.length > 0 && <ul className="sync-list">
        {remote.map((item) => <li key={item.id}>
          <span>{item.name}</span>
          {item.local
            ? <small>已在本机</small>
            : <button type="button" className="secondary-button" disabled={busy} onClick={() => void act(async () => {
              const joined = await desktop.joinRemoteWorkspace(item.id, item.name);
              onJoined(joined);
            })}>加入本机</button>}
        </li>)}
      </ul>}
    </section>}
  </Modal>;
}
