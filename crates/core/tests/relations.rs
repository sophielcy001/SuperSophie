// SPDX-License-Identifier: AGPL-3.0-or-later

mod support;

use rusqlite::Connection;
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Cursor};
use zhiye_core::{
    Collection, IntegrationCollectionQuery, IntegrationDraft, IntegrationGrant as Grant,
    IntegrationMutation as Mutation, IntegrationOperation, IntegrationScope as Scope,
    IssuedIntegration, Page, PagePatch, PropertyDefinition, PropertyDraft, PropertyKind,
    RecordDraft, RelationDraft, Result, Store,
};

fn database(store: &Store, space: &str, parent: Option<&str>, title: &str) -> Collection {
    let (id, _) = store.create_collection(space, parent, title).unwrap();
    store.collection(space, &id.id).unwrap()
}

fn record(store: &Store, space: &str, database: &Collection, title: &str) -> Page {
    store
        .create_page(space, Some(&database.container_page_id), title, "file")
        .unwrap()
}

fn field(
    store: &Store,
    space: &str,
    from: &Collection,
    to: &Collection,
    paired: bool,
    name: &str,
) -> PropertyDefinition {
    store
        .create_relation(
            space,
            &from.id,
            from.schema_revision,
            RelationDraft {
                name: name.into(),
                collection_id: to.id.clone(),
                inverse_name: paired.then(|| "回链".into()),
            },
        )
        .unwrap()
        .properties
        .into_iter()
        .find(|field| field.name == name)
        .unwrap()
}

fn local(
    store: &Store,
    space: &str,
    database: &str,
    page: &Page,
    properties: BTreeMap<String, Value>,
) -> Result<Page> {
    let schema = store.collection(space, database)?;
    store.save_record(
        space,
        database,
        &page.info.id,
        page.info.revision,
        RecordDraft {
            title: page.info.title.clone(),
            schema_revision: schema.schema_revision,
            properties,
        },
    )
}

fn edge(field: &str, value: Value) -> BTreeMap<String, Value> {
    BTreeMap::from([(field.into(), value)])
}

fn value(page: &Page, field: &str) -> Option<Value> {
    page.properties.as_ref().unwrap().get(field).cloned()
}

fn issue(
    store: &Store,
    space: &str,
    name: &str,
    grants: Vec<Grant>,
    write: bool,
) -> IssuedIntegration {
    let mut scopes = std::collections::BTreeSet::from([Scope::Read, Scope::Changes]);
    if write {
        scopes.insert(Scope::Update);
    }
    store
        .issue_integration(
            space,
            IntegrationDraft {
                name: name.into(),
                scopes,
                grants,
            },
        )
        .unwrap()
}

fn patch(
    store: &Store,
    space: &str,
    actor: &IssuedIntegration,
    page: &str,
    key: &str,
    properties: BTreeMap<String, Value>,
) -> Result<IntegrationOperation> {
    let read = store.integration_page(space, &actor.token, page)?;
    store.mutate_as_integration(
        space,
        &actor.token,
        key,
        Some(&read.etag),
        Mutation::PatchProperties {
            page_id: page.into(),
            patch: PagePatch {
                title: None,
                icon: None,
            },
            properties,
        },
    )
}

fn problem(result: Result<IntegrationOperation>, code: &str) {
    assert_eq!(result.unwrap_err().code, code);
}

#[test]
fn paired_edges_history_templates_and_backup_preserve_stable_ids() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("关联身份").unwrap().id;
    let from = database(&store, &space, None, "任务");
    let to = database(&store, &space, None, "项目");
    let a = record(&store, &space, &from, "任务一");
    let b = record(&store, &space, &to, "项目一");
    let db = Connection::open(root.path().join(&space).join("workspace.sqlite")).unwrap();
    support::remove_v11_schema(&db);
    let forward = field(&store, &space, &from, &to, true, "项目关联");
    let inverse = forward
        .relation
        .as_ref()
        .unwrap()
        .inverse_property_id
        .clone()
        .unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        22
    );
    let backup = Connection::open(
        root.path()
            .join("upgrade-backups")
            .join(format!("{space}-v10.sqlite")),
    )
    .unwrap();
    assert_eq!(
        backup
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        10
    );
    let first = local(
        &store,
        &space,
        &from.id,
        &a,
        edge(&forward.id, json!({"add":[b.info.id]})),
    )
    .unwrap();
    let other = store.page(&space, &b.info.id).unwrap();
    assert_eq!(value(&first, &forward.id), Some(json!([b.info.id])));
    assert_eq!(value(&other, &inverse), Some(json!([a.info.id])));
    assert_eq!(other.info.revision, b.info.revision + 1);
    assert_eq!(
        db.query_row("SELECT count(*) FROM relation_edges", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        2
    );
    let removed = local(
        &store,
        &space,
        &to.id,
        &other,
        edge(&inverse, json!({"remove":[a.info.id]})),
    )
    .unwrap();
    assert!(value(&removed, &inverse).is_none());
    let cleared = store.page(&space, &a.info.id).unwrap();
    assert_eq!(cleared.info.revision, first.info.revision + 1);
    assert!(value(&cleared, &forward.id).is_none());
    let restored = store
        .restore_version(
            &space,
            &a.info.id,
            cleared.info.revision,
            first.info.revision,
        )
        .unwrap();
    assert_eq!(value(&restored, &forward.id), Some(json!([b.info.id])));
    assert_eq!(
        value(&store.page(&space, &b.info.id).unwrap(), &inverse),
        Some(json!([a.info.id]))
    );

    let schema = store.collection(&space, &from.id).unwrap();
    let renamed = store
        .save_property(
            &space,
            &from.id,
            schema.schema_revision,
            PropertyDraft {
                id: Some(forward.id.clone()),
                name: "改名后仍关联".into(),
                kind: PropertyKind::Relation,
                options: vec![],
            },
        )
        .unwrap();
    let repaired = renamed
        .properties
        .iter()
        .find(|field| field.id == forward.id)
        .unwrap();
    assert_eq!(repaired.relation, forward.relation);
    let template = store
        .save_template(&space, &a.info.id, "有项目的任务模板", None)
        .unwrap();
    let cloned = store
        .apply_template(&space, &template.id, Some(&from.container_page_id), None)
        .unwrap();
    assert_eq!(value(&cloned, &forward.id), Some(json!([b.info.id])));
    let inverse_members = value(&store.page(&space, &b.info.id).unwrap(), &inverse).unwrap();
    assert_eq!(inverse_members.as_array().unwrap().len(), 2);
    assert!(
        inverse_members
            .as_array()
            .unwrap()
            .contains(&json!(cloned.info.id))
    );
    let mut archive = Cursor::new(Vec::new());
    store.write_workspace_archive(&space, &mut archive).unwrap();
    let recovered = store.read_workspace_archive(archive).unwrap();
    assert_eq!(
        value(&store.page(&recovered.id, &b.info.id).unwrap(), &inverse),
        Some(inverse_members)
    );
    assert_eq!(
        store
            .record(&recovered.id, &from.id, &a.info.id)
            .unwrap()
            .relations[&forward.id]
            .items[0]
            .title,
        "项目一"
    );
}

#[test]
fn policy_hides_relation_members_and_metadata_while_permitting_scoped_deltas() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("部分可见").unwrap().id;
    let from = database(&store, &space, None, "调查");
    let branch = store
        .create_page(&space, None, "可移动分支", "file")
        .unwrap();
    let to = database(&store, &space, Some(&branch.info.id), "原件");
    let a = record(&store, &space, &from, "编辑目标");
    let accessible = record(&store, &space, &to, "可见记录");
    let later_hidden = record(&store, &space, &to, "内部记录");
    let d = field(&store, &space, &from, &to, true, "证据");
    let inverse = d
        .relation
        .as_ref()
        .unwrap()
        .inverse_property_id
        .as_ref()
        .unwrap();
    let a = local(
        &store,
        &space,
        &from.id,
        &a,
        edge(
            &d.id,
            json!({"add":[accessible.info.id, later_hidden.info.id]}),
        ),
    )
    .unwrap();
    let writer = issue(
        &store,
        &space,
        "局部编辑",
        vec![
            Grant::Collection(from.id.clone()),
            Grant::Page(accessible.info.id.clone()),
        ],
        true,
    );
    let hidden_reader = issue(
        &store,
        &space,
        "只看引用方",
        vec![Grant::Collection(from.id.clone())],
        false,
    );
    let full = issue(
        &store,
        &space,
        "全部可见",
        vec![
            Grant::Collection(from.id.clone()),
            Grant::Collection(to.id.clone()),
        ],
        false,
    );
    let page = store
        .integration_page(&space, &writer.token, &a.info.id)
        .unwrap();
    assert!(page.properties_restricted);
    assert_eq!(page.restricted_property_ids, std::slice::from_ref(&d.id));
    assert_eq!(page.properties.unwrap()[&d.id], json!([accessible.info.id]));
    let schema = store
        .integration_collection(&space, &writer.token, &from.id)
        .unwrap();
    assert!(
        schema
            .properties
            .iter()
            .find(|field| field.id == d.id)
            .unwrap()
            .relation
            .is_none()
    );
    assert!(!serde_json::to_string(&schema).unwrap().contains(&to.id));
    let rows = store
        .integration_collection_query(
            &space,
            &writer.token,
            &from.id,
            &IntegrationCollectionQuery::default(),
        )
        .unwrap();
    assert_eq!(
        rows.page.items[0].properties[&d.id],
        json!([accessible.info.id])
    );
    assert_eq!(
        rows.page.items[0].restricted_property_ids,
        std::slice::from_ref(&d.id)
    );
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains(&later_hidden.info.id)
    );

    problem(
        patch(
            &store,
            &space,
            &writer,
            &a.info.id,
            "reject-replace",
            edge(&d.id, json!([])),
        ),
        "PARTIAL_CONTENT",
    );
    problem(
        patch(
            &store,
            &space,
            &writer,
            &a.info.id,
            "reject-guessed",
            edge(&d.id, json!({"remove":[later_hidden.info.id]})),
        ),
        "RESOURCE_NOT_FOUND",
    );
    assert_eq!(store.page(&space, &a.info.id).unwrap(), a);
    let status = from
        .properties
        .iter()
        .find(|field| field.kind == PropertyKind::Status)
        .unwrap();
    patch(
        &store,
        &space,
        &writer,
        &a.info.id,
        "ordinary",
        edge(&status.id, json!(status.options[0].id)),
    )
    .unwrap();
    let operation = patch(
        &store,
        &space,
        &writer,
        &a.info.id,
        "remove-visible",
        edge(&d.id, json!({"remove":[accessible.info.id]})),
    )
    .unwrap();
    assert_eq!(
        value(&store.page(&space, &a.info.id).unwrap(), &d.id),
        Some(json!([later_hidden.info.id]))
    );
    assert!(value(&store.page(&space, &accessible.info.id).unwrap(), inverse).is_none());
    let source = store
        .revision_attributions(&space, &accessible.info.id)
        .unwrap();
    assert_eq!(
        source.values().next().unwrap().operation_id,
        operation.operation_id
    );
    patch(
        &store,
        &space,
        &writer,
        &a.info.id,
        "reattach-visible",
        edge(&d.id, json!({"add":[accessible.info.id]})),
    )
    .unwrap();

    let get_cursor = |reader: &IssuedIntegration| {
        store
            .integration_catalog(&space, &reader.token, None, None)
            .unwrap()
            .change_cursor
            .unwrap()
    };
    let a_before = store.page(&space, &a.info.id).unwrap();
    let full_cursor = get_cursor(&full);
    let hidden_cursor = get_cursor(&hidden_reader);
    let b = store.page(&space, &accessible.info.id).unwrap();
    store
        .archive_page(&space, &b.info.id, b.info.revision)
        .unwrap();
    assert!(store.advance_derived_changes(&space).unwrap());
    let source_contents = store.record(&space, &from.id, &a.info.id).unwrap();
    assert!(source_contents.relations[&d.id].restricted);
    assert!(
        !serde_json::to_string(&source_contents)
            .unwrap()
            .contains(&accessible.info.id)
    );
    assert_eq!(store.page(&space, &a.info.id).unwrap(), a_before);
    let notices = store
        .integration_changes(&space, &full.token, None, &full_cursor)
        .unwrap();
    assert_eq!(
        notices
            .items
            .iter()
            .filter(|item| item.action == "derived.invalidated")
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        [a.info.id.as_str()]
    );
    assert!(
        store
            .integration_changes(&space, &hidden_reader.token, None, &hidden_cursor)
            .unwrap()
            .items
            .is_empty()
    );
    store
        .restore_page(&space, &b.info.id, b.info.revision + 1)
        .unwrap();
    assert!(store.advance_derived_changes(&space).unwrap());
    assert_eq!(
        store
            .record(&space, &from.id, &a.info.id)
            .unwrap()
            .relations[&d.id]
            .items
            .len(),
        2
    );
    let movable = issue(
        &store,
        &space,
        "分支授权",
        vec![Grant::Collection(from.id), Grant::Subtree(branch.info.id)],
        false,
    );
    let old_cursor = get_cursor(&movable);
    let visible = store
        .integration_page(&space, &movable.token, &a.info.id)
        .unwrap();
    let target_container = store.page(&space, &to.container_page_id).unwrap();
    store
        .move_page(
            &space,
            &to.container_page_id,
            target_container.info.revision,
            None,
        )
        .unwrap();
    let hidden = store
        .integration_page(&space, &movable.token, &a.info.id)
        .unwrap();
    assert_eq!(hidden.page_revision, visible.page_revision);
    assert_ne!(hidden.etag, visible.etag);
    assert_eq!(hidden.properties.unwrap()[&d.id], json!([]));
    assert_eq!(
        store
            .integration_changes(&space, &movable.token, None, &old_cursor)
            .unwrap_err()
            .code,
        "CURSOR_INVALID"
    );
}

#[test]
fn paired_write_failure_rolls_back_both_records_and_keeps_one_way_edges_independent() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("关联原子性").unwrap().id;
    let from = database(&store, &space, None, "左");
    let to = database(&store, &space, None, "右");
    let a = record(&store, &space, &from, "左记录");
    let b = record(&store, &space, &to, "右记录");
    let paired = field(&store, &space, &from, &to, true, "双向");
    let revision = store.collection(&space, &from.id).unwrap();
    let single = field(&store, &space, &revision, &to, false, "单向");
    let a = local(
        &store,
        &space,
        &from.id,
        &a,
        edge(&single.id, json!({"add":[b.info.id]})),
    )
    .unwrap();
    assert_eq!(store.page(&space, &b.info.id).unwrap(), b);
    let db = Connection::open(root.path().join(&space).join("workspace.sqlite")).unwrap();
    db.execute_batch(&format!(
        "CREATE TRIGGER stop_pair BEFORE UPDATE OF properties ON collection_records
         WHEN new.page_id = '{}' BEGIN SELECT RAISE(ABORT, 'fixture'); END;",
        b.info.id
    ))
    .unwrap();
    let failed = local(
        &store,
        &space,
        &from.id,
        &a,
        edge(&paired.id, json!({"add":[b.info.id]})),
    );
    assert!(failed.is_err());
    assert_eq!(store.page(&space, &a.info.id).unwrap(), a);
    assert_eq!(store.page(&space, &b.info.id).unwrap(), b);
    assert_eq!(
        db.query_row("SELECT count(*) FROM relation_edges", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn paired_creation_uses_update_scope_and_self_relations_commit_one_revision() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("同库配对").unwrap().id;
    let collection = database(&store, &space, None, "节点");
    let peer = record(&store, &space, &collection, "已有记录");
    let forward = field(&store, &space, &collection, &collection, true, "关联节点");
    let reverse = forward
        .relation
        .as_ref()
        .unwrap()
        .inverse_property_id
        .as_ref()
        .unwrap();
    let issue_creator = |update| {
        let mut scopes = std::collections::BTreeSet::from([Scope::Read, Scope::Create]);
        if update {
            scopes.insert(Scope::Update);
        }
        store
            .issue_integration(
                &space,
                IntegrationDraft {
                    name: "记录创建".into(),
                    scopes,
                    grants: vec![Grant::Collection(collection.id.clone())],
                },
            )
            .unwrap()
    };
    let create = |actor: &IssuedIntegration| {
        store.mutate_as_integration(
            &space,
            &actor.token,
            "add-node",
            None,
            Mutation::CreateRecord {
                collection_id: collection.id.clone(),
                title: "新记录".into(),
                icon: "file".into(),
                properties: edge(&forward.id, json!({"add":[peer.info.id]})),
            },
        )
    };
    problem(create(&issue_creator(false)), "FORBIDDEN");
    assert_eq!(store.pages(&space).unwrap().len(), 2);
    let founder = issue_creator(true);
    let receipt = create(&founder).unwrap();
    let new = store.page(&space, &receipt.page_id).unwrap();
    assert_eq!(value(&new, &forward.id), Some(json!([peer.info.id])));
    assert_eq!(
        value(&store.page(&space, &peer.info.id).unwrap(), reverse),
        Some(json!([new.info.id]))
    );
    let self_linked = local(
        &store,
        &space,
        &collection.id,
        &new,
        edge(&forward.id, json!({"add":[new.info.id]})),
    )
    .unwrap();
    assert_eq!(self_linked.info.revision, new.info.revision + 1);
    assert_eq!(value(&self_linked, reverse), Some(json!([new.info.id])));
    let removed_from_inverse = local(
        &store,
        &space,
        &collection.id,
        &self_linked,
        edge(reverse, json!({"remove":[new.info.id]})),
    )
    .unwrap();
    assert_eq!(
        value(&removed_from_inverse, &forward.id),
        Some(json!([peer.info.id]))
    );
    assert!(value(&removed_from_inverse, reverse).is_none());
}
