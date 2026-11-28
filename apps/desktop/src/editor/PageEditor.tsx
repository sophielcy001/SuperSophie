// SPDX-License-Identifier: AGPL-3.0-or-later

import { getMarkAttributes } from '@tiptap/core';
import { EditorContent, useEditor } from '@tiptap/react';
import type { Editor } from '@tiptap/react';
import { Fragment, Slice } from '@tiptap/pm/model';
import type { Node as ProseNode } from '@tiptap/pm/model';
import { NodeSelection, TextSelection } from '@tiptap/pm/state';
import { handlePaste as pasteTableCells } from '@tiptap/pm/tables';
import type { EditorView } from '@tiptap/pm/view';
import {
  Baseline, Bold, Bookmark as BookmarkIcon, Check, Columns2, Columns3, MessageSquare, ChevronDown, ChevronRight, Code, Code2, ExternalLink, FileText, Search, Heading1, Heading2, Heading3, Image, Italic, Link2,
  List, ListOrdered, ListTodo, ListTree, Lightbulb, LoaderCircle, Minus, Paperclip, Plus, Quote,
  Sigma, Smile, SquareArrowOutUpRight, Strikethrough, Table2, Type, Underline, CalendarDays, Database, GalleryHorizontalEnd,
  GanttChart, SquareKanban as KanbanSquare,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { forwardRef, useCallback, useEffect, useId, useImperativeHandle, useRef, useState } from 'react';
import type { CSSProperties, MouseEvent, ReactNode } from 'react';
import { createPortal } from 'react-dom';
import type { Attachment, BlockTarget, Page, PageDraft, PageInfo, ViewKind, ViewTarget } from '../contracts';
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
import { BlockMenu } from './BlockMenu';
import type { BlockMenuTarget } from './BlockMenu';
import { moveBlock, selectedMoveBlock } from './block-move';
import type { MovableBlock } from './block-move';
import { ConflictDialog } from './ConflictDialog';
import { Callout, MathBlock, TableOfContents, ToggleBlock } from './ContentBlocks';
import { CloseDraftDialog, CommentDialog, ImageCaptionDialog, WebLinkDialog } from './EditorDialogs';
import { editorExtensions, focusLocatedBlock, markSearchBlock } from './extensions';
import {
  blockKinds, blockRangeKey, blockRangeKeyDown, leaveBlockSelection, currentKind, editSelectedBlock, extendBlockSelection, kindLabel, selectBlockAtCaret, selectedUnit, stepBlockSelection, turnInto,
} from './block-actions';
import type { BlockKind } from './block-actions';
import { commentReplies, inlineColors } from './marks';
import { focusFoldTitle } from './notion-input';
import { insertColumns } from './LayoutBlocks';
import { Bookmark, markBookmarkReading } from './BookmarkCard';
import { CommentText } from './CommentText';
import { setPageDirectory } from './page-directory';
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
  /** Comments in the page, in reading order, with the text they are on. */
  comments(): PageComment[];
  /** Moves the caret into a comment, which shows its card. */
  revealComment(from: number): void;
}

export interface PageComment { from: number; text: string; quote: string; replies: number }

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
  /** Pages offered by the "@" and "[[" mention menu; references show their current titles. */
  pages?: PageInfo[];
  /** Creates a sub-page, links it at the caret and opens it ("/页面", as in Notion). */
  onCreateSubpage?(): void;
  /** Creates a database under this page with the given first view and shows it where the command was typed. */
  onCreateDatabase?(view: ViewKind, at: InsertPosition): void;
  /** Opens the icon and cover picker. */
  onAppearance?(): void;
  onPickBlockReference(at: InsertPosition, excluded?: string): void;
  onPickViewEmbed(at: InsertPosition): void;
  onPickAttachment(at: InsertPosition): void;
  onOpenAttachment(id: string): void;
  onInsertImages(files?: File[], at?: InsertPosition): void;
}

interface InsertChoice {
  label: string;
  icon: LucideIcon;
  /** Creates a database inside the page with this view first, as Notion's "/table view" and so on do. */
  database?: ViewKind;
  /** Block kinds are applied with "Turn into", keeping the block's text. */
  kind?: BlockKind;
  /** Extra words for slash filtering: English names and pinyin initials. */
  keywords: string;
  apply?(editor: Editor): void;
}

const choices: InsertChoice[] = [
  { label: '正文', icon: Type, keywords: 'text paragraph p zw zhengwen', kind: 'paragraph' },
  { label: '页面', icon: FileText, keywords: 'page subpage ym yemian zym ziyemian' },
  { label: '一级标题', icon: Heading1, keywords: 'h1 heading title bt biaoti', kind: 'h1' },
  { label: '二级标题', icon: Heading2, keywords: 'h2 heading title bt biaoti', kind: 'h2' },
  { label: '三级标题', icon: Heading3, keywords: 'h3 heading title bt biaoti', kind: 'h3' },
  { label: '项目列表', icon: List, keywords: 'bullet list ul lb liebiao', kind: 'bullet' },
  { label: '编号列表', icon: ListOrdered, keywords: 'numbered ordered list ol bh bianhao', kind: 'ordered' },
  { label: '待办清单', icon: ListTodo, keywords: 'todo task checkbox db daiban', kind: 'task' },
  { label: '引用', icon: Quote, keywords: 'quote yy yinyong', kind: 'quote' },
  { label: '代码块', icon: Code, keywords: 'code dm daima', kind: 'code' },
  { label: '分割线', icon: Minus, keywords: 'divider hr line fgx', apply: (editor) => { editor.chain().focus().setHorizontalRule().run(); } },
  { label: '图片', icon: Image, keywords: 'image picture photo tp tupian' },
  { label: '表格', icon: Table2, keywords: 'table bg biaoge', apply: (editor) => { editor.chain().focus().insertTable({ rows: 3, cols: 3, withHeaderRow: true }).run(); } },
  { label: '数据库', icon: Database, keywords: 'database inline sjk shujuku', database: 'table' },
  { label: '表格视图', icon: Table2, keywords: 'table view database bgst biaoge shitu', database: 'table' },
  { label: '看板视图', icon: KanbanSquare, keywords: 'board kanban view kb kanban', database: 'board' },
  { label: '日历视图', icon: CalendarDays, keywords: 'calendar view rl rili', database: 'calendar' },
  { label: '列表视图', icon: List, keywords: 'list view database lbst liebiao', database: 'list' },
  { label: '画廊视图', icon: GalleryHorizontalEnd, keywords: 'gallery view hl hualang', database: 'gallery' },
  { label: '时间线视图', icon: GanttChart, keywords: 'timeline gantt view sjx shijianxian', database: 'timeline' },
  { label: '两列', icon: Columns2, keywords: 'columns 2 col ll lianglie fenlan', apply: (editor) => insertColumns(editor, 2) },
  { label: '三列', icon: Columns3, keywords: 'columns 3 col sl sanlie fenlan', apply: (editor) => insertColumns(editor, 3) },
  { label: '链接到页面', icon: Link2, keywords: 'link mention page ljdym lianjie' },
  { label: '块引用', icon: SquareArrowOutUpRight, keywords: 'block reference ref kyy' },
  { label: '关联已有数据库', icon: Table2, keywords: 'linked database view gl guanlian sjk shujuku' },
  { label: '折叠', icon: ChevronRight, keywords: 'toggle collapse zd zhedie', kind: 'toggle' },
  { label: '提示块', icon: Lightbulb, keywords: 'callout tip note ts tishi', kind: 'callout' },
  { label: '公式', icon: Sigma, keywords: 'math latex equation gs gongshi', apply: (editor) => { editor.chain().focus().insertContent({ type: 'mathBlock' }).run(); } },
  { label: '目录', icon: ListTree, keywords: 'toc outline contents ml mulu', apply: (editor) => { editor.chain().focus().insertContent({ type: 'tableOfContents' }).run(); } },
  { label: '附件块', icon: Paperclip, keywords: 'file attachment fj fujian' },
  { label: '书签', icon: BookmarkIcon, keywords: 'bookmark web link sq shuqian' },
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
  /** Distance from the top of the window, or from the bottom when the menu opens upward. */
  top?: number;
  bottom?: number;
  /** Text typed after the slash. */
  query: string;
  selected?: number;
}

function slashChoices(query: string, subpages: boolean): InsertChoice[] {
  const needle = query.trim().toLowerCase();
  const available = subpages ? choices : choices.filter((choice) => choice.label !== '页面' && !choice.database);
  if (!needle) return available;
  return available.filter((choice) => choice.label.includes(needle)
    || choice.keywords.split(' ').some((word) => word.startsWith(needle)));
}

/** Text of the caret's block before the caret; inline nodes count as one character, like positions. */
function textBeforeCaret(editor: Editor) {
  const { selection } = editor.state;
  const { $from } = selection;
  if (editor.view.composing || !selection.empty || !$from.parent.isTextblock || $from.parent.type.spec.code) return;
  return $from.parent.textBetween(0, $from.parentOffset, undefined, '\ufffc');
}

function menuPlacement(editor: Editor, height: number): { left: number; top?: number; bottom?: number } {
  const rect = editor.view.coordsAtPos(editor.state.selection.from);
  const left = Math.max(8, Math.min(rect.left, window.innerWidth - 268));
  if (rect.bottom + height + 8 < window.innerHeight) return { left, top: rect.bottom + 8 };
  // Opening upward, the menu's bottom edge sits just above the line, however few entries it shows.
  return { left, bottom: Math.max(8, window.innerHeight - rect.top + 7) };
}

function slashPosition(editor: Editor, subpages: boolean, dismissed?: number): SlashMenu | null {
  const before = textBeforeCaret(editor);
  if (before === undefined) return null;
  // Like Notion: "/" at the start of a block or after a space starts a command,
  // the following letters filter it, and a space ends it.
  const slash = before.lastIndexOf('/');
  const query = before.slice(slash + 1);
  if (slash < 0 || (slash > 0 && !/\s/.test(before[slash - 1])) || query.length > 20 || /\s/.test(query)) return null;
  const to = editor.state.selection.from;
  const from = to - query.length - 1;
  if (from === dismissed || !slashChoices(query, subpages).length) return null;
  return {
    from, to, query,
    // With a filter the first match is ready for Enter; a bare "/" keeps Enter as typing.
    selected: query ? 0 : undefined,
    ...menuPlacement(editor, 324),
  };
}

const mentionLimit = 8;

function mentionChoices(pages: PageInfo[], query: string) {
  const needle = query.trim().toLowerCase();
  return pages.filter((page) => !page.trashRootId && (!needle || page.title.toLowerCase().includes(needle)))
    .sort((a, b) => b.updatedAt - a.updatedAt).slice(0, mentionLimit);
}

/** "@" (not inside a word or address) or "[[" followed by a page name, as in Notion. */
function mentionPosition(editor: Editor, pages: PageInfo[], dismissed?: number): SlashMenu | null {
  const before = textBeforeCaret(editor);
  if (before === undefined) return null;
  const brackets = before.lastIndexOf('[[');
  const at = before.lastIndexOf('@');
  const trigger = Math.max(brackets, at);
  if (trigger < 0) return null;
  const query = before.slice(trigger + (trigger === brackets ? 2 : 1));
  if (trigger === at && trigger > 0 && /[\w.+-]/.test(before[trigger - 1])) return null;
  if (query.length > 30 || /^\s|\ufffc|\]/.test(query)) return null;
  const to = editor.state.selection.from;
  const from = to - (before.length - trigger);
  if (from === dismissed) return null;
  const matches = mentionChoices(pages, query).length;
  // Spaces may be part of a page name, but end the mention once nothing matches.
  if (!matches && query.includes(' ')) return null;
  // Without matches Enter keeps typing; "Search all pages" stays reachable with the arrow keys.
  return { from, to, query, selected: matches ? 0 : undefined, ...menuPlacement(editor, 300) };
}

/** Starts an empty block below the caret's block; list items continue the list when wanted. */
function openLineBelow(editor: Editor, keepList: boolean) {
  const { $from } = editor.state.selection;
  const kind = currentKind($from);
  editor.commands.setTextSelection($from.end());
  if (kind === 'bullet' || kind === 'ordered' || kind === 'task') {
    const item = $from.node(-1).type.name;
    editor.commands.splitListItem(item);
    if (!keepList) {
      for (let guard = 0; guard < 8 && editor.isActive(item); guard++) {
        if (!editor.commands.liftListItem(item)) break;
      }
    }
    return;
  }
  const at = $from.after();
  editor.chain().insertContentAt(at, { type: 'paragraph' }).setTextSelection(at + 1).run();
}

/**
 * Reads the browser selection into the editor state. A click is reported to
 * ProseMirror asynchronously; moving focus to the format bar right after it
 * would otherwise leave the editor acting on the previous caret position.
 */
function syncDomSelection(view: EditorView) {
  // Apply typed text ProseMirror has not read yet, so positions map onto the current text.
  (view as unknown as { domObserver?: { forceFlush(): void } }).domObserver?.forceFlush();
  const dom = view.dom.ownerDocument.getSelection();
  if (!dom?.anchorNode || !dom.focusNode || !view.dom.contains(dom.anchorNode) || !view.dom.contains(dom.focusNode)
    || !(view.state.selection instanceof TextSelection)) return;
  try {
    const anchor = view.posAtDOM(dom.anchorNode, dom.anchorOffset);
    const head = view.posAtDOM(dom.focusNode, dom.focusOffset);
    const { selection } = view.state;
    if (selection.anchor === anchor && selection.head === head) return;
    view.dispatch(view.state.tr.setSelection(TextSelection.between(view.state.doc.resolve(anchor), view.state.doc.resolve(head))));
  } catch {
    // Positions inside node views without a text mapping keep the editor's own selection.
  }
}

/** Places the format bar above the selection, or below it near the top of the page. */
function toolbarPosition(editor: Editor, surface: HTMLElement | null): CSSProperties | undefined {
  if (!surface) return undefined;
  const { view } = editor;
  const { from, to } = view.state.selection;
  try {
    const start = view.coordsAtPos(from);
    const end = view.coordsAtPos(to, -1);
    const bounds = surface.getBoundingClientRect();
    const above = Math.min(start.top, end.top) - bounds.top - 44;
    const left = Math.min(start.left, end.left) - bounds.left - 8;
    return {
      top: above < -36 ? Math.max(start.bottom, end.bottom) - bounds.top + 8 : above,
      left: Math.max(-8, Math.min(left, bounds.width - 470)),
    };
  } catch {
    return undefined;
  }
}

const macKeys = typeof navigator !== 'undefined' && navigator.userAgent.includes('Mac');

const kindIcons: Record<BlockKind, LucideIcon> = {
  paragraph: Type, h1: Heading1, h2: Heading2, h3: Heading3, task: ListTodo, bullet: List, ordered: ListOrdered,
  toggle: ChevronRight, code: Code, quote: Quote, callout: Lightbulb,
};

function ColorChoice({ label, text, background, active, onPick }: {
  label: string; text?: string; background?: string; active: boolean; onPick(): void;
}) {
  return <button type="button" className="menu-item" aria-pressed={active} onMouseDown={(event) => event.preventDefault()} onClick={onPick}>
    <span className="color-swatch" data-text-color={text} data-background-color={background} aria-hidden>A</span>{label}
    {active && <Check size={14} className="menu-check" aria-hidden />}
  </button>;
}

function ToolButton({ label, icon: Icon, onClick, active, disabled, shortcut }: {
  label: string; icon: LucideIcon; onClick(): void; active?: boolean; disabled?: boolean; shortcut?: string;
}) {
  return <button type="button" className={`tool-button${active ? ' is-active' : ''}`} title={label}
    aria-label={label} aria-pressed={active} aria-keyshortcuts={shortcut} onMouseDown={(event) => event.preventDefault()}
    disabled={disabled} onClick={onClick}><Icon size={16} strokeWidth={1.7} /></button>;
}

export const PageEditor = forwardRef<EditorHandle, Props>(function PageEditor(
  { workspace, page, locked, focusTitle, database, searchTarget, headerContent, onSaved, onResolved, onDiscard, onOpenPage, onPickReference, pages = [], onCreateSubpage, onCreateDatabase,
    onAppearance, onPickBlockReference, onPickViewEmbed, onOpenView, onExitFocusMode, onInsertImages, onPickAttachment, onOpenAttachment }, ref,
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
  const [toolbarMenu, setToolbarMenu] = useState<'insert' | 'turn' | 'color'>();
  const insertOpen = toolbarMenu === 'insert';
  const setInsertOpen = (open: boolean) => setToolbarMenu(open ? 'insert' : undefined);
  const [toolbarPinned, setToolbarPinned] = useState(false);
  const surfaceRef = useRef<HTMLDivElement>(null);
  const toolbarRef = useRef<HTMLDivElement>(null);
  // The save status sits in the window's top bar when one is present.
  const [saveSlot, setSaveSlot] = useState<HTMLElement | null>();
  useEffect(() => { setSaveSlot(document.getElementById('editor-save-status')); }, []);
  const [slash, setSlash] = useState<SlashMenu | null>(null);
  const [mention, setMention] = useState<SlashMenu | null>(null);
  const mentionRef = useRef<SlashMenu | null>(null);
  const mentionList = useRef<HTMLDivElement>(null);
  const subpagesRef = useRef(Boolean(onCreateSubpage));
  subpagesRef.current = Boolean(onCreateSubpage);
  const pagesRef = useRef(pages);
  pagesRef.current = pages;
  // References read titles while the editor mounts, so fill the directory before it does.
  useState(() => setPageDirectory(pages));
  useEffect(() => { setPageDirectory(pages); }, [pages]);
  /** Start of a "/" or "@" the user dismissed with Escape; it stays plain text. */
  const dismissedMenu = useRef<number>(undefined);
  const [unavailable, setUnavailable] = useState<Props['searchTarget']>();
  const [editLink, setEditLink] = useState<{ href?: string; nameField: boolean }>();
  const [editCaption, setEditCaption] = useState<{ alt: string }>();
  const [editComment, setEditComment] = useState<{ initial?: string; reply?: boolean }>();
  const [bookmarkAt, setBookmarkAt] = useState<number>();
  /** After pasting a web address: keep it as a link or turn it into a bookmark card, as in Notion. */
  const [pasted, setPasted] = useState<SlashMenu & { url: string }>();
  const pastedRef = useRef<SlashMenu & { url: string }>(undefined);
  const showPasted = (menu?: SlashMenu & { url: string }) => { pastedRef.current = menu; setPasted(menu); };
  const [linkError, setLinkError] = useState('');
  const [movedIndex, setMovedIndex] = useState<number>();
  const [blockMenu, setBlockMenu] = useState<BlockMenuTarget>();
  const closeBlockMenu = useCallback(() => setBlockMenu(undefined), []);
  const openBlockMenu = useCallback((block: MovableBlock, grip: DOMRect, doc: ProseNode) => {
    setBlockMenu({ block, doc, left: grip.left - 252 > 8 ? grip.left - 252 : grip.right + 4, top: grip.top });
  }, []);
  const [inputMethodActive, setInputMethodActive] = useState(false);
  const slashStatus = useId();
  const editorRef = useRef<Editor | null>(null);
  /** The latest menu actions, for key handlers created with the editor. */
  const actions = useRef({ insert: (_choice: InsertChoice) => {}, insertMention: (_page: PageInfo) => {}, searchMorePages: () => {} });
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
    ToggleBlock, Callout, MathBlock, TableOfContents, Bookmark.configure({ workspace, pageId: page.id }),
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

  function showMention(menu: SlashMenu | null) {
    mentionRef.current = menu;
    setMention(menu);
  }

  /** Re-reads which inline menu the caret is in after typing or moving. */
  function updateMenus(current: Editor) {
    const dismissed = dismissedMenu.current;
    if (dismissed !== undefined && !/^[/@[]$/.test(current.state.doc.textBetween(dismissed, Math.min(dismissed + 1, current.state.doc.content.size)))) {
      dismissedMenu.current = undefined;
    }
    const nextSlash = slashPosition(current, subpagesRef.current, dismissedMenu.current);
    const nextMention = nextSlash ? null : mentionPosition(current, pagesRef.current, dismissedMenu.current);
    // Keep the highlighted row while the filter text stays the same.
    const keep = (previous: SlashMenu | null, next: SlashMenu | null) => previous && next && previous.from === next.from
      && previous.query === next.query ? { ...next, selected: previous.selected } : next;
    showSlash(keep(slashRef.current, nextSlash));
    showMention(keep(mentionRef.current, nextMention));
  }

  function insertMention(target: PageInfo) {
    const menu = mentionRef.current;
    showMention(null);
    if (!editor || locked || !menu) return;
    editor.chain().focus().insertContentAt({ from: menu.from, to: menu.to }, [
      { type: 'pageReference', attrs: { pageId: target.id, label: target.title || '无标题' } },
      { type: 'text', text: ' ' },
    ]).run();
  }

  function searchMorePages() {
    const menu = mentionRef.current;
    showMention(null);
    if (!editor || locked || !menu) return;
    editor.chain().focus().deleteRange({ from: menu.from, to: menu.to }).run();
    onPickReference();
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

  /** Alt+F10 moves keyboard focus into the format bar, as in other rich text editors. */
  function openToolbar() {
    setToolbarPinned(true);
    requestAnimationFrame(() => toolbarRef.current?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus());
  }

  const editor = useEditor({
    extensions,
    content: initialContent,
    shouldRerenderOnTransaction: true,
    // A page that starts with a database or another atom block would open with that block selected,
    // so the first Backspace would delete it. Start in the first line of text instead.
    onCreate({ editor: created }) {
      const { doc, selection } = created.state;
      if (!(selection instanceof NodeSelection)) return;
      const text = TextSelection.findFrom(doc.resolve(0), 1, true);
      if (text) created.view.dispatch(created.state.tr.setSelection(text).setMeta('addToHistory', false));
    },
    editorProps: {
      attributes: {
        class: 'document-body', 'aria-label': '页面正文', 'aria-multiline': 'true', 'aria-describedby': slashStatus,
        'aria-keyshortcuts': 'Alt+F10', role: 'textbox',
        // As in Notion, no automatic capitals or corrections (macOS applies them in the WebKit window).
        autocapitalize: 'off', autocorrect: 'off',
      },
      handleKeyDown(view, event) {
        if (composing(event) || view.composing) return false;
        // A click or caret key (End, Home…) right before this key may not have reached the editor
        // state yet; read pending text and selection first so the key acts where the caret is.
        syncDomSelection(view);
        if (event.altKey && event.key === 'F10') {
          event.preventDefault();
          syncDomSelection(view);
          openToolbar();
          return true;
        }
        // Alt+Shift+arrows, and Notion's ⌘⇧ / Ctrl+Shift arrows, move the block.
        if (event.shiftKey && (event.altKey ? !event.metaKey && !event.ctrlKey : event.metaKey !== event.ctrlKey)) {
          if (event.key === 'ArrowUp') return moveSelection(view, -1);
          if (event.key === 'ArrowDown') return moveSelection(view, 1);
        }
        // As in Notion, ⌘K (Ctrl+K) with text selected edits its link; without a selection it stays search.
        if (event.key.toLowerCase() === 'k' && event.metaKey !== event.ctrlKey && !event.altKey && !event.shiftKey
          && !view.state.selection.empty && view.state.selection instanceof TextSelection && view.editable) {
          setEditLink({ href: webAddress(getMarkAttributes(view.state, view.state.schema.marks.link).href), nameField: false });
          return true;
        }
        // ⌘⇧M (Ctrl+Shift+M) comments on the selected text, as in Notion.
        if (event.key.toLowerCase() === 'm' && event.shiftKey && event.metaKey !== event.ctrlKey && !event.altKey && view.editable
          && (!view.state.selection.empty || editorRef.current?.isActive('comment'))) {
          setEditComment({ initial: editorRef.current?.getAttributes('comment').text });
          return true;
        }
        // ⌘/ (Ctrl+/) opens the block menu for the caret's block, the keyboard route to the grip menu.
        if (event.key === '/' && event.metaKey !== event.ctrlKey && !event.altKey && !event.shiftKey) {
          const block = selectedMoveBlock(view.state);
          if (!block) return false;
          const caret = view.coordsAtPos(view.state.selection.from);
          setBlockMenu({ block, doc: view.state.doc, left: caret.left, top: caret.bottom + 6 });
          return true;
        }
        const pasteMenu = pastedRef.current;
        if (pasteMenu) {
          if (!event.altKey && !event.ctrlKey && !event.metaKey && !event.shiftKey) {
            if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
              showPasted({ ...pasteMenu, selected: pasteMenu.selected === 0 ? 1 : 0 });
              return true;
            }
            if (event.key === 'Enter') {
              if (pasteMenu.selected === 1) makeBookmark(pasteMenu);
              else showPasted();
              return true;
            }
            if (event.key === 'Escape') {
              showPasted();
              return true;
            }
          }
          showPasted();
        }
        // Notion's block selection: Esc selects the block, ↑／↓ step between blocks, Enter edits again,
        // Delete／⌘C／⌘D act on the whole block, and a second Esc leaves focus mode.
        const current = editorRef.current;
        const plain = !event.altKey && !event.ctrlKey && !event.metaKey && !event.shiftKey;
        if (current && event.shiftKey && !event.altKey && !event.ctrlKey && !event.metaKey
          && (event.key === 'ArrowUp' || event.key === 'ArrowDown')
          && extendBlockSelection(current, event.key === 'ArrowUp' ? -1 : 1)) return true;
        if (current && plain && blockRangeKeyDown(current, event.key)) return true;
        if (current && plain && !slashRef.current && !mentionRef.current) {
          if (selectedUnit(current)) {
            if (event.key === 'ArrowUp' || event.key === 'ArrowDown') return stepBlockSelection(current, event.key === 'ArrowUp' ? -1 : 1);
            if (event.key === 'Enter' && editSelectedBlock(current)) return true;
            if (event.key === 'Escape') {
              if (!onExitFocusMode) return false;
              onExitFocusMode();
              return true;
            }
          } else if (event.key === 'Escape' && view.editable && selectBlockAtCaret(current)) return true;
        }
        const menu = slashRef.current ?? mentionRef.current;
        const show = slashRef.current ? showSlash : showMention;
        if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return false;
        if (event.key === 'Escape') {
          if (menu) {
            dismissedMenu.current = menu.from;
            show(null);
          } else if (onExitFocusMode) onExitFocusMode();
          else return false;
          return true;
        }
        if (!menu) return false;
        const options = (slashRef.current ? slashList : mentionList).current?.querySelectorAll<HTMLButtonElement>('button.menu-item');
        if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
          const direction = event.key === 'ArrowDown' ? 1 : -1;
          const count = options?.length ?? 0;
          if (!count) return false;
          const start = menu.selected ?? (direction === 1 ? -1 : count);
          show({ ...menu, selected: (start + direction + count) % count });
          return true;
        }
        if ((event.key === 'Enter' || event.key === 'Tab') && menu.selected !== undefined) {
          const option = options?.[menu.selected];
          if (option) {
            option.click();
            return true;
          }
          // Typed text can reach the editor just before Enter, ahead of the menu's render:
          // act on the highlighted entry from the menu's own state.
          const latest = actions.current;
          if (slashRef.current) {
            const choice = slashChoices(slashRef.current.query, subpagesRef.current)[menu.selected];
            if (!choice) return false;
            latest.insert(choice);
            return true;
          }
          const target = mentionChoices(pagesRef.current, menu.query)[menu.selected];
          if (target) latest.insertMention(target);
          else latest.searchMorePages();
          return true;
        }
        return false;
      },
      // Typing over selected blocks continues at their end instead of deleting them.
      handleTextInput(view, _from, _to, text) {
        const current = editorRef.current;
        if (!current || view.composing || !leaveBlockSelection(current)) return false;
        view.dispatch(view.state.tr.insertText(text).scrollIntoView());
        return true;
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
        const url = view.editable && view.state.selection.empty && !view.state.selection.$from.parent.type.spec.code
          ? webAddress(text?.trim()) : undefined;
        if (url && text) {
          const pastedText = text.trim();
          // Let the paste become a link first, then offer the bookmark card under it.
          requestAnimationFrame(() => {
            const current = editorRef.current;
            if (!current || current.isDestroyed) return;
            const to = current.state.selection.from;
            const from = to - pastedText.length;
            if (from < 0 || current.state.doc.textBetween(from, to) !== pastedText) return;
            showPasted({ from, to, url, query: '', selected: 0, ...menuPlacement(current, 90) });
          });
          return false;
        }
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
      updateMenus(current);
    },
    onSelectionUpdate: ({ editor: current }) => {
      if (pastedRef.current && current.state.selection.from !== pastedRef.current.to) showPasted();
      if ((slashRef.current && !slashPosition(current, subpagesRef.current, dismissedMenu.current))
        || (mentionRef.current && !mentionPosition(current, pagesRef.current, dismissedMenu.current))) updateMenus(current);
    },
  });

  editorRef.current = editor;
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

  // Like Notion, a click in the empty space below the last block puts the caret on the last line.
  useEffect(() => {
    const scroll = surfaceRef.current?.closest<HTMLElement>('.document-scroll');
    if (!editor || !scroll) return;
    function below(event: MouseEvent | globalThis.MouseEvent) {
      const target = event.target;
      if (!editor || editor.isDestroyed || !editor.isEditable || event.button !== 0 || !(target instanceof Element)
        || target.closest('.document-body, button, a, input, textarea, select, [role="dialog"], .menu')) return;
      const bounds = editor.view.dom.getBoundingClientRect();
      if (event.clientY <= bounds.bottom || event.clientX < bounds.left - 40 || event.clientX > bounds.right + 40) return;
      // Continue on the last line when it is empty, otherwise start a new one, as in Notion.
      // Runs after the click's own focus handling, so the window's keyboard focus lands in the editor.
      const last = editor.state.doc.lastChild;
      if (last?.type.name !== 'paragraph' || last.content.size > 0) {
        editor.commands.insertContentAt(editor.state.doc.content.size, { type: 'paragraph' });
      }
      editor.commands.setTextSelection(editor.state.doc.content.size - 1);
      // Focus now: Tiptap's focus command waits a frame, and keys typed right after the click would be lost.
      editor.view.focus();
    }
    scroll.addEventListener('click', below);
    return () => scroll.removeEventListener('click', below);
  }, [editor]);

  useEffect(() => {
    function dismiss(event: PointerEvent) {
      if (event.target instanceof Node && !toolbarRef.current?.contains(event.target)) setToolbarMenu(undefined);
      if (event.target instanceof Element && !event.target.closest('.slash-menu')) { showSlash(null); showMention(null); }
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
      if (!editor?.chain().focus().insertContentAt(at, { type: 'collectionView', attrs: { ...target } }).run()) return false;
      // Like Notion, typing goes on in a line below the database instead of the database staying selected.
      const { state } = editor;
      let after: number | undefined;
      state.doc.descendants((node, pos) => {
        if (node.type.name !== 'collectionView' || node.attrs.viewId !== target.viewId) return;
        const end = pos + node.nodeSize;
        if (after === undefined || Math.abs(end - state.selection.from) < Math.abs(after - state.selection.from)) after = end;
      });
      if (after === undefined) return true;
      const tr = state.tr;
      if (!tr.doc.resolve(after).nodeAfter?.isTextblock) tr.insert(after, state.schema.nodes.paragraph.create());
      tr.setSelection(TextSelection.create(tr.doc, after + 1));
      editor.view.dispatch(tr.scrollIntoView());
      return true;
    },
    insertAttachment(attachmentId, at) {
      return editor?.chain().focus().insertContentAt(at, { type: 'attachmentFile', attrs: { attachmentId } }).run() ?? false;
    },
    pageDraft: () => draft.current,
    comments() {
      const found: PageComment[] = [];
      editor?.state.doc.descendants((node, position) => {
        if (!node.isText) return;
        const mark = node.marks.find((item) => item.type.name === 'comment');
        if (!mark) return;
        const last = found.at(-1);
        // Adjacent text pieces under one comment (e.g. partly bold) read as one quote.
        if (last && last.text === mark.attrs.text && last.from + last.quote.length === position) last.quote += node.text ?? '';
        else found.push({ from: position, text: String(mark.attrs.text), quote: node.text ?? '', replies: commentReplies(mark.attrs.replies)?.length ?? 0 });
      });
      return found;
    },
    revealComment(from) {
      if (!editor || editor.isDestroyed || from + 1 > editor.state.doc.content.size) return;
      editor.chain().focus().setTextSelection(from + 1).scrollIntoView().run();
    },
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
    let slash = slashRef.current;
    showSlash(null);
    setInsertOpen(false);
    const sourceBlock = slash ? editor.state.selection.$from.parent.attrs.id : undefined;
    if (slash) {
      const { $from } = editor.state.selection;
      if (slash.from !== $from.start() || slash.to !== $from.end()) {
        // Like Notion, a command typed after other text adds its block below that text.
        // The space that started the command goes too.
        const spaced = /\s/.test(editor.state.doc.textBetween(Math.max($from.start(), slash.from - 1), slash.from));
        editor.chain().focus().deleteRange({ from: slash.from - (spaced && slash.to === $from.end() ? 1 : 0), to: slash.to }).run();
        openLineBelow(editor, choice.kind === currentKind($from));
        const { from } = editor.state.selection;
        slash = { ...slash, from, to: from };
      }
    }
    if (choice.label === '页面' || choice.label === '链接到页面' || choice.label === '书签') {
      if (slash && slash.from !== slash.to) editor.chain().focus().deleteRange({ from: slash.from, to: slash.to }).run();
      if (choice.label === '页面') onCreateSubpage?.();
      else if (choice.label === '书签') setBookmarkAt(editor.state.selection.from);
      else onPickReference();
      return;
    }
    if (choice.database || choice.label === '块引用' || choice.label === '关联已有数据库' || choice.label === '附件块') {
      const { $from } = editor.state.selection;
      const afterBlock = $from.depth ? $from.after(1) : $from.pos;
      const position = slash ? { from: slash.from, to: slash.to } : afterBlock;
      if (choice.database) onCreateDatabase?.(choice.database, position);
      else if (choice.label === '关联已有数据库') onPickViewEmbed(position);
      else if (choice.label === '附件块') onPickAttachment(position);
      else onPickBlockReference(position, sourceBlock);
      return;
    }
    if (!choice.apply && !choice.kind) {
      const { from, to } = slash ?? editor.state.selection;
      onInsertImages(undefined, { from, to });
      return;
    }
    if (slash && slash.from !== slash.to) editor.chain().focus().deleteRange({ from: slash.from, to: slash.to }).run();
    if (!choice.kind) {
      choice.apply?.(editor);
      return;
    }
    turnInto(editor, choice.kind);
    if (choice.kind === 'toggle') {
      const { $from } = editor.state.selection;
      for (let depth = $from.depth; depth > 0; depth--) {
        if ($from.node(depth).type.name !== 'toggleBlock') continue;
        if (!$from.node(depth).attrs.title) focusFoldTitle(editor, $from.before(depth));
        break;
      }
    }
  }

  actions.current = { insert, insertMention, searchMorePages };

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

  /** Replaces a pasted web address with a bookmark card (below its text when the line has more). */
  function makeBookmark(menu: SlashMenu & { url: string }) {
    showPasted();
    const current = editorRef.current;
    if (!current || !current.isEditable || current.state.doc.textBetween(menu.from, menu.to) === '') return;
    const $from = current.state.doc.resolve(menu.from);
    const id = crypto.randomUUID();
    const card = current.schema.nodes.bookmark.create({ id, url: menu.url });
    const tr = current.state.tr;
    if (menu.from === $from.start() && menu.to === $from.end()) tr.replaceWith($from.before(), $from.after(), card);
    else {
      tr.delete(menu.from, menu.to);
      tr.insert(tr.mapping.map($from.after()), card);
    }
    current.view.dispatch(tr.scrollIntoView());
    current.view.focus();
    fillBookmark(id, menu.url);
  }

  /**
   * As in Notion, a new bookmark card reads the page's title, description and preview image
   * (saved as an attachment of this page). The card keeps its address if the page cannot be read.
   */
  function fillBookmark(id: string, url: string) {
    markBookmarkReading(id, true);
    void desktop.fetchWebTitle(url, workspace, page.id).then((summary) => {
      const current = editorRef.current;
      if (!current || current.isDestroyed) return;
      let at: number | undefined;
      current.state.doc.descendants((node, position) => {
        if (at !== undefined) return false;
        if (node.type.name === 'bookmark' && node.attrs.id === id && node.attrs.url === url) at = position;
      });
      if (at === undefined) return;
      const node = current.state.doc.nodeAt(at)!;
      current.view.dispatch(current.state.tr.setNodeMarkup(at, undefined, {
        ...node.attrs,
        title: node.attrs.title ?? summary.title ?? null,
        description: node.attrs.description ?? summary.description ?? null,
        attachmentId: node.attrs.attachmentId ?? summary.image?.attachmentId ?? null,
        mediaType: node.attrs.mediaType ?? summary.image?.mediaType ?? null,
      }));
    }).catch(() => { /* offline or unreadable: the card shows its address */ })
      .finally(() => markBookmarkReading(id, false));
  }

  function openLinkEditor() {
    if (!editor) return;
    const href = webAddress(editor.getAttributes('link').href);
    setEditLink({ href, nameField: editor.state.selection.empty && !href });
  }

  const selectedNode = editor?.state.selection instanceof NodeSelection ? editor.state.selection.node : undefined;
  const selectedImage = selectedNode?.type.name === 'attachmentImage' ? selectedNode : undefined;

  const selectionRange = editor?.state.selection;
  // A Notion-style range of whole blocks is not a text selection: no format bar for it.
  const textSelected = Boolean(editor && selectionRange && !selectionRange.empty && !selectedNode && editor.view.hasFocus()
    && !blockRangeKey.getState(editor.state));
  const showToolbar = Boolean(editor && !locked && (textSelected || selectedImage || toolbarPinned || insertOpen));
  const toolbarStyle = showToolbar && editor ? toolbarPosition(editor, surfaceRef.current) : undefined;

  const visibleStatus = inputMethodActive && status !== 'error' ? 'pending' : status;
  const statusText = inputMethodActive && visibleStatus === 'pending' ? '输入法组字中'
    : { saved: '已保存', pending: '等待保存', saving: '正在保存', error: '保存失败' }[visibleStatus];
  const saveIndicator = <span className={`save-indicator ${visibleStatus}`} role="status">
    {visibleStatus === 'saved' && <Check size={14} />}{visibleStatus === 'saving' && <LoaderCircle size={14} className="spin" />}
    {statusText}
  </span>;

  const mentionPages = mention ? mentionChoices(pages, mention.query) : [];
  const caretKind = editor && !selectedNode ? currentKind(editor.state.selection.$from) : undefined;
  const textColor: string | undefined = editor?.getAttributes('textColor').color;
  const backgroundColor: string | undefined = editor?.getAttributes('backgroundColor').color;
  function applyColor(mark: 'textColor' | 'backgroundColor', color?: string) {
    setToolbarMenu(undefined);
    const chain = editor?.chain().focus();
    if (color) chain?.setMark(mark, { color }).run();
    else chain?.unsetMark(mark).run();
  }

  // The comment under the caret shows in a card below the line, as Notion's comment popover.
  const commentText: string | undefined = editor && !locked && editor.view.hasFocus() && editor.state.selection.empty
    && editor.isActive('comment') ? editor.getAttributes('comment').text : undefined;
  const commentCard = commentText && editor ? (() => {
    const caret = editor.view.coordsAtPos(editor.state.selection.from);
    return {
      text: commentText, replies: commentReplies(editor.getAttributes('comment').replies) ?? [],
      style: { top: caret.bottom + 6, left: Math.max(8, Math.min(caret.left - 12, window.innerWidth - 308)) },
    };
  })() : undefined;

  const formatBar = showToolbar && <div ref={toolbarRef} className="format-bar" role="toolbar" aria-label="编辑工具"
    style={toolbarStyle}
    onBlur={(event) => {
      if (!event.currentTarget.contains(event.relatedTarget as globalThis.Node | null)) setToolbarPinned(false);
    }}
    onKeyDown={(event) => {
      if (event.key !== 'Escape' || composing(event.nativeEvent)) return;
      if (toolbarMenu) {
        event.preventDefault();
        setToolbarMenu(undefined);
        return;
      }
      event.preventDefault();
      setToolbarPinned(false);
      editor?.commands.focus();
    }}>
      <div className="insert-anchor" ref={insertRef}
        onKeyDown={disclosureEvents(() => setInsertOpen(false)).onKeyDown}>
        <button type="button" className="insert-trigger" aria-expanded={insertOpen} disabled={locked || !editor}
          onMouseDown={(event) => event.preventDefault()} onClick={() => setInsertOpen(!insertOpen)}>
          <Plus size={15} /> 插入 <ChevronDown size={12} />
        </button>
        {insertOpen && <div className="menu insert-menu">
          {slashChoices('', Boolean(onCreateSubpage)).map((choice) => <button key={choice.label} type="button" className="menu-item"
            onMouseDown={(event) => event.preventDefault()} onClick={() => insert(choice)}><choice.icon size={17} />{choice.label}</button>)}
        </div>}
      </div>
      {caretKind && <div className="insert-anchor">
        <button type="button" className="insert-trigger" aria-expanded={toolbarMenu === 'turn'} aria-label={`转换为，当前为${kindLabel(caretKind)}`}
          disabled={locked} onMouseDown={(event) => event.preventDefault()}
          onClick={() => setToolbarMenu(toolbarMenu === 'turn' ? undefined : 'turn')}>
          {kindLabel(caretKind)} <ChevronDown size={12} />
        </button>
        {toolbarMenu === 'turn' && <div className="menu insert-menu" role="group" aria-label="转换为">
          <div className="menu-heading" aria-hidden>转换为</div>
          {blockKinds.map(({ kind, label, shortcut }) => { const Icon = kindIcons[kind]; return <button key={kind} type="button" className="menu-item"
            aria-pressed={kind === caretKind} onMouseDown={(event) => event.preventDefault()}
            onClick={() => { setToolbarMenu(undefined); if (editor) turnInto(editor, kind); }}>
            <Icon size={17} />{label}
            {kind === caretKind && <Check size={14} className="menu-check" aria-hidden />}
            {shortcut && <kbd className="command-shortcut">{macKeys ? `⌘⌥${shortcut}` : `Ctrl+Alt+${shortcut}`}</kbd>}
          </button>; })}
        </div>}
      </div>}
      <span className="toolbar-divider" />
      <ToolButton label="加粗" icon={Bold} shortcut="Control+B Meta+B" active={editor?.isActive('bold')} disabled={locked} onClick={() => editor?.chain().focus().toggleBold().run()} />
      <ToolButton label="斜体" icon={Italic} shortcut="Control+I Meta+I" active={editor?.isActive('italic')} disabled={locked} onClick={() => editor?.chain().focus().toggleItalic().run()} />
      <ToolButton label="下划线" icon={Underline} shortcut="Control+U Meta+U" active={editor?.isActive('underline')} disabled={locked} onClick={() => editor?.chain().focus().toggleUnderline().run()} />
      <ToolButton label="删除线" icon={Strikethrough} shortcut="Control+Shift+S Meta+Shift+S" active={editor?.isActive('strike')} disabled={locked} onClick={() => editor?.chain().focus().toggleStrike().run()} />
      <ToolButton label="行内代码" icon={Code2} shortcut="Control+E Meta+E" active={editor?.isActive('code')} disabled={locked} onClick={() => editor?.chain().focus().toggleCode().run()} />
      <div className="insert-anchor">
        <button type="button" className={`tool-button color-trigger${textColor || backgroundColor ? ' is-active' : ''}`} title="颜色" aria-label="文字颜色"
          aria-expanded={toolbarMenu === 'color'} disabled={locked} onMouseDown={(event) => event.preventDefault()}
          onClick={() => setToolbarMenu(toolbarMenu === 'color' ? undefined : 'color')}>
          <span className="color-sample" data-text-color={textColor} data-background-color={backgroundColor}><Baseline size={16} strokeWidth={1.7} /></span>
        </button>
        {toolbarMenu === 'color' && <div className="menu insert-menu color-menu" role="group" aria-label="颜色">
          <div className="menu-heading" aria-hidden>文字颜色</div>
          <ColorChoice label="默认文字" active={!textColor} onPick={() => applyColor('textColor')} />
          {inlineColors.map((color) => <ColorChoice key={color.id} label={`${color.label}文字`} text={color.id}
            active={textColor === color.id} onPick={() => applyColor('textColor', color.id)} />)}
          <div className="menu-heading" aria-hidden>背景颜色</div>
          <ColorChoice label="默认背景" active={!backgroundColor} onPick={() => applyColor('backgroundColor')} />
          {inlineColors.map((color) => <ColorChoice key={color.id} label={`${color.label}背景`} background={color.id}
            active={backgroundColor === color.id} onPick={() => applyColor('backgroundColor', color.id)} />)}
        </div>}
      </div>
      <span className="toolbar-divider" />
      <ToolButton label="网页链接" icon={ExternalLink} shortcut="Control+K Meta+K" active={editor?.isActive('link')}
        disabled={locked || !editor?.can().setLink({ href: 'https://example.com/' })} onClick={openLinkEditor} />
      <ToolButton label="引用页面" icon={Link2} disabled={locked} onClick={onPickReference} />
      <ToolButton label="评论" icon={MessageSquare} shortcut="Control+Shift+M Meta+Shift+M" active={editor?.isActive('comment')}
        disabled={locked || (!textSelected && !editor?.isActive('comment'))}
        onClick={() => setEditComment({ initial: editor?.getAttributes('comment').text })} />
      {selectedImage && <button type="button" className="editor-context-button" disabled={locked}
        onMouseDown={(event) => event.preventDefault()} onClick={() => setEditCaption({ alt: selectedImage.attrs.alt })}>图片说明</button>}
    </div>;

  const body = <>
    {editor?.isActive('table') && <div className="table-tools" role="toolbar" aria-label="表格操作">
      {tableActions.map(({ label, command }) => <button key={command} type="button" className="editor-context-button"
        disabled={locked || !editor.can()[command]()} onMouseDown={(event) => event.preventDefault()}
        onClick={() => { editor.chain().focus()[command]().run(); }}>{label}</button>)}
    </div>}

    {linkError && <p className="save-error" role="alert">{linkError}</p>}
    <div ref={surfaceRef} className="editor-surface" onClick={openReference} onKeyDown={(event) => {
      if (event.key !== 'Enter' || locked) return;
      const target = event.target instanceof HTMLElement ? event.target.dataset.pageReference : undefined;
      if (target) { event.preventDefault(); onOpenPage(target); }
      if (event.target instanceof HTMLAnchorElement && event.target.hasAttribute('data-web-link')) {
        event.preventDefault();
        openLink(event.target.href);
      }
    }}>
      {formatBar}
      <EditorContent editor={editor}
        onCompositionStart={(event) => {
          if (!editor || event.target !== editor.view.dom) return;
          // An input method starting over selected blocks writes after them, not over them.
          leaveBlockSelection(editor);
          const before = editor.state.doc;
          showSlash(null);
          beginComposition(editor.view.dom, () => {
            if (!editor.isDestroyed && !before.eq(editor.state.doc)) updateBlocks(editor);
          });
        }}
        onCompositionEnd={(event) => {
          if (editor && event.target === editor.view.dom) finishComposition(editor.view.dom);
        }} />
      <BlockDrag editor={editor} disabled={locked} onMove={setMovedIndex} onMenu={openBlockMenu} />
    </div>
    {saveSlot !== undefined && (saveSlot ? createPortal(saveIndicator, saveSlot) : saveIndicator)}
    <span className="visually-hidden" role="status">{movedIndex === undefined ? '' : `块已移到同级第 ${movedIndex + 1} 个位置。`}</span>
    <span className="visually-hidden" role="status" id={slashStatus}>{slash && !locked && (slash.selected === undefined
      ? '用方向键选择插入内容。' : `${slash.selected + 1} 项，共 ${slashChoices(slash.query, Boolean(onCreateSubpage)).length} 项。${slashChoices(slash.query, Boolean(onCreateSubpage))[slash.selected]?.label ?? ''}`)}
      {mention && !locked && `提及页面：${mention.selected !== undefined && mention.selected < mentionPages.length
        ? `${mention.selected + 1} 项，共 ${mentionPages.length} 项。${mentionPages[mention.selected].title || '无标题'}`
        : mention.selected === undefined ? '没有匹配的页面' : '搜索全部页面'}`}</span>

    {slash && !locked && <div ref={slashList} className="menu slash-menu" style={{ left: slash.left, top: slash.top, bottom: slash.bottom }} role="group" aria-label="插入内容"
      onKeyDown={(event) => {
        if (event.key !== 'Escape' || composing(event.nativeEvent)) return;
        event.preventDefault();
        showSlash(null);
        editor?.commands.focus();
      }}>
      {!slash.query && <div className="menu-heading" aria-hidden>基本块</div>}
      {slashChoices(slash.query, Boolean(onCreateSubpage)).map((choice, index) => <button type="button" className={`menu-item${index === slash.selected ? ' highlighted' : ''}`} key={choice.label}
        onFocus={() => showSlash({ ...slash, selected: index })}
        onMouseDown={(event) => event.preventDefault()} onClick={() => insert(choice)}><choice.icon size={17} />{choice.label}</button>)}
    </div>}
    {blockMenu && editor && !locked && <BlockMenu editor={editor} target={blockMenu} kindIcons={kindIcons}
      shortcut={(keys) => macKeys ? `⌘${keys}` : `Ctrl+${keys.replace('⇧', 'Shift+')}`}
      onClose={closeBlockMenu} onMoved={setMovedIndex} />}
    {pasted && !locked && <div className="menu slash-menu paste-menu" style={{ left: pasted.left, top: pasted.top, bottom: pasted.bottom }} role="group" aria-label="粘贴网址">
      <button type="button" className={`menu-item${pasted.selected === 0 ? ' highlighted' : ''}`}
        onMouseDown={(event) => event.preventDefault()} onClick={() => showPasted()}><Link2 size={17} />保留为链接</button>
      <button type="button" className={`menu-item${pasted.selected === 1 ? ' highlighted' : ''}`}
        onMouseDown={(event) => event.preventDefault()} onClick={() => makeBookmark(pasted)}><BookmarkIcon size={17} />书签卡片</button>
    </div>}
    {commentCard && <div className="comment-card" role="note" aria-label="评论" style={commentCard.style}>
      <p><CommentText text={commentCard.text} onOpenPage={(id) => onOpenPage(id)} /></p>
      {commentCard.replies.length > 0 && <ol className="comment-replies" aria-label="回复">
        {commentCard.replies.map((reply, index) => <li key={index}><CommentText text={reply} onOpenPage={(id) => onOpenPage(id)} /></li>)}
      </ol>}
      <div className="comment-card-actions">
        <button type="button" className="text-button" onMouseDown={(event) => event.preventDefault()}
          onClick={() => setEditComment({ reply: true })}>回复</button>
        <button type="button" className="text-button" onMouseDown={(event) => event.preventDefault()}
          onClick={() => setEditComment({ initial: commentCard.text })}>编辑</button>
        <button type="button" className="text-button" onMouseDown={(event) => event.preventDefault()}
          onClick={() => editor?.chain().focus().extendMarkRange('comment').unsetMark('comment').run()}>解决</button>
      </div>
    </div>}
    {mention && !locked && <div ref={mentionList} className="menu slash-menu mention-menu" style={{ left: mention.left, top: mention.top, bottom: mention.bottom }}
      role="group" aria-label="提及页面" onKeyDown={(event) => {
        if (event.key !== 'Escape' || composing(event.nativeEvent)) return;
        event.preventDefault();
        showMention(null);
        editor?.commands.focus();
      }}>
      <div className="menu-heading" aria-hidden>链接到页面</div>
      {mentionPages.map((target, index) => <button type="button" key={target.id}
        className={`menu-item${index === mention.selected ? ' highlighted' : ''}`}
        onFocus={() => showMention({ ...mention, selected: index })}
        onMouseDown={(event) => event.preventDefault()} onClick={() => insertMention(target)}>
        <PageIcon kind={target.icon} size={17} /><span className="mention-title">{target.title || '无标题'}</span></button>)}
      {!mentionPages.length && <p className="menu-empty">没有匹配的页面</p>}
      <button type="button" className={`menu-item${mention.selected === mentionPages.length ? ' highlighted' : ''}`}
        onFocus={() => showMention({ ...mention, selected: mentionPages.length })}
        onMouseDown={(event) => event.preventDefault()} onClick={searchMorePages}><Search size={17} />搜索全部页面…</button>
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
    {editComment && <CommentDialog initial={editComment.initial} reply={editComment.reply} pages={pages} onClose={() => setEditComment(undefined)}
      onSave={(text) => {
        const current = editor?.getAttributes('comment');
        const chain = editor?.chain().focus();
        const attrs = editComment.reply && current?.text
          ? { text: current.text, replies: [...(commentReplies(current.replies) ?? []), text].slice(-50) }
          : { text, replies: commentReplies(current?.replies) ?? null };
        const caret = editor?.state.selection.empty ? editor.state.selection.from : undefined;
        (caret !== undefined ? chain?.extendMarkRange('comment') : chain)?.setMark('comment', attrs)
          // Keep the caret where it was, so the card stays open and shows the new reply.
          .command(({ tr }) => { if (caret !== undefined) tr.setSelection(TextSelection.create(tr.doc, caret)); return true; }).run();
        setEditComment(undefined);
      }}
      onRemove={() => { editor?.chain().focus().extendMarkRange('comment').unsetMark('comment').run(); setEditComment(undefined); }} />}
    {bookmarkAt !== undefined && <WebLinkDialog heading="书签" labelText="标题（可选）" nameField onClose={() => setBookmarkAt(undefined)}
      onFetchTitle={(href) => desktop.fetchWebTitle(href, workspace, page.id)}
      onRemove={() => setBookmarkAt(undefined)}
      onSave={(url, label, summary) => {
        const at = Math.min(bookmarkAt, editor?.state.doc.content.size ?? 0);
        const id = crypto.randomUUID();
        editor?.chain().focus().insertContentAt({ from: at, to: at }, { type: 'bookmark', attrs: {
          id, url, title: label && label !== url ? label : null, description: summary?.description ?? null,
          attachmentId: summary?.image?.attachmentId ?? null, mediaType: summary?.image?.mediaType ?? null,
        } }).run();
        setBookmarkAt(undefined);
        // Read the page automatically unless it was already read in the dialog.
        if (!summary) fillBookmark(id, url);
      }} />}
    {editCaption && <ImageCaptionDialog initial={editCaption.alt} onClose={() => setEditCaption(undefined)}
      onSave={(alt) => { editor?.chain().focus().updateAttributes('attachmentImage', { alt }).run(); setEditCaption(undefined); }} />}
  </>;

  // Default icons read as "no icon" in the header, as in Notion; the tree still shows them.
  const plainIcon = page.icon === 'file' || (database && page.icon === 'database');

  return <article className={`page-editor${database ? ' page-editor-database' : ''}`}>
    {page.cover && <div className="page-cover">
      <AttachmentFigure workspace={workspace} pageId={page.id} defaultAlt="页面封面"
        attachment={{ ...page.cover, alt: '' }} />
    </div>}
    {!plainIcon && <div className="document-mark">
      {onAppearance
        ? <button type="button" className={`document-icon icon-${page.icon}`} disabled={locked}
          aria-label="更换图标" title="更换图标" onClick={onAppearance}><PageIcon kind={page.icon} size={30} /></button>
        : <div className={`document-icon icon-${page.icon}`}><PageIcon kind={page.icon} size={30} /></div>}
    </div>}
    {onAppearance && !locked && (plainIcon || !page.cover) && <div className="document-controls">
      {plainIcon && <button type="button" className="document-control" onClick={onAppearance}>
        <Smile size={16} aria-hidden />添加图标</button>}
      {!page.cover && <button type="button" className="document-control" onClick={onAppearance}>
        <Image size={16} aria-hidden />添加封面</button>}
    </div>}
    <textarea ref={titleRef} className="document-title" aria-label="页面标题" placeholder="无标题" autoCapitalize="off" autoCorrect="off"
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
