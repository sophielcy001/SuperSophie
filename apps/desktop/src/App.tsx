// SPDX-License-Identifier: AGPL-3.0-or-later

import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import {
  Archive, BookOpen, ChevronDown, ChevronRight, CircleAlert, Command as CommandIcon, Database, DatabaseBackup, Download, FilePlus2, Link,
  History, Image as ImageIcon, Inbox, LayoutTemplate, LibraryBig, Link2, ListChecks, LoaderCircle, Maximize2, MessageSquare, Minimize2, MoreHorizontal,
  PanelLeft, Paperclip, Plug, Plus, RefreshCw, Search, Star, SunMoon, Trash2, Upload, X,
} from 'lucide-react';
import { useEffect, useEffectEvent, useMemo, useRef, useState } from 'react';
import type {
  Attachment, BlockTarget, Collection, CollectionRecord, CollectionSummary, CollectionView, ContentImport, CsvExport, HistoryEntry,
  ImportOriginal, ImportOutcome, Navigation,
  MoveImpact, Page, PageInfo, PdfTarget, ViewKind, ProposalAction, ProposalReview, PropertyDefinition, PropertyValue, RecordDraft, ResolvedLocation,
  TemplateSummary, ViewTarget, Workspace, WorkspaceOverview,
} from './contracts';
import { errorMessage } from './contracts';
import { desktop } from './desktop';
import { BackupDialog } from './BackupDialog';
import { HistoryDialog, Modal, MoveDialog, TrashDialog, WorkspaceDialog } from './dialogs';
import type { TrashReview } from './dialogs';
import { PageEditor } from './editor/PageEditor';
import { PageAppearanceDialog } from './editor/PageAppearanceDialog';
import type { EditorHandle, InsertPosition, PageComment } from './editor/PageEditor';
import { CommentText } from './editor/CommentText';
import { BacklinkDialog, BlockPicker } from './editor/ReferenceDialogs';
import { maxImageBytes } from './editor/rich-content';
import { PageTree, pageTrail } from './PageTree';
import { AttachmentDialog, attachmentRenamed } from './attachments/Attachments';
import type { AttachmentChange } from './attachments/Attachments';
import { CollectionPane } from './collections/CollectionPane';
import { CsvDialog } from './collections/CsvDialog';
import { RecordDialog } from './collections/dialogs';
import { RecordPanel } from './collections/RecordPanel';
import { autoField, computedField } from './collections/fields';
import { ExternalRecordUpdates, useExternalRecordUpdate } from './collections/recordUpdates';
import { PagePicker } from './search/PagePicker';
import { useSearchIndex } from './search/useSearchIndex';
import { ApplyTemplateDialog, ImportDialog, SaveTemplateDialog } from './library/LibraryDialogs';
import { LibraryNavigation } from './library/LibraryNavigation';
import { IntegrationsDialog } from './integrations/IntegrationsDialog';
import { SyncDialog } from './sync/SyncDialog';
import { ProposalDialog } from './integrations/ProposalDialog';
import { BatchBar } from './organize/BatchControls';
import type { BatchSelection } from './organize/BatchControls';
import { EmbedPicker, EmbeddedDatabases } from './editor/ViewEmbed';
import type { EmbeddedDatabaseHost } from './editor/ViewEmbed';
import { AppearanceDialog, themeOptions, useTheme } from './Appearance';
import { CommandPalette } from './CommandPalette';
import type { Command } from './CommandPalette';
import { composing, disclosureEvents } from './keyboard';
import { prepareWindowsIme } from './windows-ime';

type View = 'search' | 'reference' | 'workspace' | 'move' | 'archive' | 'trash' | 'backup' | 'history'
  | 'issues' | 'attachments' | 'record' | 'import' | 'importOriginal' | 'saveTemplate' | 'applyTemplate'
  | 'batchMove' | 'batchArchive' | 'csvExport' | 'integrations' | 'moveImpact' | 'proposals' | 'location'
  | 'blockReference' | 'referenceBlock' | 'backlinks' | 'embedView' | 'commands' | 'appearance' | 'pageAppearance' | 'sync' | null;

const emptyNavigation: Navigation = { inboxPageId: null, favorites: [], recentPages: [] };
const macKeys = typeof navigator !== 'undefined' && navigator.userAgent.includes('Mac');

interface Selection {
  page: Page;
  epoch: number;
  focusTitle: boolean;
  searchTarget?: { blockId: string };
}

export function App() {
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [issues, setIssues] = useState<WorkspaceOverview['issues']>([]);
  const [workspace, setWorkspace] = useState<Workspace>();
  const [pages, setPages] = useState<PageInfo[]>([]);
  const [collections, setCollections] = useState<CollectionSummary[]>([]);
  const [selectedViews, setSelectedViews] = useState<Record<string, string>>({});
  const [recordUpdate, setRecordUpdate] = useState(0);
  const [navigation, setNavigation] = useState<Navigation>(emptyNavigation);
  const [libraryParent, setLibraryParent] = useState<string | null>(null);
  const [importReport, setImportReport] = useState<ImportOutcome[]>();
  const [migration, setMigration] = useState<'pending' | 'cancelling'>();
  const [templates, setTemplates] = useState<TemplateSummary[]>([]);
  const [batchIds, setBatchIds] = useState<Set<string>>();
  const [csvDialog, setCsvDialog] = useState<{ collection: string; view: CollectionView }>();
  const [csvResult, setCsvResult] = useState<CsvExport>();
  const [recordDialog, setRecordDialog] = useState<{
    collection: Collection; record: CollectionRecord; attachments: Attachment[]; conflicts: number;
  }>();
  const [selection, setSelection] = useState<Selection>();
  const [view, setView] = useState<View>(null);
  const [trashReview, setTrashReview] = useState<TrashReview>();
  const [referencePicker, setReferencePicker] = useState<{ at: InsertPosition; excluded?: string; editor: EditorHandle }>();
  const [referencePage, setReferencePage] = useState<{ page: Page; preferred: string | null }>();
  const [incomingLocation, setIncomingLocation] = useState<string>();
  const [shownLocation, setShownLocation] = useState<{ url: string; title: string }>();
  const [moveImpacts, setMoveImpacts] = useState<MoveImpact[]>([]);
  const [proposalPage, setProposalPage] = useState<string | null>(null);
  const [revisions, setRevisions] = useState<HistoryEntry[]>([]);
  const [historyFields, setHistoryFields] = useState<PropertyDefinition[]>([]);
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [preferredAttachment, setPreferredAttachment] = useState<string>();
  const [preferredPdfPage, setPreferredPdfPage] = useState<number>();
  const [attachmentNotice, setAttachmentNotice] = useState('');
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [banner, setBanner] = useState<{ text: string; type: 'error' | 'notice'; clearOnSave?: true }>();
  const [workspaceMenu, setWorkspaceMenu] = useState(false);
  const [pageMenu, setPageMenu] = useState(false);
  /** The open page's comments, listed from the page menu as Notion's comment view. */
  const [pageComments, setPageComments] = useState<PageComment[]>();
  const [sidebar, setSidebar] = useState(true);
  const [focusMode, setFocusMode] = useState(false);
  const { theme, setTheme } = useTheme();
  const [startupError, setStartupError] = useState('');
  const [backupFailures, setBackupFailures] = useState<ReadonlySet<string>>(() => new Set());
  const [attempt, setAttempt] = useState(0);
  const editor = useRef<EditorHandle>(null);
  const windowsIme = useRef<Promise<void> | undefined>(undefined);
  const running = useRef(false);
  const workspaceAnchor = useRef<HTMLDivElement>(null);
  const pageAnchor = useRef<HTMLDivElement>(null);
  const documentScroll = useRef<HTMLDivElement>(null);
  const focusControl = useRef<HTMLButtonElement>(null);
  const workspaceId = workspace?.id;
  const externalRecordUpdate = useExternalRecordUpdate(workspaceId);
  const byId = useMemo(() => new Map(pages.map((page) => [page.id, page])), [pages]);
  const current = selection ? (byId.get(selection.page.id) ?? selection.page) : undefined;
  const collectionByPage = useMemo(() => new Map(collections.map((item) => [item.containerPageId, item.id])), [collections]);
  const relationTargets = useMemo(() => collections.flatMap(({ id, containerPageId }) => {
    const page = byId.get(containerPageId);
    return page && !page.trashRootId ? [{ id, title: pageTrail(byId, containerPageId).map((node) => node.title).join(' / ') }] : [];
  }), [collections, byId]);
  const collectionId = current ? collectionByPage.get(current.id) : undefined;
  const ownerId = current?.parentId ? collectionByPage.get(current.parentId) : undefined;
  const favorite = Boolean(current && navigation.favorites.includes(current.id));
  const collectionKey = `${workspaceId}/${collectionId}`;
  const trail = current ? pageTrail(byId, current.id) : [];
  const batchPages = useMemo(() => batchIds ? pages.filter((page) => batchIds.has(page.id) && !page.trashRootId) : [], [batchIds, pages]);
  const batchRoots = useMemo(() => batchPages.filter((page) =>
    !pageTrail(byId, page.id).slice(0, -1).some((parent) => batchIds?.has(parent.id))), [batchPages, batchIds, byId]);
  const batch: BatchSelection | undefined = batchIds ? {
    ids: batchIds,
    select(ids, checked) {
      setBatchIds((previous) => {
        if (!previous) return previous;
        const next = new Set(previous);
        for (const id of ids) { if (checked) next.add(id); else next.delete(id); }
        return next;
      });
    },
  } : undefined;
  const mayMoveBatch = batchRoots.length > 0 && batchRoots.every((page) => !page.parentId || !collectionByPage.has(page.parentId));
  const searchIndex = useSearchIndex(workspaceId, pages);

  function selectPage(page: Page | undefined, focusTitle = false, blockId?: string | null) {
    setSelection((previous) => page ? {
      page, epoch: (previous?.epoch ?? 0) + 1, focusTitle,
      searchTarget: blockId ? { blockId } : undefined,
    } : undefined);
  }

  function recordPage(info: PageInfo) {
    setPages((previous) => previous.some((page) => page.id === info.id)
      ? previous.map((page) => page.id === info.id ? info : page)
      : [...previous, info]);
    setBanner((previous) => previous?.clearOnSave ? undefined : previous);
  }

  async function showWorkspace(target: Workspace, location?: ResolvedLocation) {
    const [items, definitions, shortcuts] = await Promise.all([
      desktop.pages(target.id), desktop.collections(target.id), desktop.navigation(target.id),
    ]);
    const recent = items.find((item) => item.id === shortcuts.recentPages[0] && !item.trashRootId);
    const first = recent ?? items.find((item) => !item.trashRootId);
    const page = location?.page ?? (first ? await desktop.page(target.id, first.id) : undefined);
    setWorkspace(target);
    setPages(items);
    setCollections(definitions);
    setNavigation(shortcuts);
    setBatchIds(undefined);
    selectPage(page, false, location?.blockId);
    setWorkspaceMenu(false);
    setView(null);
  }

  useEffect(() => {
    let alive = true;
    async function initialize() {
      setLoading(true);
      setStartupError('');
      try {
        if (!isTauri()) throw new Error('请从知页桌面程序打开。');
        windowsIme.current ??= prepareWindowsIme();
        const overview = await desktop.workspaces();
        if (!alive) return;
        setWorkspaces(overview.workspaces);
        setIssues(overview.issues);
        if (overview.workspaces.length) {
          try {
            await showWorkspace(overview.workspaces[0]);
          } catch (error) {
            if (alive) setBanner({ type: 'error', text: errorMessage(error) });
          }
        }
        await windowsIme.current;
      } catch (error) {
        if (alive) setStartupError(errorMessage(error));
      } finally {
        if (alive) setLoading(false);
      }
    }
    void initialize();
    return () => { alive = false; };
  }, [attempt]);

  const pagesRef = useRef(pages);
  pagesRef.current = pages;
  const selectionRef = useRef(selection);
  selectionRef.current = selection;
  useEffect(() => {
    if (!isTauri() || !workspaceId) return;
    let alive = true;
    const subscription = desktop.onSyncApplied((id) => {
      if (!alive || id !== workspaceId) return;
      const open = selectionRef.current?.page.id;
      const before = open ? pagesRef.current.find((page) => page.id === open)?.revision : undefined;
      void reloadLibrary(id).then(async (items) => {
        const after = items.find((page) => page.id === open);
        if (!alive || !open || !after || before === undefined || after.revision === before) return;
        if (editor.current?.hasUnsavedChanges()) {
          setBanner({ type: 'notice', text: '当前页面已由同步更新。保存时会提示对比两个版本。' });
          return;
        }
        const updated = await desktop.page(id, open);
        if (alive && selectionRef.current?.page.id === open) selectPage(updated);
      }).catch((error: unknown) => {
        if (alive) setBanner({ type: 'error', text: errorMessage(error) });
      });
    }).catch(() => undefined);
    return () => {
      alive = false;
      void subscription.then((stop) => stop?.());
    };
  }, [workspaceId]);

  const selectedId = selection?.page.id;
  useEffect(() => {
    if (!workspaceId || !selectedId) return;
    let alive = true;
    void desktop.visitPage(workspaceId, selectedId).then((state) => {
      if (alive) setNavigation((previous) => ({ ...previous, recentPages: state.recentPages }));
    }).catch((error: unknown) => {
      if (alive) setBanner({ type: 'error', text: errorMessage(error) });
    });
    return () => { alive = false; };
  }, [workspaceId, selectedId]);

  useEffect(() => {
    function dismiss(event: PointerEvent) {
      if (!(event.target instanceof Node)) return;
      if (!workspaceAnchor.current?.contains(event.target)) setWorkspaceMenu(false);
      if (!pageAnchor.current?.contains(event.target)) setPageMenu(false);
    }
    document.addEventListener('pointerdown', dismiss);
    return () => document.removeEventListener('pointerdown', dismiss);
  }, []);

  useEffect(() => {
    function preventFileNavigation(event: DragEvent) {
      if (event.dataTransfer?.types.includes('Files')) event.preventDefault();
    }
    window.addEventListener('dragover', preventFileNavigation);
    window.addEventListener('drop', preventFileNavigation);
    return () => {
      window.removeEventListener('dragover', preventFileNavigation);
      window.removeEventListener('drop', preventFileNavigation);
    };
  }, []);

  async function perform(job: () => Promise<void>, flush = true) {
    if (running.current) return;
    running.current = true;
    setBusy(true);
    setBanner(undefined);
    try {
      if (flush) await editor.current?.flush();
      await job();
    } catch (error) {
      setBanner({ type: 'error', text: errorMessage(error) });
    } finally {
      running.current = false;
      setBusy(false);
    }
  }

  useEffect(() => {
    if (!isTauri()) return;
    let alive = true;
    let dispose: (() => void) | undefined;
    let reads: Promise<void> = Promise.resolve();
    const receive = () => {
      reads = reads.then(async () => {
        if (!alive) return;
        const url = await desktop.takeLocation();
        if (alive && url) setIncomingLocation(url);
      }).catch((error: unknown) => {
        if (alive) setBanner({ type: 'error', text: errorMessage(error) });
      });
    };
    void desktop.onLocation(receive).then((unlisten) => {
      if (!alive) unlisten();
      else {
        dispose = unlisten;
        receive();
      }
    }).catch((error: unknown) => {
      if (alive) setBanner({ type: 'error', text: errorMessage(error) });
    });
    return () => { alive = false; dispose?.(); };
  }, []);

  useEffect(() => {
    if (!isTauri() || !workspaceId) return;
    let alive = true;
    let dispose: (() => void) | undefined;
    let latest = 0;
    const refresh = (checked: string) => {
      if (!alive || checked !== workspaceId) return;
      const request = ++latest;
      void desktop.backupStatus(workspaceId).then(
        (status) => Boolean(status.checkError || status.lastAttempt?.state === 'failed'),
        () => true,
      ).then((failed) => {
        if (!alive || request !== latest) return;
        setBackupFailures((previous) => {
          if (previous.has(workspaceId) === failed) return previous;
          const next = new Set(previous);
          if (failed) next.add(workspaceId);
          else next.delete(workspaceId);
          return next;
        });
      });
    };
    void desktop.onBackupChecked(refresh).then((unlisten) => {
      if (alive) {
        dispose = unlisten;
        refresh(workspaceId);
      } else unlisten();
    }, () => refresh(workspaceId));
    return () => { alive = false; dispose?.(); };
  }, [workspaceId]);

  const followLocation = useEffectEvent((url: string) => {
    void perform(async () => {
      const target = await desktop.resolveLocation(url);
      await showWorkspace(target.workspace, target);
      if (!target.blockId) documentScroll.current?.scrollTo({ top: 0 });
      if (target.missingBlock) {
        setBanner({ type: 'notice', text: '链接中的正文块已不可用。已打开所属页面。' });
      }
      if (target.missingPdf) {
        setBanner({ type: 'notice', text: '链接所指的 PDF 原件版本不可用。已打开所属页面。' });
      } else if (target.pdf) {
        await showPdfAttachment(target.workspace.id, target.page.id, target.pdf);
      }
    });
  });

  useEffect(() => {
    if (!incomingLocation || loading || busy || startupError || view || running.current) return;
    setIncomingLocation(undefined);
    followLocation(incomingLocation);
  }, [incomingLocation, loading, busy, startupError, view]);

  function showLocation(pageId: string, blockId: string | null = null, pdf?: PdfTarget) {
    if (!workspaceId) return;
    setPageMenu(false);
    void perform(async () => {
      const target = pdf && { attachmentId: pdf.attachmentId, sha256: pdf.sha256, pageNumber: pdf.pageNumber };
      const url = await desktop.makeLocation(workspaceId, pageId, blockId, target);
      setShownLocation({ url, title: pdf ? 'PDF 页码链接' : blockId ? '正文块链接' : '页面链接' });
      setView('location');
    }, false);
  }

  useEffect(() => {
    if (!isTauri()) return;
    const window = getCurrentWindow();
    let alive = true;
    async function requestClose(complete: (saved: boolean) => Promise<void>) {
      const maySave = alive && !running.current;
      try {
        if (!maySave) {
          if (alive) setBanner({ type: 'notice', text: '当前操作完成后即可关闭窗口。' });
          await complete(false);
          return;
        }
        running.current = true;
        setBusy(true);
        let saved = false;
        try {
          await editor.current?.flush();
          saved = true;
        } catch (error) {
          if (alive) {
            setView(null);
            setBanner({ type: 'error', text: `页面尚未保存。${errorMessage(error)}`, clearOnSave: true });
          }
        }
        await complete(saved && alive);
      } catch (error) {
        if (alive) setBanner({ type: 'error', text: `无法关闭窗口。${errorMessage(error)}` });
      } finally {
        if (maySave) {
          running.current = false;
          if (alive) setBusy(false);
        }
      }
    }
    const subscriptions = [
      window.onCloseRequested((event) => {
        event.preventDefault();
        return requestClose(async (saved) => { if (saved) await window.destroy(); });
      }),
      desktop.onSystemQuit((request) => requestClose((saved) => desktop.finishSystemQuit(request, saved))),
    ];
    for (const subscription of subscriptions) {
      void subscription.catch((error: unknown) => {
        if (alive) setBanner({ type: 'error', text: errorMessage(error) });
      });
    }
    return () => {
      alive = false;
      for (const subscription of subscriptions) void subscription.then((stop) => stop(), () => {});
    };
  }, []);

  function toggleFocusMode() {
    const previous = document.activeElement;
    setWorkspaceMenu(false);
    setPageMenu(false);
    setFocusMode((enabled) => !enabled);
    requestAnimationFrame(() => {
      if (previous instanceof HTMLElement && !previous.getClientRects().length && document.activeElement === document.body) {
        focusControl.current?.focus();
      }
    });
  }

  function showCommands() {
    setWorkspaceMenu(false);
    setPageMenu(false);
    setView('commands');
  }

  const onShortcut = useEffectEvent((event: KeyboardEvent) => {
    if (event.defaultPrevented || composing(event) || event.altKey) return;
    const key = event.key.toLowerCase();
    const modifier = event.ctrlKey || event.metaKey;
    if (modifier && !event.shiftKey && key === 's') {
      event.preventDefault();
      if (!running.current) void editor.current?.flush().catch(() => {});
      return;
    }
    const focusShortcut = (!modifier && !event.shiftKey && key === 'escape' && focusMode)
      || (modifier && event.shiftKey && key === 'f');
    const commandShortcut = modifier && event.shiftKey && key === 'p';
    // ⌘K, and ⌘P as in Notion's Quick Find; with text selected the editor takes ⌘K for links.
    const searchShortcut = modifier && !event.shiftKey && (key === 'k' || key === 'p') && workspaceId;
    if (!focusShortcut && !commandShortcut && !searchShortcut) return;
    if (view || running.current || event.repeat || document.querySelector('dialog[open]')) return;
    event.preventDefault();
    if (focusShortcut) toggleFocusMode();
    else if (commandShortcut) showCommands();
    else setView('search');
  });
  useEffect(() => {
    const shortcuts = (event: KeyboardEvent) => onShortcut(event);
    window.addEventListener('keydown', shortcuts);
    return () => window.removeEventListener('keydown', shortcuts);
  }, []);

  const onNativeCommand = useEffectEvent((command: string) => {
    if (loading || startupError || view || running.current || document.querySelector('dialog[open]')) return;
    if (command === 'save') {
      void perform(async () => { await editor.current?.flush(); }, false);
    } else if (command === 'commands') {
      showCommands();
    } else {
      availableCommands().find(({ id }) => id === command)?.run();
    }
  });
  useEffect(() => {
    if (!isTauri()) return;
    let alive = true;
    let dispose: (() => void) | undefined;
    void desktop.onNativeCommand((command) => {
      if (alive) onNativeCommand(command);
    }).then((unlisten) => {
      if (alive) dispose = unlisten;
      else unlisten();
    }).catch((error: unknown) => {
      if (alive) setBanner({ type: 'error', text: errorMessage(error) });
    });
    return () => { alive = false; dispose?.(); };
  }, []);

  function openPage(id: string, blockId?: string | null) {
    if (!workspaceId) return;
    if (id === current?.id) {
      if (blockId !== undefined) {
        setSelection((previous) => previous && {
          ...previous, searchTarget: blockId ? { blockId } : undefined,
        });
        if (!blockId) documentScroll.current?.scrollTo({ top: 0 });
      }
      setView(null);
      return;
    }
    void perform(async () => {
      const page = await desktop.page(workspaceId, id);
      if (page.trashRootId) throw new Error('这个页面在回收站中。');
      recordPage(page);
      selectPage(page, false, blockId);
      setView(null);
    });
  }

  function pickBlockReference(at: InsertPosition, excluded?: string) {
    if (!editor.current) return;
    setReferencePicker({ at, excluded, editor: editor.current });
    setView('blockReference');
  }

  function pickSourcePage(info: PageInfo, preferred: string | null) {
    if (!workspaceId || !referencePicker) return;
    void perform(async () => {
      const draft = info.id === current?.id ? referencePicker.editor.pageDraft() : undefined;
      const page = draft ? { ...info, ...draft } : await desktop.page(workspaceId, info.id);
      if (page.trashRootId) throw new Error('这个页面在回收站中。');
      setReferencePage({ page, preferred });
      setView('referenceBlock');
    }, false);
  }

  function insertPickedBlock(insert: (editor: EditorHandle, at: InsertPosition) => boolean, failure: string) {
    if (!referencePicker) return;
    const { editor: before, at } = referencePicker;
    setView(null);
    setReferencePicker(undefined);
    setReferencePage(undefined);
    requestAnimationFrame(() => {
      if (editor.current === before && !insert(before, at)) {
        setBanner({ type: 'error', text: failure });
      }
    });
  }

  function insertBlockReference(target: BlockTarget) {
    insertPickedBlock((editor, at) => editor.insertBlockReference(target, at), '无法在这里插入块引用。');
  }

  function pickViewEmbed(at: InsertPosition) {
    if (!editor.current) return;
    setReferencePicker({ at, editor: editor.current });
    setView('embedView');
  }

  function openEmbeddedView(page: string, target: ViewTarget) {
    setSelectedViews((previous) => ({ ...previous, [workspaceId + '/' + target.collectionId]: target.viewId }));
    openPage(page);
  }

  function rebuildSearch() {
    if (!workspaceId) return;
    void perform(async () => {
      searchIndex.restart(await desktop.rebuildSearchIndex(workspaceId));
    }, false);
  }

  /** Creates a page; a `preset` fills one property of a new record, e.g. the board column it was added in. */
  function addPage(parent: string | null = null, preset?: { collection: string; field: string; value: PropertyValue }) {
    if (!workspaceId) return;
    setPageMenu(false);
    void perform(async () => {
      let page = await desktop.createPage(workspaceId, parent);
      setPages((previous) => [...previous, page]);
      if (preset) {
        page = await writeRecordField(workspaceId, preset.collection, page.id, preset.field, preset.value);
        recordPage(page);
        setRecordUpdate((previous) => previous + 1);
      }
      selectPage(page, true);
    });
  }

  /** "/页面" in the editor: create a child page, link it where the command was typed, then open it. */
  function createSubpage() {
    const target = editor.current;
    const parent = current?.id;
    if (!workspaceId || !target || !parent) return;
    void perform(async () => {
      const child = await desktop.createPage(workspaceId, parent);
      setPages((previous) => [...previous, child]);
      target.insertReference(child);
      await target.flush();
      selectPage(child, true);
    }, false);
  }

  /** "/表格视图", "/日历视图" and so on: a database under this page, shown where the command was typed. */
  function createInlineDatabase(kind: ViewKind, at: InsertPosition) {
    const target = editor.current;
    const parent = current?.id;
    if (!workspaceId || !target || !parent) return;
    void perform(async () => {
      const { collection, page } = await desktop.createCollection(workspaceId, parent, kind);
      const schema = await desktop.collection(workspaceId, collection.id);
      setCollections((previous) => [...previous, collection]);
      setPages((previous) => [...previous, page]);
      target.insertViewEmbed({ collectionId: collection.id, viewId: schema.views[0].id }, at);
      await target.flush();
    }, false);
  }

  function addCollection(parent: string | null = null) {
    if (!workspaceId) return;
    setPageMenu(false);
    void perform(async () => {
      const { collection, page } = await desktop.createCollection(workspaceId, parent);
      setCollections((previous) => [...previous, collection]);
      setPages((previous) => [...previous, page]);
      selectPage(page, true);
    });
  }

  function toggleFavorite() {
    if (!workspaceId || !current) return;
    void perform(async () => {
      const state = await desktop.setFavorite(workspaceId, current.id, !favorite);
      setNavigation((previous) => ({ ...previous, favorites: state.favorites }));
    }, false);
  }

  function setInbox() {
    setPageMenu(false);
    if (!workspaceId || !current) return;
    void perform(async () => {
      const state = await desktop.setInbox(workspaceId, current.id);
      setNavigation((previous) => ({ ...previous, inboxPageId: state.inboxPageId }));
      setBanner({ type: 'notice', text: '已设为收集箱。' });
    }, false);
  }

  function enterInbox(capture: boolean) {
    if (!workspaceId) return;
    void perform(async () => {
      const page = await desktop.enterInbox(workspaceId, capture);
      await reloadLibrary(workspaceId);
      if (page.id !== current?.id) selectPage(page, capture);
    });
  }

  async function reloadLibrary(id: string) {
    const [items, definitions, shortcuts] = await Promise.all([
      desktop.pages(id), desktop.collections(id), desktop.navigation(id),
    ]);
    setPages(items);
    setCollections(definitions);
    setNavigation(shortcuts);
    setRecordUpdate((previous) => previous + 1);
    return items;
  }

  function showImport(parent: string | null = null) {
    setPageMenu(false);
    setBanner(undefined);
    setLibraryParent(parent);
    setImportReport(undefined);
    setView('import');
  }

  function importContent(request: ContentImport) {
    if (!workspaceId) return;
    void perform(async () => {
      const staged = request.type === 'notionZip' || request.type === 'obsidianVault';
      if (staged) setMigration('pending');
      try {
        const report = await desktop.importContent(workspaceId, request);
        if (!report) return;
        setImportReport(report);
        if (report.some((item) => item.status === 'imported')) await reloadLibrary(workspaceId);
      } finally {
        if (staged) setMigration(undefined);
      }
    });
  }

  function cancelMigration() {
    if (!workspaceId || migration !== 'pending') return;
    setMigration('cancelling');
    void desktop.cancelMigration(workspaceId).then((accepted) => {
      if (!accepted) setMigration((current) => current === 'cancelling' ? 'pending' : current);
    }).catch((error: unknown) => {
      setMigration((current) => current === 'cancelling' ? 'pending' : current);
      setBanner({ type: 'error', text: errorMessage(error) });
    });
  }

  function mergeWorkspace() {
    if (!workspaceId) return;
    void perform(async () => {
      setMigration('pending');
      try {
        const result = await desktop.mergeWorkspace(workspaceId);
        if (result) {
          const message = [`已将 ${result.pages} 页合并到当前工作空间。`, ...result.notices].join(' ');
          await finishBatch(workspaceId, message, undefined, result.pageId);
        }
      } finally {
        setMigration(undefined);
      }
    });
  }

  function openImportedOriginal(original: ImportOriginal) {
    if (!workspaceId) return;
    void perform(async () => {
      const [page, items] = await Promise.all([
        desktop.page(workspaceId, original.pageId), desktop.attachments(workspaceId, original.pageId),
      ]);
      if (page.trashRootId || !items.some((item) => item.id === original.attachmentId)) {
        throw new Error('原件已不可用。');
      }
      if (page.id !== current?.id) {
        recordPage(page);
        selectPage(page);
      }
      setAttachments(items);
      setPreferredAttachment(original.attachmentId);
      setPreferredPdfPage(undefined);
      setReferencePicker(undefined);
      setAttachmentNotice('');
      setView('importOriginal');
    });
  }

  function showTemplates(mode: 'saveTemplate' | 'applyTemplate', parent: string | null = null) {
    if (!workspaceId) return;
    setPageMenu(false);
    void perform(async () => {
      setTemplates(await desktop.templates(workspaceId));
      setLibraryParent(parent);
      setView(mode);
    }, false);
  }

  function saveTemplate(name: string, templateId: string | null) {
    if (!workspaceId || !current) return;
    void perform(async () => {
      await desktop.saveTemplate(workspaceId, current.id, name, templateId);
      setView(null);
      setBanner({ type: 'notice', text: templateId ? '模板已更新。' : '模板已保存。' });
    });
  }

  function applyTemplate(id: string, parent: string | null, title: string | null) {
    if (!workspaceId) return;
    void perform(async () => {
      const page = await desktop.applyTemplate(workspaceId, id, parent, title);
      await reloadLibrary(workspaceId);
      selectPage(page);
      setView(null);
    });
  }

  function editRecord(collection: string, id: string) {
    if (!workspaceId) return;
    void perform(async () => {
      const [schema, record, items] = await Promise.all([
        desktop.collection(workspaceId, collection), desktop.record(workspaceId, collection, id), desktop.attachments(workspaceId, id),
      ]);
      setRecordDialog({ collection: schema, record, attachments: items, conflicts: 0 });
      setView('record');
    });
  }

  /** Saves one property edited in place, keeping every other value. */
  /** Saves one editable property of a record, keeping the others as stored. */
  async function writeRecordField(workspace: string, collection: string, id: string, field: string | null, value: PropertyValue | undefined,
    title?: string) {
    const [schema, record] = await Promise.all([
      desktop.collection(workspace, collection), desktop.record(workspace, collection, id),
    ]);
    const properties: RecordDraft['properties'] = {};
    for (const item of schema.properties) {
      if (item.type === 'relation' || autoField(item.type) || computedField(item.type)) continue;
      const next = item.id === field ? value : record.properties[item.id];
      if (next !== undefined) properties[item.id] = next;
    }
    return desktop.saveRecord(workspace, collection, id, record.revision,
      { title: title ?? record.title, properties, schemaRevision: schema.schemaRevision });
  }

  /** Adds a record without leaving the view, so its title is typed where it appears; resolves to its id. */
  function addRecord(collection: string, container: string, preset?: { field: string; value: PropertyValue }) {
    return new Promise<string | undefined>((resolve) => {
      if (!workspaceId) return resolve(undefined);
      void perform(async () => {
        let page = await desktop.createPage(workspaceId, container);
        if (preset) page = await writeRecordField(workspaceId, collection, page.id, preset.field, preset.value);
        recordPage(page);
        setRecordUpdate((previous) => previous + 1);
        resolve(page.id);
      }, false).finally(() => resolve(undefined));
    });
  }

  /** Renames a page other than the open one, keeping its content. */
  function renamePage(id: string, title: string) {
    if (!workspaceId) return;
    void perform(async () => {
      const page = await desktop.page(workspaceId, id);
      recordPage(await desktop.savePage(workspaceId, id, page.revision, { title, blocks: page.blocks }));
    }, false);
  }

  function renameRecord(collection: string, id: string, title: string) {
    if (!workspaceId) return;
    void perform(async () => {
      const updated = await writeRecordField(workspaceId, collection, id, null, undefined, title);
      recordPage(updated);
      setRecordUpdate((previous) => previous + 1);
    }, false);
  }

  /** What a database's views need from the app, for its own page and for views shown inside other pages. */
  function collectionActions(collection: string, container: string) {
    if (!workspaceId) return undefined;
    return {
      workspace: workspaceId, collectionId: collection, relationTargets, busy, recordUpdate,
      onAdd: (preset?: { field: string; value: PropertyValue }) => addRecord(collection, container, preset),
      onRename: (id: string, title: string) => renameRecord(collection, id, title),
      onSaveField: (id: string, field: PropertyDefinition, value: PropertyValue | undefined) => saveRecordField(collection, id, field, value),
      onOpen: openPage, onEdit: (id: string) => editRecord(collection, id),
      onRecordMoved: (record: PageInfo) => {
        recordPage(record);
        setRecordUpdate((previous) => previous + 1);
      },
      onRecordsConverted: async () => {
        try {
          await reloadLibrary(workspaceId);
        } catch (error) {
          setBanner({ type: 'error', text: `属性类型已转换。刷新页面列表失败。${errorMessage(error)}` });
        }
      },
      mutate: (job: () => Promise<void>) => { void perform(job, false); },
    };
  }

  const embeddedDatabases: EmbeddedDatabaseHost = {
    render: ({ collectionId: embedded, viewId, containerPageId, onSwitch }) => {
      const actions = collectionActions(embedded, containerPageId);
      if (!actions) return null;
      return <CollectionPane {...actions} selectedView={viewId} onSwitch={onSwitch}
        inline={{
          title: byId.get(containerPageId)?.title || '无标题数据库',
          onOpenDatabase: () => openEmbeddedView(containerPageId, { collectionId: embedded, viewId }),
          onRename: (title) => renamePage(containerPageId, title),
        }}
        onCreate={(preset) => addPage(containerPageId, preset && { collection: embedded, ...preset })} />;
    },
  };

  function saveRecordField(collection: string, id: string, field: PropertyDefinition, value: PropertyValue | undefined) {
    if (!workspaceId) return;
    void perform(async () => {
      const updated = await writeRecordField(workspaceId, collection, id, field.id, value);
      recordPage(updated);
      setRecordUpdate((previous) => previous + 1);
      if (current?.id === updated.id) selectPage(updated);
    });
  }

  function saveRecord(draft: RecordDraft, revision: number) {
    if (!recordDialog || !workspaceId) return;
    const { collection, record } = recordDialog;
    void perform(async () => {
      let updated: Page;
      try {
        updated = await desktop.saveRecord(workspaceId, collection.id, record.id, revision, draft);
      } catch (error) {
        if (typeof error !== 'object' || error === null || !('code' in error) ||
          (error.code !== 'REVISION_CONFLICT' && error.code !== 'SCHEMA_CONFLICT')) throw error;
        setRecordDialog((previous) => previous === recordDialog ? { ...recordDialog, conflicts: recordDialog.conflicts + 1 } : previous);
        return;
      }
      recordPage(updated);
      setRecordUpdate((value) => value + 1);
      if (current?.id === updated.id) selectPage(updated);
      setRecordDialog(undefined);
      setView(null);
    });
  }

  function addWorkspace(name: string) {
    void perform(async () => {
      const target = await desktop.createWorkspace(name);
      setWorkspaces((previous) => [...previous, target]);
      await showWorkspace(target);
    });
  }

  function showHistory() {
    setPageMenu(false);
    if (!workspaceId || !current) return;
    void perform(async () => {
      const [history, schema] = await Promise.all([
        desktop.history(workspaceId, current.id), ownerId ? desktop.collection(workspaceId, ownerId) : undefined,
      ]);
      setRevisions(history);
      setHistoryFields(schema?.properties ?? []);
      setView('history');
    });
  }

  function showProposals(pageId: string | null) {
    setPageMenu(false);
    setProposalPage(pageId);
    setView('proposals');
  }

  async function reviewProposal(review: ProposalReview, action: ProposalAction) {
    if (!workspaceId || running.current) throw new Error('请等当前操作完成后重试。');
    running.current = true;
    setBusy(true);
    setBanner(undefined);
    try {
      if (action !== 'reject') await editor.current?.flush();
      const { id, reviewRevision, pageId } = review.summary;
      const result = await desktop.reviewProposal(workspaceId, id, reviewRevision, action);
      if (result.summary.status === 'accepted') {
        setRecordUpdate((previous) => previous + 1);
        try {
          const page = await desktop.page(workspaceId, pageId);
          recordPage(page);
          if (current?.id === pageId) selectPage(page);
        } catch (reason) {
          setBanner({ type: 'error', text: `建议已写入，刷新页面失败。${errorMessage(reason)}` });
        }
      }
      return result;
    } finally {
      running.current = false;
      setBusy(false);
    }
  }

  function beginBatch() {
    setBatchIds((previous) => previous ?? new Set());
  }

  function pageVersions(items: PageInfo[], saved?: PageInfo) {
    return items.map(({ id, revision }) => ({ id, revision: saved?.id === id ? saved.revision : revision }));
  }

  async function finishBatch(workspace: string, notice: string, saved?: PageInfo, prefer?: string) {
    setBatchIds(undefined);
    setView(null);
    try {
      const items = await reloadLibrary(workspace);
      if (!editor.current?.hasUnsavedChanges()) {
        const active = items.filter((page) => !page.trashRootId);
        const byId = new Map(active.map((page) => [page.id, page]));
        const fallback = [...trail].reverse().find((page) => byId.has(page.id));
        const next = (prefer ? byId.get(prefer) : undefined)
          ?? (current ? byId.get(current.id) : undefined) ?? fallback ?? active[0];
        if (next?.id !== current?.id || next?.revision !== (saved?.revision ?? current?.revision)) {
          const page = next ? await desktop.page(workspace, next.id) : undefined;
          if (!editor.current?.hasUnsavedChanges()) selectPage(page);
        }
      }
      setBanner({ type: 'notice', text: notice });
    } catch (error) {
      throw new Error(`${notice}刷新页面列表失败。${errorMessage(error)}`);
    }
  }

  function moveBatch(parent: string | null) {
    if (!workspaceId || !mayMoveBatch) return;
    void perform(async () => {
      const saved = await editor.current?.flush();
      const result = await desktop.movePages(workspaceId, pageVersions(batchPages, saved), parent);
      const moved = result.pages.filter((item) => item.parentId !== byId.get(item.id)?.parentId).length;
      try {
        await finishBatch(workspaceId, `已移动 ${moved} 个页面分支。`, saved);
      } finally {
        showMoveImpact(result.impacts);
      }
    }, false);
  }

  function archiveBatch() {
    if (!workspaceId || !batchPages.length) return;
    void perform(async () => {
      const saved = await editor.current?.flush();
      const count = await desktop.archivePages(workspaceId, pageVersions(batchPages, saved));
      await finishBatch(workspaceId, `已将 ${count} 个页面移入回收站。`, saved);
    }, false);
  }

  function restoreBatch(items: PageInfo[]) {
    if (!workspaceId) return;
    void perform(async () => {
      const restoresDraftPage = items.some(({ id }) => id === current?.trashRootId)
        && editor.current?.hasUnsavedChanges();
      const saved = restoresDraftPage ? undefined : await editor.current?.flush();
      const count = await desktop.restorePages(workspaceId, pageVersions(items));
      const notice = items.length === 1 ? '页面已恢复。' : `已恢复 ${count} 个页面。`;
      await finishBatch(workspaceId, restoresDraftPage ? `${notice}草稿仍可在当前页编辑。` : notice, saved, items[0]?.id);
    }, false);
  }

  function showCsvExport(view: CollectionView) {
    if (!collectionId) return;
    setCsvDialog({ collection: collectionId, view });
    setCsvResult(undefined);
    setView('csvExport');
  }

  function exportCsv(viewId: string | null) {
    if (!workspaceId || !csvDialog) return;
    void perform(async () => {
      const result = await desktop.exportCollectionCsv(workspaceId, csvDialog.collection, viewId);
      if (result) setCsvResult(result);
    });
  }

  function movePage(parent: string | null) {
    if (!workspaceId || !current) return;
    void perform(async () => {
      const saved = await editor.current?.flush();
      const result = await desktop.movePages(workspaceId, pageVersions([current], saved), parent);
      recordPage(result.pages[0]);
      showMoveImpact(result.impacts);
      selectPage(await desktop.page(workspaceId, current.id));
    }, false);
  }

  function showMoveImpact(impacts: MoveImpact[]) {
    setMoveImpacts(impacts);
    setView(impacts.length ? 'moveImpact' : null);
  }

  function archivePage() {
    if (!workspaceId || !current) return;
    void perform(async () => {
      const saved = await editor.current?.flush();
      await desktop.archivePage(workspaceId, current.id, saved?.revision ?? current.revision);
      const [remaining, shortcuts] = await Promise.all([desktop.pages(workspaceId), desktop.navigation(workspaceId)]);
      const first = remaining.find((page) => !page.trashRootId);
      setPages(remaining);
      setNavigation(shortcuts);
      selectPage(first ? await desktop.page(workspaceId, first.id) : undefined);
      setView(null);
      setBanner({ type: 'notice', text: '页面已移入回收站。' });
    }, false);
  }

  function showTrash() {
    if (!workspaceId) return;
    void perform(async () => {
      const items = await desktop.pages(workspaceId);
      setPages(items);
      setTrashReview(undefined);
      setView('trash');
    }, false);
  }

  function reviewTrash(kind: TrashReview['kind']) {
    if (!workspaceId) return;
    void perform(async () => {
      setTrashReview(kind === 'pages'
        ? { kind, preview: await desktop.previewEmptyTrash(workspaceId) }
        : { kind, preview: await desktop.previewBlobCleanup(workspaceId) });
    });
  }

  function confirmTrash(review: TrashReview) {
    if (!workspaceId) return;
    void perform(async () => {
      if (review.kind === 'pages') {
        const result = await desktop.emptyTrash(workspaceId, review.preview.confirmation).catch(async (error: unknown) => {
          setTrashReview(undefined);
          setPages(await desktop.pages(workspaceId).catch(() => pages));
          throw error;
        });
        setTrashReview(undefined);
        const records = result.updatedRecords ? ` 已更新 ${result.updatedRecords} 条保留记录的关联。` : '';
        await finishBatch(workspaceId, `已永久删除 ${result.pages} 个页面。${records}`);
      } else {
        const result = await desktop.cleanBlobs(workspaceId, review.preview.confirmation).finally(() => setTrashReview(undefined));
        setView(null);
        const failed = result.failed ? ` ${result.failed} 份暂未清理，可以重试。` : '';
        setBanner({ type: 'notice', text: `已清理 ${result.removed} 份原件，共 ${result.bytes.toLocaleString('zh-CN')} 字节。${failed}` });
      }
    }, false);
  }

  function restoreHistory(target: number) {
    if (!workspaceId || !current) return;
    void perform(async () => {
      const saved = await editor.current?.flush();
      const page = await desktop.restoreVersion(workspaceId, current.id, saved?.revision ?? current.revision, target);
      recordPage(page);
      selectPage(page);
      setView(null);
      setBanner({ type: 'notice', text: '历史内容已保存为新版本。' });
    }, false);
  }

  function exportPage(format: 'Markdown' | 'HTML') {
    setPageMenu(false);
    if (!workspaceId || !current) return;
    void perform(async () => {
      const result = format === 'HTML'
        ? await desktop.exportHtml(workspaceId, current.id)
        : await desktop.exportMarkdown(workspaceId, current.id);
      if (result?.companionFolder) setBanner({
        type: 'notice',
        text: `${format} 已导出。导出报告位于同目录的「${result.companionFolder}」文件夹。`,
      });
    });
  }

  function showPageAppearance() {
    setPageMenu(false);
    if (!workspaceId || !current) return;
    void perform(async () => {
      setAttachments(await desktop.attachments(workspaceId, current.id));
      setView('pageAppearance');
    });
  }

  function closePageAppearance() {
    setView(null);
    requestAnimationFrame(() => pageAnchor.current?.querySelector('button')?.focus());
  }

  async function importCoverImage() {
    if (!workspaceId || !current) return;
    let image: Attachment | undefined;
    await perform(async () => {
      const result = await desktop.pickImages(workspaceId, current.id, true);
      if (result.failed.length) throw new Error(result.failed[0].error.message);
      const imported = result.added[0]?.attachment;
      if (imported) {
        image = imported;
        setAttachments((previous) => [...previous.filter(({ id }) => id !== imported.id), imported]);
      }
    }, false);
    return image;
  }

  function savePageAppearance(icon: string, coverAttachmentId: string | null) {
    if (!workspaceId || !current) return;
    void perform(async () => {
      const saved = await editor.current?.flush();
      const updated = await desktop.setPageAppearance(workspaceId, current.id, saved?.revision ?? current.revision, icon, coverAttachmentId);
      recordPage(updated);
      selectPage(updated);
      closePageAppearance();
      setBanner({ type: 'notice', text: '页面外观已保存。' });
    }, false);
  }

  function showAttachments(preferred?: string, position?: InsertPosition) {
    if (!workspaceId || !current) return;
    const editing = editor.current;
    void perform(async () => {
      const items = await desktop.attachments(workspaceId, current.id);
      if (editing !== editor.current) return;
      setAttachments(items);
      setPreferredAttachment(preferred);
      setPreferredPdfPage(undefined);
      setReferencePicker(position !== undefined && editing ? { at: position, editor: editing } : undefined);
      setAttachmentNotice('');
      setView('attachments');
    });
  }

  async function showPdfAttachment(space: string, page: string, target: PdfTarget) {
    const items = await desktop.attachments(space, page);
    if (!items.some((item) => item.id === target.attachmentId
      && item.sha256 === target.sha256 && item.mediaType === 'application/pdf')) {
      setView(null);
      setBanner({ type: 'notice', text: '搜索结果所指的 PDF 原件版本不可用。' });
      return;
    }
    setAttachments(items);
    setPreferredAttachment(target.attachmentId);
    setPreferredPdfPage(target.pageNumber);
    setReferencePicker(undefined);
    setAttachmentNotice('');
    setView('attachments');
  }

  function openPdfResult(id: string, pdf: PdfTarget) {
    if (!workspaceId) return;
    void perform(async () => {
      const page = await desktop.page(workspaceId, id);
      if (page.trashRootId) throw new Error('这个页面在回收站中。');
      if (id !== current?.id) {
        recordPage(page);
        selectPage(page);
      }
      await showPdfAttachment(workspaceId, id, pdf);
    });
  }

  function addAttachments() {
    if (!workspaceId || !current) return;
    void perform(async () => {
      const result = await desktop.addAttachments(workspaceId, current.id);
      if (!result.added.length && !result.failed.length) return;
      setAttachments(await desktop.attachments(workspaceId, current.id));
      const duplicates = result.added.filter((item) => item.alreadyAttached).length;
      const reused = result.added.filter((item) => item.reusedBlob && !item.alreadyAttached).length;
      const parts = [`已添加 ${result.added.length - duplicates} 个附件`];
      if (duplicates) parts.push(`${duplicates} 个文件已在此页面`);
      if (reused) parts.push(`${reused} 个原件已复用`);
      setAttachmentNotice(`${parts.join('，')}。`);
      if (result.failed.length) {
        setBanner({ type: 'error', text: result.failed.map(({ name, error }) => `${name} · ${error.message}`).join('\n') });
      }
    }, false);
  }

  function insertEditorImages(files?: File[], at?: InsertPosition) {
    if (!workspaceId || !current) return;
    void perform(async () => {
      const images: Attachment[] = [];
      const failures: string[] = [];
      if (files) {
        if (files.length > 50) throw new Error('每次最多添加 50 张图片。');
        for (const file of files) {
          const name = file.name || '粘贴的图片';
          try {
            if (file.size > maxImageBytes) throw new Error('正文图片最大为 24 MiB。');
            const result = await desktop.attachImageBytes(workspaceId, current.id, name, await file.arrayBuffer());
            images.push(result.attachment);
          } catch (error) {
            failures.push(`${name} · ${errorMessage(error)}`);
          }
        }
      } else {
        const result = await desktop.pickImages(workspaceId, current.id);
        images.push(...result.added.map((item) => item.attachment));
        failures.push(...result.failed.map(({ name, error }) => `${name} · ${error.message}`));
      }
      if (images.length && !editor.current?.insertImages(images, at)) {
        failures.push('无法在当前光标处插入图片。原件已保存到附件。');
      }
      if (failures.length) setBanner({ type: 'error', text: failures.join('\n') });
    });
  }

  const viewingImportOriginal = view === 'importOriginal';
  const showingAttachments = view === 'attachments' || viewingImportOriginal;
  const close = () => {
    setView(viewingImportOriginal ? 'import' : null);
    if (view === 'referenceBlock' || view === 'blockReference' || view === 'embedView' || showingAttachments) {
      setReferencePicker(undefined);
      setReferencePage(undefined);
    }
    if (view === 'record') {
      setRecordDialog(undefined);
      setBanner(undefined);
    }
  };

  function availableCommands(): Command[] {
    const commands: Command[] = [
      { id: 'focus', name: focusMode ? '退出专注模式' : '进入专注模式', shortcut: macKeys ? '⌘⇧F' : 'Ctrl+Shift+F', run: toggleFocusMode },
      { id: 'appearance', name: '外观', run: () => setView('appearance') },
      ...themeOptions.map(({ name, value }) => ({ id: `theme-${value}`, name, run: () => setTheme(value) })),
      { id: 'workspace', name: '新建工作空间', run: () => setView('workspace') },
      { id: 'backup', name: '备份与恢复', run: () => setView('backup') },
      { id: 'sync', name: '同步', run: () => setView('sync') },
    ];
    if (!focusMode) commands.push({ id: 'sidebar', name: sidebar ? '收起侧栏' : '展开侧栏', run: () => setSidebar(!sidebar) });
    if (!workspaceId) return commands;
    commands.unshift(
      { id: 'search', name: '搜索页面', shortcut: macKeys ? '⌘K' : 'Ctrl+K', run: () => setView('search') },
      { id: 'new-page', name: '新建页面', run: () => addPage() },
      { id: 'new-database', name: '新建数据库', run: () => addCollection() },
      { id: 'inbox', name: '打开收集箱', run: () => enterInbox(false) },
      { id: 'import', name: '导入资料', run: () => showImport() },
      { id: 'trash', name: '回收站', run: showTrash },
      { id: 'integration', name: '集成与 API', run: () => setView('integrations') },
    );
    if (current) {
      const page = current.id;
      commands.push(
        { id: 'new-child', name: collectionId ? '新建记录' : '新建子页面', run: () => addPage(page) },
        { id: 'attachments', name: '当前页附件', run: () => showAttachments() },
        { id: 'history', name: '当前页历史', run: showHistory },
        { id: 'location', name: '当前页定位链接', run: () => showLocation(page) },
      );
      if (!ownerId) commands.push({ id: 'move', name: '移动页面', run: () => setView('move') });
    }
    return commands;
  }

  if (loading || startupError) return <main className="startup">
    <div className="app-mark"><LibraryBig size={23} /></div><h1>知页</h1>
    {loading ? <p><LoaderCircle size={16} className="spin" /> 正在打开工作空间…</p> : <>
      <p role="alert">{startupError}</p><button className="primary-button" type="button" onClick={() => setAttempt(attempt + 1)}>重试</button>
    </>}
  </main>;

  const showSidebar = sidebar && !focusMode;
  return <div className={`app-shell${showSidebar ? '' : ' sidebar-closed'}${focusMode ? ' focus-mode' : ''}`} aria-busy={busy}>
    <a className="skip-link" href="#page-content" onClick={(event) => {
      event.preventDefault();
      documentScroll.current?.focus();
    }}>跳到页面内容</a>
    <aside id="workspace-sidebar" className="sidebar" aria-label="工作空间侧栏" hidden={!showSidebar}>
      <div className="workspace-anchor" ref={workspaceAnchor} {...disclosureEvents(() => setWorkspaceMenu(false))}>
        <button type="button" className="workspace-button" aria-label="切换工作空间" aria-expanded={workspaceMenu}
          disabled={busy} onClick={() => setWorkspaceMenu(!workspaceMenu)}>
          <span className="app-mark"><LibraryBig size={19} strokeWidth={1.75} /></span>
          <span className="workspace-label"><strong>知页</strong><small>{workspace?.name ?? '工作空间'}</small></span>
          <ChevronDown size={15} className="workspace-chevron" />
        </button>
        {workspaceMenu && <div className="menu workspace-menu">
          {workspaces.map((space) => <button type="button" className="menu-item" key={space.id} onClick={() => {
            setWorkspaceMenu(false);
            if (space.id !== workspaceId) void perform(() => showWorkspace(space));
          }}><BookOpen size={16} /><span>{space.name}</span>{space.id === workspaceId && <span className="current-dot" />}</button>)}
          <div className="menu-rule" />
          <button type="button" className="menu-item" onClick={() => { setWorkspaceMenu(false); setView('workspace'); }}><Plus size={16} />新建工作空间</button>
          <button type="button" className="menu-item" disabled={!workspace} onClick={() => {
            setWorkspaceMenu(false);
            setView('integrations');
          }}><Plug size={16} />集成与 API</button>
          <button type="button" className="menu-item" onClick={() => { setWorkspaceMenu(false); setView('sync'); }}>
            <RefreshCw size={16} />同步</button>
          <button type="button" className="menu-item" onClick={() => { setWorkspaceMenu(false); setView('appearance'); }}>
            <SunMoon size={16} />外观</button>
        </div>}
      </div>

      <button type="button" className="sidebar-search" aria-label="搜索" aria-keyshortcuts="Control+K Meta+K Control+P Meta+P"
        disabled={!workspace || busy} onClick={() => setView('search')}>
        <Search size={17} strokeWidth={1.75} aria-hidden /><span>搜索</span>
        <kbd aria-hidden>{macKeys ? '⌘ K' : 'Ctrl K'}</kbd>
      </button>
      <div className="sidebar-scroll">
        {workspaceId && <LibraryNavigation key={`shortcuts/${workspaceId}`} navigation={navigation} pages={byId}
          selected={current?.id} busy={busy} onInbox={enterInbox} onOpen={openPage} />}
        <div className="tree-heading"><span>页面</span><div className="tree-actions">
          <button type="button" className="icon-button" title="批量整理页面" aria-label="批量整理页面"
            aria-pressed={Boolean(batch)} disabled={!workspace || busy}
            onClick={() => batch ? setBatchIds(undefined) : beginBatch()}><ListChecks size={16} /></button>
          <button type="button" className="icon-button" title="新建根页面"
            aria-label="新建根页面" disabled={!workspace || busy} onClick={() => addPage()}><Plus size={17} /></button>
        </div></div>
        <PageTree key={`tree/${workspaceId}`} byId={byId} collectionByPage={collectionByPage}
          selected={current?.id} disabled={busy} batch={batch} onOpen={openPage} />
      </div>
      <button type="button" className="new-page-button" disabled={busy || !workspace} onClick={() => addPage()}><Plus size={17} />新建页面</button>
      <button type="button" className="new-page-button" disabled={busy || !workspace} onClick={() => addCollection()}><Database size={16} />新建数据库</button>

      <div className="sidebar-bottom">
        {issues.length > 0 && <button type="button" className="sidebar-action workspace-warning" disabled={busy}
          onClick={() => setView('issues')}><CircleAlert size={17} />{issues.length} 个工作空间无法打开</button>}
        <button type="button" className="sidebar-action" disabled={!workspace || busy} onClick={() => showImport()}><Upload size={17} />导入资料</button>
        <button type="button" className="sidebar-action" disabled={!workspace || busy} onClick={() => showTemplates('applyTemplate')}><LayoutTemplate size={17} />模板</button>
        <button type="button" className={`sidebar-action${workspaceId && backupFailures.has(workspaceId) ? ' workspace-warning' : ''}`}
          disabled={busy} onClick={() => { setBanner(undefined); setView('backup'); }}><DatabaseBackup size={17} />备份与恢复
          {workspaceId && backupFailures.has(workspaceId) && <CircleAlert size={14} aria-label="自动备份失败" />}
        </button>
        <button type="button" className="sidebar-action" disabled={!workspace || busy} onClick={showTrash}><Trash2 size={17} />回收站</button>
        <div className="local-status"><span />本地工作空间</div>
      </div>
    </aside>

    <main className="main-pane">
      <header className="topbar">
        {!focusMode && <><button type="button" className="icon-button sidebar-toggle" title={sidebar ? '收起侧栏' : '展开侧栏'}
          aria-label={sidebar ? '收起侧栏' : '展开侧栏'} aria-expanded={sidebar} aria-controls="workspace-sidebar"
          onClick={() => setSidebar(!sidebar)}><PanelLeft size={18} /></button>
        <nav className="breadcrumb" aria-label="页面位置">
          {trail.length ? trail.map((page, index) => <span key={page.id}>
            {index > 0 && <ChevronRight size={12} />}
            {index === trail.length - 1 ? <span aria-current="page">{page.title}</span> :
              <button type="button" disabled={busy} onClick={() => openPage(page.id)}>{page.title}</button>}
          </span>) : <span>{workspace?.name ?? '知页'}</span>}
        </nav></>}
        <div className="topbar-actions">
          <span id="editor-save-status" className="save-slot" />
          <button type="button" className="icon-button" title="命令面板" aria-label="命令面板"
            aria-keyshortcuts="Control+Shift+P Meta+Shift+P" disabled={busy} onClick={showCommands}><CommandIcon size={17} /></button>
          <button ref={focusControl} type="button" className="icon-button" title={focusMode ? '退出专注模式' : '进入专注模式'}
            aria-label={focusMode ? '退出专注模式' : '进入专注模式'} aria-keyshortcuts="Control+Shift+F Meta+Shift+F"
            disabled={busy} onClick={toggleFocusMode}>{focusMode ? <Minimize2 size={16} /> : <Maximize2 size={16} />}</button>
          {!focusMode && current && <>
            <button type="button" className="icon-button" aria-label={favorite ? '取消收藏' : '收藏页面'}
              title={favorite ? '取消收藏' : '收藏页面'} aria-pressed={favorite} disabled={busy}
              onClick={toggleFavorite}><Star size={17} fill={favorite ? 'currentColor' : 'none'} /></button>
            <button type="button" className="topbar-button" aria-label="附件" disabled={busy} onClick={() => showAttachments()}><Paperclip size={16} /> <span>附件</span></button>
            <button type="button" className="topbar-button" aria-label="历史" disabled={busy} onClick={showHistory}><History size={16} /> <span>历史</span></button>
            <div className="page-menu-anchor" ref={pageAnchor} {...disclosureEvents(() => setPageMenu(false))}>
              <button type="button" className="icon-button" title="页面操作" aria-label="页面操作" aria-expanded={pageMenu}
                disabled={busy} onClick={() => setPageMenu(!pageMenu)}><MoreHorizontal size={20} /></button>
              {pageMenu && <div className="menu page-menu">
                <button type="button" className="menu-item" onClick={() => addPage(current.id)}><FilePlus2 size={17} />{collectionId ? '新建记录' : '新建子页面'}</button>
                {!collectionId && <button type="button" className="menu-item" onClick={() => addCollection(current.id)}><Database size={17} />新建子数据库</button>}
                <button type="button" className="menu-item" onClick={showPageAppearance}><ImageIcon size={17} />页面外观</button>
                <button type="button" className="menu-item"
                  onClick={() => showTemplates('applyTemplate', current.id)}><LayoutTemplate size={17} />{collectionId ? '从模板新建记录' : '从模板新建子页面'}</button>
                {!collectionId && <button type="button" className="menu-item" onClick={() => showTemplates('saveTemplate')}>
                  <LayoutTemplate size={17} />{ownerId ? '保存为记录模板' : '保存为页面模板'}</button>}
                {!ownerId && <button type="button" className="menu-item" onClick={() => { setPageMenu(false); setView('move'); }}><Archive size={17} />移动页面</button>}
                <button type="button" className="menu-item" onClick={() => {
                  setPageMenu(false);
                  setPageComments(editor.current?.comments() ?? []);
                }}><MessageSquare size={17} />评论</button>
                <button type="button" className="menu-item" onClick={() => showProposals(current.id)}>
                  <ListChecks size={17} />修改建议
                </button>
                <button type="button" className="menu-item" onClick={() => showLocation(current.id)}>
                  <Link2 size={17} />页面链接
                </button>
                <button type="button" className="menu-item" onClick={() => {
                  setPageMenu(false);
                  void perform(async () => { setView('backlinks'); });
                }}><Link size={17} />反向链接</button>
                {navigation.inboxPageId !== current.id && <button type="button" className="menu-item" onClick={setInbox}><Inbox size={17} />设为收集箱</button>}
                <button type="button" className="menu-item" onClick={() => showImport(current.id)}><Upload size={17} />{collectionId ? '导入 CSV' : '导入资料到此处'}</button>
                <button type="button" className="menu-item" onClick={() => exportPage('Markdown')}><Download size={17} />导出 Markdown</button>
                <button type="button" className="menu-item" onClick={() => exportPage('HTML')}><Download size={17} />导出 HTML</button>
                <button type="button" className="menu-item" onClick={() => {
                  if (!workspaceId) return;
                  setPageMenu(false);
                  void perform(async () => {
                    const [updated, items] = await Promise.all([
                      desktop.page(workspaceId, current.id), desktop.pages(workspaceId),
                    ]);
                    setPages(items);
                    selectPage(updated);
                    setRecordUpdate((previous) => previous + 1);
                  });
                }}><RefreshCw size={17} />重新读取页面</button>
                <div className="menu-rule" />
                <button type="button" className="menu-item danger" onClick={() => { setPageMenu(false); setView('archive'); }}><Trash2 size={17} />移入回收站</button>
              </div>}
            </div>
          </>}
        </div>
      </header>
      {batch && <BatchBar count={batchPages.length} canMove={mayMoveBatch} disabled={busy}
        onMove={() => setView('batchMove')} onArchive={() => setView('batchArchive')} onClose={() => setBatchIds(undefined)} />}

      <div ref={documentScroll} id="page-content" tabIndex={-1} role="region" aria-label="页面内容" className="document-scroll"
        key={`${workspaceId ?? 'empty'}/${selection?.page.id ?? ''}`}>
        <ExternalRecordUpdates.Provider value={externalRecordUpdate}><EmbeddedDatabases.Provider value={embeddedDatabases}>
          {selection && workspaceId ? <PageEditor key={`${workspaceId}/${selection.page.id}/${selection.epoch}`} ref={editor}
            workspace={workspaceId} page={selection.page} locked={busy} focusTitle={selection.focusTitle}
            database={Boolean(collectionId)} searchTarget={selection.searchTarget} pages={pages}
            onCreateSubpage={collectionId ? undefined : createSubpage}
            onCreateDatabase={collectionId ? undefined : createInlineDatabase}
            onAppearance={showPageAppearance}
            headerContent={collectionId ? <CollectionPane {...collectionActions(collectionId, selection.page.id)!}
              batch={batch} selectedView={selectedViews[collectionKey]}
              onSwitch={(type) => setSelectedViews((previous) => ({ ...previous, [collectionKey]: type }))}
              onCreate={(preset) => addPage(selection.page.id, preset && { collection: collectionId, ...preset })}
              onBeginBatch={beginBatch} onExportCsv={showCsvExport} /> : ownerId ?
              <RecordPanel workspace={workspaceId} collectionId={ownerId} pageId={selection.page.id} recordUpdate={recordUpdate} onOpen={openPage}
                updatedAt={current?.updatedAt ?? selection.page.updatedAt} busy={busy} onEdit={() => editRecord(ownerId, selection.page.id)}
                onSaveField={(field, value) => saveRecordField(ownerId, selection.page.id, field, value)} /> : undefined}
            onSaved={recordPage} onResolved={(page) => { recordPage(page); selectPage(page); setBanner(undefined); }}
            onDiscard={() => {
              selectPage(undefined);
              setBanner(undefined);
              setPageMenu(false);
              requestAnimationFrame(() => documentScroll.current?.focus({ preventScroll: true }));
            }}
            onOpenPage={openPage} onOpenView={openEmbeddedView} onPickViewEmbed={pickViewEmbed}
            onExitFocusMode={focusMode ? () => setFocusMode(false) : undefined}
            onPickReference={() => setView('reference')} onPickBlockReference={pickBlockReference}
            onInsertImages={insertEditorImages} onPickAttachment={(at) => showAttachments(undefined, at)}
            onOpenAttachment={showAttachments} /> :
            <div className="empty-workspace"><span className="document-icon"><BookOpen size={26} /></span>
              <h1>{workspace?.name ?? '知页'}</h1><p>{workspace ? '从一页新的笔记开始。' : '创建工作空间来整理你的知识。'}</p>
              <button type="button" className="primary-button" disabled={busy} onClick={() => workspace ? addPage() : setView('workspace')}>
                <Plus size={17} />{workspace ? '新建页面' : '新建工作空间'}</button>
            </div>}
        </EmbeddedDatabases.Provider></ExternalRecordUpdates.Provider>
      </div>
    </main>

    {view === 'commands' && <CommandPalette commands={availableCommands()} onClose={close}
      onChoose={(command) => { close(); command.run(); }} />}
    {view === 'appearance' && <AppearanceDialog theme={theme} onChoose={setTheme} onClose={close} />}
    {view === 'pageAppearance' && workspaceId && selection && <PageAppearanceDialog workspace={workspaceId}
      page={selection.page} attachments={attachments} busy={busy} onClose={closePageAppearance}
      onImport={importCoverImage} onSave={savePageAppearance} />}
    {workspaceId && (view === 'search' || view === 'reference' || view === 'blockReference') && <PagePicker workspace={workspaceId} pages={pages}
      collections={collections} progress={searchIndex.progress} onRebuild={rebuildSearch} mode={view} busy={busy} onClose={close}
      onShowLocation={({ page, blockId, pdf }) => showLocation(page.id, blockId, pdf)}
      onChoose={({ page, blockId, pdf }) => {
        if (view === 'search') {
          if (pdf) openPdfResult(page.id, pdf);
          else openPage(page.id, blockId);
        }
        else if (view === 'blockReference') pickSourcePage(page, blockId);
        else {
          close();
          // Insert before navigation can flush this editor. Tiptap restores focus on the next frame.
          editor.current?.insertReference(page);
        }
      }} />}
    {pageComments && <Modal title="评论" onClose={() => setPageComments(undefined)}>
      {pageComments.length ? <ul className="comment-list">
        {pageComments.map((comment) => <li key={comment.from}>
          <button type="button" className="comment-list-item" onClick={() => {
            setPageComments(undefined);
            editor.current?.revealComment(comment.from);
          }}><q>{comment.quote}</q><span><CommentText text={comment.text} /></span>
            {comment.replies > 0 && <small>{comment.replies} 条回复</small>}</button>
        </li>)}
      </ul> : <p className="dialog-copy">这一页还没有评论。选中文字后按 {navigator.userAgent.includes('Mac') ? '⌘⇧M' : 'Ctrl+Shift+M'} 添加。</p>}
    </Modal>}
    {view === 'referenceBlock' && referencePage && <BlockPicker key={referencePage.page.id} page={referencePage.page}
      preferred={referencePage.preferred} excluded={referencePage.page.id === current?.id ? referencePicker?.excluded : undefined}
      onClose={close} onBack={() => setView('blockReference')} onChoose={insertBlockReference} />}
    {view === 'embedView' && workspaceId && <EmbedPicker workspace={workspaceId} collections={relationTargets} onClose={close}
      onChoose={(target) => insertPickedBlock((editor, at) => editor.insertViewEmbed(target, at), '无法在这里插入数据库视图。')} />}
    {view === 'backlinks' && workspaceId && current && <BacklinkDialog key={`${workspaceId}/${current.id}`}
      workspace={workspaceId} page={current.id} onClose={close} onOpen={openPage} />}
    {view === 'workspace' && <WorkspaceDialog busy={busy} onClose={close} onCreate={addWorkspace} />}
    {view === 'location' && shownLocation && <Modal title={shownLocation.title} onClose={close}>
      <div className="dialog-form">
        <label htmlFor="zhiye-location-url">知页定位链接</label>
        <input id="zhiye-location-url" readOnly value={shownLocation.url} onFocus={(event) => event.currentTarget.select()} />
        <div className="dialog-actions">
          <button type="button" className="primary-button" onClick={close}>完成</button>
        </div>
      </div>
    </Modal>}
    {view === 'sync' && <SyncDialog workspace={workspace} pages={byId} onClose={close}
      onOpenPage={(id) => { close(); openPage(id); }}
      onBeforeSync={async () => { await editor.current?.flush(); }}
      onJoined={(joined) => {
        void perform(async () => {
          const overview = await desktop.workspaces();
          setWorkspaces(overview.workspaces);
          setIssues(overview.issues);
          await showWorkspace(joined);
        }, false);
      }} />}
    {view === 'integrations' && workspace && <IntegrationsDialog workspace={workspace} onClose={close}
      onProposals={() => showProposals(null)} />}
    {view === 'proposals' && workspaceId && <ProposalDialog workspace={workspaceId} pageId={proposalPage}
      onClose={proposalPage ? close : () => setView('integrations')} onAction={reviewProposal} />}
    {(view === 'import' || viewingImportOriginal) && workspaceId && <ImportDialog workspace={workspaceId} pages={pages} collectionByPage={collectionByPage} initialParent={libraryParent}
      busy={busy} report={importReport} onClose={close} onImport={importContent} onOpen={openPage} onOpenOriginal={openImportedOriginal}
      migration={migration} error={banner?.type === 'error' ? banner.text : undefined} onCancelMigration={cancelMigration}
      onRestart={() => { setBanner(undefined); setImportReport(undefined); }} onSaveReport={(report) => {
        void perform(async () => { await desktop.saveImportReport(report); }, false);
      }} />}
    {view === 'saveTemplate' && current && <SaveTemplateDialog page={current} templates={templates}
      collectionId={ownerId ?? null} busy={busy} onClose={close} onSave={saveTemplate} />}
    {view === 'applyTemplate' && workspaceId && <ApplyTemplateDialog pages={pages} collectionByPage={collectionByPage}
      initialParent={libraryParent} templates={templates} busy={busy} onClose={close} onApply={applyTemplate} onDelete={(id) => {
        void perform(async () => {
          await desktop.deleteTemplate(workspaceId, id);
          setTemplates((previous) => previous.filter((item) => item.id !== id));
        }, false);
      }} />}
    {view === 'csvExport' && csvDialog && <CsvDialog view={csvDialog.view} busy={busy} result={csvResult} onExport={exportCsv} onClose={close} />}
    {view === 'move' && current && <MoveDialog pages={pages} roots={[current]} collectionByPage={collectionByPage}
      busy={busy} onClose={close} onMove={movePage} />}
    {view === 'batchMove' && <MoveDialog pages={pages} roots={batchRoots} batch collectionByPage={collectionByPage}
      busy={busy} onClose={close} onMove={moveBatch} />}
    {view === 'moveImpact' && <Modal title="移动后的集成授权" onClose={close} busy={busy}>
      <ul className="move-impacts">
        {moveImpacts.map(({ integrationName, added, removed }, index) => <li key={index}>
          <strong>{integrationName}</strong>
          {added || removed ? ` · 增加 ${added} 页，减少 ${removed} 页` : ' · 归档页面的授权范围变化'}
        </li>)}
      </ul>
      <p className="dialog-copy">受影响的集成需要重新编目。</p>
      <div className="dialog-actions dialog-pad"><button type="button" className="primary-button"
        disabled={busy} onClick={close}>完成</button></div>
    </Modal>}
    {view === 'batchArchive' && <Modal title="批量归档" onClose={close} busy={busy}>
      <p className="dialog-copy">将 {batchRoots.length} 个页面分支及子页移入回收站。可以从回收站恢复。</p>
      <footer className="dialog-actions dialog-pad"><button type="button" className="secondary-button" disabled={busy} onClick={close}>取消</button>
        <button type="button" className="danger-button" disabled={busy || !batchRoots.length} onClick={archiveBatch}>移入回收站</button></footer>
    </Modal>}
    {view === 'archive' && current && <Modal title="移入回收站" onClose={close} busy={busy}>
      <p className="dialog-copy">「{current.title}」及其子页面将移入回收站，之后可以恢复。</p>
      <footer className="dialog-actions dialog-pad"><button type="button" className="secondary-button" disabled={busy} onClick={close}>取消</button>
        <button type="button" className="danger-button" disabled={busy} onClick={archivePage}>移入回收站</button></footer>
    </Modal>}
    {view === 'trash' && workspaceId && <TrashDialog pages={pages} busy={busy} review={trashReview}
      onClose={close} onBack={() => setTrashReview(undefined)} onRestore={restoreBatch}
      onReview={reviewTrash} onConfirm={confirmTrash} />}
    {view === 'backup' && <BackupDialog key={workspaceId} busy={busy} onClose={close} workspace={workspace}
      migration={migration} actionError={banner?.type === 'error' ? banner.text : undefined}
      onMerge={mergeWorkspace} onCancelMerge={cancelMigration} onRun={async (id) => {
      await editor.current?.flush();
      return desktop.runWorkspaceBackup(id);
    }} onRecovery={() => {
      if (!workspaceId) return;
      void perform(async () => {
        if (await desktop.exportRecovery(workspaceId)) {
          close();
          setBanner({ type: 'notice', text: '整库恢复包已保存。' });
        }
      });
    }} onExport={(includeHistory) => {
      if (!workspaceId) return;
      void perform(async () => {
        if (await desktop.exportWorkspace(workspaceId, includeHistory)) {
          close();
          setBanner({ type: 'notice', text: '工作空间包已导出。' });
        }
      });
    }} onImport={() => {
      void perform(async () => {
        const imported = await desktop.importWorkspace();
        if (!imported) return;
        setWorkspaces((previous) => [...previous, imported]);
        await showWorkspace(imported);
        setBanner({ type: 'notice', text: '已恢复到新的工作空间。' });
      });
    }} />}
    {view === 'history' && <HistoryDialog entries={revisions} fields={historyFields} busy={busy} onClose={close} onRestore={restoreHistory} />}
    {view === 'record' && recordDialog && workspaceId && <RecordDialog workspace={workspaceId} {...recordDialog} busy={busy}
      error={banner?.type === 'error' ? banner.text : undefined} onClose={close} onSave={saveRecord} />}
    {showingAttachments && workspaceId && current && <AttachmentDialog key={`${workspaceId}/${current.id}`} workspace={workspaceId} page={current.id}
      items={attachments} preferredId={preferredAttachment} preferredPdfPage={preferredPdfPage}
      pdfIndexRevision={searchIndex.progress?.pdf?.revision} notice={attachmentNotice}
      error={banner?.type === 'error' ? banner.text : undefined} busy={busy} onClose={close}
      onAdd={addAttachments} onReload={() => { void perform(async () => {
        setAttachments(await desktop.attachments(workspaceId, current.id));
        setAttachmentNotice('');
      }, false); }} onInsertFile={referencePicker ? (item) => {
        insertPickedBlock((editor, at) => editor.insertAttachment(item.id, at), '无法在这里插入附件块。');
      } : undefined} onInsertImage={(item) => {
        if (referencePicker) {
          insertPickedBlock((editor, at) => editor.insertImages([item], at), '无法在这里插入图片。');
          return;
        }
        setView(null);
        void perform(async () => {
          if (!editor.current?.insertImages([item])) throw new Error('无法在当前光标处插入图片。');
        });
      }} onRename={async (item, name) => {
        let success = false;
        await perform(async () => {
          const updated = await desktop.renameAttachment(workspaceId, current.id, item.id, name);
          setAttachments((items) => items.map((existing) => existing.id === updated.id ? updated : existing));
          window.dispatchEvent(new CustomEvent<AttachmentChange>(attachmentRenamed, {
            detail: { workspace: workspaceId, item: updated },
          }));
          setAttachmentNotice('附件名称已更新。');
          success = true;
        }, false);
        return success;
      }} onSave={(item) => {
        void perform(async () => {
          if (await desktop.saveAttachmentAs(workspaceId, current.id, item.id)) setAttachmentNotice('附件原件已另存。');
        }, false);
      }} />}
    {view === 'issues' && <Modal title="无法打开的工作空间" onClose={close}>
      <div className="dialog-list">
        {issues.map((issue) => <div key={issue.id} className="workspace-issue">
          <span>{issue.error.message}</span><small>{issue.id}</small>
        </div>)}
      </div>
      <footer className="dialog-actions dialog-pad"><button type="button" className="primary-button" onClick={() => setView('backup')}>备份与恢复</button></footer>
    </Modal>}

    {banner && !view && <div className={`toast ${banner.type}`} role={banner.type === 'error' ? 'alert' : 'status'}>
      {banner.type === 'error' && <CircleAlert size={18} />}<span>{banner.text}</span>
      <button type="button" className="icon-button" aria-label="关闭提示" onClick={() => setBanner(undefined)}><X size={16} /></button>
    </div>}
    {banner?.type === 'error' && view && view !== 'attachments' && view !== 'record' && view !== 'import' && view !== 'backup'
      && <ModalError text={banner.text} />}
  </div>;
}

function ModalError({ text }: { text: string }) {
  useEffect(() => {
    const dialog = document.querySelector('dialog[open]');
    if (!dialog) return;
    const alert = document.createElement('p');
    alert.className = 'dialog-error';
    alert.setAttribute('role', 'alert');
    alert.textContent = text;
    dialog.append(alert);
    return () => { alert.remove(); };
  }, [text]);
  return null;
}
