// SPDX-License-Identifier: AGPL-3.0-or-later

import { Node } from '@tiptap/core';
import type { JSONContent } from '@tiptap/core';
import { NodeViewWrapper, ReactNodeViewRenderer } from '@tiptap/react';
import type { NodeViewProps } from '@tiptap/react';
import { ArrowUpRight } from 'lucide-react';
import { createElement, useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import type { BlockTarget, ReferenceSource } from '../contracts';
import { errorMessage } from '../contracts';
import { desktop } from '../desktop';
import { FileCard } from './AttachmentBlock';
import { AttachmentFigure } from './AttachmentImage';
import { blockNode, blockTarget, fileTarget } from './blocks';
import { calloutLabels, calloutTone } from './content-shapes';
import { MathPreview } from './ContentBlocks';
import { imageAttributes, tableAlignment } from './rich-content';

function ReferenceView({ node, selected, extension }: NodeViewProps) {
  const target = blockTarget(node.attrs);
  const { workspace } = extension.options as { workspace: string };
  return <NodeViewWrapper className="block-reference" contentEditable={false}>
    {target && <ReferenceCard key={`${workspace}/${target.pageId}/${target.blockId}`}
      workspace={workspace} target={target} selected={selected} />}
  </NodeViewWrapper>;
}

function ReferenceCard({ workspace, target, selected }: { workspace: string; target: BlockTarget; selected: boolean }) {
  const { pageId, blockId } = target;
  const panel = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(false);
  const [preview, setPreview] = useState<Exclude<ReferenceSource, { state: 'unchanged' }> | { state: 'error'; message: string }>();
  const shouldLoad = visible || selected;

  useEffect(() => {
    if (!panel.current) return;
    const observer = new IntersectionObserver(([entry]) => setVisible(entry.isIntersecting));
    observer.observe(panel.current);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (!shouldLoad) return;
    let alive = true;
    let reading = false;
    let timer: ReturnType<typeof setTimeout>;
    let revision: number | null = null;
    async function update() {
      if (!alive || reading || document.visibilityState === 'hidden') return;
      clearTimeout(timer);
      reading = true;
      try {
        const result = await desktop.referenceSource(workspace, pageId, blockId, revision);
        if (alive && result.state !== 'unchanged') {
          revision = result.revision;
          setPreview(result);
        }
      } catch (error) {
        revision = null;
        if (alive) setPreview({ state: 'error', message: errorMessage(error) });
      } finally {
        reading = false;
        if (alive) timer = setTimeout(() => { void update(); }, 2500);
      }
    }
    void update();
    document.addEventListener('visibilitychange', update);
    window.addEventListener('focus', update);
    return () => {
      alive = false;
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', update);
      window.removeEventListener('focus', update);
    };
  }, [workspace, pageId, blockId, shouldLoad]);

  return <div ref={panel} className={selected ? 'block-reference-selected' : undefined}>
    {preview?.state === 'available' ?
      <>
        <button type="button" className="block-reference-source" data-source-page={pageId} data-source-block={blockId}
          aria-label={`打开源块：${preview.title}`}><ArrowUpRight size={15} aria-hidden />{preview.title}</button>
        <div className="block-reference-content">
          <ReadOnlyBlock node={previewNode(blockNode(preview.block))} workspace={workspace} pageId={pageId} />
        </div>
      </> :
      <span className="block-reference-status" role={preview ? 'status' : undefined}>
        {preview?.state === 'unavailable' ? '源块不可用' : preview?.state === 'error' ? preview.message : '块引用'}
      </span>}
  </div>;
}

const markTags: Record<string, string> = { bold: 'strong', italic: 'em', strike: 's', underline: 'u', code: 'code', link: 'u' };

function ReadOnlyBlock({ node, workspace, pageId }: { node: JSONContent; workspace: string; pageId: string }): ReactNode {
  const { attrs = {}, type } = node;
  const content = node.content?.map((child, index) =>
    <ReadOnlyBlock key={child.attrs?.id ?? index} node={child} workspace={workspace} pageId={pageId} />);
  switch (type) {
    case 'text':
      return node.marks?.reduce<ReactNode>((text, mark) => createElement(markTags[mark.type], {}, text), node.text) ?? node.text;
    case 'pageReference': return <span className="reference-inline-label">{attrs.label}</span>;
    case 'hardBreak': return <br />;
    case 'paragraph': return <p>{content}</p>;
    case 'heading': return createElement(`h${attrs.level}`, {}, content);
    case 'blockquote': return <blockquote>{content}</blockquote>;
    case 'bulletList': return <ul>{content}</ul>;
    case 'orderedList': return <ol start={attrs.start} type={attrs.type}>{content}</ol>;
    case 'taskList': return <ul className="reference-tasks">{content}</ul>;
    case 'listItem': return <li>{content}</li>;
    case 'taskItem': return <li><span role="img" aria-label={attrs.checked ? '已完成' : '未完成'}>{attrs.checked ? '☑' : '☐'}</span>{content}</li>;
    case 'horizontalRule': return <hr />;
    case 'codeBlock': return <pre><code>{content}</code></pre>;
    case 'table': return <div className="tableWrapper"><table><tbody>{content}</tbody></table></div>;
    case 'tableRow': return <tr>{content}</tr>;
    case 'tableCell':
    case 'tableHeader': return createElement(type === 'tableHeader' ? 'th' : 'td', {
      colSpan: attrs.colspan, rowSpan: attrs.rowspan, style: { textAlign: tableAlignment(attrs.align) },
    }, content);
    case 'attachmentImage': return <div className="attachment-image-block">
      <AttachmentFigure workspace={workspace} pageId={pageId} attachment={imageAttributes(attrs)} />
    </div>;
    case 'attachmentFile': {
      const target = fileTarget(attrs);
      return target && <FileCard workspace={workspace} pageId={pageId} {...target} />;
    }
    case 'toggleBlock': return <details className="fold-block fold-preview" open={!attrs.collapsed}>
      <summary>{attrs.title || '折叠'}</summary>{content}
    </details>;
    case 'callout': {
      const tone = calloutTone(attrs.tone) ?? 'note';
      return <aside className="callout-block" data-tone={tone}>
        <strong className="callout-header">{calloutLabels[tone]}</strong>
        <div className="callout-body">{content}</div>
      </aside>;
    }
    case 'mathBlock': return <div className="math-block"><MathPreview latex={attrs.latex} /></div>;
    case 'tableOfContents': return <span className="block-reference-status">目录</span>;
    case 'blockReference': return <span className="block-reference-status">块引用</span>;
    default: return <span className="block-reference-status">保留块</span>;
  }
}

function previewNode(node: JSONContent): JSONContent {
  switch (node.type) {
    case 'taskItem': return { type: 'taskList', content: [node] };
    case 'listItem': return { type: 'bulletList', content: [node] };
    case 'tableRow': return { type: 'table', content: [node] };
    case 'tableCell':
    case 'tableHeader': return { type: 'table', content: [{ type: 'tableRow', content: [node] }] };
    default: return node;
  }
}

export const BlockReference = Node.create<{ workspace: string }>({
  name: 'blockReference',
  group: 'block',
  atom: true,
  draggable: true,
  addOptions() {
    return { workspace: '' };
  },
  addAttributes() {
    return {
      pageId: { default: null, rendered: false },
      blockId: { default: null, rendered: false },
    };
  },
  parseHTML() {
    return [{
      tag: 'div[data-zhiye-block-reference]',
      getAttrs: (element) => element.getAttribute('data-workspace') === this.options.workspace
        ? blockTarget({ pageId: element.getAttribute('data-source-page'), blockId: element.getAttribute('data-source-block') }) || false
        : false,
    }];
  },
  renderHTML({ node, HTMLAttributes }) {
    return ['div', {
      ...HTMLAttributes,
      'data-zhiye-block-reference': 'true',
      'data-workspace': this.options.workspace,
      'data-source-page': node.attrs.pageId,
      'data-source-block': node.attrs.blockId,
    }, '块引用'];
  },
  addNodeView() {
    return ReactNodeViewRenderer(ReferenceView, {
      attrs: ({ node }) => ({ 'data-id': node.attrs.id ?? '' }),
    });
  },
});
