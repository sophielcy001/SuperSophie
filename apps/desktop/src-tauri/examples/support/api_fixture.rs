// SPDX-License-Identifier: AGPL-3.0-or-later

use serde_json::{Value, json};
use std::collections::BTreeMap;
use zhiye_core::{
    BackupPolicy, Block, Calculation, ComputedDraft, IntegrationDraft, IntegrationGrant,
    IntegrationScope, PageDraft, PdfExtractionStatus, PdfIndexResult, PdfPageText, PropertyDraft,
    PropertyKind, RecordDraft, RelationDraft, Result, Store,
};

fn block(kind: &str, number: u32, payload: Value) -> Block {
    Block {
        id: format!("a2160000-0000-4000-8000-{number:012x}"),
        kind: kind.into(),
        schema_version: 1,
        payload,
        children: vec![],
        extra: Default::default(),
    }
}

pub fn seed(store: &Store) -> Result<Value> {
    use IntegrationScope::{
        Archive, Changes, Create, Propose, Read, ReadAssets, Update, WriteAssets,
    };
    store.set_backup_policy(BackupPolicy {
        enabled: false,
        ..BackupPolicy::default()
    })?;
    let workspace = store.create_workspace("API v1 验收工作空间")?.id;
    let other = store.create_workspace("第二个验收工作空间")?.id;
    let root = store
        .create_page(&workspace, None, "验收资料目录", "folder")?
        .info
        .id;
    let note = store
        .create_page(&workspace, Some(&root), "冻结合成笔记甲", "file")?
        .info
        .id;
    let sibling = store
        .create_page(&workspace, Some(&root), "冻结合成笔记乙", "file")?
        .info
        .id;
    let source = store
        .create_page(&workspace, Some(&root), "可定位引用来源", "file")?
        .info
        .id;
    let archive = store
        .create_page(&workspace, Some(&root), "待归档分支", "file")?
        .info
        .id;
    let child = store
        .create_page(&workspace, Some(&archive), "待归档子页", "file")?
        .info
        .id;
    let movable = store
        .create_page(&workspace, Some(&root), "移动时收回权限", "file")?
        .info
        .id;
    let hidden = store
        .create_page(&workspace, None, "验收范围外资料", "file")?
        .info
        .id;

    let source_block = block(
        "paragraph",
        60,
        json!({"inline":[{"type":"text","text":"授权源段落"}]}),
    );
    let anchor = source_block.id.clone();
    store.save_page(
        &workspace,
        &source,
        1,
        PageDraft {
            title: "可定位引用来源".into(),
            blocks: vec![source_block],
        },
    )?;
    let reference = block(
        "blockReference",
        56,
        json!({"pageId":source,"blockId":anchor}),
    );
    let reference_id = reference.id.clone();
    let visible = block(
        "paragraph",
        1,
        json!({"inline":[
            {"type":"text","text":"接口固定关键词"},
            {"type":"pageReference","pageId":hidden,"label":"夹具保密链接文字"}
        ]}),
    );
    let visible_id = visible.id.clone();
    let mut blocks = vec![visible];
    blocks.extend((2..=54).map(|number| {
        block(
            "paragraph",
            number,
            json!({"inline":[{"type":"text","text":format!("验收正文段落 {number}")}]}),
        )
    }));
    blocks.push(reference);
    store.save_page(
        &workspace,
        &note,
        1,
        PageDraft {
            title: "冻结合成笔记甲".into(),
            blocks,
        },
    )?;
    let asset = store
        .add_attachment(
            &workspace,
            &note,
            "验收原件.bin",
            &vec![90_u8; 160 * 1024][..],
        )?
        .attachment;
    let hidden_asset = store
        .add_attachment(&workspace, &hidden, "授权外原件.txt", &b"outside"[..])?
        .attachment;
    let pdf = store
        .add_attachment(
            &workspace,
            &note,
            "契约文字页.pdf",
            &b"%PDF-1.4\n%%EOF\n"[..],
        )?
        .attachment;
    // The response fixture supplies extracted text. The desktop PDF suite
    // verifies actual text extraction and rendering from full documents.
    let job = store
        .poll_pdf_index(&workspace)?
        .job
        .expect("the PDF fixture has one pending index job");
    assert_eq!(job.attachment.id, pdf.id);
    store.finish_pdf_index(
        &workspace,
        PdfIndexResult {
            generation: job.generation,
            attachment_id: pdf.id.clone(),
            sha256: pdf.sha256.clone(),
            status: PdfExtractionStatus::Indexed,
            page_count: Some(1),
            pages: vec![PdfPageText {
                page_number: 1,
                text: "PDF原件契约词".into(),
            }],
        },
    )?;

    let (collection, container) = store.create_collection(&workspace, Some(&root), "验收数据库")?;
    let schema = store.collection(&workspace, &collection.id)?;
    let schema = store.save_property(
        &workspace,
        &collection.id,
        schema.schema_revision,
        PropertyDraft {
            id: None,
            name: "评分".into(),
            kind: PropertyKind::Number,
            options: vec![],
        },
    )?;
    let score = schema.properties[1].id.clone();
    let schema = store.save_computed_property(
        &workspace,
        &collection.id,
        schema.schema_revision,
        ComputedDraft {
            id: None,
            name: "倍数".into(),
            calculation: Calculation::Formula {
                expression: "prop(\"评分\") * 2".into(),
            },
        },
    )?;
    let formula = schema.properties[2].id.clone();
    let (related, related_container) =
        store.create_collection(&workspace, Some(&root), "关联目标数据库")?;
    let visible_related = store
        .create_page(
            &workspace,
            Some(&related_container.info.id),
            "可读关联",
            "file",
        )?
        .info
        .id;
    let hidden_related = store
        .create_page(
            &workspace,
            Some(&related_container.info.id),
            "受限关联",
            "file",
        )?
        .info
        .id;
    let schema = store.create_relation(
        &workspace,
        &collection.id,
        schema.schema_revision,
        RelationDraft {
            name: "跨库关联".into(),
            collection_id: related.id,
            inverse_name: None,
        },
    )?;
    let relation = schema.properties[3].id.clone();
    let mut ids = Vec::new();
    for (title, value) in [("甲条目", 8), ("乙条目", 3), ("丙条目", 1)] {
        let row = store.create_page(&workspace, Some(&container.info.id), title, "file")?;
        store.save_record(
            &workspace,
            &collection.id,
            &row.info.id,
            row.info.revision,
            RecordDraft {
                title: title.into(),
                schema_revision: schema.schema_revision,
                properties: BTreeMap::from([(score.clone(), json!(value))]),
            },
        )?;
        ids.push(row.info.id);
    }
    let row = store.page(&workspace, &ids[0])?;
    store.save_record(
        &workspace,
        &collection.id,
        &row.info.id,
        row.info.revision,
        RecordDraft {
            title: row.info.title,
            schema_revision: schema.schema_revision,
            properties: BTreeMap::from([
                (score.clone(), json!(8)),
                (
                    relation.clone(),
                    json!({"add":[visible_related,hidden_related]}),
                ),
            ]),
        },
    )?;
    while store.advance_search_index(&workspace)?.pending > 0 {}
    while store.advance_derived_changes(&workspace)? {}

    let issue =
        |name: &str, scopes: &[IntegrationScope], grants: Vec<IntegrationGrant>| -> Result<Value> {
            let issued = store.issue_integration(
                &workspace,
                IntegrationDraft {
                    name: format!("{name}验收令牌"),
                    scopes: scopes.iter().copied().collect(),
                    grants,
                },
            )?;
            Ok(json!({"id":issued.integration.id,"token":issued.token}))
        };
    let subtree = || vec![IntegrationGrant::Subtree(root.clone())];
    let tokens = json!({
        "writer":issue("完整", &[Read, Create, Update, Archive, ReadAssets, WriteAssets, Changes, Propose], subtree())?,
        "reader":issue("只读", &[Read], subtree())?,
        "proposer":issue("建议", &[Read, Propose], subtree())?,
        "single":issue("单页归档", &[Read, Archive], vec![IntegrationGrant::Page(archive.clone())])?,
        "rate":issue("计限", &[Read], vec![IntegrationGrant::Page(note.clone())])?,
        "outside":issue("其他范围", &[Read, Update, Propose], vec![IntegrationGrant::Page(hidden.clone())])?,
        "relation":issue("部分关联", &[Read,Update], vec![IntegrationGrant::Collection(collection.id.clone()),IntegrationGrant::Page(visible_related.clone())])?,
    });
    Ok(json!({
        "workspace":workspace, "other":other, "root":root, "note":note, "sibling":sibling, "source":source,
        "anchor":anchor, "visibleBlock":visible_id,"reference":reference_id,"archive":archive,"child":child,
        "movable":movable,"hidden":hidden,"asset":asset,"hiddenAsset":hidden_asset.id,"pdf":pdf,
        "collection":collection.id,"container":container.info.id,"score":score,"formula":formula,
        "high":ids[0],"visibleRelated":visible_related,"hiddenRelated":hidden_related,"relation":relation,"tokens":tokens,
        "defaultAttachmentMiB":store.api_attachment_limit_mib(&workspace)?,
    }))
}
