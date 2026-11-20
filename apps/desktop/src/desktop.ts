// SPDX-License-Identifier: AGPL-3.0-or-later

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type {
  ArchiveMerge, Attachment, AttachmentBatch, BackupAttempt, BackupPolicy, BackupStatus, BacklinkPage, BlobCleanup, BlobCleanupPreview, Collection, CollectionRecord, CollectionSummary, ComputedDraft, ContentImport, CsvExport,
  HistoryEntry, ImportOutcome, Integration, IntegrationDraft, IssuedIntegration, LocalApiStatus, MigrationBatch, Navigation,
  Page, PageDraft, PageInfo, PageMoves, PageVersion, ProposalAction, ProposalReview, ProposalSummary,
  PropertyConversionPreview, PropertyDraft, RecordDraft, RecordPage, ReferenceSource, RelationDraft, RelationTargetPage, ResolvedLocation, ScheduleMove,
  SearchIndexStatus, SearchRequest, SearchResponse, TemplateSummary, TrashPreview, TrashRemoval, ViewDraft, ViewRange, Workspace, WorkspaceOverview,
  PdfDocumentStatus, PdfIndexPoll, PdfIndexResult, PdfProgress, PdfTarget,
} from './contracts';

type PageListRow = [id: string, parentId: string | null, title: string, revision: number,
  updatedAt: number, icon: PageInfo['icon'], trashRootId: string | null];

export const desktop = {
  onNativeCommand: (receive: (command: string) => void) =>
    listen<string>('native-command', (event) => receive(event.payload)),
  onSystemQuit: (receive: (request: string) => Promise<void>) =>
    listen<string>('system-quit-requested', (event) => receive(event.payload)),
  finishSystemQuit: (request: string, saved: boolean) => invoke<void>('finish_macos_quit', { request, saved }),
  onLocation: (receive: () => void) => listen('location-ready', receive),
  onWorkspaceContentChanged: (receive: (workspace: string) => void) =>
    listen<string>('workspace-content-changed', (event) => receive(event.payload)),
  takeLocation: () => invoke<string | null>('take_location'),
  resolveLocation: (url: string) => invoke<ResolvedLocation>('resolve_location', { url }),
  makeLocation: (workspace: string, page: string, block: string | null, pdf?: PdfTarget) =>
    invoke<string>('make_location', { workspace, page, block, pdf }),
  workspaces: () => invoke<WorkspaceOverview>('list_workspaces'),
  createWorkspace: (name: string) => invoke<Workspace>('create_workspace', { name }),
  pages: (workspace: string) => invoke<PageListRow[]>('list_pages', { workspace, compact: true })
    .then((rows) => rows.map(([id, parentId, title, revision, updatedAt, icon, trashRootId]): PageInfo =>
      ({ id, parentId, title, revision, updatedAt, icon, trashRootId }))),
  page: (workspace: string, id: string) => invoke<Page>('read_page', { workspace, id }),
  referenceSource: (workspace: string, page: string, block: string, knownRevision: number | null) =>
    invoke<ReferenceSource>('read_block_reference', { workspace, page, block, knownRevision }),
  backlinks: (workspace: string, page: string, after: string | null) =>
    invoke<BacklinkPage>('list_backlinks', { workspace, page, after }),
  createPage: (workspace: string, parentId: string | null) => invoke<Page>('create_page', { workspace, parentId }),
  savePage: (workspace: string, id: string, revision: number, draft: PageDraft) =>
    invoke<PageInfo>('save_page', { workspace, id, revision, draft }),
  setPageAppearance: (workspace: string, id: string, revision: number, icon: string, coverAttachmentId: string | null) =>
    invoke<Page>('set_page_appearance', { workspace, id, revision, icon, coverAttachmentId }),
  archivePage: (workspace: string, id: string, revision: number) =>
    invoke<void>('archive_page', { workspace, id, revision }),
  restorePage: (workspace: string, id: string, revision: number) =>
    invoke<void>('restore_page', { workspace, id, revision }),
  movePages: (workspace: string, pages: PageVersion[], parentId: string | null) =>
    invoke<PageMoves>('move_pages_with_impact', { workspace, pages, parentId }),
  archivePages: (workspace: string, pages: PageVersion[]) => invoke<number>('archive_pages', { workspace, pages }),
  restorePages: (workspace: string, pages: PageVersion[]) => invoke<number>('restore_pages', { workspace, pages }),
  previewEmptyTrash: (workspace: string) => invoke<TrashPreview>('preview_empty_trash', { workspace }),
  emptyTrash: (workspace: string, confirmation: string) => invoke<TrashRemoval>('empty_trash', { workspace, confirmation }),
  previewBlobCleanup: (workspace: string) => invoke<BlobCleanupPreview>('preview_blob_cleanup', { workspace }),
  cleanBlobs: (workspace: string, confirmation: string) => invoke<BlobCleanup>('clean_blobs', { workspace, confirmation }),
  history: (workspace: string, id: string) => invoke<HistoryEntry[]>('page_history', { workspace, id }),
  integrations: (workspace: string) => invoke<Integration[]>('list_integrations', { workspace }),
  createIntegration: (workspace: string, draft: IntegrationDraft, expiresAt: number | null = null) =>
    invoke<IssuedIntegration>('create_integration', { workspace, draft, expiresAt }),
  revokeIntegration: (workspace: string, id: string) => invoke<void>('revoke_integration', { workspace, id }),
  proposals: (workspace: string, pageId: string | null) =>
    invoke<ProposalSummary[]>('list_proposals', { workspace, pageId }),
  proposal: (workspace: string, id: string) => invoke<ProposalReview>('read_proposal', { workspace, id }),
  reviewProposal: (workspace: string, id: string, reviewRevision: number, action: ProposalAction) =>
    invoke<ProposalReview>('review_proposal', { workspace, id, reviewRevision, action }),
  apiAttachmentLimit: (workspace: string) => invoke<number>('api_attachment_limit', { workspace }),
  setApiAttachmentLimit: (workspace: string, limit: number) => invoke<void>('set_api_attachment_limit', { workspace, limit }),
  localApiStatus: () => invoke<LocalApiStatus>('local_api_status'),
  setLocalApi: (enabled: boolean) => invoke<LocalApiStatus>('set_local_api', { enabled }),
  restoreVersion: (workspace: string, id: string, revision: number, target: number) =>
    invoke<Page>('restore_version', { workspace, id, revision, target }),
  search: (workspace: string, request: SearchRequest) => invoke<SearchResponse>('search_text', { workspace, request }),
  advanceSearchIndex: (workspace: string) => invoke<SearchIndexStatus>('advance_search_index', { workspace }),
  rebuildSearchIndex: (workspace: string) => invoke<SearchIndexStatus>('rebuild_search_index', { workspace }),
  pollPdfIndex: (workspace: string) => invoke<PdfIndexPoll>('poll_pdf_index', { workspace }),
  finishPdfIndex: (workspace: string, result: PdfIndexResult) => invoke<PdfProgress>('finish_pdf_index', { workspace, result }),
  pdfDocumentStatus: (workspace: string, page: string) => invoke<PdfDocumentStatus[]>('pdf_document_status', { workspace, page }),
  collections: (workspace: string) => invoke<CollectionSummary[]>('list_collections', { workspace }),
  createCollection: (workspace: string, parentId: string | null) =>
    invoke<{ collection: CollectionSummary; page: Page }>('create_collection', { workspace, parentId }),
  collection: (workspace: string, collection: string) =>
    invoke<Collection>('read_collection', { workspace, collection }),
  records: (workspace: string, collection: string, view: string, offset: number, range?: ViewRange) =>
    invoke<RecordPage>('list_records', { workspace, collection, view, offset, range }),
  record: (workspace: string, collection: string, id: string) =>
    invoke<CollectionRecord>('read_record', { workspace, collection, id }),
  saveProperty: (workspace: string, collection: string, revision: number, draft: PropertyDraft) =>
    invoke<Collection>('save_property', { workspace, collection, revision, draft }),
  previewPropertyConversion: (workspace: string, collection: string, revision: number, draft: PropertyDraft) =>
    invoke<PropertyConversionPreview>('preview_property_conversion', { workspace, collection, revision, draft }),
  applyPropertyConversion: (workspace: string, collection: string, revision: number, draft: PropertyDraft, fingerprint: string) =>
    invoke<Collection>('apply_property_conversion', { workspace, collection, revision, draft, fingerprint }),
  saveComputedProperty: (workspace: string, collection: string, revision: number, draft: ComputedDraft) =>
    invoke<Collection>('save_computed_property', { workspace, collection, revision, draft }),
  createRelation: (workspace: string, collection: string, revision: number, draft: RelationDraft) =>
    invoke<Collection>('create_relation', { workspace, collection, revision, draft }),
  relationTargets: (workspace: string, collection: string, query: string, offset: number) =>
    invoke<RelationTargetPage>('list_relation_targets', { workspace, collection, query, offset }),
  saveView: (workspace: string, collection: string, draft: ViewDraft) =>
    invoke<Collection>('upsert_collection_view', { workspace, collection, draft }),
  deleteView: (workspace: string, collection: string, view: string, revision: number) =>
    invoke<Collection>('delete_collection_view', { workspace, collection, view, revision }),
  moveScheduledRecord: (workspace: string, collection: string, change: ScheduleMove) =>
    invoke<Page>('move_scheduled_record', { workspace, collection, change }),
  saveRecord: (workspace: string, collection: string, id: string, revision: number, draft: RecordDraft) =>
    invoke<Page>('save_record', { workspace, collection, id, revision, draft }),
  exportWorkspace: (workspace: string, includeHistory = true) =>
    invoke<boolean>('export_workspace', { workspace, includeHistory }),
  exportRecovery: (workspace: string) => invoke<boolean>('export_recovery', { workspace }),
  backupStatus: (workspace: string | null) => invoke<BackupStatus>('backup_status', { workspace }),
  chooseBackupDirectory: () => invoke<string | null>('choose_backup_directory'),
  setBackupPolicy: (policy: BackupPolicy) => invoke<BackupPolicy>('set_backup_policy', { policy }),
  runWorkspaceBackup: (workspace: string) => invoke<BackupAttempt | null>('run_workspace_backup', { workspace }),
  onBackupChecked: (receive: (workspace: string) => void) =>
    listen<string>('backup-checked', (event) => receive(event.payload)),
  exportCollectionCsv: (workspace: string, collection: string, viewId: string | null) =>
    invoke<CsvExport | null>('export_collection_csv', { workspace, collection, viewId }),
  importWorkspace: () => invoke<Workspace | null>('import_workspace'),
  mergeWorkspace: (workspace: string) => invoke<ArchiveMerge | null>('merge_workspace', { workspace }),
  importContent: (workspace: string, request: ContentImport) => {
    if (request.type === 'markdown') {
      return invoke<ImportOutcome[] | null>('import_markdown_files', { workspace, parentId: request.parentId });
    }
    if (request.type === 'csv') {
      return invoke<ImportOutcome[] | null>('import_csv_file', { workspace, destination: request.destination });
    }
    return invoke<ImportOutcome[] | null>('import_migration', { workspace, parentId: request.parentId, format: request.type });
  },
  latestMigration: (workspace: string, nativeContent = false) =>
    invoke<MigrationBatch | null>('latest_migration', { workspace, nativeContent }),
  cancelMigration: (workspace: string) => invoke<boolean>('cancel_migration', { workspace }),
  saveImportReport: (report: string) => invoke<boolean>('save_import_report', { report }),
  navigation: (workspace: string) => invoke<Navigation>('read_navigation', { workspace }),
  visitPage: (workspace: string, page: string) => invoke<Navigation>('visit_page', { workspace, page }),
  setFavorite: (workspace: string, page: string, favorite: boolean) =>
    invoke<Navigation>('set_favorite', { workspace, page, favorite }),
  setInbox: (workspace: string, page: string) => invoke<Navigation>('set_inbox', { workspace, page }),
  enterInbox: (workspace: string, capture: boolean) => invoke<Page>('enter_inbox', { workspace, capture }),
  templates: (workspace: string) => invoke<TemplateSummary[]>('list_templates', { workspace }),
  saveTemplate: (workspace: string, source: string, name: string, templateId: string | null) =>
    invoke<TemplateSummary>('save_template', { workspace, source, name, templateId }),
  applyTemplate: (workspace: string, id: string, parentId: string | null, title: string | null) =>
    invoke<Page>('apply_template', { workspace, id, parentId, title }),
  deleteTemplate: (workspace: string, id: string) => invoke<void>('delete_template', { workspace, id }),
  attachments: (workspace: string, page: string) => invoke<Attachment[]>('list_attachments', { workspace, page }),
  attachment: (workspace: string, page: string, id: string) => invoke<Attachment>('read_attachment', { workspace, page, id }),
  addAttachments: (workspace: string, page: string) => invoke<AttachmentBatch>('add_attachments', { workspace, page }),
  pickImages: (workspace: string, page: string, single = false) => invoke<AttachmentBatch>('pick_images', { workspace, page, single }),
  attachImageBytes: (workspace: string, page: string, name: string, bytes: ArrayBuffer) =>
    invoke<AttachmentBatch['added'][number]>('attach_image_bytes', bytes, {
      headers: { 'x-zhiye-workspace': workspace, 'x-zhiye-page': page, 'x-zhiye-name': encodeURIComponent(name) },
    }),
  renameAttachment: (workspace: string, page: string, id: string, name: string) => invoke<Attachment>('rename_attachment', { workspace, page, id, name }),
  previewAttachment: (workspace: string, page: string, id: string) => invoke<ArrayBuffer>('preview_attachment', { workspace, page, id }),
  saveAttachmentAs: (workspace: string, page: string, id: string) => invoke<boolean>('save_attachment_as', { workspace, page, id }),
  exportMarkdown: (workspace: string, id: string, draft: PageDraft | null = null) =>
    invoke<{ companionFolder: string | null } | null>('export_markdown', { workspace, id, draft }),
  exportHtml: (workspace: string, id: string) =>
    invoke<{ companionFolder: string } | null>('export_html', { workspace, id }),
  exportPageDraft: (workspace: string, id: string, revision: number, draft: PageDraft) =>
    invoke<boolean>('export_page_draft', { workspace, id, revision, draft }),
  exportRecordDraft: (workspace: string, collection: string, id: string, revision: number, draft: RecordDraft) =>
    invoke<boolean>('export_record_draft', { workspace, collection, id, revision, draft }),
  openWebLink: (href: string) => invoke<void>('open_web_link', { href }),
};
