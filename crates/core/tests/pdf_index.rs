// SPDX-License-Identifier: AGPL-3.0-or-later

mod support;

use rusqlite::Connection;
use std::io::Cursor;
use zhiye_core::{
    Attachment, IntegrationDraft, IntegrationGrant, IntegrationScope, IntegrationSearchRequest,
    IntegrationSearchSource, PageLocation, PdfExtractionStatus, PdfIndexResult, PdfPageText,
    SearchRequest, Store,
};

fn attach(store: &Store, workspace: &str, page: &str, name: &str) -> Attachment {
    let fixture = format!("%PDF-1.7\n{page}/{name}");
    store
        .add_attachment(workspace, page, name, fixture.as_bytes())
        .unwrap()
        .attachment
}

fn extracted(file: &Attachment, generation: i64, texts: &[(u32, &str)]) -> PdfIndexResult {
    PdfIndexResult {
        generation,
        attachment_id: file.id.clone(),
        sha256: file.sha256.clone(),
        status: if texts.is_empty() {
            PdfExtractionStatus::NoText
        } else {
            PdfExtractionStatus::Indexed
        },
        page_count: Some(3),
        pages: texts
            .iter()
            .map(|(number, text)| PdfPageText {
                page_number: *number,
                text: (*text).into(),
            })
            .collect(),
    }
}

#[test]
fn text_and_pdf_results_keep_their_order_across_page_boundaries() {
    use serde_json::json;
    use zhiye_core::{Block, PageDraft};

    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let workspace = store.create_workspace("跨页搜索").unwrap().id;
    for number in 0..21 {
        store
            .create_page(&workspace, None, &format!("海棠候选 {number:02}"), "file")
            .unwrap();
    }
    let page = store
        .create_page(&workspace, None, "正文和原件", "file")
        .unwrap();
    let paragraph = |text: &str| Block {
        id: uuid::Uuid::new_v4().to_string(),
        kind: "paragraph".into(),
        schema_version: 1,
        payload: json!({"inline": [{"type": "text", "text": text}]}),
        children: vec![],
        extra: Default::default(),
    };
    let first_block = paragraph("海棠候选出现在首个段落");
    let first_id = first_block.id.clone();
    store
        .save_page(
            &workspace,
            &page.info.id,
            page.info.revision,
            PageDraft {
                title: page.info.title,
                blocks: vec![first_block, paragraph("海棠候选也出现在下一个段落")],
            },
        )
        .unwrap();
    let file = attach(&store, &workspace, &page.info.id, "合成.pdf");
    let job = store.poll_pdf_index(&workspace).unwrap().job.unwrap();
    store
        .finish_pdf_index(
            &workspace,
            extracted(&file, job.generation, &[(1, "海棠候选"), (2, "海棠候选")]),
        )
        .unwrap();
    while store.advance_search_index(&workspace).unwrap().pending > 0 {}

    for term in ["海棠", "海棠候选"] {
        let search = |offset, page_only| {
            store
                .search_text(
                    &workspace,
                    &SearchRequest {
                        query: term.into(),
                        offset,
                        page_only,
                        ..Default::default()
                    },
                )
                .unwrap()
        };
        let first = search(0, false);
        assert_eq!(first.hits.len(), 20);
        assert_eq!(first.next_offset, Some(20));
        assert!(
            first
                .hits
                .iter()
                .all(|hit| hit.block_id.is_none() && hit.pdf.is_none())
        );
        let next = search(20, false);
        assert_eq!(next.hits.len(), 4);
        assert_eq!(next.next_offset, None);
        assert!(next.hits[0].page.title.contains(term));
        assert_eq!(next.hits[1].block_id.as_deref(), Some(first_id.as_str()));
        assert_eq!(
            next.hits[2..]
                .iter()
                .map(|hit| {
                    let target = &hit.pdf.as_ref().unwrap().target;
                    assert_eq!(target.attachment_id, file.id);
                    target.page_number
                })
                .collect::<Vec<_>>(),
            [1, 2]
        );
        let last = search(23, false);
        assert_eq!(last.hits.len(), 1);
        assert_eq!(last.hits[0].pdf.as_ref().unwrap().target.page_number, 2);
        assert!(search(22, true).hits.is_empty());
    }
}

#[test]
fn imports_and_rebuilds_pdf_sources_without_losing_identity_or_revisions() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path()).unwrap();
    let workspace = store.create_workspace("PDF 索引").unwrap().id;
    let page = store
        .create_page(&workspace, None, "PDF 提取验收", "file")
        .unwrap()
        .info;
    let file = attach(&store, &workspace, &page.id, "原件.pdf");
    let database = folder.path().join(&workspace).join("workspace.sqlite");
    support::remove_v14_schema(&Connection::open(&database).unwrap());
    let poll = store.poll_pdf_index(&workspace).unwrap();
    assert_eq!(poll.progress.pending, 1);
    let job = poll.job.unwrap();
    assert_eq!(job.attachment, file);
    let snapshot = folder
        .path()
        .join("upgrade-backups")
        .join(format!("{workspace}-v13.sqlite"));
    assert_eq!(
        Connection::open(snapshot)
            .unwrap()
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        13
    );

    let search = |workspace: &str, query: &str| {
        store
            .search_text(
                workspace,
                &SearchRequest {
                    query: query.into(),
                    ..Default::default()
                },
            )
            .unwrap()
    };
    let texts = [(1, "核心 samples"), (2, "赤铜配比 核心 samples")];
    let rejected = store.finish_pdf_index(
        &workspace,
        extracted(&file, job.generation, &[(0, "坏结果")]),
    );
    assert_eq!(rejected.unwrap_err().code, "PDF_INDEX_RESULT");
    assert_eq!(
        store.poll_pdf_index(&workspace).unwrap().progress.pending,
        1
    );
    store.rebuild_search_index(&workspace).unwrap();
    let stale = store
        .finish_pdf_index(&workspace, extracted(&file, job.generation, &texts))
        .unwrap();
    assert_eq!(stale.pending, 1);
    assert!(search(&workspace, "赤铜配比").hits.is_empty());
    let latest = store.poll_pdf_index(&workspace).unwrap().job.unwrap();
    assert_ne!(latest.generation, job.generation);
    let done = store
        .finish_pdf_index(&workspace, extracted(&file, latest.generation, &texts))
        .unwrap();
    assert_eq!((done.pending, done.indexed), (0, 1));
    assert_eq!(
        store
            .finish_pdf_index(&workspace, extracted(&file, latest.generation, &texts))
            .unwrap(),
        done
    );
    assert_eq!(
        store.page(&workspace, &page.id).unwrap().info.revision,
        page.revision
    );
    let hits = search(&workspace, "核心").hits;
    assert_eq!(
        hits.iter()
            .map(|hit| hit.pdf.as_ref().unwrap().target.page_number)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    let from = PageLocation::page(&workspace, &page.id);
    let target = from.pdf(&file.id, &file.sha256, 2);
    let location = target.to_string();
    assert_eq!(PageLocation::parse(&location).unwrap(), target);
    assert!(PageLocation::parse(&location.replace("/2", "/02")).is_err());
    assert!(
        store
            .resolve_location(&from.pdf(&file.id, &"a".repeat(64), 2).to_string())
            .unwrap()
            .missing_pdf
    );

    store
        .rename_attachment(&workspace, &page.id, &file.id, "新名称.pdf")
        .unwrap();
    let matches = search(&workspace, "赤铜配比");
    let item = matches.hits[0].pdf.as_ref().unwrap();
    assert_eq!(
        (item.target.sha256.as_str(), item.name.as_str()),
        (file.sha256.as_str(), "新名称.pdf")
    );
    assert_eq!(
        store.resolve_location(&location).unwrap().pdf.unwrap(),
        item.target
    );

    let no_text = attach(&store, &workspace, &page.id, "图像 PDF.pdf");
    let scan_job = store.poll_pdf_index(&workspace).unwrap().job.unwrap();
    store
        .finish_pdf_index(&workspace, extracted(&no_text, scan_job.generation, &[]))
        .unwrap();
    assert!(
        store
            .pdf_document_status(&workspace, &page.id)
            .unwrap()
            .iter()
            .any(|status| status.attachment_id == no_text.id
                && status.status.as_deref() == Some("noText"))
    );

    store
        .archive_page(&workspace, &page.id, page.revision)
        .unwrap();
    assert!(search(&workspace, "赤铜配比").hits.is_empty());
    assert!(store.resolve_location(&location).is_err());
    let archived = store.page(&workspace, &page.id).unwrap();
    store
        .restore_page(&workspace, &page.id, archived.info.revision)
        .unwrap();
    assert_eq!(search(&workspace, "赤铜配比").hits.len(), 1);

    let mut archive = Cursor::new(Vec::new());
    store
        .write_workspace_archive(&workspace, &mut archive)
        .unwrap();
    let restored = store.read_workspace_archive(archive).unwrap();
    assert_eq!(
        store.poll_pdf_index(&restored.id).unwrap().progress.pending,
        2
    );
    assert!(search(&restored.id, "赤铜配比").hits.is_empty());
    let restored_link = PageLocation::page(&restored.id, &page.id)
        .pdf(&file.id, &file.sha256, 2)
        .to_string();
    assert_eq!(
        store.resolve_location(&restored_link).unwrap().pdf.unwrap(),
        item.target
    );
    let mut bytes = Vec::new();
    store
        .copy_attachment(&restored.id, &page.id, &file.id, &mut bytes)
        .unwrap();
    let mut source = Vec::new();
    store
        .copy_attachment(&workspace, &page.id, &file.id, &mut source)
        .unwrap();
    assert_eq!(bytes, source);
}

#[test]
fn pdf_search_respects_attachment_scope_and_visible_cursor_state() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path()).unwrap();
    let workspace = store.create_workspace("PDF 授权").unwrap().id;
    let page = store
        .create_page(&workspace, None, "原件来源", "file")
        .unwrap()
        .info;
    let hidden = store
        .create_page(&workspace, None, "内部资料", "file")
        .unwrap()
        .info;
    let visible_file = attach(&store, &workspace, &page.id, "授权.pdf");
    let hidden_file = attach(&store, &workspace, &hidden.id, "隐藏.pdf");
    let next_file = attach(&store, &workspace, &page.id, "稍后.pdf");
    let issue = |name: String, assets: bool| {
        store
            .issue_integration(
                &workspace,
                IntegrationDraft {
                    name: name,
                    grants: vec![IntegrationGrant::Page(page.id.clone())],
                    scopes: [IntegrationScope::Read]
                        .into_iter()
                        .chain(assets.then_some(IntegrationScope::ReadAssets))
                        .collect(),
                },
            )
            .unwrap()
            .token
    };
    let reader = issue("附件读取".into(), true);
    let limited = issue("仅页面读取".into(), false);
    let query = IntegrationSearchRequest {
        query: "Revision Horizon".into(),
        limit: Some(1),
        ..Default::default()
    };
    let generation = store
        .poll_pdf_index(&workspace)
        .unwrap()
        .job
        .unwrap()
        .generation;
    store
        .finish_pdf_index(
            &workspace,
            extracted(
                &visible_file,
                generation,
                &[(1, "Revision horizon"), (3, "Revision horizon")],
            ),
        )
        .unwrap();
    let first = store
        .integration_search(&workspace, &reader, &query)
        .unwrap();
    assert_eq!(first.pdf_index.unwrap().pending, 1);
    assert!(matches!(
        first.items[0].source,
        IntegrationSearchSource::Pdf
    ));
    assert_eq!(first.items[0].pdf.as_ref().unwrap().target.page_number, 1);
    let private = store
        .integration_search(&workspace, &limited, &query)
        .unwrap();
    assert!(private.items.is_empty() && private.pdf_index.is_none());
    store
        .finish_pdf_index(
            &workspace,
            extracted(&hidden_file, generation, &[(2, "Revision horizon hidden")]),
        )
        .unwrap();
    let resume = IntegrationSearchRequest {
        cursor: first.next_cursor,
        query: query.query.clone(),
        limit: query.limit,
        ..Default::default()
    };
    let second = store
        .integration_search(&workspace, &reader, &resume)
        .unwrap();
    assert_eq!(second.items[0].pdf.as_ref().unwrap().target.page_number, 3);
    assert_eq!(second.snapshot_revision, first.snapshot_revision);
    assert!(second.next_cursor.is_none());
    assert_eq!(second.pdf_index, first.pdf_index);

    store
        .finish_pdf_index(
            &workspace,
            extracted(&next_file, generation, &[(2, "Revision horizon extra")]),
        )
        .unwrap();
    assert_eq!(
        store
            .integration_search(&workspace, &reader, &resume)
            .unwrap_err()
            .code,
        "CURSOR_INVALID"
    );
    let current = store
        .integration_search(&workspace, &reader, &query)
        .unwrap();
    assert_eq!(current.pdf_index.unwrap().indexed, 2);
    assert!(current.items[0].pdf.as_ref().unwrap().target.attachment_id != hidden_file.id);
}
