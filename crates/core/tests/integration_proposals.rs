// SPDX-License-Identifier: AGPL-3.0-or-later

use rusqlite::Connection;
use serde_json::{Value, json};
use zhiye_core::{
    Block, IntegrationDraft, IntegrationGrant as Grant, IntegrationMutation,
    IntegrationScope as Scope, IssuedIntegration, PageDraft, PagePatch, ProposalAction,
    ProposalDraft, ProposalStatus, Store,
};

fn setup() -> (tempfile::TempDir, Store, String) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("建议验收").unwrap().id;
    (root, store, space)
}

fn issue(store: &Store, space: &str, id: &str, grants: &[Scope]) -> IssuedIntegration {
    store
        .issue_integration(
            space,
            IntegrationDraft {
                name: "资料整理助手".into(),
                scopes: std::iter::once(Scope::Read)
                    .chain(grants.iter().copied())
                    .collect(),
                grants: vec![Grant::Page(id.into())],
            },
        )
        .unwrap()
}

fn draft(id: &str, revision: u32, changes: Value) -> ProposalDraft {
    serde_json::from_value(json!({"page_id": id, "base_revision": revision, "changes": changes}))
        .unwrap()
}

#[test]
fn stale_review_cannot_overwrite_and_rebase_needs_a_second_review() {
    let (_root, store, space) = setup();
    let note = store.create_page(&space, None, "最初标题", "file").unwrap();
    let page = &note.info.id;
    let integration = issue(&store, &space, page, &[Scope::Propose, Scope::Changes]);
    let token = &integration.token;
    let change_cursor = store
        .integration_catalog(&space, token, None, None)
        .unwrap()
        .change_cursor
        .unwrap();
    let proposal = || {
        draft(
            page,
            1,
            json!({
                "title": "建议标题",
                "operations": [{"op":"append","block":{"type":"paragraph","payload":{"text":"建议正文"}}}]
            }),
        )
    };
    let receipt = store
        .submit_integration_proposal(&space, token, "request", proposal())
        .unwrap();
    assert_eq!(
        store
            .submit_integration_proposal(&space, token, "request", proposal())
            .unwrap(),
        receipt
    );
    assert_eq!(
        store
            .submit_integration_proposal(
                &space,
                token,
                "request",
                draft(page, 1, json!({"title": "不同建议"}))
            )
            .unwrap_err()
            .code,
        "IDEMPOTENCY_CONFLICT"
    );
    let other = issue(&store, &space, page, &[Scope::Propose]);
    assert_eq!(
        store
            .integration_proposal(&space, &other.token, &receipt.id)
            .unwrap_err()
            .code,
        "RESOURCE_NOT_FOUND"
    );
    let projected = store.integration_page(&space, token, page).unwrap();
    assert_eq!(
        store
            .mutate_as_integration(
                &space,
                token,
                "direct",
                Some(&projected.etag),
                IntegrationMutation::PatchPage {
                    page_id: page.clone(),
                    patch: PagePatch {
                        title: Some("直接改正文".into()),
                        icon: None
                    },
                }
            )
            .unwrap_err()
            .code,
        "FORBIDDEN"
    );
    assert_eq!(store.page(&space, page).unwrap(), note);
    assert!(
        store
            .integration_changes(&space, token, None, &change_cursor)
            .unwrap()
            .items
            .is_empty()
    );
    let preview = store.proposal_review(&space, &receipt.id).unwrap();
    assert_eq!(preview.base.title, "最初标题");
    assert_eq!(preview.candidate.title, "建议标题");
    assert!(preview.candidate.markdown.contains("建议正文"));

    let saved = store
        .save_page(
            &space,
            page,
            1,
            PageDraft {
                title: "用户新标题".into(),
                blocks: vec![],
            },
        )
        .unwrap();
    let old = preview.summary.review_revision;
    let conflict = store
        .review_proposal(&space, &receipt.id, old, ProposalAction::Accept)
        .unwrap();
    assert_eq!(conflict.summary.status, ProposalStatus::Conflict);
    assert_eq!(
        conflict.summary.conflict.as_deref(),
        Some("revision_changed")
    );
    assert_eq!(conflict.current.unwrap().title, "用户新标题");
    assert_eq!(store.page(&space, page).unwrap().info, saved);
    assert_eq!(
        store
            .integration_proposal(&space, token, &receipt.id)
            .unwrap()
            .status,
        ProposalStatus::Conflict
    );
    assert_eq!(
        store
            .submit_integration_proposal(&space, token, "request", proposal())
            .unwrap(),
        receipt
    );

    let fresh = store
        .review_proposal(&space, &receipt.id, old, ProposalAction::Rebase)
        .unwrap();
    assert_eq!(fresh.summary.base_revision, 2);
    assert_eq!(fresh.summary.review_revision, old + 1);
    assert_eq!(fresh.base.title, "用户新标题");
    assert_eq!(
        store
            .review_proposal(&space, &receipt.id, old, ProposalAction::Accept)
            .unwrap_err()
            .code,
        "PROPOSAL_REVIEW_CHANGED"
    );
    let accepted = store
        .review_proposal(
            &space,
            &receipt.id,
            fresh.summary.review_revision,
            ProposalAction::Accept,
        )
        .unwrap();
    assert_eq!(accepted.summary.status, ProposalStatus::Accepted);
    assert_eq!(accepted.summary.accepted_revision, Some(3));
    let page_after = store.page(&space, page).unwrap();
    assert_eq!(page_after.info.title, "建议标题");
    assert_eq!(page_after.blocks, fresh.candidate.blocks);
    let attributions = store.revision_attributions(&space, page).unwrap();
    assert_eq!(attributions[&3].integration_id, integration.integration.id);
    assert_eq!(attributions[&3].operation_id, receipt.id);
    assert_eq!(
        store
            .integration_proposal(&space, token, &receipt.id)
            .unwrap()
            .accepted_revision,
        Some(3)
    );
    let changes = store
        .integration_changes(&space, token, None, &change_cursor)
        .unwrap();
    assert_eq!(
        changes
            .items
            .iter()
            .map(|event| event.revision)
            .collect::<Vec<_>>(),
        [2, 3]
    );
}

#[test]
fn restricted_blocks_revoke_reject_and_expiry_never_replace_content() {
    let (root, store, space) = setup();
    let note = store.create_page(&space, None, "公开笔记", "file").unwrap();
    let private = store.create_page(&space, None, "私有笔记", "file").unwrap();
    let id = &note.info.id;
    let block = Block {
        id: uuid::Uuid::new_v4().to_string(),
        kind: "paragraph".into(),
        schema_version: 1,
        payload: json!({"inline": [{"type": "pageReference", "pageId": private.info.id, "label": "私有标题"}]}),
        children: vec![],
        extra: Default::default(),
    };
    store
        .save_page(
            &space,
            id,
            1,
            PageDraft {
                title: "公开笔记".into(),
                blocks: vec![block.clone()],
            },
        )
        .unwrap();
    let proposer = issue(&store, &space, id, &[Scope::Propose]);
    let reader = issue(&store, &space, id, &[]);
    let proposed = || draft(id, 2, json!({"title": "候选标题"}));
    assert_eq!(
        store
            .submit_integration_proposal(&space, &reader.token, "read-only", proposed())
            .unwrap_err()
            .code,
        "FORBIDDEN"
    );
    assert_eq!(
        store
            .submit_integration_proposal(
                &space,
                &proposer.token,
                "hidden",
                draft(
                    id,
                    2,
                    json!({"operations":[{"op":"delete","block_id":block.id}]})
                )
            )
            .unwrap_err()
            .code,
        "PARTIAL_CONTENT"
    );
    let submit = |key| {
        store
            .submit_integration_proposal(&space, &proposer.token, key, proposed())
            .unwrap()
    };
    let rejected = submit("reject");
    let expired = submit("expire");
    let revoked = submit("revoke");
    let preview = store.proposal_review(&space, &revoked.id).unwrap();
    assert_eq!(preview.candidate.blocks, vec![block]);
    let exposed = serde_json::to_value(
        store
            .integration_proposal(&space, &proposer.token, &revoked.id)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(exposed.as_object().unwrap().len(), 7);
    assert!(!exposed.to_string().contains(&private.info.id));
    assert_eq!(
        store
            .review_proposal(&space, &rejected.id, 1, ProposalAction::Reject)
            .unwrap()
            .summary
            .status,
        ProposalStatus::Rejected
    );
    let db = Connection::open(root.path().join(&space).join("workspace.sqlite")).unwrap();
    db.execute(
        "UPDATE integration_proposals SET expires_at = created_at WHERE id = ?1",
        [&expired.id],
    )
    .unwrap();
    assert_eq!(
        store
            .review_proposal(&space, &expired.id, 1, ProposalAction::Accept)
            .unwrap()
            .summary
            .status,
        ProposalStatus::Expired
    );
    store
        .revoke_integration(&space, &proposer.integration.id)
        .unwrap();
    let inaccessible = store
        .review_proposal(&space, &revoked.id, 1, ProposalAction::Accept)
        .unwrap();
    assert_eq!(
        inaccessible.summary.conflict.as_deref(),
        Some("authorization_changed")
    );
    assert_eq!(
        store
            .integration_proposal(&space, &proposer.token, &revoked.id)
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );
    assert_eq!(store.page(&space, id).unwrap().info.revision, 2);
    assert_eq!(store.page(&space, id).unwrap().blocks, preview.base.blocks);
}
