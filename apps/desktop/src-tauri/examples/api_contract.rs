// SPDX-License-Identifier: AGPL-3.0-or-later

#[path = "support/api_fixture.rs"]
mod fixture;
#[path = "../src/local_api.rs"]
mod local_api;

use local_api::LocalApi;
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, BufRead, Write},
    path::PathBuf,
    sync::Arc,
};
use zhiye_core::{
    Error, IntegrationDraft, IntegrationGrant, IntegrationScope, PageDraft, ProposalAction, Result,
    Store,
};

fn emit(body: &Value) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{body}")?;
    stdout.flush()
}

fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value[field]
        .as_str()
        .ok_or_else(|| Error::new("FIXTURE_ARGUMENT", format!("Missing {field}")))
}

fn control(
    request: &Value,
    fixture: &Value,
    store: &Arc<Store>,
    server: &mut Option<LocalApi>,
) -> Result<Value> {
    let workspace = text(fixture, "workspace")?;
    match text(request, "op")? {
        "stop" => {
            drop(server.take());
        }
        "start" => {
            if server.is_none() {
                *server = Some(LocalApi::start(Arc::clone(store), |_| {})?);
            }
            return Ok(json!(LocalApi::status(server.as_ref()).address));
        }
        "revoke" => {
            let token = &fixture["tokens"][text(request, "name")?];
            store.revoke_integration(workspace, text(token, "id")?)?;
        }
        "issueExpiring" => {
            let expires_at = request["expiresAt"]
                .as_i64()
                .ok_or_else(|| Error::new("FIXTURE_ARGUMENT", "Missing expiresAt"))?;
            let issued = store.issue_integration_with_expiry(
                workspace,
                IntegrationDraft {
                    name: "短期接口验收".into(),
                    scopes: [IntegrationScope::Read].into_iter().collect(),
                    grants: vec![IntegrationGrant::Page(text(fixture, "note")?.into())],
                },
                Some(expires_at),
            )?;
            return Ok(json!({"token": issued.token, "expiresAt": issued.integration.expires_at}));
        }
        "edit" => {
            let page = store.page(workspace, text(fixture, "note")?)?;
            store.save_page(
                workspace,
                &page.info.id,
                page.info.revision,
                PageDraft {
                    title: "桌面同步的新修订".into(),
                    blocks: page.blocks,
                },
            )?;
        }
        "moveOutside" => {
            let page = store.page(workspace, text(fixture, "movable")?)?;
            store.move_page(
                workspace,
                &page.info.id,
                page.info.revision,
                Some(text(fixture, "hidden")?),
            )?;
        }
        "accept" => {
            let id = text(request, "proposal")?;
            let review = store.proposal_review(workspace, id)?;
            return Ok(json!(
                store
                    .review_proposal(
                        workspace,
                        id,
                        review.summary.review_revision,
                        ProposalAction::Accept
                    )?
                    .summary
            ));
        }
        "limitOneMiB" => store.set_api_attachment_limit_mib(workspace, 1)?,
        _ => return Err(Error::new("FIXTURE_COMMAND", "Unknown API fixture command")),
    }
    Ok(Value::Null)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    if let Some(mode) = args.next() {
        if mode != "--seed" {
            return Err("Usage: api_contract [--seed <empty data directory>]".into());
        }
        let root = PathBuf::from(args.next().ok_or("Missing synthetic data directory")?);
        if root.exists() && fs::read_dir(&root)?.next().is_some() {
            return Err("API contract seed requires an empty data directory".into());
        }
        emit(&fixture::seed(&Store::open(root)?)?)?;
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let store = Arc::new(Store::open(root.path())?);
    let fixture = fixture::seed(&store)?;
    let mut server = Some(LocalApi::start(Arc::clone(&store), |_| {})?);
    emit(&json!({"origin":LocalApi::status(server.as_ref()).address,"fixture":fixture}))?;
    let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines().map_while(io::Result::ok) {
            if send.send(line).is_err() {
                break;
            }
        }
    });
    while let Some(line) = receive.recv().await {
        let request: Value = serde_json::from_str(&line)?;
        let id = &request["id"];
        let response = match control(&request, &fixture, &store, &mut server) {
            Ok(value) => json!({"id":id,"result":value}),
            Err(error) => json!({"id":id,"error":error}),
        };
        emit(&response)?;
    }
    Ok(())
}
