// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useMemo, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import type {
  CollectionSummary, Integration, IntegrationGrant, IntegrationScope, LocalApiStatus, PageInfo, Workspace,
} from '../contracts';
import { errorMessage } from '../contracts';
import { desktop } from '../desktop';
import { Modal } from '../dialogs';
import { pageTrail } from '../PageTree';
import './integrations.css';

const grantNames = { page: '单页', subtree: '页面子树', collection: '整个数据库' };
const scopeNames: Record<IntegrationScope, string> = {
  'content:read': '读取内容',
  'content:create': '创建页面或记录',
  'content:update': '修改页面、属性和正文',
  'content:archive': '将页面移入回收站',
  'assets:read': '下载附件',
  'assets:write': '上传附件',
  'changes:read': '读取授权资源的增量变化',
  'proposals:create': '提交修改建议',
};
const optionalScopes = [
  'content:create', 'content:update', 'content:archive', 'assets:read', 'assets:write',
  'changes:read', 'proposals:create',
] as const;
const dateFormat = new Intl.DateTimeFormat('zh-CN', { dateStyle: 'medium', timeStyle: 'short' });
const expiryOptions = [[0, '手动撤销'], [1, '1 小时'], [24, '1 天'], [168, '7 天'], [720, '30 天']] as const;

interface Settings {
  integrations: Integration[];
  api: LocalApiStatus;
  pages: PageInfo[];
  collections: CollectionSummary[];
  uploadLimit: number;
}

export function IntegrationsDialog({ workspace, onClose, onProposals }: {
  workspace: Workspace; onClose(): void; onProposals(): void;
}) {
  const [settings, setSettings] = useState<Settings>();
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [loadAttempt, setLoadAttempt] = useState(0);
  const [creating, setCreating] = useState(false);
  const [revokeTarget, setRevokeTarget] = useState<Integration>();
  const [token, setToken] = useState<string>();
  const [name, setName] = useState('');
  const [validHours, setValidHours] = useState(0);
  const [observedAt, setObservedAt] = useState(Date.now);
  const [kind, setKind] = useState<IntegrationGrant['kind']>('page');
  const [resource, setResource] = useState('');
  const [grants, setGrants] = useState<IntegrationGrant[]>([]);
  const [scopes, setScopes] = useState<IntegrationScope[]>([]);
  const [limitInput, setLimitInput] = useState('');
  const tokenField = useRef<HTMLInputElement>(null);
  const nameField = useRef<HTMLInputElement>(null);

  useEffect(() => {
    let alive = true;
    Promise.all([
      desktop.integrations(workspace.id), desktop.localApiStatus(),
      desktop.pages(workspace.id), desktop.collections(workspace.id),
      desktop.apiAttachmentLimit(workspace.id),
    ]).then(([integrations, api, pages, collections, uploadLimit]) => {
      if (alive) {
        setSettings({ integrations, api, pages, collections, uploadLimit });
        setLimitInput(String(uploadLimit));
      }
    }).catch((reason: unknown) => {
      if (alive) setError(errorMessage(reason));
    });
    return () => { alive = false; };
  }, [workspace.id, loadAttempt]);

  const nextExpiry = settings?.integrations.reduce((nearest, entry) => {
    if (entry.revokedAt !== null || entry.expiresAt === null || entry.expiresAt <= observedAt) return nearest;
    return Math.min(nearest, entry.expiresAt);
  }, Infinity) ?? Infinity;
  useEffect(() => {
    if (nextExpiry === Infinity) return;
    const remaining = nextExpiry - Date.now();
    const timer = window.setTimeout(() => setObservedAt(Date.now()), Math.min(remaining, 2_147_483_647));
    return () => window.clearTimeout(timer);
  }, [nextExpiry, observedAt]);

  const locations = useMemo(() => {
    const byId = new Map(settings?.pages.map((page) => [page.id, page]));
    return (settings?.pages ?? []).filter((page) => !page.trashRootId).map((page) => ({
      id: page.id,
      label: pageTrail(byId, page.id).map((part) => part.title).join(' / '),
    }));
  }, [settings?.pages]);
  const collectionByPage = useMemo(
    () => new Map(settings?.collections.map((item) => [item.containerPageId, item.id])),
    [settings?.collections],
  );
  const pageByCollection = useMemo(
    () => new Map(settings?.collections.map((item) => [item.id, item.containerPageId])),
    [settings?.collections],
  );
  const choices = kind === 'collection'
    ? locations.flatMap(({ id, label }) => {
      const collection = collectionByPage.get(id);
      return collection ? [{ id: collection, label }] : [];
    })
    : locations;
  const canCreate = grants.some((grant) => grant.kind !== 'page');
  const alreadyAdded = grants.some((grant) => grant.kind === kind && grant.id === resource);

  function targetPage(grant: IntegrationGrant) {
    return grant.kind === 'collection' ? pageByCollection.get(grant.id) : grant.id;
  }

  function describe(grant: IntegrationGrant) {
    return locations.find((page) => page.id === targetPage(grant))?.label ?? '资源已归档或不可用';
  }

  function refreshSettings(change: (value: Settings) => Settings) {
    setSettings((current) => current && change(current));
  }

  async function perform(job: () => Promise<void>) {
    setError('');
    setBusy(true);
    try {
      await job();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  function beginCreation() {
    setCreating(true);
    setError('');
    setName('');
    setValidHours(0);
    setGrants([]);
    setScopes([]);
    setKind('page');
    setResource('');
    requestAnimationFrame(() => nameField.current?.focus());
  }

  function issue(event: FormEvent) {
    event.preventDefault();
    if (!name.trim() || !grants.length) return;
    void perform(async () => {
      const expiresAt = validHours === 0 ? null : Date.now() + validHours * 3_600_000;
      const issued = await desktop.createIntegration(workspace.id, {
        name: name.trim(),
        scopes: ['content:read', ...scopes.filter(canGrantScope)],
        grants,
      }, expiresAt);
      refreshSettings((current) => ({ ...current, integrations: [issued.integration, ...current.integrations] }));
      setCreating(false);
      setToken(issued.token);
      requestAnimationFrame(() => tokenField.current?.focus());
    });
  }

  function canGrantScope(scope: IntegrationScope) {
    if (scope === 'content:create') return canCreate;
    if (scope === 'assets:write') return scopes.includes('content:update');
    return true;
  }

  const enteredLimit = Number(limitInput);
  const validLimit = Number.isInteger(enteredLimit) && enteredLimit >= 1 && enteredLimit <= 100;

  function saveLimit(event: FormEvent) {
    event.preventDefault();
    if (!validLimit) return;
    void perform(async () => {
      await desktop.setApiAttachmentLimit(workspace.id, enteredLimit);
      refreshSettings((current) => ({ ...current, uploadLimit: enteredLimit }));
    });
  }

  function updateApi(enabled: boolean) {
    void perform(async () => {
      const api = await desktop.setLocalApi(enabled);
      refreshSettings((current) => ({ ...current, api }));
    });
  }

  function revoke(integration: Integration) {
    void perform(async () => {
      await desktop.revokeIntegration(workspace.id, integration.id);
      setRevokeTarget(undefined);
      const integrations = await desktop.integrations(workspace.id);
      refreshSettings((current) => ({ ...current, integrations }));
    });
  }

  return <Modal title="集成与 API" onClose={onClose} busy={busy} className="integrations-dialog">
    {error && <p className="dialog-error" role="alert">{error}</p>}
    {!settings && (error
      ? <div className="dialog-actions dialog-pad"><button type="button" className="secondary-button" onClick={() => {
        setError(''); setLoadAttempt((attempt) => attempt + 1);
      }}>重试</button></div>
      : <p className="dialog-copy" role="status">正在读取集成设置…</p>)}

    <section className="integration-section integration-heading">
      <h3>集成提交的修改建议</h3>
      <button type="button" className="secondary-button" disabled={busy} onClick={onProposals}>审阅修改建议</button>
    </section>

    {settings && <section className="integration-section" aria-label="本机 API">
      <div className="integration-heading">
        <div><h3>本机 API</h3><p role="status">{settings.api.address ? '运行中' : '已关闭'}</p></div>
        <button type="button" className="secondary-button" disabled={busy}
          onClick={() => updateApi(!settings.api.address)}>{settings.api.address ? '关闭本机 API' : '启用本机 API'}</button>
      </div>
      {settings.api.address && <dl className="integration-endpoint">
        <dt>「{workspace.name}」请求前缀</dt>
        <dd><code>{settings.api.address}/api/v1/workspaces/{workspace.id}</code></dd>
      </dl>}
      <form className="integration-limit" onSubmit={saveLimit}>
        <label htmlFor="attachment-limit">单个上传附件上限（MiB）</label>
        <input id="attachment-limit" type="number" min={1} max={100} value={limitInput}
          disabled={busy} onChange={(event) => setLimitInput(event.currentTarget.value)} />
        <button type="submit" className="secondary-button" disabled={busy || !validLimit || enteredLimit === settings.uploadLimit}>保存上限</button>
      </form>
    </section>}

    {token && <section className="integration-section integration-token" aria-label="新集成令牌">
      <label htmlFor="integration-token">新集成令牌</label>
      <p>令牌仅显示一次。请将它保存到客户端的安全配置中。</p>
      <input id="integration-token" aria-label="新集成令牌" ref={tokenField} value={token} readOnly
        spellCheck={false} autoComplete="off" onFocus={(event) => event.currentTarget.select()} />
      <div className="dialog-actions"><button type="button" className="primary-button" onClick={() => setToken(undefined)}>完成</button></div>
    </section>}

    {creating && <form className="dialog-form integration-section" onSubmit={issue} aria-label="新建集成">
      <label htmlFor="integration-name">集成名称</label>
      <input id="integration-name" ref={nameField} maxLength={64} value={name} disabled={busy}
        onChange={(event) => setName(event.currentTarget.value)} required />
      <label htmlFor="integration-expiry">令牌有效期</label>
      <select id="integration-expiry" value={validHours} disabled={busy}
        onChange={(event) => setValidHours(Number(event.currentTarget.value))}>
        {expiryOptions.map(([hours, label]) => <option key={hours} value={hours}>{label}</option>)}
      </select>
      <fieldset disabled={busy}>
        <legend>授权资源</legend>
        <div className="integration-picker">
          <select aria-label="资源范围类型" value={kind} onChange={(event) => {
            setKind(event.currentTarget.value as IntegrationGrant['kind']);
            setResource('');
          }}>
            {Object.entries(grantNames).map(([value, label]) => <option key={value} value={value}>{label}</option>)}
          </select>
          <select aria-label="选择授权资源" value={resource} onChange={(event) => setResource(event.currentTarget.value)}>
            <option value="" disabled>{choices.length ? '选择资源' : '没有可选资源'}</option>
            {choices.map((choice) => <option key={choice.id} value={choice.id}>{choice.label}</option>)}
          </select>
          <button type="button" className="secondary-button" disabled={!resource || alreadyAdded}
            onClick={() => { setGrants([...grants, { kind, id: resource }]); setResource(''); }}>加入范围</button>
        </div>
        {kind !== 'page' && <p>{kind === 'subtree' ? '包含当前及未来新增的所有子页。' : '包含数据库页面和当前及未来新增的记录。'}</p>}
        <ul className="integration-grants">
          {grants.map((grant) => <li key={`${grant.kind}/${grant.id}`}>
            <span>{grantNames[grant.kind]} · {describe(grant)}</span>
            <button type="button" className="text-button" aria-label={`移除 ${describe(grant)}`}
              onClick={() => setGrants(grants.filter((item) => item !== grant))}>移除</button>
          </li>)}
        </ul>
      </fieldset>
      <fieldset disabled={busy} className="integration-scopes">
        <legend>允许的操作</legend>
        <label><input type="checkbox" checked disabled />读取内容</label>
        {optionalScopes.map((scope) => {
          const enabled = canGrantScope(scope);
          return <label key={scope}><input type="checkbox" checked={enabled && scopes.includes(scope)} disabled={!enabled}
            onChange={(event) => setScopes(event.currentTarget.checked
              ? [...scopes, scope] : scopes.filter((item) => item !== scope))} />{scopeNames[scope]}</label>;
        })}
      </fieldset>
      <div className="dialog-actions">
        <button type="button" className="secondary-button" disabled={busy} onClick={() => { setError(''); setCreating(false); }}>取消</button>
        <button type="submit" className="primary-button" disabled={busy || !name.trim() || !grants.length}>生成令牌</button>
      </div>
    </form>}

    {settings && <section className="integration-section" aria-label="当前工作空间的集成">
      <div className="integration-heading">
        <h3>「{workspace.name}」的集成</h3>
        <button type="button" className="secondary-button" disabled={busy || creating || Boolean(token) || Boolean(revokeTarget)}
          onClick={beginCreation}>新建集成</button>
      </div>
      {settings.integrations.length === 0 && <p>此工作空间还没有集成。</p>}
      <ul className="integration-list">
        {settings.integrations.map((entry) => <li key={entry.id}>
          <div className="integration-heading">
            <div><strong>{entry.name}</strong><p>指纹 <code>{entry.fingerprint}</code> · {entry.revokedAt
              ? '已撤销' : entry.expiresAt !== null && entry.expiresAt <= observedAt ? '已过期'
                : entry.lastUsedAt ? `最近访问 ${dateFormat.format(entry.lastUsedAt)}` : '尚未访问'}</p>
              {entry.expiresAt !== null && <p>有效至 {dateFormat.format(entry.expiresAt)}</p>}</div>
            {!entry.revokedAt && <button type="button" className="secondary-button"
              aria-label={`撤销 ${entry.name}`} disabled={busy || Boolean(revokeTarget) || creating || Boolean(token)}
              onClick={() => { setError(''); setRevokeTarget(entry); }}>撤销</button>}
          </div>
          {revokeTarget?.id === entry.id && <div className="integration-revoke">
            <p>撤销后，此令牌会立即失效。</p>
            <div className="dialog-actions">
              <button type="button" className="secondary-button" disabled={busy} onClick={() => setRevokeTarget(undefined)}>取消</button>
              <button type="button" className="danger-button" disabled={busy} onClick={() => revoke(entry)}>确认撤销</button>
            </div>
          </div>}
          <p>{entry.scopes.map((scope) => scopeNames[scope]).join(' · ')}</p>
          <details><summary>授权资源与 ID</summary>
            <ul className="integration-resource-list">
              {entry.grants.map((grant) => <li key={`${grant.kind}/${grant.id}`}>
                {grantNames[grant.kind]} · {describe(grant)}
                <code>{targetPage(grant) ?? grant.id}</code>
              </li>)}
            </ul>
          </details>
        </li>)}
      </ul>
    </section>}
  </Modal>;
}
