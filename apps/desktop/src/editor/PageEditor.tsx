// SPDX-License-Identifier: AGPL-3.0-or-later

import { EditorContent, useEditor } from '@tiptap/react';
import type { Editor } from '@tiptap/react';
import { Fragment, Slice } from '@tiptap/pm/model';
import { NodeSelection } from '@tiptap/pm/state';
import { handlePaste as pasteTableCells } from '@tiptap/pm/tables';
import type { EditorView } from '@tiptap/pm/view';
import {
  ArrowDown, ArrowUp, Bold, Check, ChevronDown, ChevronRight, Code, ExternalLink, Heading1, Heading2, Heading3, Image, Italic, Link2,
  List, ListOrdered, ListTodo, ListTree, Lightbulb, LoaderCircle, Minus, Paperclip, Plus, Quote, Redo2,
  Sigma, SquareArrowOutUpRight, Strikethrough, Table2, Type, Underline, Undo2,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { forwardRef, useEffect, useId, useImperativeHandle, useRef, useState } from 'react';
import type { MouseEvent, ReactNode } from 'react';
import type { Attachment, BlockTarget, Page, PageDraft, PageInfo, ViewTarget } from '../contracts';
import { errorMessage } from '../contracts';
import { desktop } from '../desktop';
import { PageIcon } from '../icons';
import { composing, disclosureEvents } from '../keyboard';
import { fromBlocks, toBlocks } from './blocks';
import { AttachmentFigure, AttachmentImage } from './AttachmentImage';
import { AttachmentBlock } from './AttachmentBlock';
import { BlockReference } from './BlockReference';
import { ViewEmbed } from './ViewEmbed';
import { BlockDrag } from './BlockDrag';
import { blockMove, moveBlock, selectedMoveBlock } from './block-move';
import { ConflictDialog } from './ConflictDialog';
import { Callout, MathBlock, TableOfContents, ToggleBlock } from './ContentBlocks';
import { CloseDraftDialog, ImageCaptionDialog, WebLinkDialog } from './EditorDialogs';
import { editorExtensions, focusLocatedBlock, markSearchBlock } from './extensions';
import { imageNode, spreadsheetTable, webAddress } from './rich-content';
import { createSaveQueue } from './save-queue';
import type { SaveStatus } from './save-queue';

export interface EditorHandle {
  flush(): Promise<PageInfo | undefined>;
  hasUnsavedChanges(): boolean;
  insertReference(page: PageInfo): void;
  insertBlockReference(target: BlockTarget, at: InsertPosition): boolean;
  insertViewEmbed(target: ViewTarget, at: InsertPosition): boolean;
  insertAttachment(id: string, at: InsertPosition): boolean;
  pageDraft(): PageDraft;
  insertImages(images: Attachment[], at?: InsertPosition): boolean;
}

export type InsertPosition = number | { from: number; to: number };

interface Props {
  workspace: string;
  page: Page;
  locked: boolean;
  focusTitle: boolean;
  database?: boolean;
  searchTarget?: { blockId: string };
  headerContent?: ReactNode;
  onSaved(info: PageInfo): void;
  onResolved(page: Page): void;
  onDiscard(): void;
  onOpenPage(id: string, blockId?: string): void;
  onOpenView(page: string, target: ViewTarget): void;
  onExitFocusMode?(): void;
  onPickReference(): void;
  onPickBlockReference(at: InsertPosition, excluded?: string): void;
  onPickViewEmbed(at: InsertPosition): void;
  onPickAttachment(at: InsertPosition): void;
  onOpenAttachment(id: string): void;
  onInsertImages(files?: File[], at?: InsertPosition): void;
}

interface InsertChoice {
  label: string;
  icon: LucideIcon;
  apply?(editor: Editor): void;
}

const choices: InsertChoice[] = [
  { label: '正文', icon: Type, apply: (editor) => { editor.chain().focus().setParagraph().run(); } },
  { label: '一级标题', icon: Heading1, apply: (editor) => { editor.chain().focus().toggleHeading({ level: 1 }).run(); } },
  { label: '二级标题', icon: Heading2, apply: (editor) => { editor.chain().focus().toggleHeading({ level: 2 }).run(); } },
  { label: '三级标题', icon: Heading3, apply: (editor) => { editor.chain().focus().toggleHeading({ level: 3 }).run(); } },
  { label: '项目列表', icon: List, apply: (editor) => { editor.chain().focus().toggleBulletList().run(); } },
  { label: '编号列表', icon: ListOrdered, apply: (editor) => { editor.chain().focus().toggleOrderedList().run(); } },
  { label: '待办清单', icon: ListTodo, apply: (editor) => { editor.chain().focus().toggleTaskList().run(); } },
  { label: '引用', icon: Quote, apply: (editor) => { editor.chain().focus().toggleBlockquote().run(); } },
  { label: '代码块', icon: Code, apply: (editor) => { editor.chain().focus().toggleCodeBlock().run(); } },
  { label: '分割线', icon: Minus, apply: (editor) => { editor.chain().focus().setHorizontalRule().run(); } },
  { label: '图片', icon: Image },
  { label: '表格', icon: Table2, apply: (editor) => { editor.chain().focus().insertTable({ rows: 3, cols: 3, withHeaderRow: true }).run(); } },
  { label: '块引用', icon: SquareArrowOutUpRight },
  { label: '数据库视图', icon: Table2 },
  { label: '折叠', icon: ChevronRight, apply: (editor) => {
    editor.chain().focus().insertContent({ type: 'toggleBlock', content: [{ type: 'paragraph' }] }).run();
  } },
  { label: '提示块', icon: Lightbulb, apply: (editor) => {
    editor.chain().focus().insertContent({ type: 'callout', content: [{ type: 'paragraph' }] }).run();
  } },
  { label: '公式', icon: Sigma, apply: (editor) => { editor.chain().focus().insertContent({ type: 'mathBlock' }).run(); } },
  { label: '目录', icon: ListTree, apply: (editor) => { editor.chain().focus().insertContent({ type: 'tableOfContents' }).run(); } },
  { label: '附件块', icon: Paperclip },
];

const tableActions = [
  { label: '下方加行', command: 'addRowAfter' },
  { label: '右侧加列', command: 'addColumnAfter' },
  { label: '切换表头行', command: 'toggleHeaderRow' },
  { label: '删除行', command: 'deleteRow' },
  { label: '删除列', command: 'deleteColumn' },
  { label: '删除表格', command: 'deleteTable' },
] as const;

interface SlashMenu {
  from: number;
  to: number;
  left: number;
  top: number;
  selected?: number;
}

function slashPosition(editor: Editor): SlashMenu | null {
  const { selection } = editor.state;
  const { $from } = selection;
  if (editor.view.composing || !selection.empty || $from.parent.type.name !== 'paragraph'
    || $from.parent.textContent !== '/' || $from.parentOffset !== 1) return null;
  const rect = editor.view.coordsAtPos($from.pos);
  return {
    from: $from.pos - 1,
    to: $from.pos,
    left: Math.max(8, Math.min(rect.left, window.innerWidth - 228)),
    top: Math.max(8, rect.bottom + 332 < window.innerHeight ? rect.bottom + 8 : rect.top - 331),
  };
}

function ToolButton({ label, icon: Icon, onClick, active, disabled, shortcut }: {
  label: string; icon: LucideIcon; onClick(): void; active?: boolean; disabled?: boolean; shortcut?: string;
}) {
  return <button type="button" className={`tool-button${active ? ' is-active' : ''}`} title={label}
    aria-label={label} aria-pressed={active} aria-keyshortcuts={shortcut} onMouseDown={(event) => event.preventDefault()}
    disabled={disabled} onClick={onClick}><Icon size={16} strokeWidth={1.7} /></button>;
}

export const PageEditor = forwardRef<EditorHandle, Props>(function PageEditor(
  { workspace, page, locked, focusTitle, database, searchTarget, headerContent, onSaved, onResolved, onDiscard, onOpenPage, onPickReference,
    onPickBlockReference, onPickViewEmbed, onOpenView, onExitFocusMode, onInsertImages, onPickAttachment, onOpenAttachment }, ref,
) {
  const [title, setTitle] = useState(focusTitle && page.title === '无标题' ? '' : page.title);
  const [status, setStatus] = useState<SaveStatus>('saved');
  const [failure, setFailure] = useState('');
  const [closingDraft, setClosingDraft] = useState(false);
  const [hasConflict, setHasConflict] = useState(false);
  const [comparison, setComparison] = useState<{ saved: Page; local: PageDraft }>();
  const [comparing, setComparing] = useState(false);
  const [exportingDraft, setExportingDraft] = useState(false);
  const [draftNotice, setDraftNotice] = useState<{ message: string; failed?: boolean }>();
  const [insertOpen, setInsertOpen] = useState(false);
  const [slash, setSlash] = useState<SlashMenu | null>(null);
  const [unavailable, setUnavailable] = useState<Props['searchTarget']>();
  const [editLink, setEditLink] = useState<{ href?: string; nameField: boolean }>();
  const [editCaption, setEditCaption] = useState<{ alt: string }>();
  const [linkError, setLinkError] = useState('');
  const [movedIndex, setMovedIndex] = useState<number>();
  const [inputMethodActive, setInputMethodActive] = useState(false);
  const slashStatus = useId();
  const slashRef = useRef<SlashMenu | null>(null);
  const slashList = useRef<HTMLDivElement>(null);
  const titleRef = useRef<HTMLTextAreaElement>(null);
  const notesRef = useRef<HTMLDetailsElement>(null);
  const insertRef = useRef<HTMLDivElement>(null);
  const draft = useRef<PageDraft>({ title: page.title, blocks: page.blocks });
  const composition = useRef<{ target: HTMLElement; accept(): void; finished?: Promise<void> } | undefined>(undefined);
  const [initialContent] = useState(() => fromBlocks(page.blocks));
  const [extensions] = useState(() => [
    ...editorExtensions, AttachmentImage.configure({ workspace, pageId: page.id }), BlockReference.configure({ workspace }),
    ViewEmbed.configure({ workspace, openPage: onOpenPage, openView: onOpenView }),
    AttachmentBlock.configure({ workspace, pageId: page.id, openFile: onOpenAttachment }),
    ToggleBlock, Callout, MathBlock, TableOfContents,
  ]);
  const [queue] = useState(() => createSaveQueue(page.revision, async (next: PageDraft, revision) => {
    const info = await desktop.savePage(workspace, page.id, revision, next);
    onSaved(info);
    return info;
  }, (next, error) => {
    setStatus(next);
    setFailure(next === 'error' ? errorMessage(error) : '');
    setHasConflict(next === 'error' && typeof error === 'object' && error !== null &&
      'code' in error && error.code === 'REVISION_CONFLICT');
    if (next === 'pending' || next === 'saved') setDraftNotice(undefined);
  }));

  function beginComposition(target: HTMLElement, accept: () => void) {
    composition.current = { target, accept };
    setInputMethodActive(true);
  }

  function finishComposition(target: HTMLElement) {
    const current = composition.current;
    if (!current || current.target !== target || current.finished) return;
    // Read the text after the input event and ProseMirror's DOM observer have settled.
    current.finished = Promise.resolve().then(() => {
      if (composition.current !== current) return;
      composition.current = undefined;
      current.accept();
      setInputMethodActive(false);
    });
  }

  async function flush() {
    const current = composition.current;
    if (current && !current.finished) current.target.blur();
    await current?.finished;
    if (composition.current) throw new Error('请先确认或取消输入法组字，再重试。');
    return queue.flush();
  }

  function updateTitle(text: string) {
    const value = text.replace(/[\r\n]+/g, ' ');
    setTitle(value);
    if (composition.current?.target === titleRef.current || value === draft.current.title) return;
    draft.current = { ...draft.current, title: value };
    queue.update(draft.current);
  }

  function updateBlocks(current: Editor) {
    draft.current = { ...draft.current, blocks: toBlocks(current.getJSON()) };
    queue.update(draft.current);
  }

  async function exportDraft(markdown = false) {
    setExportingDraft(true);
    setDraftNotice(undefined);
    try {
      const exported = markdown
        ? await desktop.exportMarkdown(workspace, page.id, draft.current)
        : await desktop.exportPageDraft(workspace, page.id, queue.revision, draft.current);
      if (exported) setDraftNotice({ message: '草稿已导出。' });
    } catch (error) {
      setDraftNotice({ message: errorMessage(error), failed: true });
    } finally {
      setExportingDraft(false);
    }
  }

  async function compareVersions() {
    setComparing(true);
    setDraftNotice(undefined);
    try {
      const saved = await desktop.page(workspace, page.id);
      setComparison({ saved, local: draft.current });
    } catch (error) {
      setDraftNotice({ message: errorMessage(error), failed: true });
    } finally {
      setComparing(false);
    }
  }

  function showSlash(menu: SlashMenu | null) {
    slashRef.current = menu;
    setSlash(menu);
  }

  useEffect(() => {
    if (slash?.selected !== undefined) {
      slashList.current?.querySelector('.highlighted')?.scrollIntoView({ block: 'nearest' });
    }
  }, [slash?.selected]);

  function moveSelection(view: EditorView, direction: -1 | 1) {
    const block = selectedMoveBlock(view.state);
    if (!block) return false;
    const moved = moveBlock(view, block, block.index + (direction < 0 ? -1 : 2));
    if (moved === undefined) return false;
    setMovedIndex(moved);
    return true;
  }

  const editor = useEditor({
    extensions,
    content: initialContent,
    shouldRerenderOnTransaction: true,
    editorProps: {
      attributes: { class: 'document-body', 'aria-label': '页面正文', 'aria-multiline': 'true', 'aria-describedby': slashStatus, role: 'textbox' },
      handleKeyDown(view, event) {
        if (composing(event) || view.composing) return false;
        if (event.altKey && event.shiftKey && !event.metaKey && !event.ctrlKey) {
          if (event.key === 'ArrowUp') return moveSelection(view, -1);
          if (event.key === 'ArrowDown') return moveSelection(view, 1);
        }
        const menu = slashRef.current;
        if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return false;
        if (event.key === 'Escape') {
          if (menu) showSlash(null);
          else if (onExitFocusMode) onExitFocusMode();
          else return false;
          return true;
        }
        if (!menu) return false;
        if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
          const direction = event.key === 'ArrowDown' ? 1 : -1;
          const start = menu.selected ?? (direction === 1 ? -1 : choices.length);
          showSlash({ ...menu, selected: Math.max(0, Math.min(start + direction, choices.length - 1)) });
          return true;
        }
        if (event.key === 'Enter' && menu.selected !== undefined) {
          const option = slashList.current?.querySelectorAll<HTMLButtonElement>('button')[menu.selected];
          if (!option) return false;
          option.click();
          return true;
        }
        return false;
      },
      handlePaste(view, event) {
        const files = Array.from(event.clipboardData?.files ?? []);
        if (files.length) {
          if (view.editable) {
            const { from, to } = view.state.selection;
            onInsertImages(files, { from, to });
          }
          return true;
        }
        const text = event.clipboardData?.getData('text/plain');
        if (!view.editable || !text?.includes('\t') || event.clipboardData?.getData('text/html')
          || view.state.selection.$from.parent.type.spec.code) return false;
        const table = spreadsheetTable(text);
        if (!table) return false;
        const slice = new Slice(Fragment.from(view.state.schema.nodeFromJSON(table)), 0, 0);
        if (pasteTableCells(view, event, slice)) return true;
        view.dispatch(view.state.tr.replaceSelection(slice).scrollIntoView().setMeta('paste', true).setMeta('uiEvent', 'paste'));
        return true;
      },
      handleDrop(view, event) {
        const files = Array.from(event.dataTransfer?.files ?? []);
        if (!files.length) return false;
        if (view.editable) {
          const at = view.posAtCoords({ left: event.clientX, top: event.clientY })?.pos ?? view.state.selection.from;
          onInsertImages(files, at);
        }
        return true;
      },
    },
    onUpdate: ({ editor: current }) => {
      if (!current.view.composing && composition.current?.target !== current.view.dom) updateBlocks(current);
      showSlash(slashPosition(current));
    },
    onSelectionUpdate: ({ editor: current }) => {
      if (slashRef.current && !slashPosition(current)) showSlash(null);
    },
  });

  useEffect(() => { editor?.setEditable(!locked, false); }, [editor, locked]);

  useEffect(() => {
    if (!focusTitle) return;
    titleRef.current?.focus();
    titleRef.current?.select();
  }, [focusTitle]);

  useEffect(() => {
    if (!editor || !searchTarget) return;
    let highlighted = false;
    const frame = requestAnimationFrame(() => {
      if (editor.isDestroyed) return;
      let position: number | undefined;
      editor.state.doc.descendants((node, offset) => {
        if (position !== undefined) return false;
        if (node.attrs.id === searchTarget.blockId) {
          position = offset;
          return false;
        }
      });
      if (position === undefined) {
        setUnavailable(searchTarget);
        return;
      }
      if (notesRef.current) notesRef.current.open = true;
      highlighted = focusLocatedBlock(editor, position);
      if (!highlighted) setUnavailable(searchTarget);
    });
    return () => {
      cancelAnimationFrame(frame);
      if (highlighted && !editor.isDestroyed) markSearchBlock(editor, null);
    };
  }, [editor, searchTarget]);

  useEffect(() => {
    function dismiss(event: PointerEvent) {
      if (event.target instanceof Node && !insertRef.current?.contains(event.target)) setInsertOpen(false);
      if (event.target instanceof Element && !event.target.closest('.slash-menu')) showSlash(null);
    }
    document.addEventListener('pointerdown', dismiss);
    return () => document.removeEventListener('pointerdown', dismiss);
  }, []);

  useImperativeHandle(ref, () => ({
    flush,
    hasUnsavedChanges: () => queue.dirty || Boolean(composition.current),
    insertReference(target) {
      editor?.chain().focus().insertContent([
        { type: 'pageReference', attrs: { pageId: target.id, label: target.title } },
        { type: 'text', text: ' ' },
      ]).run();
    },
    insertBlockReference(target, at) {
      return editor?.chain().focus().insertContentAt(at, { type: 'blockReference', attrs: { ...target } }).run() ?? false;
    },
    insertViewEmbed(target, at) {
      return editor?.chain().focus().insertContentAt(at, { type: 'collectionView', attrs: { ...target } }).run() ?? false;
    },
    insertAttachment(attachmentId, at) {
      return editor?.chain().focus().insertContentAt(at, { type: 'attachmentFile', attrs: { attachmentId } }).run() ?? false;
    },
    pageDraft: () => draft.current,
    insertImages(attachments, at) {
      if (!editor || editor.isDestroyed) return false;
      const { from, to } = editor.state.selection;
      const inserted = editor.commands.insertContentAt(at ?? { from, to }, attachments.map(imageNode));
      if (inserted) requestAnimationFrame(() => {
        if (!editor.isDestroyed) editor.commands.focus();
      });
      return inserted;
    },
  }), [editor, queue]);

  function insert(choice: InsertChoice) {
    if (!editor || locked) return;
    const slash = slashRef.current;
    showSlash(null);
    setInsertOpen(false);
    if (choice.label === '块引用' || choice.label === '数据库视图' || choice.label === '附件块') {
      const { $from } = editor.state.selection;
      const afterBlock = $from.depth ? $from.after(1) : $from.pos;
      const position = slash ? { from: slash.from, to: slash.to } : afterBlock;
      if (choice.label === '数据库视图') onPickViewEmbed(position);
      else if (choice.label === '附件块') onPickAttachment(position);
      else onPickBlockReference(position, slash ? $from.parent.attrs.id : undefined);
      return;
    }
    if (!choice.apply) {
      const { from, to } = slash ?? editor.state.selection;
      onInsertImages(undefined, { from, to });
      return;
    }
    if (slash) editor.chain().focus().deleteRange({ from: slash.from, to: slash.to }).run();
    choice.apply(editor);
  }

  function openLink(href: string) {
    setLinkError('');
    void desktop.openWebLink(href).catch((error: unknown) => setLinkError(errorMessage(error)));
  }

  function openReference(event: MouseEvent<HTMLDivElement>) {
    const source = event.target instanceof Element ? event.target.closest<HTMLButtonElement>('button[data-source-block]') : null;
    if (source?.dataset.sourcePage && source.dataset.sourceBlock) {
      if (!locked) onOpenPage(source.dataset.sourcePage, source.dataset.sourceBlock);
      return;
    }
    const reference = event.target instanceof Element ? event.target.closest<HTMLElement>('[data-page-reference]') : null;
    if (reference?.dataset.pageReference) {
      event.preventDefault();
      onOpenPage(reference.dataset.pageReference);
      return;
    }
    const web = event.target instanceof Element ? event.target.closest<HTMLAnchorElement>('a[data-web-link]') : null;
    if (web) {
      event.preventDefault();
      if (!locked && (event.metaKey || event.ctrlKey)) openLink(web.href);
    }
  }

  function openLinkEditor() {
    if (!editor) return;
    const href = webAddress(editor.getAttributes('link').href);
    setEditLink({ href, nameField: editor.state.selection.empty && !href });
  }

  const selectedNode = editor?.state.selection instanceof NodeSelection ? editor.state.selection.node : undefined;
  const selectedImage = selectedNode?.type.name === 'attachmentImage' ? selectedNode : undefined;
  const movable = editor ? selectedMoveBlock(editor.state) : undefined;

  const visibleStatus = inputMethodActive && status !== 'error' ? 'pending' : status;
  const statusText = inputMethodActive && visibleStatus === 'pending' ? '输入法组字中'
    : { saved: '已保存', pending: '等待保存', saving: '正在保存', error: '保存失败' }[visibleStatus];
  const saveIndicator = <span className={`save-indicator ${visibleStatus}`} role="status">
    {visibleStatus === 'saved' && <Check size={14} />}{visibleStatus === 'saving' && <LoaderCircle size={14} className="spin" />}
    {statusText}
  </span>;

  const body = <>
    <div className="editor-toolbar" role="toolbar" aria-label="编辑工具">
      <div className="insert-anchor" ref={insertRef}
        onKeyDown={disclosureEvents(() => setInsertOpen(false)).onKeyDown}>
        <button type="button" className="insert-trigger" aria-expanded={insertOpen} disabled={locked || !editor}
          onMouseDown={(event) => event.preventDefault()} onClick={() => setInsertOpen(!insertOpen)}>
          <Plus size={15} /> 插入 <ChevronDown size={12} />
        </button>
        {insertOpen && <div className="menu insert-menu">
          {choices.map(({ label, icon: Icon, apply }) => <button key={label} type="button" className="menu-item"
            onMouseDown={(event) => event.preventDefault()} onClick={() => insert({ label, icon: Icon, apply })}><Icon size={17} />{label}</button>)}
        </div>}
      </div>
      <span className="toolbar-divider" />
      <ToolButton label="加粗" icon={Bold} active={editor?.isActive('bold')} disabled={locked} onClick={() => editor?.chain().focus().toggleBold().run()} />
      <ToolButton label="斜体" icon={Italic} active={editor?.isActive('italic')} disabled={locked} onClick={() => editor?.chain().focus().toggleItalic().run()} />
      <ToolButton label="下划线" icon={Underline} active={editor?.isActive('underline')} disabled={locked} onClick={() => editor?.chain().focus().toggleUnderline().run()} />
      <ToolButton label="删除线" icon={Strikethrough} active={editor?.isActive('strike')} disabled={locked} onClick={() => editor?.chain().focus().toggleStrike().run()} />
      <ToolButton label="引用页面" icon={Link2} disabled={locked} onClick={onPickReference} />
      <ToolButton label="网页链接" icon={ExternalLink} active={editor?.isActive('link')}
        disabled={locked || !editor?.can().setLink({ href: 'https://example.com/' })} onClick={openLinkEditor} />
      {selectedImage && <button type="button" className="editor-context-button" disabled={locked}
        onMouseDown={(event) => event.preventDefault()} onClick={() => setEditCaption({ alt: selectedImage.attrs.alt })}>图片说明</button>}
      <span className="toolbar-divider" />
      <ToolButton label="上移块" icon={ArrowUp} shortcut="Alt+Shift+ArrowUp"
        disabled={locked || !movable || !blockMove(movable, movable.index - 1)}
        onClick={() => { if (editor) moveSelection(editor.view, -1); }} />
      <ToolButton label="下移块" icon={ArrowDown} shortcut="Alt+Shift+ArrowDown"
        disabled={locked || !movable || !blockMove(movable, movable.index + 2)}
        onClick={() => { if (editor) moveSelection(editor.view, 1); }} />
      <span className="toolbar-divider" />
      <ToolButton label="撤销" icon={Undo2} disabled={locked || !editor?.can().undo()} onClick={() => editor?.chain().focus().undo().run()} />
      <ToolButton label="重做" icon={Redo2} disabled={locked || !editor?.can().redo()} onClick={() => editor?.chain().focus().redo().run()} />
      {!database && saveIndicator}
    </div>
    {editor?.isActive('table') && <div className="table-tools" role="toolbar" aria-label="表格操作">
      {tableActions.map(({ label, command }) => <button key={command} type="button" className="editor-context-button"
        disabled={locked || !editor.can()[command]()} onMouseDown={(event) => event.preventDefault()}
        onClick={() => { editor.chain().focus()[command]().run(); }}>{label}</button>)}
    </div>}

    {page.blocks.length === 0 && !queue.dirty && editor?.isEmpty && <p className="editor-empty-hint">写下想法，或输入 / 插入内容。</p>}
    {linkError && <p className="save-error" role="alert">{linkError}</p>}
    <div className="editor-surface" onClick={openReference} onKeyDown={(event) => {
      if (event.key !== 'Enter' || locked) return;
      const target = event.target instanceof HTMLElement ? event.target.dataset.pageReference : undefined;
      if (target) { event.preventDefault(); onOpenPage(target); }
      if (event.target instanceof HTMLAnchorElement && event.target.hasAttribute('data-web-link')) {
        event.preventDefault();
        openLink(event.target.href);
      }
    }}>
      <EditorContent editor={editor}
        onCompositionStart={(event) => {
          if (!editor || event.target !== editor.view.dom) return;
          const before = editor.state.doc;
          showSlash(null);
          beginComposition(editor.view.dom, () => {
            if (!editor.isDestroyed && !before.eq(editor.state.doc)) updateBlocks(editor);
          });
        }}
        onCompositionEnd={(event) => {
          if (editor && event.target === editor.view.dom) finishComposition(editor.view.dom);
        }} />
      <BlockDrag editor={editor} disabled={locked} onMove={setMovedIndex} />
    </div>
    <span className="visually-hidden" role="status">{movedIndex === undefined ? '' : `块已移到同级第 ${movedIndex + 1} 个位置。`}</span>
    <span className="visually-hidden" role="status" id={slashStatus}>{slash && !locked && (slash.selected === undefined
      ? '用方向键选择插入内容。' : `${slash.selected + 1} 项，共 ${choices.length} 项。${choices[slash.selected].label}`)}</span>

    {slash && !locked && <div ref={slashList} className="menu slash-menu" style={{ left: slash.left, top: slash.top }} role="group" aria-label="插入内容"
      onKeyDown={(event) => {
        if (event.key !== 'Escape' || composing(event.nativeEvent)) return;
        event.preventDefault();
        showSlash(null);
        editor?.commands.focus();
      }}>
      {choices.map((choice, index) => <button type="button" className={`menu-item${index === slash.selected ? ' highlighted' : ''}`} key={choice.label}
        onFocus={() => showSlash({ ...slash, selected: index })}
        onMouseDown={(event) => event.preventDefault()} onClick={() => insert(choice)}><choice.icon size={17} />{choice.label}</button>)}
    </div>}
    {editLink && <WebLinkDialog initialHref={editLink.href} nameField={editLink.nameField} onClose={() => setEditLink(undefined)}
      onSave={(href, label) => {
        if (editLink.nameField) editor?.chain().focus().insertContent([
          { type: 'text', text: label, marks: [{ type: 'link', attrs: { href } }] },
          { type: 'text', text: ' ' },
        ]).run();
        else editor?.chain().focus().extendMarkRange('link').setLink({ href }).run();
        setEditLink(undefined);
      }} onRemove={() => { editor?.chain().focus().extendMarkRange('link').unsetLink().run(); setEditLink(undefined); }} />}
    {editCaption && <ImageCaptionDialog initial={editCaption.alt} onClose={() => setEditCaption(undefined)}
      onSave={(alt) => { editor?.chain().focus().updateAttributes('attachmentImage', { alt }).run(); setEditCaption(undefined); }} />}
  </>;

  return <article className={`page-editor${database ? ' page-editor-database' : ''}`}>
    {page.cover && <div className="page-cover">
      <AttachmentFigure workspace={workspace} pageId={page.id} defaultAlt="页面封面"
        attachment={{ ...page.cover, alt: '' }} />
    </div>}
    <div className="document-mark">
      <div className={`document-icon icon-${page.icon}`}><PageIcon kind={page.icon} size={30} /></div>
      {database && saveIndicator}
    </div>
    <textarea ref={titleRef} className="document-title" aria-label="页面标题" placeholder="无标题"
      value={title} rows={1} maxLength={200} disabled={locked}
      onChange={(event) => updateTitle(event.currentTarget.value)}
      onCompositionStart={(event) => {
        const target = event.currentTarget;
        beginComposition(target, () => updateTitle(target.value));
      }}
      onCompositionEnd={(event) => finishComposition(event.currentTarget)}
      onKeyDown={(event) => {
        if (event.key === 'Enter' && !composing(event.nativeEvent) && !composition.current) {
          event.preventDefault();
          if (notesRef.current) notesRef.current.open = true;
          editor?.commands.focus('start');
        }
      }} />
    {failure && <div className="save-error">
      <span role="alert" aria-label={failure}>{failure}</span>
      {hasConflict
        ? <button type="button" className="text-button" disabled={locked || exportingDraft || comparing}
          onClick={() => { void compareVersions(); }}>对比版本</button>
        : <button type="button" className="text-button" disabled={locked || exportingDraft}
          onClick={() => { void flush().catch(() => {}); }}>重试</button>}
      <button type="button" className="text-button" disabled={locked || exportingDraft} onClick={() => { void exportDraft(); }}>导出草稿</button>
      <button type="button" className="text-button" disabled={locked || exportingDraft} onClick={() => { void exportDraft(true); }}>另存 Markdown</button>
      <button type="button" className="text-button" disabled={locked || exportingDraft || comparing}
        onClick={() => setClosingDraft(true)}>关闭页面</button>
      {draftNotice && <span role={draftNotice.failed ? 'alert' : 'status'}>{draftNotice.message}</span>}
    </div>}
    {searchTarget && unavailable === searchTarget && <p className="search-location-note" role="status">已打开页面，未找到该匹配块的位置。</p>}
    {headerContent}
    {database ? <details className="database-notes" ref={notesRef}><summary>数据库说明</summary>{body}</details> : body}
    {closingDraft && <CloseDraftDialog busy={locked || status === 'saving'} onClose={() => setClosingDraft(false)}
      onDiscard={() => { if (queue.discard()) onDiscard(); }} />}
    {comparison && <ConflictDialog saved={comparison.saved} local={comparison.local} onClose={() => setComparison(undefined)}
      onSave={async (merged) => {
        const info = await desktop.savePage(workspace, page.id, comparison.saved.revision, merged);
        onResolved({ ...comparison.saved, ...info, blocks: merged.blocks });
      }} />}
  </article>;
});
