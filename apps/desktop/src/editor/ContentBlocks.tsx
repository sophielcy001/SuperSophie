// SPDX-License-Identifier: AGPL-3.0-or-later

import { Node } from '@tiptap/core';
import type { Node as DocumentNode } from '@tiptap/pm/model';
import { NodeSelection, TextSelection } from '@tiptap/pm/state';
import type { Decoration } from '@tiptap/pm/view';
import { NodeViewContent, NodeViewWrapper, ReactNodeViewRenderer, useEditorState } from '@tiptap/react';
import type { NodeViewProps } from '@tiptap/react';
import { ChevronDown, ChevronRight, CircleAlert, Info, Lightbulb, Pencil } from 'lucide-react';
import { useEffect, useId, useRef, useState } from 'react';
import type { MouseEvent } from 'react';
import 'katex/dist/katex.min.css';
import { calloutLabels, calloutTone, mathSource, toggleTitle } from './content-shapes';
import { focusLocatedBlock, markSearchBlock } from './extensions';
import { inlineText } from './rich-content';

function revealsFold(decorations: readonly Decoration[]) {
  return decorations.some((decoration) => decoration.spec.revealFold);
}

function ToggleView({ node, editor, decorations, getPos, updateAttributes }: NodeViewProps) {
  const id = useId();
  const open = node.attrs.collapsed !== true || revealsFold(decorations);
  function toggle(event: MouseEvent<HTMLButtonElement>) {
    if (!editor.isEditable) return;
    const position = getPos();
    if (typeof position !== 'number') return;
    event.currentTarget.focus({ preventScroll: true });
    const change = editor.state.tr;
    if (node.attrs.collapsed !== open) change.setNodeAttribute(position, 'collapsed', open);
    if (open) change.setSelection(NodeSelection.create(change.doc, position));
    markSearchBlock(editor, null, change);
  }
  return <NodeViewWrapper className="fold-block" data-fold-open={open ? 'true' : 'false'}>
    <div className="fold-header" contentEditable={false}>
      <button type="button" className="icon-button" aria-label={open ? '收起折叠块' : '展开折叠块'}
        aria-expanded={open} aria-controls={id} onClick={toggle}>
        {open ? <ChevronDown size={18} /> : <ChevronRight size={18} />}
      </button>
      <input type="text" aria-label="折叠标题" placeholder="折叠标题" maxLength={512} value={node.attrs.title}
        onChange={(event) => {
          const title = toggleTitle(event.currentTarget.value);
          if (editor.isEditable && title !== undefined) updateAttributes({ title });
        }}
        onKeyDown={(event) => {
          // Enter moves from the title into the fold's content, as in Notion.
          if (event.key !== 'Enter' || event.nativeEvent.isComposing || event.keyCode === 229) return;
          const position = getPos();
          if (typeof position !== 'number' || !editor.isEditable) return;
          event.preventDefault();
          const change = editor.state.tr;
          if (node.attrs.collapsed === true) change.setNodeAttribute(position, 'collapsed', false);
          change.setSelection(TextSelection.near(change.doc.resolve(position + 2)));
          editor.view.dispatch(change);
          editor.view.focus();
        }} />
    </div>
    <NodeViewContent id={id} className="fold-body" />
  </NodeViewWrapper>;
}

export const ToggleBlock = Node.create({
  name: 'toggleBlock', group: 'block', content: 'block+', defining: true,
  addAttributes() {
    return { title: { default: '', rendered: false }, collapsed: { default: false, rendered: false } };
  },
  parseHTML() {
    return [{ tag: 'div[data-zhiye-fold]', getAttrs: (element) => {
      const title = toggleTitle(element.getAttribute('data-title'));
      return title === undefined ? false : { title, collapsed: element.getAttribute('data-collapsed') === 'true' };
    } }];
  },
  renderHTML({ node, HTMLAttributes }) {
    return ['div', { ...HTMLAttributes, 'data-zhiye-fold': '', 'data-title': node.attrs.title,
      'data-collapsed': String(node.attrs.collapsed) }, 0];
  },
  addNodeView() {
    return ReactNodeViewRenderer(ToggleView, {
      update({ oldNode, newNode, oldDecorations, newDecorations, updateProps }) {
        if (oldNode !== newNode || revealsFold(oldDecorations) !== revealsFold(newDecorations)) updateProps();
        return true;
      },
    });
  },
});

const calloutIcons = { note: Info, tip: Lightbulb, warning: CircleAlert };

function CalloutView({ node, editor, updateAttributes }: NodeViewProps) {
  const tone = calloutTone(node.attrs.tone) ?? 'note';
  const Icon = calloutIcons[tone];
  return <NodeViewWrapper className="callout-block" data-tone={tone}>
    <div className="callout-header" contentEditable={false}>
      <Icon size={16} aria-hidden />
      <select aria-label="提示样式" value={tone} onChange={(event) => {
        const tone = calloutTone(event.currentTarget.value);
        if (editor.isEditable && tone) updateAttributes({ tone });
      }}>
        {Object.entries(calloutLabels).map(([value, label]) => <option key={value} value={value}>{label}</option>)}
      </select>
    </div>
    <NodeViewContent className="callout-body" />
  </NodeViewWrapper>;
}

export const Callout = Node.create({
  name: 'callout', group: 'block', content: 'block+', defining: true,
  addAttributes() { return { tone: { default: 'note', rendered: false } }; },
  parseHTML() {
    return [{ tag: 'div[data-zhiye-callout]', getAttrs: (element) => {
      const tone = calloutTone(element.getAttribute('data-zhiye-callout'));
      return tone ? { tone } : false;
    } }];
  },
  renderHTML({ node, HTMLAttributes }) {
    return ['div', { ...HTMLAttributes, 'data-zhiye-callout': node.attrs.tone }, 0];
  },
  addNodeView() { return ReactNodeViewRenderer(CalloutView); },
});

export function MathPreview({ latex }: { latex: string }) {
  const output = useRef<HTMLDivElement>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let active = true;
    const element = output.current;
    if (!element) return;
    setFailed(false);
    element.replaceChildren();
    if (!latex.trim()) return;
    void import('katex').then(({ default: katex }) => {
      if (!active) return;
      katex.render(latex, element, {
        displayMode: true, output: 'htmlAndMathml', strict: 'ignore',
        throwOnError: true, trust: false, maxExpand: 256, maxSize: 16,
      });
    }).catch(() => { if (active) { element.replaceChildren(); setFailed(true); } });
    return () => { active = false; };
  }, [latex]);
  return <div className="math-display">
    <div ref={output} className="math-output" />
    {!latex.trim() && <span className="block-hint">添加 LaTeX 公式</span>}
    {failed && <div className="math-failure" role="status">LaTeX 排版遇到格式错误。<code>{latex}</code></div>}
  </div>;
}

function MathView({ node, editor, updateAttributes }: NodeViewProps) {
  const id = useId();
  const latex = String(node.attrs.latex ?? '');
  const [editing, setEditing] = useState(!latex);
  const [draft, setDraft] = useState(latex);
  const valid = mathSource(draft) !== undefined;
  return <NodeViewWrapper className="math-block" contentEditable={false}>
    <MathPreview latex={latex} />
    {editing ? <div className="math-editor">
      <label htmlFor={id}>LaTeX 公式</label>
      <textarea id={id} rows={3} maxLength={4096} spellCheck={false} value={draft}
        onChange={(event) => setDraft(event.currentTarget.value)} />
      {!valid && <span role="status">公式最多 4,096 字节。</span>}
      <div className="math-actions">
        <button type="button" className="secondary-button" onClick={() => setEditing(false)}>取消</button>
        <button type="button" className="primary-button" disabled={!valid} onClick={() => {
          if (editor.isEditable && valid) { updateAttributes({ latex: draft }); setEditing(false); }
        }}>保存公式</button>
      </div>
    </div> : <button type="button" className="icon-button math-edit" title="编辑公式" aria-label="编辑公式"
      onClick={() => { if (editor.isEditable) { setDraft(latex); setEditing(true); } }}><Pencil size={15} /></button>}
  </NodeViewWrapper>;
}

export const MathBlock = Node.create({
  name: 'mathBlock', group: 'block', atom: true, draggable: true,
  addAttributes() { return { latex: { default: '', rendered: false } }; },
  parseHTML() {
    return [{ tag: 'div[data-zhiye-math]', getAttrs: (element) => {
      const latex = mathSource(element.getAttribute('data-zhiye-math'));
      return latex === undefined ? false : { latex };
    } }];
  },
  renderHTML({ node, HTMLAttributes }) {
    return ['div', { ...HTMLAttributes, 'data-zhiye-math': node.attrs.latex }, node.attrs.latex];
  },
  addNodeView() { return ReactNodeViewRenderer(MathView); },
});

interface Heading { id: string; position: number; level: number; text: string }
const outlines = new WeakMap<DocumentNode, Heading[]>();

function outline(document: DocumentNode) {
  const existing = outlines.get(document);
  if (existing) return existing;
  const headings: Heading[] = [];
  document.descendants((node, position) => {
    if (node.type.name !== 'heading' || typeof node.attrs.id !== 'string') return;
    const text = inlineText(node);
    if (text) headings.push({ id: node.attrs.id, position, level: Number(node.attrs.level), text });
    return false;
  });
  outlines.set(document, headings);
  return headings;
}

function OutlineView({ editor }: NodeViewProps) {
  const headings = useEditorState({ editor, selector: ({ editor }) => outline(editor.state.doc) });
  return <NodeViewWrapper className="outline-block" contentEditable={false}>
    <nav aria-label="目录">
      <strong>目录</strong>
      {headings.length ? <ul>{headings.map((heading) => <li key={heading.id} data-level={heading.level}>
        <button type="button" className="text-button" onClick={() => {
          const target = outline(editor.state.doc).find((entry) => entry.id === heading.id);
          if (target) focusLocatedBlock(editor, target.position);
        }}>{heading.text}</button>
      </li>)}</ul> : <p className="block-hint">添加标题后会在这里显示。</p>}
    </nav>
  </NodeViewWrapper>;
}

export const TableOfContents = Node.create({
  name: 'tableOfContents', group: 'block', atom: true, draggable: true,
  parseHTML() { return [{ tag: 'div[data-zhiye-outline]' }]; },
  renderHTML({ HTMLAttributes }) { return ['div', { ...HTMLAttributes, 'data-zhiye-outline': '' }, '目录']; },
  addNodeView() { return ReactNodeViewRenderer(OutlineView); },
});
