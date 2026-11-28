// SPDX-License-Identifier: AGPL-3.0-or-later

export interface Workspace {
  id: string;
  name: string;
}

export interface WorkspaceOverview {
  workspaces: Workspace[];
  issues: { id: string; error: { code: string; message: string } }[];
}

export interface BackupPolicy {
  enabled: boolean;
  directory: string | null;
  keepDaily: number;
  keepWeekly: number;
}

export interface BackupAttempt {
  workspaceId: string;
  state: 'running' | 'completed' | 'interrupted' | 'failed';
  directory: string;
  startedAt: number;
  finishedAt: number | null;
  error: string | null;
}

export interface BackupStatus {
  policy: BackupPolicy;
  directory: string;
  defaultDirectory: string;
  lastAttempt: BackupAttempt | null;
  lastSuccess: { createdAt: number; path: string } | null;
  checkError: string | null;
  retentionError: string | null;
  upgradeDirectory: string;
}

export interface Block {
  id: string;
  type: string;
  schemaVersion: number;
  payload: Record<string, unknown>;
  children: Block[];
  [key: string]: unknown;
}

export interface PageInfo {
  id: string;
  parentId: string | null;
  title: string;
  revision: number;
  updatedAt: number;
  icon: string;
  trashRootId: string | null;
}

export interface Page extends PageInfo {
  blocks: Block[];
  properties?: PropertyValues;
  cover?: { attachmentId: string; mediaType: string };
}

export interface ResolvedLocation {
  workspace: Workspace;
  page: Page;
  blockId: string | null;
  missingBlock: boolean;
  pdf?: PdfTarget;
  missingPdf?: boolean;
}

export interface BlockTarget {
  pageId: string;
  blockId: string;
}

export type ReferenceSource =
  | { state: 'available'; revision: number; title: string; block: Block }
  | { state: 'unavailable'; revision: number | null }
  | { state: 'unchanged' };

export interface BacklinkPage {
  items: { page_id: string; block_id: string; title: string; icon: string; target_kind: 'page' | 'block' }[];
  next_cursor: string | null;
}

export interface PageDraft {
  title: string;
  blocks: Block[];
}

export interface HistoryEntry extends Page {
  attribution?: { integrationId: string; integrationName: string; operationId: string };
  historicalProperties?: PropertyDefinition[];
}

export type IntegrationScope = 'content:read' | 'content:create' | 'content:update' | 'content:archive'
  | 'assets:read' | 'assets:write' | 'changes:read' | 'proposals:create';
export interface IntegrationGrant { kind: 'page' | 'subtree' | 'collection'; id: string }
export interface IntegrationDraft { name: string; scopes: IntegrationScope[]; grants: IntegrationGrant[] }
export interface Integration extends IntegrationDraft {
  id: string;
  fingerprint: string;
  createdAt: number;
  expiresAt: number | null;
  lastUsedAt: number | null;
  revokedAt: number | null;
}
export interface IssuedIntegration { integration: Integration; token: string }
export interface LocalApiStatus { address: string | null }

export interface MoveImpact { integrationName: string; added: number; removed: number }
export interface PageMoves { pages: PageInfo[]; impacts: MoveImpact[] }
export interface TrashPreview {
  confirmation: string;
  pages: number;
  branches: number;
  affectedRecords: number;
  templates: number;
}
export interface TrashRemoval { pages: number; updatedRecords: number }
export interface BlobCleanupPreview { confirmation: string; waiting: number; files: number; bytes: number }
export interface BlobCleanup { removed: number; bytes: number; failed: number }
export type ProposalStatus = 'pending' | 'conflict' | 'accepted' | 'rejected' | 'expired';
export type ProposalAction = 'accept' | 'reject' | 'rebase';
export interface ProposalSummary {
  id: string;
  pageId: string;
  pageTitle: string;
  integrationName: string;
  baseRevision: number;
  reviewRevision: number;
  status: ProposalStatus;
  conflict: 'revision_changed' | 'authorization_changed' | 'candidate_changed' | null;
  createdAt: number;
  expiresAt: number;
  acceptedRevision: number | null;
}
export interface ProposalSnapshot {
  title: string;
  icon: string;
  markdown: string;
  blocks: Block[];
  properties: PropertyValues | null;
}
export interface ProposalReview {
  summary: ProposalSummary;
  base: ProposalSnapshot;
  candidate: ProposalSnapshot;
  current: ProposalSnapshot | null;
  currentRevision: number;
  fields: PropertyDefinition[];
}

export interface SearchRequest {
  query: string;
  collectionId?: string;
  filter?: ViewSettings['filter'];
  offset?: number;
  pageOnly?: boolean;
}

export interface SearchIndexStatus { pending: number; pdf: PdfProgress }
export interface SearchSnippet { before: string; matched: string; after: string }
export interface SearchHit {
  page: PageInfo;
  blockId: string | null;
  snippet: SearchSnippet | null;
  pdf?: PdfTextMatch;
}
export interface SearchResponse { hits: SearchHit[]; index: SearchIndexStatus; nextOffset: number | null }

export interface PdfTarget { attachmentId: string; sha256: string; pageNumber: number }
export interface PdfTextMatch extends PdfTarget { name: string }
export type PdfExtractionStatus = 'indexed' | 'partial' | 'noText' | 'locked' | 'unreadable' | 'limitReached';
export interface PdfDocumentStatus {
  attachmentId: string;
  status: PdfExtractionStatus | null;
  pageCount: number | null;
  indexedPages: number;
}
export interface PdfProgress {
  pending: number;
  indexed: number;
  partial: number;
  noText: number;
  failed: number;
  revision: number;
}
export interface PdfIndexJob { generation: number; attachment: Attachment }
export interface PdfIndexPoll { progress: PdfProgress; job: PdfIndexJob | null }
export interface PdfIndexResult {
  generation: number;
  attachmentId: string;
  sha256: string;
  status: PdfExtractionStatus;
  pageCount: number | null;
  pages: { pageNumber: number; text: string }[];
}

export interface Attachment {
  id: string;
  pageId: string;
  name: string;
  sha256: string;
  bytes: number;
  mediaType: string;
  createdAt: number;
}

export interface AttachmentBatch {
  added: { attachment: Attachment; alreadyAttached: boolean; reusedBlob: boolean }[];
  failed: { name: string; error: { code: string; message: string } }[];
}

export type PropertyKind = 'text' | 'number' | 'checkbox' | 'date' | 'dateTime' | 'status' | 'select'
  | 'multiSelect' | 'url' | 'attachment' | 'relation' | 'formula' | 'rollup' | 'createdAt' | 'updatedAt';
export type PropertyValue = string | number | boolean | string[];
export type PropertyValues = Record<string, PropertyValue>;

export interface PropertyOption { id: string; name: string }
export interface PropertyDefinition {
  id: string;
  name: string;
  type: PropertyKind;
  options: PropertyOption[];
  relation?: { collectionId: string; inversePropertyId: string | null };
  calculation?: Calculation;
}
export type RollupFunction = 'count' | 'sum' | 'min' | 'max' | 'average';
export interface RollupCalculation {
  type: 'rollup';
  relationPropertyId: string;
  function: RollupFunction;
  targetPropertyId: string | null;
}
export type Calculation = { type: 'formula'; expression: string } | RollupCalculation;
export interface ComputedDraft { id: string | null; name: string; calculation: Calculation }
export type ComputedError = 'CYCLE' | 'LIMIT_EXCEEDED' | 'RESTRICTED' | 'TYPE_ERROR'
  | 'DIVISION_BY_ZERO' | 'OUT_OF_RANGE' | 'INVALID_DATE';
export type ComputedValue = { status: 'value'; value: string | number | boolean | null }
  | { status: 'error'; code: ComputedError };
export interface PropertyDraft {
  id: string | null;
  name: string;
  type: PropertyKind;
  options: { id: string | null; name: string }[];
}

export interface PropertyConversionPreview {
  fingerprint: string;
  previous: PropertyDefinition;
  target: PropertyDefinition;
  convertedRecords: number;
  clearedRecords: number;
  convertedTemplates: string[];
  clearedTemplates: string[];
  viewChanges: string[];
  samples: { title: string; previous: PropertyValue; converted: PropertyValue | null }[];
}

export type ViewKind = 'table' | 'board' | 'list' | 'calendar' | 'gallery' | 'timeline';
export type FilterOperator = 'equal' | 'contains' | 'empty' | 'notEmpty';
export interface ViewSettings {
  filter: { fieldId: string; operator: FilterOperator; value: string | number | boolean | null } | null;
  sort: { fieldId: string; direction: 'asc' | 'desc' } | null;
  groupBy: string | null;
}
export type TimelineZoom = 'week' | 'fortnight' | 'month';
export type ViewLayout =
  | { type: 'calendar'; dateField: string; endField?: string | null }
  | { type: 'gallery'; coverField: string | null }
  | { type: 'timeline'; startField: string; endField: string; zoom: TimelineZoom };
export type ViewRange = { mode: 'between'; from: string; through: string } | { mode: 'unscheduled' };
export interface CollectionView {
  id: string; type: ViewKind; revision: number; settings: ViewSettings; name?: string; layout?: ViewLayout;
}
export interface ViewDraft {
  id: string | null; revision: number | null; type: ViewKind; name: string; settings: ViewSettings; layout: ViewLayout | null;
}
export interface ViewTarget { collectionId: string; viewId: string }
export interface ScheduleMove { viewId: string; viewRevision: number; pageId: string; pageRevision: number; day: string }
export interface ScheduleResize extends ScheduleMove { edge: 'start' | 'end' }
export interface CollectionSummary { id: string; containerPageId: string }
export interface Collection extends CollectionSummary {
  schemaRevision: number;
  properties: PropertyDefinition[];
  views: CollectionView[];
}

export interface CollectionRecord {
  id: string;
  title: string;
  revision: number;
  createdAt: number;
  updatedAt: number;
  properties: PropertyValues;
  relations?: Record<string, { items: RelatedRecord[]; restricted: boolean }>;
  computed?: Record<string, ComputedValue>;
  computedPending?: boolean;
  cover?: { attachmentId: string; name: string; mediaType: string };
}
export interface RelatedRecord { id: string; title: string }
export interface RelationDraft { name: string; collectionId: string; inverseName: string | null }
export interface RelationTargetPage { items: RelatedRecord[]; nextOffset: number | null }
export interface RelationDelta { add: string[]; remove: string[] }
export interface RecordDraft { title: string; schemaRevision: number; properties: Record<string, PropertyValue | RelationDelta> }
export interface RecordPage { records: CollectionRecord[]; total: number; offset: number; pageSize: number }

export type PageVersion = Pick<PageInfo, 'id' | 'revision'>;

export interface CsvExport {
  rows: number;
  attachments: number;
  companionFolder: string;
  limitations: string[];
}

export interface Navigation {
  inboxPageId: string | null;
  favorites: string[];
  recentPages: string[];
}

export interface TemplateSummary {
  id: string;
  name: string;
  collectionId: string | null;
  pageCount: number;
}

export type CsvDestination =
  | { type: 'new'; parentId: string | null }
  | { type: 'existing'; collectionId: string };
export type MigrationFormat = 'notionZip' | 'obsidianVault';
export type ContentImport =
  | { type: 'markdown' | MigrationFormat; parentId: string | null }
  | { type: 'csv'; destination: CsvDestination };

export interface MigrationBatch {
  id: string;
  kind: MigrationFormat | 'nativeContent';
  source: string;
  status: 'preparing' | 'completed' | 'cancelled' | 'failed' | 'interrupted';
  pageId: string | null;
  detail: string | null;
}

export interface ArchiveMerge {
  pageId: string;
  pages: number;
  collections: number;
  attachments: number;
  revisions: number;
  templates: number;
  notices: string[];
}

export interface ImportOriginal {
  pageId: string;
  attachmentId: string;
  name: string;
}

export interface ImportOutcome {
  name: string;
  status: 'imported' | 'duplicate' | 'preserved' | 'unsupported' | 'failed';
  pageId: string | null;
  records: number;
  notes: string[];
  notesOmitted: number;
  original?: ImportOriginal;
  unmatchedLinks?: number;
}

export interface SyncDevice {
  server: string;
  deviceId: string;
  deviceName: string;
  registeredAt: number;
}

export type SyncSkipReason = 'ORIGINAL_MISSING' | 'PARENT_SKIPPED';

export interface SyncSkip {
  pageId: string;
  title: string;
  reason: SyncSkipReason;
}

export interface SyncPendingItem {
  objectId: string;
  version: number;
  reason: string;
}

export interface SyncStatus {
  enabled: boolean;
  serverUrl: string | null;
  lastAttemptAt: number | null;
  lastSuccessAt: number | null;
  lastError: string | null;
  cursor: number;
  tracked: number;
  unsent: number;
  pending: SyncPendingItem[];
  skipped: SyncSkip[];
}

export type SyncConflictKind = 'EDIT_EDIT' | 'DELETE_EDIT' | 'EDIT_DELETE' | 'PARENT_CYCLE' | 'SCHEMA_MERGED' | 'REJECTED';

export interface SyncConflict {
  pageId: string;
  kind: SyncConflictKind;
  copyPageId: string | null;
}

export interface SyncReport {
  accepted: number;
  applied: number;
  conflicts: SyncConflict[];
  pending: SyncPendingItem[];
  cursor: number;
}

export interface SyncOverview {
  device: SyncDevice | null;
  status: SyncStatus;
}

export interface RemoteWorkspace {
  id: string;
  name: string;
  local: boolean;
}

export function errorMessage(error: unknown) {
  if (typeof error === 'object' && error !== null && 'message' in error) {
    return String(error.message);
  }
  return typeof error === 'string' ? error : '操作失败，请重试。';
}
