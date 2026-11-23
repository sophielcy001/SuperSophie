use serde_json::{Value, json};
use zhiye_core::{Block, IntegrationDraft, IntegrationGrant, IntegrationScope, PageDraft, Result, Store};

pub fn seed(store: &Store) -> Result<Value> {
    let workspace = store.create_workspace("00 原生集成验收")?.id;
    let wrong_workspace = store.create_workspace("99 非目标空间")?.id;
    let root = store.create_page(&workspace, None, "研究资料", "file")?;
    let scoped = store.create_page(&workspace, Some(&root.info.id), "原生集成样稿", "file")?;
    let outside = store.create_page(&workspace, None, "不可见的私人资料", "file")?;
    let bytes = vec![101_u8; 160 * 1024];
    let asset = store.add_attachment(&workspace, &scoped.info.id, "共享原件.bin", bytes.as_slice())?.attachment;
    let private = store.add_attachment(&workspace, &outside.info.id, "私有原件.txt", &b"private"[..])?.attachment;
    let mut blocks: Vec<Block> = (0..55)
        .map(|index| Block {
            id: format!("a0000000-0000-4000-8000-{index:012x}"),
            kind: "paragraph".into(),
            schema_version: 1,
            payload: json!({"inline": [{"type": "text", "text": format!("第 {} 段原生测试", index + 1)}]}),
            children: vec![],
            extra: Default::default(),
        })
        .collect();
    blocks[0].payload["inline"].as_array_mut().unwrap().push(
        json!({"type":"pageReference","pageId": outside.info.id,"label": "保密引用标签"}),
    );
    let block = blocks[0].id.clone();
    store.save_page(&workspace, &scoped.info.id, scoped.info.revision, PageDraft {
        title: scoped.info.title.clone(),
        blocks,
    })?;
    let issue = |name: &str, scopes: &[IntegrationScope]| {
        store.issue_integration(&workspace, IntegrationDraft {
            name: name.into(),
            scopes: scopes.iter().copied().collect(),
            grants: vec![IntegrationGrant::Subtree(root.info.id.clone())],
        })
    };
    use IntegrationScope::{Create, Read, ReadAssets, Update};
    let reader = issue("原生读取夹具", &[Read])?;
    let writer = issue("原生写入夹具", &[Read, Create, Update, ReadAssets])?;
    Ok(json!({
        "workspace": workspace,
        "wrong_workspace": wrong_workspace,
        "root": root.info.id,
        "page": scoped.info.id,
        "outside": outside.info.id,
        "block": block,
        "asset": asset,
        "private_asset": private.id,
        "reader": reader.token,
        "writer": writer.token,
    }))
}

pub fn seed_query_release(store: &Store) -> Result<Value> {
    use std::collections::BTreeMap;
    use zhiye_core::{PropertyDraft, PropertyKind, RecordDraft};
    use IntegrationScope::{Archive, Create, Read, ReadAssets, Update, WriteAssets};

    let workspace = store.create_workspace("00 KB018 原生验收")?.id;
    let other = store.create_workspace("99 API 非目标空间")?.id;
    let root = store.create_page(&workspace, None, "科考目录", "file")?;
    let note = store.create_page(&workspace, Some(&root.info.id), "科考笔记甲", "file")?;
    let second = store.create_page(&workspace, Some(&root.info.id), "科考笔记乙", "file")?;
    let third = store.create_page(&workspace, Some(&root.info.id), "科考笔记丙", "file")?;
    let hidden = store.create_page(&workspace, None, "范围外保密标题", "file")?;
    let branch = store.create_page(&workspace, Some(&root.info.id), "归档测试分支", "file")?;
    let branch_child = store.create_page(&workspace, Some(&branch.info.id), "归档测试子页", "file")?;

    let block_id = |index| format!("c1000000-0000-4000-8000-{index:012x}");
    let sample = |index, payload| Block { id: block_id(index), kind: "paragraph".into(),
        schema_version: 1, payload, children: vec![], extra: Default::default() };
    let image = store.add_image(&workspace, &note.info.id, "示意图.png", &b"\x89PNG\r\n\x1a\nfixture"[..])?.attachment;
    store.save_page(&workspace, &note.info.id, 1, PageDraft {
        title: note.info.title,
        blocks: vec![
            sample(1, json!({"inline":[{"type":"pageReference","pageId":hidden.info.id,"label":"保密候选命中与标签"}]})),
            sample(2, json!({"inline":[
                {"type":"text","text":"可见候选命中在这里"},
                {"type":"pageReference","pageId":hidden.info.id,"label":"缺少权限的标签"},
                {"type":"pageReference","pageId":root.info.id,"label":"可读目录"},
            ]})),
            Block {
                id: block_id(3), kind: "attachmentImage".into(), schema_version: 1,
                payload: json!({"alt":"保密图注","attachmentId":image.id,"mediaType":"image/png"}),
                children: vec![], extra: Default::default(),
            },
            sample(4, json!({"inline":[{"type":"text","text":"保留第二页 Markdown"}]})),
        ],
    })?;
    let (collection, container) = store.create_collection(&workspace, Some(&root.info.id), "采样数据库")?;
    let mut schema = store.collection(&workspace, &collection.id)?;
    for (name, kind) in [("评分", PropertyKind::Number), ("原件", PropertyKind::Attachment), ("来源", PropertyKind::Text)] {
        schema = store.save_property(&workspace, &collection.id, schema.schema_revision, PropertyDraft {
            id: None, name: name.into(), kind, options: vec![],
        })?;
    }
    let score = &schema.properties[1].id;
    let asset_field = &schema.properties[2].id;
    let log = &schema.properties[3].id;
    let high = store.create_page(&workspace, Some(&container.info.id), "采样记录甲", "file")?;
    let low = store.create_page(&workspace, Some(&container.info.id), "采样记录乙", "file")?;
    let record_file = store.add_attachment(&workspace, &high.info.id, "采样原件.txt", &b"authorized record bytes"[..])?.attachment;
    store.save_record(&workspace, &collection.id, &high.info.id, 1, RecordDraft {
        title: high.info.title, schema_revision: schema.schema_revision,
        properties: BTreeMap::from([(score.clone(), json!(9)), (asset_field.clone(), json!([record_file.id])), (log.clone(), json!("源记录"))]),
    })?;
    store.save_record(&workspace, &collection.id, &low.info.id, 1, RecordDraft {
        title: low.info.title, schema_revision: schema.schema_revision,
        properties: BTreeMap::from([(score.clone(), json!(2))]),
    })?;
    let issue = |name: &str, scopes: &[IntegrationScope], grant: IntegrationGrant| {
        store.issue_integration(&workspace, IntegrationDraft {
            name: name.into(), scopes: scopes.iter().copied().collect(), grants: vec![grant],
        })
    };
    let whole = || IntegrationGrant::Subtree(root.info.id.clone());
    let only_note = || IntegrationGrant::Page(note.info.id.clone());
    let reader = issue("原生只读查询", &[Read], whole())?;
    let writer = issue("原生写入查询", &[Read, Update, Create, Archive, ReadAssets, WriteAssets], whole())?;
    let partial = issue("原生单页归档", &[Read, Archive], IntegrationGrant::Page(branch.info.id.clone()))?;
    let uploader = issue("原生只上传", &[Read, Update, WriteAssets], only_note())?;
    let rate = issue("原生独立限流", &[Read], only_note())?;
    let single = issue("原生范围外库", &[Read], only_note())?;
    while store.advance_search_index(&workspace)?.pending != 0 {}
    Ok(json!({
        "workspace":workspace,"other":other,"root":root.info.id,"note":note.info.id,
        "second":second.info.id,"third":third.info.id,"hidden":hidden.info.id,"first_block":block_id(1),
        "visible_block":block_id(2),"image":image.id,"last_block":block_id(4),"archive":branch.info.id,
        "archive_child":branch_child.info.id,"collection":collection.id,"container":container.info.id,
        "score":score,"asset_field":asset_field,"log":log,"high": high.info.id,"low":low.info.id,
        "reader":reader.token,"writer":writer.token,"partial":partial.token,"uploader":uploader.token,
        "rate":rate.token,"single":single.token,
    }))
}

pub fn seed_feed_release(store: &Store) -> Result<Value> {
    use IntegrationScope::{Archive, Changes, Create, Propose, Read, ReadAssets, Update, WriteAssets};

    let workspace = store.create_workspace("KB019 原生验收")?.id;
    let root = store.create_page(&workspace, None, "变更授权分支", "file")?;
    let note = store.create_page(&workspace, Some(&root.info.id), "建议审阅页", "file")?;
    let archived = store.create_page(&workspace, Some(&root.info.id), "待归档样本", "file")?;
    let hidden = store.create_page(&workspace, None, "不可见私有分支", "file")?;
    let (collection, _) = store.create_collection(&workspace, Some(&root.info.id), "测试数据库")?;
    let issue = |name: &str, scopes: &[IntegrationScope], grant: IntegrationGrant| {
        store.issue_integration(&workspace, IntegrationDraft {
            name: name.into(), scopes: scopes.iter().copied().collect(), grants: vec![grant],
        })
    };
    let grant = || IntegrationGrant::Subtree(root.info.id.clone());
    let writer = issue("变更脚本", &[Read, Changes, Archive, Create, Update, ReadAssets, WriteAssets], grant())?;
    let proposer = issue("建议脚本", &[Read, Changes, Propose], grant())?;
    let outsider = issue("私有分支脚本", &[Read, Changes, Update, Propose], IntegrationGrant::Page(hidden.info.id.clone()))?;
    Ok(json!({
        "workspace":workspace, "root":root.info.id, "note":note.info.id, "archive":archived.info.id,
        "hidden":hidden.info.id, "collection":collection.id,
        "writer":writer.token, "proposer":proposer.token, "outsider":outsider.token,
        "integration_ids":[writer.integration.id, proposer.integration.id, outsider.integration.id],
    }))
}

pub fn seed_mcp_release(store: &Store) -> Result<Value> {
    let mut fixture = seed_feed_release(store)?;
    let space = fixture["workspace"].as_str().unwrap();
    let root = fixture["root"].as_str().unwrap();
    let note = fixture["note"].as_str().unwrap();
    let hidden = fixture["hidden"].as_str().unwrap();
    let reader = store.issue_integration(space, IntegrationDraft {
        name: "独立只读客户端".into(),
        scopes: [IntegrationScope::Read].into(),
        grants: vec![IntegrationGrant::Subtree(root.into())],
    })?;
    let block = |index: u32, text: &str| Block {
        id: format!("ae170000-0000-4000-8000-{index:012x}"),
        kind: "paragraph".into(),
        schema_version: 1,
        payload: json!({"inline":[{"type":"text","text":text}]}),
        children: vec![],
        extra: Default::default(),
    };
    store.save_page(space, note, 1, PageDraft {
        title: "MCP 审阅页".into(),
        blocks: vec![block(1, "独立检索关键词：蜉蝣观测"), block(2, "第二段可分页正文")],
    })?;
    store.save_page(space, hidden, 1, PageDraft {
        title: "私有测试记录".into(),
        blocks: vec![block(3, "独立检索关键词：蜉蝣观测，隐藏内容")],
    })?;
    let asset = store.add_attachment(space, hidden, "不可读取的原件.txt", &b"private fixture"[..])?.attachment;
    while store.advance_search_index(space)?.pending != 0 {}
    fixture["reader"] = json!(reader.token);
    fixture["reader_id"] = json!(reader.integration.id);
    fixture["private_asset"] = json!(asset.id);
    fixture["integration_ids"].as_array_mut().unwrap().push(json!(reader.integration.id));
    Ok(fixture)
}

pub fn seed_location_release(store: &Store) -> Result<Value> {
    use zhiye_core::PageLocation;

    let mut fixture = seed_mcp_release(store)?;
    let workspace = fixture["workspace"].as_str().unwrap();
    let note = fixture["note"].as_str().unwrap();
    let archive = fixture["archive"].as_str().unwrap();
    let origin = store.create_workspace("00 定位切换验收")?;
    let start = store.create_page(&origin.id, None, "另一空间起始页", "file")?;
    let page = store.page(workspace, note)?;
    let target = PageLocation::page(workspace, note);
    let cold_link = target.block(&page.blocks[1].id).to_string();
    let warm_page = PageLocation::page(&origin.id, &start.info.id).to_string();
    let archived_link = PageLocation::page(workspace, archive).to_string();
    let missing_link = target.block("ae170000-0000-4000-8000-000000000099").to_string();
    store.archive_page(workspace, archive, 1)?;
    fixture["origin"] = json!({"workspace": origin.id, "page": start.info.id});
    fixture["cold_link"] = json!(cold_link);
    fixture["warm_link"] = json!(warm_page);
    fixture["archived_link"] = json!(archived_link);
    fixture["missing_link"] = json!(missing_link);
    Ok(fixture)
}
