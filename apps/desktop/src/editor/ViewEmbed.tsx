// SPDX-License-Identifier: AGPL-3.0-or-later

import { Node as EditorNode, mergeAttributes } from '@tiptap/core';
import { NodeViewWrapper, ReactNodeViewRenderer } from '@tiptap/react';
import type { NodeViewProps } from '@tiptap/react';
import { ChevronLeft, ChevronRight, ExternalLink, RefreshCw, Table2 } from 'lucide-react';
import { useEffect, useId, useState } from 'react';
import type { Collection, PropertyDefinition, RelatedRecord, ViewTarget } from '../contracts';
import { errorMessage } from '../contracts';
import { CollectionRecords, RecordLink } from '../collections/CollectionRecords';
import { ScheduleRecords, ScheduleToolbar } from '../collections/ScheduleViews';
import { useViewRecords } from '../collections/useViewRecords';
import { useSchedule, viewLabels } from '../collections/viewDates';
import { desktop } from '../desktop';
import { Modal } from '../dialogs';
import { viewTarget } from './blocks';

interface EmbedOptions {
  workspace: string;
  openPage(id: string): void;
  openView(page: string, target: ViewTarget): void;
}

const noFields: PropertyDefinition[] = [];

function useCollection(workspace: string, id: string | undefined, refresh = 0) {
  const [data, setData] = useState<{ key: string; collection?: Collection; error?: string }>();
  const key = workspace + '/' + id + '/' + refresh;
  useEffect(() => {
    if (!id) return;
    let active = true;
    void desktop.collection(workspace, id).then((collection) => {
      if (active) setData({ key, collection });
    }).catch((error: unknown) => {
      if (active) setData({ key, error: errorMessage(error) });
    });
    return () => { active = false; };
  }, [workspace, id, key]);
  return data?.key === key ? data : undefined;
}

function EmbeddedView({ node, extension }: NodeViewProps) {
  const { workspace, openPage, openView } = extension.options as EmbedOptions;
  const target = viewTarget(node.attrs);
  const [refresh, setRefresh] = useState(0);
  const [offset, setOffset] = useState(0);
  const loaded = useCollection(workspace, target?.collectionId, refresh);
  const collection = loaded?.collection;
  const view = collection?.views.find((item) => item.id === target?.viewId);
  const fields = collection?.properties ?? noFields;
  const schedule = useSchedule(view, fields);
  const result = useViewRecords(workspace, collection, view, offset, schedule.range, String(refresh));
  const unavailable = !target || Boolean(loaded?.error) || Boolean(collection && !view);
  const rows = result?.data;
  return <NodeViewWrapper className="embedded-view" contentEditable={false} data-drag-handle="">
    <div className="embedded-view-heading">
      <Table2 size={16} />
      <strong>{view ? view.name ?? viewLabels[view.type] : '数据库视图'}</strong>
      <button type="button" className="icon-button" aria-label="刷新视图" title="刷新视图"
        onClick={() => setRefresh((value) => value + 1)}><RefreshCw size={14} /></button>
      {collection && target && <button type="button" className="secondary-button"
        onClick={() => openView(collection.containerPageId, target)}><ExternalLink size={14} />打开数据库</button>}
    </div>
    {unavailable ? <p className="embedded-view-status" role="status">此视图暂不可用。</p> : <>
      <ScheduleToolbar schedule={schedule} busy={false} onNavigate={() => setOffset(0)} />
      {result?.error ? <p className="embedded-view-status" role="alert">{result.error}</p> : !rows ?
        <p className="embedded-view-status" role="status">正在读取视图…</p> : <>
          {rows.total === 0 && <p className="embedded-view-status">暂无匹配记录。</p>}
          <div className="embedded-view-data">
            {schedule.range?.mode === 'between' ? <ScheduleRecords schedule={schedule} records={rows.records} busy={false}
              render={(record) => <RecordLink record={record} onOpen={openPage} busy={false} />} /> :
              <CollectionRecords workspace={workspace} view={schedule.range ? 'list' : view?.type ?? 'list'}
                fields={fields} grouping={fields.find((field) => field.id === view?.settings.groupBy)}
                rows={rows.records} busy={false} onOpen={openPage} />}
          </div>
          {rows.total > rows.pageSize && <div className="collection-footer">
            <span>共 {rows.total} 条记录</span>
            <div className="collection-pagination">
              <button type="button" className="icon-button" aria-label="上一页记录" disabled={rows.offset === 0}
                onClick={() => setOffset(rows.offset - rows.pageSize)}><ChevronLeft size={15} /></button>
              <button type="button" className="icon-button" aria-label="下一页记录" disabled={rows.offset + rows.pageSize >= rows.total}
                onClick={() => setOffset(rows.offset + rows.pageSize)}><ChevronRight size={15} /></button>
            </div>
          </div>}
        </>}
    </>}
  </NodeViewWrapper>;
}

export const ViewEmbed = EditorNode.create<EmbedOptions>({
  name: 'collectionView',
  group: 'block',
  atom: true,
  draggable: true,
  addOptions() { return { workspace: '', openPage() {}, openView() {} }; },
  addAttributes() {
    return {
      collectionId: { default: null, rendered: false },
      viewId: { default: null, rendered: false },
    };
  },
  parseHTML() {
    return [{ tag: 'div[data-zhiye-view]', getAttrs: (element) => {
      if (element.getAttribute('data-workspace') !== this.options.workspace) return false;
      return viewTarget({ viewId: element.getAttribute('data-zhiye-view'),
        collectionId: element.getAttribute('data-zhiye-collection') }) ?? false;
    } }];
  },
  renderHTML({ node, HTMLAttributes }) {
    return ['div', mergeAttributes(HTMLAttributes, {
      'data-workspace': this.options.workspace,
      'data-zhiye-collection': node.attrs.collectionId,
      'data-zhiye-view': node.attrs.viewId,
    }), '数据库视图'];
  },
  addNodeView() { return ReactNodeViewRenderer(EmbeddedView); },
});

export function EmbedPicker({ workspace, collections, onClose, onChoose }: {
  workspace: string; collections: RelatedRecord[]; onClose(): void; onChoose(target: ViewTarget): void;
}) {
  const scope = useId();
  const [collectionId, setCollectionId] = useState('');
  const [selected, setSelected] = useState('');
  const loaded = useCollection(workspace, collectionId || undefined);
  const views = loaded?.collection?.views ?? [];
  const viewId = views.some((item) => item.id === selected) ? selected : views[0]?.id ?? '';
  return <Modal title="嵌入数据库视图" onClose={onClose}>
    <form className="dialog-form" onSubmit={(event) => {
      event.preventDefault();
      if (viewId && loaded?.collection) onChoose({ collectionId: loaded.collection.id, viewId });
    }}>
      <label htmlFor={scope + '-collection'}>选择数据库</label>
      <select id={scope + '-collection'} value={collectionId} required onChange={(event) => {
        setCollectionId(event.currentTarget.value);
        setSelected('');
      }}>
        <option value="">{collections.length ? '请选择数据库' : '暂无可用数据库'}</option>
        {collections.map((item) => <option key={item.id} value={item.id}>{item.title}</option>)}
      </select>
      <label htmlFor={scope + '-view'}>选择保存视图</label>
      <select id={scope + '-view'} value={viewId} disabled={!views.length}
        onChange={(event) => setSelected(event.currentTarget.value)}>
        {!views.length && <option value="">选择数据库后加载</option>}
        {views.map((item) => <option key={item.id} value={item.id}>{item.name ?? viewLabels[item.type]}</option>)}
      </select>
      {loaded?.error && <p className="collection-form-error" role="alert">{loaded.error}</p>}
      <footer className="dialog-actions">
        <button type="button" className="secondary-button" onClick={onClose}>取消</button>
        <button type="submit" className="primary-button" disabled={!viewId}>插入视图</button>
      </footer>
    </form>
  </Modal>;
}
