// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::{run, wire_page};
use serde::Serialize;
use tauri::AppHandle;
use zhiye_core::{
    Collection, CollectionSummary, CollectionView, ComputedDraft, Page, PropertyConversionPreview,
    PropertyDraft, Record, RecordDraft, RecordPage, RelationDraft, RelationTargetPage, Result,
    ScheduleMove, ScheduleResize, ViewDraft, ViewKind, ViewRange, ViewSettings,
};

#[derive(Serialize)]
pub(crate) struct CreatedCollection {
    collection: CollectionSummary,
    page: Page,
}

#[tauri::command]
pub(crate) async fn list_collections(
    app: AppHandle,
    workspace: String,
) -> Result<Vec<CollectionSummary>> {
    run(app, move |store| store.collections(&workspace)).await
}

#[tauri::command]
pub(crate) async fn create_collection(
    app: AppHandle,
    workspace: String,
    parent_id: Option<String>,
    view: Option<ViewKind>,
) -> Result<CreatedCollection> {
    run(app, move |store| {
        let (collection, page) = match view {
            Some(first) => {
                let (collection, page) = store.create_collection_with_view(
                    &workspace,
                    parent_id.as_deref(),
                    "",
                    first,
                )?;
                (
                    CollectionSummary {
                        id: collection.id,
                        container_page_id: collection.container_page_id,
                    },
                    page,
                )
            }
            None => store.create_collection(&workspace, parent_id.as_deref(), "")?,
        };
        Ok(CreatedCollection {
            collection,
            page: wire_page(store, &workspace, page)?,
        })
    })
    .await
}

#[tauri::command]
pub(crate) async fn create_relation(
    app: AppHandle,
    workspace: String,
    collection: String,
    revision: u32,
    draft: RelationDraft,
) -> Result<Collection> {
    run(app, move |store| {
        store.create_relation(&workspace, &collection, revision, draft)
    })
    .await
}

#[tauri::command]
pub(crate) async fn list_relation_targets(
    app: AppHandle,
    workspace: String,
    collection: String,
    query: String,
    offset: u32,
) -> Result<RelationTargetPage> {
    run(app, move |store| {
        store.relation_targets(&workspace, &collection, &query, offset)
    })
    .await
}

#[tauri::command]
pub(crate) async fn read_collection(
    app: AppHandle,
    workspace: String,
    collection: String,
) -> Result<Collection> {
    run(app, move |store| store.collection(&workspace, &collection)).await
}

#[tauri::command]
pub(crate) async fn list_records(
    app: AppHandle,
    workspace: String,
    collection: String,
    view: String,
    offset: u32,
    range: Option<ViewRange>,
) -> Result<RecordPage> {
    run(app, move |store| {
        store.query_view_records(&workspace, &collection, &view, offset, range.as_ref())
    })
    .await
}

#[tauri::command]
pub(crate) async fn read_record(
    app: AppHandle,
    workspace: String,
    collection: String,
    id: String,
) -> Result<Record> {
    run(app, move |store| store.record(&workspace, &collection, &id)).await
}

#[tauri::command]
pub(crate) async fn save_property(
    app: AppHandle,
    workspace: String,
    collection: String,
    revision: u32,
    draft: PropertyDraft,
) -> Result<Collection> {
    run(app, move |store| {
        store.save_property(&workspace, &collection, revision, draft)
    })
    .await
}

#[tauri::command]
pub(crate) async fn preview_property_conversion(
    app: AppHandle,
    workspace: String,
    collection: String,
    revision: u32,
    draft: PropertyDraft,
) -> Result<PropertyConversionPreview> {
    run(app, move |store| {
        store.preview_property_conversion(&workspace, &collection, revision, draft)
    })
    .await
}

#[tauri::command]
pub(crate) async fn apply_property_conversion(
    app: AppHandle,
    workspace: String,
    collection: String,
    revision: u32,
    draft: PropertyDraft,
    fingerprint: String,
) -> Result<Collection> {
    run(app, move |store| {
        store.apply_property_conversion(&workspace, &collection, revision, draft, &fingerprint)
    })
    .await
}

#[tauri::command]
pub(crate) async fn save_computed_property(
    app: AppHandle,
    workspace: String,
    collection: String,
    revision: u32,
    draft: ComputedDraft,
) -> Result<Collection> {
    run(app, move |store| {
        store.save_computed_property(&workspace, &collection, revision, draft)
    })
    .await
}

#[tauri::command]
pub(crate) async fn save_collection_view(
    app: AppHandle,
    workspace: String,
    collection: String,
    view: String,
    revision: u32,
    settings: ViewSettings,
) -> Result<CollectionView> {
    run(app, move |store| {
        store.save_view(&workspace, &collection, &view, revision, settings)
    })
    .await
}

#[tauri::command]
pub(crate) async fn upsert_collection_view(
    app: AppHandle,
    workspace: String,
    collection: String,
    draft: ViewDraft,
) -> Result<Collection> {
    run(app, move |store| {
        store.upsert_view(&workspace, &collection, draft)
    })
    .await
}

#[tauri::command]
pub(crate) async fn delete_collection_view(
    app: AppHandle,
    workspace: String,
    collection: String,
    view: String,
    revision: u32,
) -> Result<Collection> {
    run(app, move |store| {
        store.delete_view(&workspace, &collection, &view, revision)
    })
    .await
}

#[tauri::command]
pub(crate) async fn move_scheduled_record(
    app: AppHandle,
    workspace: String,
    collection: String,
    change: ScheduleMove,
) -> Result<Page> {
    run(app, move |store| {
        let page = store.move_scheduled_record(&workspace, &collection, change)?;
        wire_page(store, &workspace, page)
    })
    .await
}

#[tauri::command]
pub(crate) async fn resize_scheduled_record(
    app: AppHandle,
    workspace: String,
    collection: String,
    change: ScheduleResize,
) -> Result<Page> {
    run(app, move |store| {
        let page = store.resize_scheduled_record(&workspace, &collection, change)?;
        wire_page(store, &workspace, page)
    })
    .await
}

#[tauri::command]
pub(crate) async fn save_record(
    app: AppHandle,
    workspace: String,
    collection: String,
    id: String,
    revision: u32,
    draft: RecordDraft,
) -> Result<Page> {
    run(app, move |store| {
        wire_page(
            store,
            &workspace,
            store.save_record(&workspace, &collection, &id, revision, draft)?,
        )
    })
    .await
}
