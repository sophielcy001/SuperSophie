// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useId, useMemo, useRef, useState } from 'react';
import type { ContentImport, ImportOriginal, ImportOutcome, MigrationBatch, PageInfo, TemplateSummary } from '../contracts';
import { desktop } from '../desktop';
import { Modal } from '../dialogs';
import { pageTrail } from '../PageTree';

interface DialogActions { busy: boolean; onClose(): void }
interface Locations {
  pages: PageInfo[];
  collectionByPage: ReadonlyMap<string, string>;
  initialParent: string | null;
}

function useLocations(pages: PageInfo[]) {
  return useMemo(() => {
    const byId = new Map(pages.map((page) => [page.id, page]));
    return pages.filter((page) => !page.trashRootId).map((page) => ({
      id: page.id,
      label: pageTrail(byId, page.id).map((item) => item.title).join(' / '),
    }));
  }, [pages]);
}

function LocationField({ label, choices, value, allowRoot, disabled, onChange }: {
  label: string; choices: ReturnType<typeof useLocations>; value: string;
  allowRoot: boolean; disabled: boolean; onChange(id: string): void;
}) {
  const id = useId();
  return <>
    <label htmlFor={id}>{label}</label>
    <select id={id} value={value} disabled={disabled} onChange={(event) => onChange(event.currentTarget.value)}>
      {allowRoot ? <option value="">工作空间根目录</option> : <option value="" disabled>选择数据库</option>}
      {choices.map((page) => <option key={page.id} value={page.id}>{page.label}</option>)}
    </select>
  </>;
}

const importStatus = {
  imported: '已导入', duplicate: '重复', preserved: '已留存原件', unsupported: '未支持', failed: '失败',
} as const;
const migrationStatus = { interrupted: '上次迁移中断', cancelled: '上次迁移已取消', failed: '上次迁移失败' } as const;
const importLimit = 50;

function importSummary(report: ImportOutcome[], migration: boolean) {
  if (migration && report[0]?.status === 'duplicate') return '此来源已迁移到当前位置。';
  const sources = migration ? report.slice(1) : report;
  const count = { imported: 0, duplicate: 0, preserved: 0, unsupported: 0, failed: 0 };
  let unmatched = 0;
  for (const source of sources) {
    count[source.status]++;
    unmatched += source.unmatchedLinks ?? 0;
  }
  if (migration) {
    return `迁移已完成。${count.imported} 个文件已转换，${count.preserved} 个仅留存原件，${count.unsupported} 个未支持，${unmatched} 处链接失配。`;
  }
  return `${count.imported} 个文件已导入，${count.duplicate} 个重复，${count.failed} 个失败。`;
}

function reportText(report: ImportOutcome[], csv: boolean, migration: boolean) {
  return ['知页导入报告', importSummary(report, migration), ...report.map((item) => {
    const records = (csv || item.records > 0) && item.status === 'imported' ? ` · ${item.records} 条记录` : '';
    const page = item.pageId ? `页面 ID ${item.pageId}` : '';
    const original = item.original ? `原件 ${item.original.name} · 页面 ${item.original.pageId} · 附件 ${item.original.attachmentId}` : '';
    const unmatched = item.unmatchedLinks ? `${item.unmatchedLinks} 处链接失配。` : '';
    const omitted = item.notesOmitted ? `另有 ${item.notesOmitted} 条提示，详情可查原件。` : '';
    return [`${item.name} · ${importStatus[item.status]}${records}`, page, original, unmatched, ...item.notes, omitted]
      .filter(Boolean).join('\n');
  })].join('\n\n') + '\n';
}

export function ImportDialog({
  workspace, pages, collectionByPage, initialParent, busy, onClose, report, onImport, onOpen, onOpenOriginal,
  onRestart, onSaveReport, migration, error, onCancelMigration,
}: DialogActions & Locations & {
  workspace: string;
  report?: ImportOutcome[]; onImport(request: ContentImport): void; onOpen(id: string): void;
  onOpenOriginal(original: ImportOriginal): void;
  migration?: 'pending' | 'cancelling'; error?: string; onCancelMigration(): void;
  onRestart(): void; onSaveReport(text: string): void;
}) {
  const startsInDatabase = initialParent !== null && collectionByPage.has(initialParent);
  const [format, setFormat] = useState<ContentImport['type']>(startsInDatabase ? 'csv' : 'markdown');
  const [parent, setParent] = useState(initialParent ?? '');
  const [mode, setMode] = useState<'new' | 'existing'>(startsInDatabase ? 'existing' : 'new');
  const [database, setDatabase] = useState(startsInDatabase ? initialParent : '');
  const [visible, setVisible] = useState(importLimit);
  const [previous, setPrevious] = useState<MigrationBatch | null>(null);
  const formatField = useRef<HTMLSelectElement>(null);
  const locations = useLocations(pages);
  const databaseLocations = locations.filter((location) => collectionByPage.has(location.id));
  const isCsv = format === 'csv';
  const isMigration = format === 'notionZip' || format === 'obsidianVault';
  const intoExisting = isCsv && mode === 'existing';
  const collectionId = collectionByPage.get(database);
  const parentChoices = isCsv ? locations.filter((location) => !collectionByPage.has(location.id)) : locations;
  const parentId = parent && parentChoices.some((choice) => choice.id === parent) ? parent : null;
  const active = useMemo(() => new Set(locations.map((location) => location.id)), [locations]);
  useEffect(() => {
    if (busy || report) return;
    let alive = true;
    void desktop.latestMigration(workspace).then((batch) => {
      if (alive) setPrevious(batch);
    }).catch(() => { if (alive) setPrevious(null); });
    return () => { alive = false; };
  }, [workspace, busy, report]);

  if (report) return <Modal title="导入报告" onClose={onClose} busy={busy} className="import-dialog">
    <p className="dialog-copy" role="status">{importSummary(report, isMigration)}</p>
    {error && <p className="dialog-error" role="alert">{error}</p>}
    <div className="import-results">
      {report.slice(0, visible).map((item, index) => {
        const pageId = item.pageId && active.has(item.pageId) ? item.pageId : null;
        const original = item.original && active.has(item.original.pageId) ? item.original : undefined;
        return <section key={index} className="import-result">
          <header>
            <div><h3>{item.name}</h3>
              <p>{importStatus[item.status]}{(isCsv || item.records > 0) && item.status === 'imported' && ` · ${item.records} 条记录`}</p></div>
            {pageId && <button type="button" className="secondary-button" aria-label={`打开 ${item.name}`}
              disabled={busy} onClick={() => onOpen(pageId)}>打开</button>}
          </header>
          {original && <p><button type="button" className="text-button" disabled={busy}
            onClick={() => onOpenOriginal(original)}>查看原件 {original.name}</button></p>}
          {!original && !isMigration && item.status === 'imported' && <p>原件已存到附件。</p>}
          {!!item.unmatchedLinks && <p>{item.unmatchedLinks} 处链接失配。</p>}
          {!!item.notes.length && <ul>{item.notes.map((note, index) => <li key={index}>{note}</li>)}</ul>}
          {item.notesOmitted > 0 && <p>另有 {item.notesOmitted} 条提示，详情可查原件。</p>}
        </section>;
      })}
      {visible < report.length && <button type="button" className="text-button import-more" disabled={busy}
        onClick={() => setVisible((count) => count + importLimit)}>显示更多（{visible} / {report.length}）</button>}
    </div>
    <footer className="dialog-actions dialog-pad">
      <button type="button" className="secondary-button" disabled={busy}
        onClick={() => onSaveReport(reportText(report, isCsv, isMigration))}>保存报告</button>
      <button type="button" className="primary-button" disabled={busy}
        onClick={() => { setVisible(importLimit); onRestart(); }}>继续导入</button>
    </footer>
  </Modal>;

  const previousProblem = previous && (
    previous.status === 'interrupted' || previous.status === 'cancelled' || previous.status === 'failed'
  ) ? `${migrationStatus[previous.status]} · ${previous.source}。${previous.detail ?? '可重新选择来源。'}` : null;
  return <Modal title="导入资料" onClose={onClose} busy={busy} initialFocus={formatField} className="import-dialog">
    <form className="dialog-form" onSubmit={(event) => {
      event.preventDefault();
      if (busy) return;
      if (!isCsv) onImport({ type: format, parentId });
      else if (!intoExisting) onImport({ type: 'csv', destination: { type: 'new', parentId } });
      else if (collectionId) onImport({ type: 'csv', destination: { type: 'existing', collectionId } });
    }}>
      <label htmlFor="import-format">文件格式</label>
      <select id="import-format" ref={formatField} value={format} disabled={busy}
        onChange={(event) => setFormat(event.currentTarget.value as ContentImport['type'])}>
        <option value="markdown">Markdown</option>
        <option value="csv">CSV</option>
        <option value="notionZip">Notion ZIP（Markdown 与 CSV）</option>
        <option value="obsidianVault">Obsidian 资料库目录</option>
      </select>
      {isCsv && <>
        <label htmlFor="csv-destination">写入方式</label>
        <select id="csv-destination" value={mode} disabled={busy}
          onChange={(event) => setMode(event.currentTarget.value === 'existing' ? 'existing' : 'new')}>
          <option value="new">新建数据库</option>
          <option value="existing" disabled={!databaseLocations.length}>追加到现有数据库</option>
        </select>
      </>}
      {intoExisting ? <LocationField label="目标数据库" choices={databaseLocations} value={database}
        disabled={busy} allowRoot={false} onChange={setDatabase} /> :
        <LocationField label="放入位置" choices={parentChoices} value={parentId ?? ''}
          disabled={busy} allowRoot onChange={setParent} />}
      <p className="library-hint">{format === 'notionZip' ? '在 Notion 中选择 Markdown 与 CSV 格式并包含子页面。导出 ZIP 中的页面、CSV 数据库和附件会放入同一个页面分支。' :
        format === 'obsidianVault' ? '选择整个资料库目录。可识别的笔记链接和附件会迁入知页，其他文件保留为原件。' :
        intoExisting ? 'CSV 需要 UTF-8 列名。按列名匹配属性，优先用「标题」或 Title 作为记录标题。' :
        isCsv ? 'CSV 需要 UTF-8 列名。优先用「标题」或 Title 作为记录标题，其余列导为文本。' :
          '可同时选择最多 32 个 Markdown 文件。共同选中的文件可保留相互引用。'}</p>
      {migration && <p className="library-hint" role="status">{migration === 'cancelling' ?
        '正在取消迁移…' : '正在准备迁移。校验完成后会一次写入工作空间。'}</p>}
      {error && <p className="dialog-error import-error" role="alert">{error}</p>}
      {!busy && !error && previousProblem && <p className="library-hint" role="status">{previousProblem}</p>}
      <footer className="dialog-actions">
        {migration ? <button type="button" className="secondary-button" disabled={migration === 'cancelling'}
          onClick={onCancelMigration}>取消迁移</button> :
          <button type="button" className="secondary-button" disabled={busy} onClick={onClose}>取消</button>}
        <button type="submit" className="primary-button" disabled={busy || (intoExisting && !collectionId)}>
          {format === 'notionZip' ? '选择 Notion ZIP' : format === 'obsidianVault' ? '选择资料库目录' :
            isCsv ? '选择 CSV 文件' : '选择 Markdown 文件'}
        </button>
      </footer>
    </form>
  </Modal>;
}

export function SaveTemplateDialog({ page, templates, collectionId, busy, onClose, onSave }: DialogActions & {
  page: PageInfo; templates: TemplateSummary[]; collectionId: string | null;
  onSave(name: string, templateId: string | null): void;
}) {
  const suggestedName = Array.from(page.title).slice(0, 80).join('');
  const [name, setName] = useState(suggestedName);
  const [template, setTemplate] = useState('');
  const nameField = useRef<HTMLInputElement>(null);
  const matching = templates.filter((item) => item.collectionId === collectionId);
  return <Modal title={collectionId ? '保存记录模板' : '保存页面模板'}
    onClose={onClose} busy={busy} initialFocus={nameField}>
    <form className="dialog-form" onSubmit={(event) => {
      event.preventDefault();
      if (name.trim()) onSave(name.trim(), template || null);
    }}>
      {matching.length > 0 && <>
        <label htmlFor="template-existing">保存方式</label>
        <select id="template-existing" value={template} disabled={busy} onChange={(event) => {
          const id = event.currentTarget.value;
          setTemplate(id);
          setName(matching.find((item) => item.id === id)?.name ?? suggestedName);
        }}>
          <option value="">新建模板</option>
          {matching.map((item) => <option key={item.id} value={item.id}>更新「{item.name}」</option>)}
        </select>
      </>}
      <label htmlFor="template-name">模板名称</label>
      <input id="template-name" ref={nameField} value={name} maxLength={80} disabled={busy}
        onChange={(event) => setName(event.currentTarget.value)} />
      <p className="library-hint">保存「{page.title}」及其子页面的当前内容，供以后新建时使用。</p>
      <footer className="dialog-actions">
        <button type="button" className="secondary-button" disabled={busy} onClick={onClose}>取消</button>
        <button type="submit" className="primary-button" disabled={busy || !name.trim()}>
          {template ? '更新模板' : '保存模板'}
        </button>
      </footer>
    </form>
  </Modal>;
}

export function ApplyTemplateDialog({ pages, collectionByPage, initialParent, templates, busy, onClose, onApply, onDelete }: DialogActions & Locations & {
  templates: TemplateSummary[]; onApply(id: string, parent: string | null, title: string | null): void; onDelete(id: string): void;
}) {
  const [parent, setParent] = useState(initialParent ?? '');
  const [picked, setPicked] = useState('');
  const [title, setTitle] = useState('');
  const [confirmDelete, setConfirmDelete] = useState(false);
  const templateField = useRef<HTMLSelectElement>(null);
  const locations = useLocations(pages);
  const scope = collectionByPage.get(parent) ?? null;
  const matching = templates.filter((item) => item.collectionId === scope);
  const selected = matching.find((item) => item.id === picked) ?? matching[0];
  return <Modal title="从模板创建" onClose={onClose} busy={busy} initialFocus={templateField}>
    <form className="dialog-form" onSubmit={(event) => {
      event.preventDefault();
      if (selected && !confirmDelete) onApply(selected.id, parent || null, title.trim() || null);
    }}>
      <LocationField label="放入位置" choices={locations} value={parent}
        disabled={busy} allowRoot onChange={(value) => { setParent(value); setPicked(''); setConfirmDelete(false); }} />
      {selected ? <>
        <label htmlFor="template-pick">模板</label>
        <select id="template-pick" ref={templateField} value={selected.id} disabled={busy} onChange={(event) => {
          setPicked(event.currentTarget.value); setConfirmDelete(false);
        }}>
          {matching.map((item) => <option key={item.id} value={item.id}>{item.name} · {item.pageCount} 页</option>)}
        </select>
        <label htmlFor="template-page-title">新页面标题</label>
        <input id="template-page-title" value={title} placeholder="沿用模板里的页面标题" maxLength={200}
          disabled={busy} onChange={(event) => setTitle(event.currentTarget.value)} />
      </> : <p className="library-hint">{scope ? '这个数据库还没有记录模板。请在一条记录的页面操作中保存模板。' :
        '还没有页面模板。请在普通页面的页面操作中保存模板。'}</p>}
      {selected && confirmDelete && <p className="library-hint">删除模板「{selected.name}」？以前创建的页面会保留。</p>}
      <footer className="dialog-actions library-template-actions">
        {selected && <button type="button" className={confirmDelete ? 'danger-button' : 'text-button'}
          disabled={busy} onClick={() => {
            if (confirmDelete) { setConfirmDelete(false); onDelete(selected.id); }
            else setConfirmDelete(true);
          }}>{confirmDelete ? '确认删除' : '删除模板'}</button>}
        <button type="button" className="secondary-button" disabled={busy} onClick={confirmDelete ? () => setConfirmDelete(false) : onClose}>取消</button>
        <button type="submit" className="primary-button" disabled={busy || !selected || confirmDelete}>{scope ? '创建记录' : '创建页面'}</button>
      </footer>
    </form>
  </Modal>;
}
