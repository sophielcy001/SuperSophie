// SPDX-License-Identifier: AGPL-3.0-or-later

mod archive;
mod assets;
mod backup;
mod blob_cleanup;
mod bundle;
mod collections;
mod desktop_blocks;
mod desktop_pages;
mod file_sync;
mod html_export;
mod imports;
mod integrations;
mod locations;
mod markdown;
mod migrations;
mod model;
mod navigation;
mod page_management;
mod references;
mod rich_blocks;
mod search;
mod store;
mod sync;
mod templates;
mod trash;

pub use archive::ArchiveMerge;
pub use assets::{Attachment, AttachmentImport, MAX_ATTACHMENT_BYTES, MAX_PREVIEW_BYTES};
pub use backup::{
    BackupAttempt, BackupCheck, BackupPolicy, BackupRunState, BackupStatus, RecoveryKind,
    RecoveryPoint,
};
pub use blob_cleanup::{BlobCleanup, BlobCleanupPreview};
pub use collections::{
    Calculation, Collection, CollectionSummary, CollectionView, ComputedDraft, ComputedError,
    ComputedPage, ComputedValue, ComputedValues, CsvExportReport, FilterOperator, GalleryCover,
    PropertyConversionPreview, PropertyConversionSample, PropertyDefinition, PropertyDraft,
    PropertyKind, PropertyOption, PropertyOptionDraft, Record, RecordDraft, RecordPage,
    RelatedRecord, RelationDefinition, RelationDraft, RelationTargetPage, RelationValue,
    RollupFunction, ScheduleEdge, ScheduleMove, ScheduleResize, SortDirection, TimelineZoom,
    ViewDraft, ViewFilter, ViewKind, ViewLayout, ViewRange, ViewSettings, ViewSort,
};
pub use desktop_blocks::{decode_desktop_blocks, encode_desktop_blocks};
pub use desktop_pages::DesktopPageList;
pub use file_sync::sync_export_file;
pub use imports::{
    CsvDestination, ImportFile, ImportOriginal, ImportOutcome, ImportStatus, MAX_CSV_IMPORT_BYTES,
    MAX_MARKDOWN_IMPORT_BYTES, MAX_MARKDOWN_IMPORT_FILES, MigrationBatch,
};
pub use integrations::{
    CatalogItem, CatalogPage, ChangeEvent, ChangePage, Integration, IntegrationAssetDownload,
    IntegrationAssetUpload, IntegrationBlock, IntegrationBlockDraft, IntegrationBlockOperation,
    IntegrationCollection, IntegrationCollectionQuery, IntegrationCollectionRecords,
    IntegrationDraft, IntegrationFilter, IntegrationGrant, IntegrationMarkdown,
    IntegrationMutation, IntegrationOperation, IntegrationPage, IntegrationQueryPage,
    IntegrationRecord, IntegrationReference, IntegrationScope, IntegrationSearchHit,
    IntegrationSearchKind, IntegrationSearchRequest, IntegrationSearchSource, IntegrationSort,
    IntegrationSource, IntegrationUploadResult, IssuedIntegration, LocatedIntegrationBlock,
    MoveImpact, PagePatch, ProposalAction, ProposalChanges, ProposalDraft, ProposalReceipt,
    ProposalReview, ProposalSnapshot, ProposalStatus, ProposalSummary, RevisionAttribution,
    integration_markdown,
};
pub use locations::{PageLocation, ResolvedLocation};
pub use markdown::{
    MarkdownExport, PageExport, PageExportFormat, render_markdown, render_markdown_document,
    render_markdown_export, render_markdown_with_images, render_page_export, render_page_markdown,
    render_workspace_markdown,
};
pub use model::{
    Block, Error, MAX_BUNDLE_BYTES, Page, PageCover, PageDraft, PageInfo, Result, Workspace,
};
pub use navigation::Navigation;
pub use page_management::{PageMoves, PageVersion};
pub use references::{Backlink, BacklinkPage, DesktopReference};
pub use rich_blocks::{image_extension, web_url};
pub use search::pdf::{
    PdfDocumentStatus, PdfExtractionStatus, PdfIndexJob, PdfIndexPoll, PdfIndexResult, PdfPageText,
    PdfProgress, PdfTarget, PdfTextMatch,
};
pub use search::{SearchHit, SearchIndexStatus, SearchRequest, SearchResponse, SearchSnippet};
pub use store::{Store, WorkspaceIssue, WorkspaceOverview};
pub use sync::{SyncConflict, SyncPendingItem, SyncPush, SyncReport, SyncSkip, SyncStatus};
pub use templates::TemplateSummary;
pub use trash::{TrashPreview, TrashRemoval};
pub use zhiye_sync_protocol as sync_protocol;
