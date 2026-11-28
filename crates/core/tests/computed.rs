// SPDX-License-Identifier: AGPL-3.0-or-later

mod support;

use rusqlite::Connection;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use zhiye_core::{
    Calculation, ChangeEvent, Collection, ComputedDraft, ComputedError, ComputedPage,
    ComputedValue, IntegrationDraft, IntegrationGrant as Grant, IntegrationScope as Scope,
    IssuedIntegration, Page, PropertyDraft, PropertyKind, RecordDraft, RelationDraft,
    RollupFunction, Store,
};

fn database(store: &Store, space: &str, title: &str) -> Collection {
    let (id, _) = store.create_collection(space, None, title).unwrap();
    store.collection(space, &id.id).unwrap()
}

fn record(store: &Store, space: &str, collection: &Collection, title: &str) -> Page {
    store
        .create_page(space, Some(&collection.container_page_id), title, "file")
        .unwrap()
}

fn property(
    store: &Store,
    space: &str,
    collection: &Collection,
    id: Option<String>,
    name: &str,
    kind: PropertyKind,
) -> String {
    let revision = store
        .collection(space, &collection.id)
        .unwrap()
        .schema_revision;
    store
        .save_property(
            space,
            &collection.id,
            revision,
            PropertyDraft {
                name: name.into(),
                id,
                kind,
                options: vec![],
            },
        )
        .unwrap()
        .properties
        .into_iter()
        .find(|field| field.name == name)
        .unwrap()
        .id
}

fn calculate(
    store: &Store,
    space: &str,
    collection: &Collection,
    name: &str,
    rule: Calculation,
) -> String {
    let revision = store
        .collection(space, &collection.id)
        .unwrap()
        .schema_revision;
    store
        .save_computed_property(
            space,
            &collection.id,
            revision,
            ComputedDraft {
                id: None,
                name: name.into(),
                calculation: rule,
            },
        )
        .unwrap()
        .properties
        .into_iter()
        .find(|field| field.name == name)
        .unwrap()
        .id
}

fn formula(expression: &str) -> Calculation {
    Calculation::Formula {
        expression: expression.into(),
    }
}

fn save(
    store: &Store,
    space: &str,
    collection: &Collection,
    page: &Page,
    values: BTreeMap<String, Value>,
) -> Page {
    let revision = store
        .collection(space, &collection.id)
        .unwrap()
        .schema_revision;
    store
        .save_record(
            space,
            &collection.id,
            &page.info.id,
            page.info.revision,
            RecordDraft {
                title: page.info.title.clone(),
                schema_revision: revision,
                properties: values,
            },
        )
        .unwrap()
}

fn issue(store: &Store, space: &str, label: &str, grants: Vec<Grant>) -> IssuedIntegration {
    store
        .issue_integration(
            space,
            IntegrationDraft {
                name: label.into(),
                scopes: BTreeSet::from([Scope::Read, Scope::Changes]),
                grants,
            },
        )
        .unwrap()
}

fn page(store: &Store, space: &str, reader: &IssuedIntegration, record: &Page) -> ComputedPage {
    store
        .integration_computed(space, &reader.token, &record.info.id)
        .unwrap()
}

fn number(data: &ComputedPage, field: &str) -> f64 {
    match &data.properties[field] {
        ComputedValue::Value { value } => value.as_f64().unwrap(),
        error => panic!("expected number, got {error:?}"),
    }
}

fn cursor(store: &Store, space: &str, reader: &IssuedIntegration) -> String {
    store
        .integration_catalog(space, &reader.token, None, None)
        .unwrap()
        .change_cursor
        .unwrap()
}

fn derived(
    store: &Store,
    space: &str,
    reader: &IssuedIntegration,
    cursor: &str,
) -> Vec<ChangeEvent> {
    store
        .integration_changes(space, &reader.token, Some(100), cursor)
        .unwrap()
        .items
        .into_iter()
        .filter(|event| event.action == "derived.invalidated")
        .collect()
}

#[test]
fn formula_uses_stable_fields_scalars_dates_and_bounded_evaluation() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("公式").unwrap().id;
    let collection = database(&store, &space, "计算");
    let quantity = property(
        &store,
        &space,
        &collection,
        None,
        "数量",
        PropertyKind::Number,
    );
    let date = property(
        &store,
        &space,
        &collection,
        None,
        "日期",
        PropertyKind::Date,
    );
    let assets = property(
        &store,
        &space,
        &collection,
        None,
        "原件",
        PropertyKind::Attachment,
    );
    let note = record(&store, &space, &collection, "资料甲");
    let note = save(
        &store,
        &space,
        &collection,
        &note,
        BTreeMap::from([(quantity.clone(), json!(4)), (date, json!("2024-02-28"))]),
    );
    let calc = |name, expression| calculate(&store, &space, &collection, name, formula(expression));
    let arithmetic = calc("运算", r#"-(1 + prop("数量")) * 2 + 6 / 3"#);
    let date_test = calc("天数", r#"daysBetween(prop("日期"), date("2024-03-01"))"#);
    let text = calc("文本", r#"upper(concat(prop("标题"), "ab"))"#);
    let lazy = calc(
        "选择",
        r#"if((false && prop("选择")) || 2 <= 3, coalesce(null, 7), 1/0)"#,
    );
    let cycle = calc("自循环", r#"prop("自循环") + 1"#);
    let deep = ["1"; 80].join(" + ");
    let limit = calc("计算深度", &deep);
    let div = calc("除数", "1 / 0");
    let invalid = calc("类型", r#""1" + 2"#);
    let restricted = calc("附件表达式", r#"prop("原件")"#);
    let null = calc("空值", "null + 3");
    let local = store.record(&space, &collection.id, &note.info.id).unwrap();
    assert!(!local.properties.contains_key(&arithmetic));
    assert_eq!(
        local.computed[&restricted],
        ComputedValue::Error {
            code: ComputedError::TypeError
        }
    );
    let reader = issue(
        &store,
        &space,
        "公式授权",
        vec![Grant::Collection(collection.id.clone())],
    );
    let numbers = page(&store, &space, &reader, &note);
    assert_eq!(number(&numbers, &arithmetic), -8.0);
    assert_eq!(number(&numbers, &date_test), 2.0);
    assert_eq!(number(&numbers, &lazy), 7.0);
    assert_eq!(
        numbers.properties[&text],
        ComputedValue::Value {
            value: json!("资料甲AB")
        }
    );
    assert_eq!(
        numbers.properties[&null],
        ComputedValue::Value { value: Value::Null }
    );
    for (id, code) in [
        (cycle, ComputedError::Cycle),
        (limit, ComputedError::LimitExceeded),
        (div, ComputedError::DivisionByZero),
        (invalid, ComputedError::TypeError),
        (restricted, ComputedError::Restricted),
    ] {
        assert_eq!(numbers.properties[&id], ComputedValue::Error { code });
    }
    property(
        &store,
        &space,
        &collection,
        Some(quantity.clone()),
        "改名",
        PropertyKind::Number,
    );
    let renamed = store
        .integration_collection(&space, &reader.token, &collection.id)
        .unwrap();
    let rule = renamed
        .properties
        .iter()
        .find(|item| item.id == arithmetic)
        .unwrap()
        .calculation
        .as_ref()
        .unwrap();
    assert!(
        matches!(rule, Calculation::Formula { expression } if expression.contains(&quantity) && !expression.contains("数量"))
    );
    let after_rename = page(&store, &space, &reader, &note);
    assert_eq!(
        after_rename.representation_version,
        numbers.representation_version
    );
    assert_eq!(
        store
            .save_computed_property(
                &space,
                &collection.id,
                renamed.schema_revision,
                ComputedDraft {
                    name: "非法调用".into(),
                    id: None,
                    calculation: formula(r#"fetch("https://example.invalid")"#),
                }
            )
            .unwrap_err()
            .code,
        "FORMULA_SYNTAX"
    );
    assert_eq!(
        store
            .save_record(
                &space,
                &collection.id,
                &note.info.id,
                note.info.revision,
                RecordDraft {
                    title: note.info.title.clone(),
                    schema_revision: renamed.schema_revision,
                    properties: BTreeMap::from([(arithmetic, json!(0))]),
                }
            )
            .unwrap_err()
            .code,
        "PROPERTY_VALUE"
    );
    assert!(
        !store
            .integration_page(&space, &reader.token, &note.info.id)
            .unwrap()
            .properties
            .unwrap()
            .contains_key(&assets)
    );
}

#[test]
fn rollups_filter_before_count_and_notify_only_the_readable_projection() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("汇总").unwrap().id;
    let project = database(&store, &space, "项目");
    let costs = database(&store, &space, "成本");
    let value = property(&store, &space, &costs, None, "金额", PropertyKind::Number);
    let doubled = calculate(
        &store,
        &space,
        &costs,
        "双倍",
        formula(r#"prop("金额") * 2"#),
    );
    let visible = record(&store, &space, &costs, "公开");
    let visible = save(
        &store,
        &space,
        &costs,
        &visible,
        BTreeMap::from([(value.clone(), json!(2))]),
    );
    let hidden = record(&store, &space, &costs, "内部");
    let hidden = save(
        &store,
        &space,
        &costs,
        &hidden,
        BTreeMap::from([(value.clone(), json!(90))]),
    );
    let empty = record(&store, &space, &costs, "空值");
    let relation = store
        .create_relation(
            &space,
            &project.id,
            project.schema_revision,
            RelationDraft {
                name: "来源".into(),
                collection_id: costs.id.clone(),
                inverse_name: None,
            },
        )
        .unwrap()
        .properties
        .into_iter()
        .find(|field| field.kind == PropertyKind::Relation)
        .unwrap()
        .id;
    let one = record(&store, &space, &project, "关联账本");
    let one = save(
        &store,
        &space,
        &project,
        &one,
        BTreeMap::from([(
            relation.clone(),
            json!({ "add": [visible.info.id, hidden.info.id, empty.info.id] }),
        )]),
    );
    let methods = [
        ("条数", RollupFunction::Count, 3.0, 2.0),
        ("合计", RollupFunction::Sum, 184.0, 4.0),
        ("最小", RollupFunction::Min, 4.0, 4.0),
        ("最大", RollupFunction::Max, 180.0, 4.0),
        ("平均", RollupFunction::Average, 92.0, 4.0),
    ];
    let ids: Vec<_> = methods
        .into_iter()
        .map(|(name, function, all, partial)| {
            (
                calculate(
                    &store,
                    &space,
                    &project,
                    name,
                    Calculation::Rollup {
                        relation_property_id: relation.clone(),
                        function,
                        target_property_id: (function != RollupFunction::Count)
                            .then(|| doubled.clone()),
                    },
                ),
                all,
                partial,
            )
        })
        .collect();
    let formula = calculate(
        &store,
        &space,
        &project,
        "追加运算",
        formula(r#"prop("合计") + 1"#),
    );
    while store.advance_derived_changes(&space).unwrap() {}
    let full = issue(
        &store,
        &space,
        "全部",
        vec![
            Grant::Collection(project.id.clone()),
            Grant::Collection(costs.id.clone()),
        ],
    );
    let partial = issue(
        &store,
        &space,
        "部分",
        vec![
            Grant::Collection(project.id.clone()),
            Grant::Page(visible.info.id.clone()),
            Grant::Page(empty.info.id.clone()),
        ],
    );
    let scoped = issue(
        &store,
        &space,
        "无来源",
        vec![Grant::Collection(project.id.clone())],
    );
    let before = page(&store, &space, &partial, &one);
    let all = page(&store, &space, &full, &one);
    let none = page(&store, &space, &scoped, &one);
    for (id, all_expected, partial_expected) in &ids {
        assert_eq!(number(&all, id), *all_expected);
        assert_eq!(number(&before, id), *partial_expected);
    }
    assert_eq!(number(&none, &ids[0].0), 0.0);
    assert_eq!(number(&none, &ids[1].0), 0.0);
    assert_eq!(
        none.properties[&ids[2].0],
        ComputedValue::Value { value: Value::Null }
    );
    assert_eq!(number(&before, &formula), 5.0);
    let masked = store
        .integration_collection(&space, &partial.token, &project.id)
        .unwrap();
    assert!(
        masked
            .properties
            .iter()
            .filter(|field| field.kind == PropertyKind::Rollup)
            .all(|field| field.calculation.is_none())
    );
    assert_eq!(
        store
            .integration_computed(&space, &partial.token, &hidden.info.id)
            .unwrap_err()
            .code,
        "RESOURCE_NOT_FOUND"
    );
    let raw = store
        .integration_page(&space, &partial.token, &one.info.id)
        .unwrap();
    let from_full = cursor(&store, &space, &full);
    let from_partial = cursor(&store, &space, &partial);
    save(
        &store,
        &space,
        &costs,
        &hidden,
        BTreeMap::from([(value.clone(), json!(100))]),
    );
    let unchanged = page(&store, &space, &partial, &one);
    assert!(!unchanged.stale);
    assert_eq!(
        unchanged.representation_version,
        before.representation_version
    );
    assert_eq!(unchanged.properties, before.properties);
    let pending = page(&store, &space, &full, &one);
    assert!(pending.stale && pending.properties.is_empty());
    assert_eq!(pending.computed_at, all.computed_at);
    assert!(store.advance_derived_changes(&space).unwrap());
    let refreshed = page(&store, &space, &full, &one);
    assert!(!refreshed.stale);
    assert_ne!(refreshed.representation_version, all.representation_version);
    assert!(derived(&store, &space, &partial, &from_partial).is_empty());
    assert_eq!(
        derived(&store, &space, &full, &from_full)[0].id,
        one.info.id
    );
    let from_full = cursor(&store, &space, &full);
    let from_partial = cursor(&store, &space, &partial);
    let visible = save(
        &store,
        &space,
        &costs,
        &visible,
        BTreeMap::from([(value, json!(3))]),
    );
    let pending = page(&store, &space, &partial, &one);
    assert!(pending.stale && pending.properties.is_empty());
    assert_eq!(pending.page_revision, before.page_revision);
    assert!(store.advance_derived_changes(&space).unwrap());
    let updated = page(&store, &space, &partial, &one);
    assert!(!updated.stale);
    assert_eq!(number(&updated, &formula), 7.0);
    assert_eq!(updated.page_revision, before.page_revision);
    assert_ne!(
        updated.representation_version,
        before.representation_version
    );
    assert_eq!(
        store
            .integration_page(&space, &partial.token, &one.info.id)
            .unwrap()
            .etag,
        raw.etag
    );
    assert_eq!(derived(&store, &space, &full, &from_full).len(), 1);
    assert_eq!(derived(&store, &space, &partial, &from_partial).len(), 1);

    let from_partial = cursor(&store, &space, &partial);
    store
        .archive_page(&space, &visible.info.id, visible.info.revision)
        .unwrap();
    assert!(page(&store, &space, &partial, &one).stale);
    drop(store);
    let restarted = Store::open(root.path()).unwrap();
    let db = Connection::open(root.path().join(&space).join("workspace.sqlite")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER fail_computed_publish BEFORE INSERT ON feed_derived_audience
        BEGIN SELECT RAISE(ABORT, 'simulated publish failure'); END;",
    )
    .unwrap();
    assert!(restarted.advance_derived_changes(&space).is_err());
    assert!(page(&restarted, &space, &partial, &one).stale);
    assert!(derived(&restarted, &space, &partial, &from_partial).is_empty());
    db.execute_batch("DROP TRIGGER fail_computed_publish; UPDATE derived_jobs SET retry_at=0;")
        .unwrap();
    assert!(restarted.advance_derived_changes(&space).unwrap());
    assert!(!restarted.advance_derived_changes(&space).unwrap());
    assert_eq!(
        number(&page(&restarted, &space, &partial, &one), &formula),
        1.0
    );
    let invalidations = derived(&restarted, &space, &partial, &from_partial);
    assert_eq!(
        invalidations.len(),
        1,
        "relation and computations share the archive event"
    );
    assert_eq!(invalidations[0].revision, i64::from(one.info.revision));
    restarted
        .restore_page(&space, &visible.info.id, visible.info.revision + 1)
        .unwrap();
    assert!(page(&restarted, &space, &partial, &one).stale);
    assert!(restarted.advance_derived_changes(&space).unwrap());
    assert_eq!(
        number(&page(&restarted, &space, &partial, &one), &formula),
        7.0
    );
}

#[test]
fn queued_updates_keep_their_history_and_publish_in_bounded_batches_after_reopen() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("分批重算").unwrap().id;
    let entries = database(&store, &space, "账本");
    let inputs = database(&store, &space, "原始数值");
    let value = property(&store, &space, &inputs, None, "金额", PropertyKind::Number);
    let input = |title: &str, amount: u32| {
        let row = record(&store, &space, &inputs, title);
        save(
            &store,
            &space,
            &inputs,
            &row,
            BTreeMap::from([(value.clone(), json!(amount))]),
        )
    };
    let visible = input("可读", 1);
    let hidden = input("隐藏", 9);
    let relation = store
        .create_relation(
            &space,
            &entries.id,
            entries.schema_revision,
            RelationDraft {
                name: "来源".into(),
                collection_id: inputs.id.clone(),
                inverse_name: None,
            },
        )
        .unwrap()
        .properties
        .into_iter()
        .find(|field| field.kind == PropertyKind::Relation)
        .unwrap()
        .id;
    let copies: Vec<_> = (0..35)
        .map(|index| {
            let row = record(&store, &space, &entries, &format!("第 {index} 项"));
            save(
                &store,
                &space,
                &entries,
                &row,
                BTreeMap::from([(
                    relation.clone(),
                    json!({"add": [visible.info.id, hidden.info.id]}),
                )]),
            )
        })
        .collect();
    let total = calculate(
        &store,
        &space,
        &entries,
        "合计",
        Calculation::Rollup {
            relation_property_id: relation.clone(),
            function: RollupFunction::Sum,
            target_property_id: Some(value.clone()),
        },
    );
    while store.advance_derived_changes(&space).unwrap() {}
    let full = issue(
        &store,
        &space,
        "完整",
        vec![
            Grant::Collection(entries.id.clone()),
            Grant::Collection(inputs.id.clone()),
        ],
    );
    let partial = issue(
        &store,
        &space,
        "受限",
        vec![
            Grant::Collection(entries.id.clone()),
            Grant::Page(visible.info.id.clone()),
        ],
    );
    for copy in &copies {
        assert_eq!(number(&page(&store, &space, &full, copy), &total), 10.0);
        assert_eq!(number(&page(&store, &space, &partial, copy), &total), 1.0);
    }
    let full_cursor = cursor(&store, &space, &full);
    let partial_cursor = cursor(&store, &space, &partial);
    let limited_before = page(&store, &space, &partial, &copies[0]);
    let db = Connection::open(root.path().join(&space).join("workspace.sqlite")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER worker_only BEFORE INSERT ON derived_job_computed
         BEGIN SELECT RAISE(ABORT, 'worker only'); END;",
    )
    .unwrap();
    save(
        &store,
        &space,
        &inputs,
        &hidden,
        BTreeMap::from([(value.clone(), json!(19))]),
    );
    let invisible = page(&store, &space, &partial, &copies[0]);
    assert!(!invisible.stale);
    assert_eq!(
        invisible.representation_version,
        limited_before.representation_version
    );
    assert_eq!(invisible.computed_at, limited_before.computed_at);
    let visible = save(
        &store,
        &space,
        &inputs,
        &visible,
        BTreeMap::from([(value.clone(), json!(2))]),
    );
    db.execute_batch("DROP TRIGGER worker_only;").unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM computed_record_before", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM derived_job_computed", [], |row| row
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );

    drop(store);
    let store = Store::open(root.path()).unwrap();
    let ready = || page(&store, &space, &partial, &copies[0]);
    let pending = ready();
    assert!(pending.stale && pending.properties.is_empty());
    let local = store
        .record(&space, &entries.id, &copies[0].info.id)
        .unwrap();
    assert!(local.computed_pending && local.computed.is_empty());
    db.execute_batch("UPDATE derived_jobs SET retry_at=9000000000000 WHERE id=(SELECT min(id) FROM derived_jobs);").unwrap();
    assert!(!store.advance_derived_changes(&space).unwrap());
    db.execute_batch("UPDATE derived_jobs SET retry_at=0;")
        .unwrap();

    assert!(store.advance_derived_changes(&space).unwrap());
    assert_eq!(derived(&store, &space, &full, &full_cursor).len(), 16);
    assert!(derived(&store, &space, &partial, &partial_cursor).is_empty());
    while store.advance_derived_changes(&space).unwrap() {}
    let full_changes = derived(&store, &space, &full, &full_cursor);
    let partial_changes = derived(&store, &space, &partial, &partial_cursor);
    assert_eq!(full_changes.len(), 70);
    assert_eq!(partial_changes.len(), 35);
    assert_eq!(
        partial_changes
            .iter()
            .map(|event| &event.id)
            .collect::<BTreeSet<_>>(),
        copies.iter().map(|copy| &copy.info.id).collect()
    );
    for copy in &copies {
        let current = page(&store, &space, &partial, copy);
        assert!(!current.stale);
        assert_eq!(number(&current, &total), 2.0);
        assert_eq!(current.page_revision, copy.info.revision);
        let local = store.record(&space, &entries.id, &copy.info.id).unwrap();
        assert!(!local.computed_pending);
        assert_eq!(
            local.computed[&total],
            ComputedValue::Value { value: json!(21.0) }
        );
    }

    let cursor_before_unlink = cursor(&store, &space, &partial);
    save(
        &store,
        &space,
        &inputs,
        &visible,
        BTreeMap::from([(value, json!(3))]),
    );
    let detached = save(
        &store,
        &space,
        &entries,
        &copies[0],
        BTreeMap::from([(relation, json!({"remove": [visible.info.id]}))]),
    );
    while store.advance_derived_changes(&space).unwrap() {}
    assert_eq!(
        derived(&store, &space, &partial, &cursor_before_unlink).len(),
        35
    );
    assert_eq!(
        number(&page(&store, &space, &partial, &detached), &total),
        0.0
    );
}

#[test]
fn format_eleven_upgrades_and_native_zip_restores_formula_and_index() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("早期格式").unwrap().id;
    let collection = database(&store, &space, "计算");
    let field = property(
        &store,
        &space,
        &collection,
        None,
        "数量",
        PropertyKind::Number,
    );
    let entry = record(&store, &space, &collection, "记录");
    let entry = save(
        &store,
        &space,
        &collection,
        &entry,
        BTreeMap::from([(field.clone(), json!(3))]),
    );
    let db = Connection::open(root.path().join(&space).join("workspace.sqlite")).unwrap();
    support::remove_v12_schema(&db);
    let score = calculate(
        &store,
        &space,
        &collection,
        "得分",
        formula(r#"prop("数量") * 4"#),
    );
    let version: u32 = db
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 22);
    let backup = Connection::open(
        root.path()
            .join("upgrade-backups")
            .join(format!("{space}-v11.sqlite")),
    )
    .unwrap();
    assert_eq!(
        backup
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        11
    );
    let mut archive = Cursor::new(Vec::new());
    store.write_workspace_archive(&space, &mut archive).unwrap();
    let restored = store.read_workspace_archive(archive).unwrap().id;
    assert_eq!(
        store
            .record(&restored, &collection.id, &entry.info.id)
            .unwrap()
            .computed[&score],
        ComputedValue::Value { value: json!(12.0) }
    );
    let restored_db =
        Connection::open(root.path().join(&restored).join("workspace.sqlite")).unwrap();
    assert_eq!(restored_db.query_row(
        "SELECT input_id FROM computed_dependencies WHERE collection_id=?1 AND output_id=?2",
        (&collection.id, &score), |row| row.get::<_, String>(0),
    ).unwrap(), field);
}
