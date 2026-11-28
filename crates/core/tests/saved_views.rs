// SPDX-License-Identifier: AGPL-3.0-or-later

mod support;

use rusqlite::Connection;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use zhiye_core::{
    Block, Collection, CollectionView, IntegrationDraft, IntegrationGrant as Grant,
    IntegrationMutation, IntegrationScope as Scope, Page, PageDraft, PagePatch, PropertyDraft,
    PropertyKind, RecordDraft, ScheduleEdge, ScheduleMove, ScheduleResize, Store, TimelineZoom,
    ViewDraft, ViewKind, ViewLayout, ViewRange, ViewSettings,
};

struct Fixture {
    store: Store,
    space: String,
    id: String,
    container: String,
}

impl Fixture {
    fn new(root: &std::path::Path) -> Self {
        let store = Store::open(root).unwrap();
        let space = store.create_workspace("视图验收").unwrap().id;
        let (collection, _) = store.create_collection(&space, None, "安排").unwrap();
        Self {
            store,
            space,
            id: collection.id,
            container: collection.container_page_id,
        }
    }

    fn schema(&self) -> Collection {
        self.store.collection(&self.space, &self.id).unwrap()
    }

    fn field(&self, name: &str, kind: PropertyKind) -> String {
        self.store
            .save_property(
                &self.space,
                &self.id,
                self.schema().schema_revision,
                PropertyDraft {
                    id: None,
                    name: name.into(),
                    kind,
                    options: vec![],
                },
            )
            .unwrap()
            .properties
            .last()
            .unwrap()
            .id
            .clone()
    }

    fn page(&self, title: &str) -> Page {
        self.store
            .create_page(&self.space, Some(&self.container), title, "file")
            .unwrap()
    }

    fn record(&self, title: &str, values: &[(&str, Value)]) -> Page {
        let page = self.page(title);
        self.store
            .save_record(
                &self.space,
                &self.id,
                &page.info.id,
                page.info.revision,
                RecordDraft {
                    title: title.into(),
                    schema_revision: self.schema().schema_revision,
                    properties: values
                        .iter()
                        .map(|(id, value)| (id.to_string(), value.clone()))
                        .collect(),
                },
            )
            .unwrap()
    }

    fn view(&self, kind: ViewKind) -> CollectionView {
        self.schema()
            .views
            .into_iter()
            .find(|view| view.kind == kind)
            .unwrap()
    }

    fn set_view(&self, old: &CollectionView, layout: ViewLayout) -> CollectionView {
        self.store
            .upsert_view(
                &self.space,
                &self.id,
                ViewDraft {
                    id: Some(old.id.clone()),
                    revision: Some(old.revision),
                    name: old.name.clone().unwrap(),
                    kind: old.kind,
                    settings: old.settings.clone(),
                    layout: Some(layout),
                },
            )
            .unwrap()
            .views
            .into_iter()
            .find(|item| item.id == old.id)
            .unwrap()
    }

    fn move_to(&self, view: &CollectionView, page: &Page, day: &str) -> zhiye_core::Result<Page> {
        self.store.move_scheduled_record(
            &self.space,
            &self.id,
            ScheduleMove {
                view_id: view.id.clone(),
                view_revision: view.revision,
                page_id: page.info.id.clone(),
                page_revision: page.info.revision,
                day: day.into(),
            },
        )
    }
}

fn between(from: &str, through: &str) -> ViewRange {
    ViewRange::Between {
        from: from.into(),
        through: through.into(),
    }
}

#[test]
fn gallery_uses_the_page_cover_by_default_and_honors_a_cover_property() {
    let root = tempfile::tempdir().unwrap();
    let fx = Fixture::new(root.path());
    let field = fx.field("画廊图片", PropertyKind::Attachment);
    let note = fx.page("图册");
    let image = |name| {
        fx.store
            .add_image(
                &fx.space,
                &note.info.id,
                name,
                &b"\x89PNG\r\n\x1a\nexample"[..],
            )
            .unwrap()
            .attachment
            .id
    };
    let first = image("一.png");
    let second = image("二.png");
    let gallery = fx.view(ViewKind::Gallery);
    let shown = |id| {
        fx.store
            .query_records(&fx.space, &fx.id, id, 0)
            .unwrap()
            .records
            .remove(0)
            .cover
            .map(|cover| cover.attachment_id)
    };
    let fallback = shown(&gallery.id).unwrap();
    let selected = if fallback == first { second } else { first };
    let with_cover = fx
        .store
        .set_page_appearance(
            &fx.space,
            &note.info.id,
            note.info.revision,
            "file",
            Some(&selected),
        )
        .unwrap();
    assert_eq!(shown(&gallery.id).as_deref(), Some(selected.as_str()));

    let record = fx
        .store
        .save_record(
            &fx.space,
            &fx.id,
            &note.info.id,
            with_cover.info.revision,
            RecordDraft {
                title: note.info.title,
                schema_revision: fx.schema().schema_revision,
                properties: BTreeMap::from([(field.clone(), json!([fallback]))]),
            },
        )
        .unwrap();
    let explicit = fx.set_view(
        &gallery,
        ViewLayout::Gallery {
            cover_field: Some(field),
        },
    );
    assert_eq!(shown(&explicit.id).as_deref(), Some(fallback.as_str()));
    let default = fx.set_view(&explicit, ViewLayout::Gallery { cover_field: None });
    fx.store
        .set_page_appearance(
            &fx.space,
            &record.info.id,
            record.info.revision,
            "file",
            None,
        )
        .unwrap();
    assert_eq!(shown(&default.id), Some(fallback));
}

#[test]
fn expands_legacy_views_in_place_and_deleting_a_view_keeps_records() {
    let root = tempfile::tempdir().unwrap();
    let fx = Fixture::new(root.path());
    let note = fx.page("原始记录");
    let table = fx.view(ViewKind::Table);
    let board = fx.view(ViewKind::Board);
    let mut old = serde_json::to_value(fx.schema().views).unwrap();
    old.as_array_mut().unwrap().truncate(3);
    for item in old.as_array_mut().unwrap() {
        item.as_object_mut().unwrap().remove("name");
    }
    let database = root.path().join(&fx.space).join("workspace.sqlite");
    let db = Connection::open(&database).unwrap();
    db.execute(
        "UPDATE collections SET views=?1 WHERE id=?2",
        (old.to_string(), &fx.id),
    )
    .unwrap();
    support::remove_v14_schema(&db);
    db.pragma_update(None, "user_version", 12).unwrap();
    let expanded = fx.schema();
    assert_eq!(expanded.views.len(), 6);
    assert_eq!(expanded.views[0].id, table.id);
    assert_eq!(expanded.views[1], board);
    let snapshot = root
        .path()
        .join("upgrade-backups")
        .join(format!("{}-v12.sqlite", fx.space));
    assert_eq!(
        Connection::open(snapshot)
            .unwrap()
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        12
    );
    let draft = |name: &str, id, revision| ViewDraft {
        id,
        revision,
        name: name.into(),
        kind: ViewKind::Table,
        settings: ViewSettings::default(),
        layout: None,
    };
    let added = fx
        .store
        .upsert_view(&fx.space, &fx.id, draft("研究表格", None, None))
        .unwrap();
    let second = added.views.last().unwrap();
    assert_ne!(second.id, table.id);
    assert_eq!(
        fx.store
            .query_records(&fx.space, &fx.id, &second.id, 0)
            .unwrap()
            .records[0]
            .id,
        note.info.id
    );
    assert!(
        fx.store
            .upsert_view(&fx.space, &fx.id, draft("研究表格", None, None))
            .is_err()
    );
    let revised = fx
        .store
        .upsert_view(
            &fx.space,
            &fx.id,
            draft("简报表格", Some(second.id.clone()), Some(second.revision)),
        )
        .unwrap();
    let revised = revised.views.last().unwrap();
    assert_eq!(
        fx.store
            .delete_view(&fx.space, &fx.id, &second.id, second.revision)
            .unwrap_err()
            .code,
        "VIEW_CONFLICT"
    );
    fx.store
        .delete_view(&fx.space, &fx.id, &revised.id, revised.revision)
        .unwrap();
    assert_eq!(fx.schema().views.len(), 6);
    assert_eq!(fx.store.page(&fx.space, &note.info.id).unwrap(), note);
}

#[test]
fn date_windows_filter_before_paging_and_moves_keep_civil_dates_and_offsets() {
    let root = tempfile::tempdir().unwrap();
    let fx = Fixture::new(root.path());
    let day = fx.field("全天", PropertyKind::Date);
    let time = fx.field("会议时间", PropertyKind::DateTime);
    let note = fx.record(
        "待排期",
        &[
            (&day, json!("2026-11-16")),
            (&time, json!("2026-11-16T00:30:00+09:30")),
        ],
    );
    for index in 0..55 {
        fx.page(&format!("无日期 {index}"));
    }
    let original = fx.view(ViewKind::Calendar);
    let list = |view: &CollectionView, range: ViewRange| {
        fx.store
            .query_view_records(&fx.space, &fx.id, &view.id, 0, Some(&range))
            .unwrap()
    };
    assert_eq!(
        list(&original, between("2026-11-16", "2026-11-16")).records[0].id,
        note.info.id
    );
    assert_eq!(list(&original, ViewRange::Unscheduled).total, 55);
    let timed = fx.set_view(
        &original,
        ViewLayout::Calendar {
            date_field: time.clone(),
            end_field: None,
        },
    );
    assert_eq!(
        list(&timed, between("2026-11-15", "2026-11-15")).records[0].id,
        note.info.id
    );
    assert_eq!(list(&timed, between("2026-11-16", "2026-11-16")).total, 0);
    assert_eq!(
        fx.move_to(&original, &note, "2026-11-16").unwrap_err().code,
        "VIEW_CONFLICT"
    );
    let moved = fx.move_to(&timed, &note, "2026-11-16").unwrap();
    let values = moved.properties.as_ref().unwrap();
    assert_eq!(values[&time], json!("2026-11-17T00:30:00+09:30"));
    assert_eq!(values[&day], json!("2026-11-16"));
    assert_eq!(
        list(&timed, between("2026-11-16", "2026-11-16")).records[0].id,
        note.info.id
    );
    assert_eq!(
        fx.move_to(&timed, &note, "2026-11-17").unwrap_err().code,
        "REVISION_CONFLICT"
    );
    assert_eq!(fx.store.history(&fx.space, &note.info.id).unwrap()[1], note);

    let writer = fx
        .store
        .issue_integration(
            &fx.space,
            IntegrationDraft {
                name: "日期接口".into(),
                scopes: BTreeSet::from([Scope::Read, Scope::Update]),
                grants: vec![Grant::Collection(fx.id.clone())],
            },
        )
        .unwrap();
    let raw = fx
        .store
        .integration_page(&fx.space, &writer.token, &note.info.id)
        .unwrap();
    let patch = |value: &str| IntegrationMutation::PatchProperties {
        page_id: note.info.id.clone(),
        patch: PagePatch {
            title: None,
            icon: None,
        },
        properties: BTreeMap::from([(time.clone(), json!(value))]),
    };
    assert_eq!(
        fx.store
            .mutate_as_integration(
                &fx.space,
                &writer.token,
                "invalid",
                Some(&raw.etag),
                patch("2026-11-17T01:00:00")
            )
            .unwrap_err()
            .code,
        "PROPERTY_VALUE"
    );
    fx.store
        .mutate_as_integration(
            &fx.space,
            &writer.token,
            "valid",
            Some(&raw.etag),
            patch("2026-11-18T14:00:00-03:30"),
        )
        .unwrap();
    assert_eq!(
        list(&timed, between("2026-11-18", "2026-11-18")).records[0].id,
        note.info.id
    );
}

#[test]
fn timeline_gallery_and_embeds_share_identity_and_round_trip_in_a_native_archive() {
    let root = tempfile::tempdir().unwrap();
    let fx = Fixture::new(root.path());
    let start = fx.field("开始", PropertyKind::Date);
    let end = fx.field("完成", PropertyKind::Date);
    let cover = fx.field("封面", PropertyKind::Attachment);
    let mut note = fx.record(
        "区间",
        &[(&start, json!("2026-11-14")), (&end, json!("2026-11-19"))],
    );
    let timeline = fx.set_view(
        &fx.view(ViewKind::Timeline),
        ViewLayout::Timeline {
            start_field: start.clone(),
            end_field: end.clone(),
            zoom: TimelineZoom::Week,
        },
    );
    let period = between("2026-11-17", "2026-11-20");
    let overlap = fx
        .store
        .query_view_records(&fx.space, &fx.id, &timeline.id, 0, Some(&period))
        .unwrap();
    assert_eq!(overlap.records[0].id, note.info.id);
    note = fx.move_to(&timeline, &note, "2026-11-21").unwrap();
    assert_eq!(
        note.properties.as_ref().unwrap()[&start],
        json!("2026-11-21")
    );
    assert_eq!(note.properties.as_ref().unwrap()[&end], json!("2026-11-26"));
    assert_eq!(
        fx.store
            .query_view_records(&fx.space, &fx.id, &timeline.id, 0, Some(&period))
            .unwrap()
            .total,
        0
    );
    let image = fx
        .store
        .add_image(
            &fx.space,
            &note.info.id,
            "封面.png",
            &b"\x89PNG\r\n\x1a\nexample"[..],
        )
        .unwrap()
        .attachment;
    let mut properties = note.properties.clone().unwrap();
    properties.insert(cover.clone(), json!([image.id]));
    let note = fx
        .store
        .save_record(
            &fx.space,
            &fx.id,
            &note.info.id,
            note.info.revision,
            RecordDraft {
                title: note.info.title,
                schema_revision: fx.schema().schema_revision,
                properties,
            },
        )
        .unwrap();
    let gallery = fx.set_view(
        &fx.view(ViewKind::Gallery),
        ViewLayout::Gallery {
            cover_field: Some(cover),
        },
    );
    let tiles = fx
        .store
        .query_records(&fx.space, &fx.id, &gallery.id, 0)
        .unwrap();
    assert_eq!(tiles.records[0].id, note.info.id);
    assert_eq!(
        tiles.records[0].cover.as_ref().unwrap().attachment_id,
        image.id
    );
    let memo = fx
        .store
        .create_page(&fx.space, None, "嵌入页", "file")
        .unwrap();
    let embed = Block {
        id: uuid::Uuid::new_v4().to_string(),
        kind: "collectionView".into(),
        schema_version: 1,
        payload: json!({"collectionId":fx.id,"viewId":timeline.id}),
        children: vec![],
        extra: Default::default(),
    };
    fx.store
        .save_page(
            &fx.space,
            &memo.info.id,
            memo.info.revision,
            PageDraft {
                title: memo.info.title.clone(),
                blocks: vec![embed.clone()],
            },
        )
        .unwrap();
    let issue = |name: &str, grants| {
        fx.store
            .issue_integration(
                &fx.space,
                IntegrationDraft {
                    name: name.into(),
                    scopes: BTreeSet::from([Scope::Read]),
                    grants,
                },
            )
            .unwrap()
    };
    let partial = issue("只可读嵌入页", vec![Grant::Page(memo.info.id.clone())]);
    let full = issue(
        "也可读数据库",
        vec![
            Grant::Page(memo.info.id.clone()),
            Grant::Collection(fx.id.clone()),
        ],
    );
    let hidden = fx
        .store
        .integration_page(&fx.space, &partial.token, &memo.info.id)
        .unwrap();
    assert_eq!(hidden.blocks[0].kind, "restricted");
    assert!(
        !serde_json::to_string(&hidden.blocks)
            .unwrap()
            .contains(&fx.id)
    );
    let visible = fx
        .store
        .integration_page(&fx.space, &full.token, &memo.info.id)
        .unwrap();
    assert_eq!(visible.blocks[0].payload, embed.payload);

    let mut data = Cursor::new(Vec::new());
    fx.store
        .write_workspace_archive(&fx.space, &mut data)
        .unwrap();
    let archived = fx.store.read_workspace_archive(data).unwrap();
    let recovered = fx.store.collection(&archived.id, &fx.id).unwrap();
    assert_eq!(recovered.views, fx.schema().views);
    assert_eq!(
        fx.store.page(&archived.id, &memo.info.id).unwrap().blocks,
        vec![embed]
    );
    assert_eq!(
        fx.store
            .query_records(&archived.id, &fx.id, &gallery.id, 0)
            .unwrap()
            .records[0]
            .cover,
        tiles.records[0].cover
    );
    fx.store
        .delete_view(&fx.space, &fx.id, &timeline.id, timeline.revision)
        .unwrap();
    assert_eq!(
        fx.store
            .integration_page(&fx.space, &full.token, &memo.info.id)
            .unwrap()
            .blocks[0]
            .kind,
        "restricted"
    );
    assert_eq!(fx.store.page(&fx.space, &note.info.id).unwrap(), note);
}

#[test]
fn calendar_spans_end_dates_and_resizing_moves_one_edge() {
    let root = tempfile::tempdir().unwrap();
    let fx = Fixture::new(root.path());
    let start = fx.field("日期", PropertyKind::Date);
    let end = fx.field("结束日期", PropertyKind::Date);
    let calendar = fx.set_view(
        &fx.view(ViewKind::Calendar),
        ViewLayout::Calendar {
            date_field: start.clone(),
            end_field: Some(end.clone()),
        },
    );
    let list = |range: ViewRange| {
        fx.store
            .query_view_records(&fx.space, &fx.id, &calendar.id, 0, Some(&range))
            .unwrap()
    };
    let single = fx.record("单日", &[(&start, json!("2026-11-03"))]);
    let span = fx.record(
        "三天",
        &[(&start, json!("2026-11-03")), (&end, json!("2026-11-05"))],
    );
    // A record without an end value lasts its start day; a span shows on each of its days.
    assert_eq!(list(between("2026-11-03", "2026-11-03")).total, 2);
    assert_eq!(
        list(between("2026-11-05", "2026-11-06")).records[0].id,
        span.info.id
    );
    assert_eq!(list(ViewRange::Unscheduled).total, 0);

    let resize = |page: &Page, edge, day: &str| {
        fx.store.resize_scheduled_record(
            &fx.space,
            &fx.id,
            ScheduleResize {
                view_id: calendar.id.clone(),
                view_revision: calendar.revision,
                page_id: page.info.id.clone(),
                page_revision: page.info.revision,
                edge,
                day: day.into(),
            },
        )
    };
    let longer = resize(&single, ScheduleEdge::End, "2026-11-07").unwrap();
    let values = longer.properties.as_ref().unwrap();
    assert_eq!(values[&start], json!("2026-11-03"));
    assert_eq!(values[&end], json!("2026-11-07"));
    // An edge cannot pass the other one; a span back to one day drops the end date.
    let earlier = resize(&span, ScheduleEdge::Start, "2026-11-09").unwrap();
    let values = earlier.properties.as_ref().unwrap();
    assert_eq!(values[&start], json!("2026-11-05"));
    assert!(!values.contains_key(&end));
    // Moving keeps the length; a record without an end moves alone.
    let moved = fx.move_to(&calendar, &longer, "2026-11-10").unwrap();
    let values = moved.properties.as_ref().unwrap();
    assert_eq!(values[&start], json!("2026-11-10"));
    assert_eq!(values[&end], json!("2026-11-14"));
    let alone = fx.move_to(&calendar, &earlier, "2026-11-01").unwrap();
    assert_eq!(
        alone.properties.as_ref().unwrap()[&start],
        json!("2026-11-01")
    );

    // The end property must be a different date property of the same type.
    let timed = fx.field("时刻", PropertyKind::DateTime);
    let refused = fx.store.upsert_view(
        &fx.space,
        &fx.id,
        ViewDraft {
            id: Some(calendar.id.clone()),
            revision: Some(calendar.revision),
            name: "日历".into(),
            kind: ViewKind::Calendar,
            settings: calendar.settings.clone(),
            layout: Some(ViewLayout::Calendar {
                date_field: start.clone(),
                end_field: Some(timed),
            }),
        },
    );
    assert_eq!(refused.unwrap_err().code, "VIEW_LAYOUT");
}

#[test]
fn a_database_can_start_with_any_view_and_its_date_properties() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("新建视图").unwrap().id;
    let parent = store.create_page(&space, None, "笔记", "file").unwrap();
    let (calendar, page) = store
        .create_collection_with_view(&space, Some(&parent.info.id), "", ViewKind::Calendar)
        .unwrap();
    assert_eq!(
        page.info.parent_id.as_deref(),
        Some(parent.info.id.as_str())
    );
    assert_eq!(calendar.views[0].kind, ViewKind::Calendar);
    assert_eq!(calendar.views.len(), 6);
    let date = calendar
        .properties
        .iter()
        .find(|p| p.name == "日期")
        .unwrap();
    assert_eq!(
        calendar.views[0].layout,
        Some(ViewLayout::Calendar {
            date_field: date.id.clone(),
            end_field: None,
        })
    );
    assert_eq!(store.collection(&space, &calendar.id).unwrap(), calendar);

    let (timeline, _) = store
        .create_collection_with_view(&space, None, "计划", ViewKind::Timeline)
        .unwrap();
    assert!(matches!(
        &timeline.views[0].layout,
        Some(ViewLayout::Timeline { start_field, end_field, .. })
            if start_field != end_field
    ));
    let (board, _) = store
        .create_collection_with_view(&space, None, "", ViewKind::Board)
        .unwrap();
    assert_eq!(board.views[0].kind, ViewKind::Board);
    assert_eq!(
        board.views[0].settings.group_by.as_ref(),
        Some(&board.properties[0].id)
    );
    assert_eq!(board.properties.len(), 1);
}
