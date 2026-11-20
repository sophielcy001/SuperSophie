// SPDX-License-Identifier: AGPL-3.0-or-later

use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use zhiye_core::{
    DesktopPageList, Error, PageDraft, Result, Store, ViewRange, decode_desktop_blocks,
    encode_desktop_blocks,
};

fn field<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args[key].as_str().ok_or_else(|| {
        Error::new(
            "PERFORMANCE_INPUT",
            format!("Missing fixture argument {key}"),
        )
    })
}

fn call(store: &Store, command: &str, args: &Value) -> Result<Value> {
    if command == "list_workspaces" {
        return Ok(json!(store.workspace_overview()?));
    }
    if command == "backup_status" {
        return Ok(json!(store.backup_status(args["workspace"].as_str())?));
    }
    let workspace = field(args, "workspace")?;
    let id = || field(args, "id");
    let collection = || field(args, "collection");
    Ok(match command {
        "list_pages" => json!(DesktopPageList::new(
            store.pages(workspace)?,
            args["compact"].as_bool().unwrap_or_default(),
        )),
        "list_collections" => json!(store.collections(workspace)?),
        "read_navigation" => json!(store.navigation(workspace)?),
        "visit_page" => json!(store.visit_page(workspace, field(args, "page")?)?),
        "read_page" => {
            let mut page = store.project_desktop_page(workspace, store.page(workspace, id()?)?)?;
            page.blocks = encode_desktop_blocks(page.blocks)?;
            json!(page)
        }
        "save_page" => {
            let mut draft: PageDraft = serde_json::from_value(args["draft"].clone())?;
            draft.blocks = decode_desktop_blocks(draft.blocks)?;
            let revision = serde_json::from_value(args["revision"].clone())?;
            json!(store.save_page(workspace, id()?, revision, draft)?)
        }
        "read_collection" => json!(store.collection(workspace, collection()?)?),
        "list_records" => json!(store.query_view_records(
            workspace,
            collection()?,
            field(args, "view")?,
            serde_json::from_value(args["offset"].clone())?,
            serde_json::from_value::<Option<ViewRange>>(args["range"].clone())?.as_ref(),
        )?),
        "search_text" => {
            json!(store.search_text(workspace, &serde_json::from_value(args["request"].clone())?)?)
        }
        "advance_search_index" => json!(store.advance_search_index(workspace)?),
        "rebuild_search_index" => json!(store.rebuild_search_index(workspace)?),
        "poll_pdf_index" => json!(store.poll_pdf_index(workspace)?),
        "list_integrations" => json!(store.integrations(workspace)?),
        "create_integration" => json!(
            store.issue_integration(workspace, serde_json::from_value(args["draft"].clone())?)?
        ),
        "revoke_integration" => {
            store.revoke_integration(workspace, id()?)?;
            Value::Null
        }
        _ => {
            return Err(Error::new(
                "PERFORMANCE_COMMAND",
                format!("Unrecognized fixture command {command}"),
            ));
        }
    })
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os()
        .nth(1)
        .ok_or("usage: m5_adapter <generated fixture directory>")?;
    let store = Store::open(root)?;
    let overview = store.workspace_overview()?;
    if overview.workspaces.len() != 1 || !overview.workspaces[0].name.starts_with("知页 M5 ") {
        return Err("Expected one generated M5 workspace".into());
    }
    let mut output = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let request: Value = serde_json::from_str(&line?)?;
        let response = match call(&store, field(&request, "cmd")?, &request["args"]) {
            Ok(result) => json!({"id": request["id"], "result": result}),
            Err(error) => json!({"id": request["id"], "error": error}),
        };
        writeln!(output, "{response}")?;
        output.flush()?;
    }
    Ok(())
}
