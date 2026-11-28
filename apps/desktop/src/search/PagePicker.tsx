// SPDX-License-Identifier: AGPL-3.0-or-later

import { ChevronLeft, ChevronRight, CornerDownLeft, FileText, RotateCcw, Search, SlidersHorizontal } from 'lucide-react';
import { useEffect, useId, useMemo, useRef, useState } from 'react';
import type { Collection, CollectionSummary, PageInfo, SearchHit, SearchRequest, SearchResponse, SearchSnippet } from '../contracts';
import { errorMessage } from '../contracts';
import { FilterFields, filterDraft, readFilter } from '../collections/FilterFields';
import { fieldText, operatorNames } from '../collections/fields';
import { desktop } from '../desktop';
import { Modal } from '../dialogs';
import { PageIcon } from '../icons';
import { pageTrail } from '../PageTree';
import { composing } from '../keyboard';
import type { SearchProgress } from './useSearchIndex';

type Scope = Pick<SearchRequest, 'collectionId' | 'filter'>;

export function PagePicker({ workspace, pages, collections, mode, busy, progress, onRebuild, onChoose, onShowLocation, onClose }: {
  workspace: string; pages: PageInfo[]; collections: CollectionSummary[]; mode: 'search' | 'reference' | 'blockReference'; busy: boolean;
  progress?: SearchProgress; onRebuild(): void; onChoose(hit: SearchHit): void;
  onShowLocation(hit: SearchHit): void; onClose(): void;
}) {
  const [query, setQuery] = useState('');
  const [scope, setScope] = useState<Scope>({});
  const [result, setResult] = useState<{ key: string; response?: SearchResponse; error?: string }>();
  const [selectionIndex, setSelectionIndex] = useState(0);
  const [cursor, setCursor] = useState<{ key: string; offset: number }>();
  const input = useRef<HTMLInputElement>(null);
  const id = useId();
  const term = query.trim();
  const byId = useMemo(() => new Map(pages.map((page) => [page.id, page])), [pages]);
  const recent = useMemo(() => pages.filter((page) => !page.trashRootId)
    .sort((a, b) => b.updatedAt - a.updatedAt).slice(0, 8)
    .map((page): SearchHit => ({ page, blockId: null, snippet: null })), [pages]);
  const searching = Boolean(term || scope.collectionId);
  const pdf = mode === 'search' ? progress?.pdf : undefined;
  const searchKey = JSON.stringify([workspace, term, scope, mode, progress?.refresh, pdf?.revision]);
  if (cursor && cursor.key !== searchKey) setCursor(undefined);
  const offset = cursor?.key === searchKey ? cursor.offset : 0;
  const queryKey = `${searchKey}/${offset}`;
  const active = searching && result?.key === queryKey ? result : undefined;
  const visible = searching ? (active?.response?.hits ?? []) : recent;
  const highlight = Math.min(selectionIndex, Math.max(visible.length - 1, 0));
  const highlightedHit = visible[highlight];
  const querying = searching && !active;
  const remaining = progress?.pending ?? active?.response?.index.pending;

  useEffect(() => {
    if (!searching) return;
    let alive = true;
    const timer = setTimeout(() => {
      desktop.search(workspace, { query: term, ...scope, offset, pageOnly: mode !== 'search' }).then((response) => {
        if (alive) setResult({ key: queryKey, response });
      }).catch((error: unknown) => {
        if (alive) setResult({ key: queryKey, error: errorMessage(error) });
      });
    }, 110);
    return () => { alive = false; clearTimeout(timer); };
  }, [searching, queryKey, scope, term, workspace, offset, mode]);

  useEffect(() => {
    document.getElementById(`${id}-result-${highlight}`)?.scrollIntoView({ block: 'nearest' });
  }, [id, highlight, visible.length]);

  function turnPage(target: number) {
    setCursor({ key: searchKey, offset: target });
    setSelectionIndex(0);
  }

  const title = { search: '搜索页面', reference: '引用页面', blockReference: '选择源页面' }[mode];
  return <Modal title={title} onClose={onClose} busy={busy} bare
    className="picker-dialog" initialFocus={input}>
    <div className="picker-input"><Search size={19} /><input ref={input}
      aria-label={mode === 'search' ? '搜索关键词' : mode === 'blockReference' ? '查找源页面' : '查找引用页面'} aria-controls={`${id}-results`}
      aria-describedby={`${id}-highlight`}
      placeholder={mode === 'search' ? '搜索标题、正文或 PDF…' : '输入标题或正文关键词…'} value={query} disabled={busy} maxLength={200}
      onChange={(event) => { setQuery(event.currentTarget.value); setSelectionIndex(0); }}
      onKeyDown={(event) => {
        if (composing(event.nativeEvent)) return;
        if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
          event.preventDefault();
          const step = event.key === 'ArrowDown' ? 1 : -1;
          setSelectionIndex(Math.max(0, Math.min(highlight + step, visible.length - 1)));
        }
        if (event.key === 'Enter' && visible[highlight]) { event.preventDefault(); onChoose(visible[highlight]); }
      }} /></div>
    <p className="visually-hidden" id={`${id}-highlight`} role="status">
      {highlightedHit && `第 ${highlight + 1} 项，共 ${visible.length} 项。${highlightedHit.page.title}${highlightedHit.pdf
        ? `，${highlightedHit.pdf.name}第 ${highlightedHit.pdf.pageNumber} 页` : ''}`}
    </p>
    {mode === 'search' && <SearchScope workspace={workspace} collections={collections} pages={byId} busy={busy}
      applied={scope} onApply={(value) => { setScope(value); setSelectionIndex(0); input.current?.focus(); }} />}
    {Boolean(remaining) && <p className="picker-index-note" role="status">索引待处理 {remaining} 页，结果正在补全。</p>}
    {progress?.error && <p className="picker-index-note" role="alert">索引更新失败。{progress.error}</p>}
    {pdf && (pdf.pending > 0 || pdf.noText > 0 || pdf.failed > 0 || pdf.partial > 0) &&
      <p className="picker-index-note" role="status">{[
        pdf.pending > 0 && `${pdf.pending} 份 PDF 待提取`,
        pdf.noText > 0 && `${pdf.noText} 份 PDF 无文本层`,
        pdf.failed > 0 && `${pdf.failed} 份 PDF 无法提取`,
        pdf.partial > 0 && `${pdf.partial} 份 PDF 部分提取`,
      ].filter(Boolean).join(' · ')}</p>}
    {mode === 'search' && progress?.pdfError && <p className="picker-index-note" role="alert">PDF 索引更新失败。{progress.pdfError}</p>}
    <div id={`${id}-results`} className="picker-results" aria-label="匹配页面" aria-busy={querying}>
      {querying && <p className="dialog-empty" role="status">正在搜索…</p>}
      {active?.error && <p className="dialog-empty" role="alert">{active.error}</p>}
      {!querying && !active?.error && visible.length === 0 && (!searching || !remaining) && !pdf?.pending &&
        <p className="dialog-empty">{searching ? '没有找到匹配的页面。' : '这个工作空间还没有页面。'}</p>}
      {visible.map((hit, index) => {
        const { page, blockId, snippet, pdf } = hit;
        const parent = pageTrail(byId, page.id).slice(0, -1).map((item) => item.title).join(' / ');
        return <button id={`${id}-result-${index}`} type="button" key={pdf ? `${pdf.attachmentId}/${pdf.pageNumber}` : page.id}
          className={`picker-row${index === highlight ? ' highlighted' : ''}`}
          onFocus={() => setSelectionIndex(index)} onMouseEnter={() => setSelectionIndex(index)} onClick={() => onChoose(hit)} disabled={busy}>
          <span className="picker-page-icon">{pdf ? <FileText size={17} aria-hidden /> : <PageIcon kind={page.icon} />}</span>
          <span className="picker-result-text">
            <strong>{!blockId && !pdf && snippet ? <Match snippet={snippet} /> : page.title}</strong>
            {parent && <small className="picker-path">{parent}</small>}
            {pdf && <small className="picker-path">{pdf.name} · PDF 第 {pdf.pageNumber} 页</small>}
            {(blockId || pdf) && snippet && <small className="picker-excerpt"><Match snippet={snippet} /></small>}
          </span>
          {index === highlight && <CornerDownLeft size={15} className="picker-enter" aria-hidden />}
        </button>;
      })}
    </div>
    {searching && (offset > 0 || active?.response?.nextOffset != null) &&
      <div className="picker-pages">
        <button type="button" className="icon-button" aria-label="上一页搜索结果"
          disabled={busy || querying || offset === 0} onClick={() => turnPage(Math.max(0, offset - 20))}><ChevronLeft size={16} /></button>
        <span>第 {Math.floor(offset / 20) + 1} 页</span>
        <button type="button" className="icon-button" aria-label="下一页搜索结果"
          disabled={busy || active?.response?.nextOffset == null}
          onClick={() => { if (active?.response?.nextOffset != null) turnPage(active.response.nextOffset); }}><ChevronRight size={16} /></button>
      </div>}
    <footer className="picker-footer">
      <span><kbd>↑</kbd><kbd>↓</kbd> 移动</span><span><kbd>Enter</kbd> {mode === 'search' ? '打开' : mode === 'blockReference' ? '选择' : '插入'}</span><span><kbd>Esc</kbd> 关闭</span>
      {mode === 'search' && visible[highlight] && <button type="button" className="secondary-button"
        disabled={busy || querying} onClick={() => onShowLocation(visible[highlight])}>定位链接</button>}
      {mode === 'search' && <button type="button" className="picker-rebuild" disabled={busy} onClick={onRebuild}>
        <RotateCcw size={13} aria-hidden />重建索引
      </button>}
    </footer>
  </Modal>;
}

function Match({ snippet }: { snippet: SearchSnippet }) {
  return <>{snippet.before}<mark>{snippet.matched}</mark>{snippet.after}</>;
}

function SearchScope({ workspace, collections, pages, applied, busy, onApply }: {
  workspace: string; collections: CollectionSummary[]; pages: ReadonlyMap<string, PageInfo>; applied: Scope;
  busy: boolean; onApply(scope: Scope): void;
}) {
  const id = useId();
  const panel = useRef<HTMLDetailsElement>(null);
  const [collectionId, setCollectionId] = useState(applied.collectionId ?? '');
  const [draft, setDraft] = useState(() => filterDraft(applied.filter));
  const [loaded, setLoaded] = useState<{ id: string; collection?: Collection; error?: string }>();
  const [formError, setFormError] = useState('');
  const choices = collections.flatMap((collection) => {
    const page = pages.get(collection.containerPageId);
    return page && !page.trashRootId ? [{ id: collection.id, name: pageTrail(pages, page.id).map((part) => part.title).join(' / ') }] : [];
  });
  const current = collectionId === loaded?.id ? loaded : undefined;
  const schema = current?.collection;
  const waiting = Boolean(collectionId && !current);
  const fields = schema?.properties ?? [];
  const appliedName = choices.find((choice) => choice.id === applied.collectionId)?.name;
  const selectedField = schema?.id === applied.collectionId ? fields.find((field) => field.id === applied.filter?.fieldId) : undefined;
  const summary = selectedField && applied.filter
    ? `${selectedField.name} ${operatorNames[applied.filter.operator]}${applied.filter.value === null ? '' : ` ${fieldText(selectedField, applied.filter.value)}`}`
    : null;

  useEffect(() => {
    if (!collectionId) return;
    let active = true;
    desktop.collection(workspace, collectionId).then((collection) => {
      if (active) setLoaded({ id: collectionId, collection });
    }).catch((error: unknown) => {
      if (active) setLoaded({ id: collectionId, error: errorMessage(error) });
    });
    return () => { active = false; };
  }, [workspace, collectionId]);

  if (!choices.length) return null;
  return <details ref={panel} className="picker-scope">
    <summary><SlidersHorizontal size={14} aria-hidden />
      <span>{appliedName ? [appliedName, summary].filter(Boolean).join(' · ') : '按数据库属性筛选'}</span></summary>
    <form className="dialog-form picker-scope-form" onSubmit={(event) => {
      event.preventDefault();
      try {
        const filter = readFilter(draft, fields);
        setFormError('');
        if (panel.current) panel.current.open = false;
        onApply(collectionId ? { collectionId, filter } : {});
      } catch (error) {
        setFormError(errorMessage(error));
      }
    }}>
      <fieldset className="collection-form-fields" disabled={busy}>
        <div className="form-field"><label htmlFor={id}>搜索范围</label>
          <select id={id} value={collectionId} onChange={(event) => {
            setCollectionId(event.currentTarget.value);
            setDraft(filterDraft());
            setFormError('');
          }}>
            <option value="">整个工作空间</option>
            {choices.map((choice) => <option key={choice.id} value={choice.id}>{choice.name}</option>)}
          </select></div>
        {schema && <FilterFields fields={fields} draft={draft} onChange={setDraft} />}
      </fieldset>
      {waiting && <p className="picker-index-note" role="status">正在读取数据库属性…</p>}
      {(current?.error || formError) && <p className="collection-form-error" role="alert">{current?.error || formError}</p>}
      <footer className="dialog-actions">
        {applied.collectionId && <button type="button" className="secondary-button" disabled={busy} onClick={() => {
          setCollectionId(''); setDraft(filterDraft()); setFormError('');
          if (panel.current) panel.current.open = false;
          onApply({});
        }}>清除筛选</button>}
        <button type="submit" className="primary-button" disabled={busy || Boolean(collectionId && !schema)}>应用筛选</button>
      </footer>
    </form>
  </details>;
}
