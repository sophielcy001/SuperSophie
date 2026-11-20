use serde_json::{Value, json};
use std::fs::File;
use std::io::{BufRead, Write};
use std::path::Path;
use zhiye_core::{
    DesktopPageList, ImportFile, Page, PageVersion, Result, Store, decode_desktop_blocks, encode_desktop_blocks,
};

mod scale;
mod integrations;

fn fixture(name: &str, bytes: impl AsRef<[u8]>) -> ImportFile {
    ImportFile {
        name: name.into(),
        bytes: bytes.as_ref().to_vec(),
    }
}

fn text<'a>(args: &'a Value, name: &str) -> &'a str {
    args[name].as_str().unwrap()
}

fn wire(mut page: Page) -> Result<Value> {
    page.blocks = encode_desktop_blocks(page.blocks)?;
    Ok(serde_json::to_value(page)?)
}

fn dispatch(
    store: &Store,
    backup: &Path,
    image: Option<&Path>,
    cmd: &str,
    args: &Value,
) -> Result<Value> {
    let space = || text(args, "workspace");
    let collection = || text(args, "collection");
    let id = || text(args, "id");
    let revision = || args["revision"].as_u64().unwrap() as u32;
    let parent = || args["parentId"].as_str();
    let pages = || serde_json::from_value::<Vec<PageVersion>>(args["pages"].clone());
    Ok(match cmd {
        "list_workspaces" => json!(store.workspace_overview()?),
        "create_workspace" => json!(store.create_workspace(text(args, "name"))?),
        "verify_seed_native_integrations" => integrations::seed(store)?,
        "verify_seed_query_release" => integrations::seed_query_release(store)?,
        "verify_seed_feed_release" => integrations::seed_feed_release(store)?,
        "verify_seed_mcp_release" => integrations::seed_mcp_release(store)?,
        "verify_seed_location_release" => integrations::seed_location_release(store)?,
        "verify_seed_thousand_pages" => scale::seed(store)?,
        "verify_restore_thousand_pages" => scale::roundtrip(store, space(), backup)?,
        "list_pages" => json!(DesktopPageList::new(
            store.pages(space())?,
            args["compact"].as_bool().unwrap_or_default(),
        )),
        "list_integrations" => json!(store.integrations(space())?),
        "list_proposals" => json!(store.proposals(space(), args["pageId"].as_str())?),
        "read_proposal" => json!(store.proposal_review(space(), id())?),
        "review_proposal" => json!(store.review_proposal(
            space(), id(), args["reviewRevision"].as_u64().unwrap() as u32,
            serde_json::from_value(args["action"].clone())?,
        )?),
        "verify_submit_proposal" => json!(store.submit_integration_proposal(
            space(), text(args, "token"), text(args, "key"), serde_json::from_value(args["draft"].clone())?,
        )?),
        "api_attachment_limit" => json!(store.api_attachment_limit_mib(space())?),
        "set_api_attachment_limit" => {
            store.set_api_attachment_limit_mib(space(), args["limit"].as_u64().unwrap() as u32)?;
            Value::Null
        }
        "create_integration" => json!(store.issue_integration_with_expiry(
            space(), serde_json::from_value(args["draft"].clone())?,
            serde_json::from_value(args["expiresAt"].clone())?,
        )?),
        "revoke_integration" => { store.revoke_integration(space(), id())?; Value::Null }
        "verify_read_integration" => json!(store.integration_page(space(), text(args, "token"), id())?),
        "verify_derived_catalog" => json!(store.integration_catalog(space(), text(args, "token"), None, None)?),
        "verify_read_computed" => json!(store.integration_computed(space(), text(args, "token"), id())?),
        "verify_advance_derived" => json!(store.advance_derived_changes(space())?),
        "verify_patch_integration" => {
            let token = text(args, "token");
            let etag = store.integration_page(space(), token, id())?.etag;
            json!(store.mutate_as_integration(
                space(), token, "test-change", Some(&etag),
                zhiye_core::IntegrationMutation::PatchPage {
                    page_id: id().into(),
                    patch: zhiye_core::PagePatch { title: Some(text(args, "title").into()), icon: None },
                },
            )?)
        }
        "list_collections" => json!(store.collections(space())?),
        "create_collection" => {
            let (collection, page) = store.create_collection(space(), parent(), "")?;
            json!({"collection": collection, "page": wire(page)?})
        }
        "read_collection" => json!(store.collection(space(), collection())?),
        "create_page" => wire(store.create_page(space(), parent(), "无标题", "file")?)?,
        "read_page" => wire(store.project_desktop_page(space(), store.page(space(), id())?)?)?,
        "read_block_reference" => json!(store.desktop_reference(
            space(), text(args, "page"), text(args, "block"),
            args["knownRevision"].as_u64().map(|revision| revision as u32),
        )?),
        "list_backlinks" => json!(store.backlinks(space(), text(args, "page"), args["after"].as_str())?),
        "make_location" => {
            let target = zhiye_core::PageLocation::page(space(), text(args, "page"));
            if let Some(pdf) = serde_json::from_value::<Option<zhiye_core::PdfTarget>>(args["pdf"].clone())? {
                json!(target.pdf(&pdf.attachment_id, &pdf.sha256, pdf.page_number).to_string())
            } else {
                json!(args["block"].as_str().map_or(target, |id| target.block(id)).to_string())
            }
        }
        "resolve_location" => {
            let location = store.resolve_location(text(args, "url"))?;
            json!({"workspace": location.workspace, "page": wire(location.page)?,
                "blockId": location.block_id, "missingBlock": location.missing_block,
                "pdf": location.pdf, "missingPdf": location.missing_pdf})
        }
        "page_history" => {
            let attributions = store.revision_attributions(space(), id())?;
            let items = store.history(space(), id())?.into_iter().map(|page| {
                let attribution = attributions.get(&page.info.revision);
                let mut serialized = wire(page)?;
                if let Some(attribution) = attribution { serialized["attribution"] = json!(attribution); }
                Ok(serialized)
            }).collect::<Result<Vec<_>>>()?;
            json!(items)
        }
        "set_page_appearance" => wire(store.set_page_appearance(
            space(), id(), revision(), text(args, "icon"), args["coverAttachmentId"].as_str(),
        )?)?,
        "restore_version" => wire(store.restore_version(space(), id(), revision(), args["target"].as_u64().unwrap() as u32)?)?,
        "move_page" => json!(store.move_page(space(), id(), revision(), parent())?),
        "move_pages" => json!(store.move_pages(space(), &pages()?, parent())?),
        "move_pages_with_impact" => json!(store.move_pages_with_impact(space(), &pages()?, parent())?),
        "archive_pages" => json!(store.archive_pages(space(), &pages()?)?),
        "restore_pages" => json!(store.restore_pages(space(), &pages()?)?),
        "preview_empty_trash" => json!(store.preview_empty_trash(space())?),
        "empty_trash" => json!(store.empty_trash(space(), text(args, "confirmation"))?),
        "preview_blob_cleanup" => json!(store.preview_blob_cleanup(space())?),
        "clean_blobs" => json!(store.clean_blobs(space(), text(args, "confirmation"))?),
        "read_navigation" => json!(store.navigation(space())?),
        "visit_page" => json!(store.visit_page(space(), text(args, "page"))?),
        "set_favorite" => json!(store.set_favorite(space(), text(args, "page"), args["favorite"].as_bool().unwrap())?),
        "set_inbox" => json!(store.set_inbox(space(), text(args, "page"))?),
        "enter_inbox" => wire(if args["capture"].as_bool().unwrap() {
            store.capture_page(space(), "无标题")?
        } else {
            store.open_inbox(space())?
        })?,
        "list_templates" => json!(store.templates(space())?),
        "save_template" => json!(store.save_template(space(), text(args, "source"), text(args, "name"), args["templateId"].as_str())?),
        "apply_template" => wire(store.apply_template(space(), id(), parent(), args["title"].as_str())?)?,
        "delete_template" => { store.delete_template(space(), id())?; Value::Null }
        "import_markdown_files" => json!(store.import_markdown(space(), parent(), vec![
            fixture("导入首页.md", "# 导入标题\n\n[下一页](下一页.md#锚点)\n\n![示例](https://example.test/example.png)"),
            fixture("下一页.md", "# 第二页\n\n中文导入正文"),
            fixture("非UTF8.md", [0xff]),
        ])?),
        "import_csv_file" => {
            let destination = serde_json::from_value(args["destination"].clone())?;
            let input = if args["destination"]["type"] == "existing" {
                fixture("追加数据.csv", "标题,评分,状态,外部备注\n架构评审,2.5,进行中,原件\n历史案例,文字,已完成,原件")
            } else {
                fixture("导入清单.csv", "说明,标题,编号\n\"原件内容,多行\n第二行\",清单一,001\n多一列,无效,,超出\n核对,清单二,002")
            };
            json!([store.import_csv(space(), destination, input)?])
        }
        "import_migration" => {
            let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migration/fixtures");
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let report = if args["format"] == "notionZip" {
                let name = "Notion 样例.zip";
                store.import_notion_zip(space(), parent(), name, File::open(fixtures.join(name))?, &cancel)?
            } else {
                store.import_obsidian_vault(space(), parent(), &fixtures.join("Obsidian 样例"), &cancel)?
            };
            json!(report)
        }
        "latest_migration" => json!(store.latest_migration_for(
            space(), args["nativeContent"].as_bool().unwrap_or(false),
        )?),
        "save_import_report" => {
            std::fs::write(backup.with_file_name("import-report.txt"), text(args, "report"))?;
            json!(true)
        }
        "search_text" => json!(store.search_text(space(), &serde_json::from_value(args["request"].clone())?)?),
        "advance_search_index" => json!(store.advance_search_index(space())?),
        "rebuild_search_index" => json!(store.rebuild_search_index(space())?),
        "poll_pdf_index" => json!(store.poll_pdf_index(space())?),
        "finish_pdf_index" => json!(store.finish_pdf_index(space(), serde_json::from_value(args["result"].clone())?)?),
        "pdf_document_status" => json!(store.pdf_document_status(space(), text(args, "page"))?),
        "save_page" => {
            let mut draft: zhiye_core::PageDraft = serde_json::from_value(args["draft"].clone())?;
            draft.blocks = decode_desktop_blocks(draft.blocks)?;
            json!(store.save_page(space(), id(), revision(), draft)?)
        }
        "list_records" => json!(store.query_view_records(
            space(), collection(), text(args, "view"), args["offset"].as_u64().unwrap() as u32,
            serde_json::from_value::<Option<zhiye_core::ViewRange>>(args["range"].clone())?.as_ref(),
        )?),
        "read_record" => json!(store.record(space(), collection(), id())?),
        "list_attachments" => json!(store.attachments(space(), text(args, "page"))?),
        "read_attachment" => json!(store.attachment(space(), text(args, "page"), id())?),
        "add_attachments" => {
            let file = store.add_attachment(space(), text(args, "page"), "研究备注.txt",
                "知页附件块原件\n".as_bytes())?;
            json!({"added": [file], "failed": []})
        }
        "verify_add_pdf_attachments" => {
            let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pdf-index/fixtures");
            let mut files = Vec::new();
            for name in ["研究样本.pdf", "扫描样本.pdf", "加密样本.pdf", "损坏样本.pdf"] {
                let input = File::open(directory.join(name))?;
                files.push(store.add_attachment(space(), text(args, "page"), name, input)?.attachment);
            }
            json!(files)
        }
        "pick_images" => {
            let file = File::open(image.expect("test image was configured"))?;
            let imported = store.add_image(space(), text(args, "page"), "海湾.png", file)?;
            json!({"added": [imported], "failed": []})
        }
        "attach_image_bytes" => {
            let bytes: Vec<u8> = serde_json::from_value(args["bytes"].clone())?;
            json!(store.add_image(space(), text(args, "page"), text(args, "name"), bytes.as_slice())?)
        }
        "preview_attachment" => {
            let mut bytes = Vec::new();
            store.copy_attachment(space(), text(args, "page"), id(), &mut bytes)?;
            json!(bytes)
        }
        "rename_attachment" => json!(store.rename_attachment(space(), text(args, "page"), id(), text(args, "name"))?),
        "save_attachment_as" => {
            store.copy_attachment(space(), text(args, "page"), id(), File::create(backup.with_file_name("saved-attachment.bin"))?)?;
            json!(true)
        }
        "open_web_link" => {
            let url = zhiye_core::web_url(text(args, "href")).unwrap();
            std::fs::write(backup.with_file_name("link-opened.txt"), url)?;
            Value::Null
        }
        "export_page_draft" => {
            let mut draft: zhiye_core::PageDraft = serde_json::from_value(args["draft"].clone())?;
            draft.blocks = decode_desktop_blocks(draft.blocks)?;
            let contents = json!({"format": "zhiye.page-draft", "formatVersion": 1,
                "workspaceId": space(), "pageId": id(), "baseRevision": revision(), "draft": draft});
            std::fs::write(backup.with_file_name("editor-draft.json"), serde_json::to_vec(&contents)?)?;
            json!(true)
        }
        "export_markdown" | "export_html" => {
            if let Some(draft) = args.get("draft").filter(|value| !value.is_null()) {
                let draft: zhiye_core::PageDraft = serde_json::from_value(draft.clone())?;
                let blocks = decode_desktop_blocks(draft.blocks)?;
                let markdown = zhiye_core::render_workspace_markdown(&draft.title, &blocks, &Default::default(), Some(space()));
                std::fs::write(backup.with_file_name("editor-draft.md"), markdown)?;
                return Ok(json!({"companionFolder": null}));
            }
            let page = store.page(space(), id())?;
            let mut assets = Vec::new();
            let directory = backup.with_file_name("images");
            std::fs::create_dir_all(&directory)?;
            for item in store.markdown_attachments(space(), &page)? {
                let extension = zhiye_core::image_extension(&item.media_type).unwrap_or("txt");
                let name = format!("{}.{extension}", item.id);
                store.copy_attachment(space(), id(), &item.id, File::create(directory.join(&name))?)?;
                assets.push((item, format!("images/{name}")));
            }
            let format = if cmd == "export_html" {
                zhiye_core::PageExportFormat::Html
            } else {
                zhiye_core::PageExportFormat::Markdown
            };
            let document = zhiye_core::render_page_export(&page, &assets, space(), format);
            std::fs::write(directory.join("导出报告.txt"), document.report)?;
            std::fs::write(backup.with_file_name(format!("page.{}", format.extension())), document.document)?;
            json!({"companionFolder": "images"})
        }
        "save_property" => json!(store.save_property(
            space(), collection(), revision(), serde_json::from_value(args["draft"].clone())?,
        )?),
        "preview_property_conversion" => json!(store.preview_property_conversion(
            space(), collection(), revision(), serde_json::from_value(args["draft"].clone())?,
        )?),
        "apply_property_conversion" => json!(store.apply_property_conversion(
            space(), collection(), revision(), serde_json::from_value(args["draft"].clone())?,
            text(args, "fingerprint"),
        )?),
        "save_computed_property" => json!(store.save_computed_property(
            space(), collection(), revision(), serde_json::from_value(args["draft"].clone())?,
        )?),
        "create_relation" => json!(store.create_relation(
            space(), collection(), revision(), serde_json::from_value(args["draft"].clone())?,
        )?),
        "list_relation_targets" => json!(store.relation_targets(
            space(), collection(), text(args, "query"), args["offset"].as_u64().unwrap_or(0) as u32,
        )?),
        "save_collection_view" => json!(store.save_view(
            space(), collection(), text(args, "view"), revision(), serde_json::from_value(args["settings"].clone())?,
        )?),
        "upsert_collection_view" => json!(store.upsert_view(
            space(), collection(), serde_json::from_value(args["draft"].clone())?,
        )?),
        "delete_collection_view" => json!(store.delete_view(
            space(), collection(), text(args, "view"), revision(),
        )?),
        "move_scheduled_record" => wire(store.project_desktop_page(space(), store.move_scheduled_record(
            space(), collection(), serde_json::from_value(args["change"].clone())?,
        )?)?)?,
        "save_record" => wire(store.project_desktop_page(space(), store.save_record(
            space(), collection(), id(), revision(), serde_json::from_value(args["draft"].clone())?,
        )?)?)?,
        "export_workspace" => {
            let include_history = args["includeHistory"].as_bool().unwrap_or(true);
            store.write_workspace_archive_with_history(space(), include_history, File::create(backup)?)?;
            json!(true)
        }
        "merge_workspace" => json!(store.merge_workspace_package(
            space(),
            "workspace.zhiye.zip",
            File::open(backup)?,
            &std::sync::atomic::AtomicBool::new(false),
        )?),
        "backup_status" => json!(store.backup_status(args["workspace"].as_str())?),
        "set_backup_policy" => json!(store.set_backup_policy(serde_json::from_value(args["policy"].clone())?)?),
        "choose_backup_directory" => {
            let directory = backup.with_file_name("automatic-backups");
            std::fs::create_dir_all(&directory)?;
            json!(directory.canonicalize()?)
        }
        "run_workspace_backup" => json!(store.run_workspace_backup(space())?),
        "export_recovery" => {
            store.save_recovery_package(space(), &backup.with_file_name("manual.zhiye-recovery.zip"))?;
            json!(true)
        }
        "export_collection_csv" => {
            let folder = backup.with_file_name("csv-files");
            std::fs::create_dir_all(&folder)?;
            let report = store.write_collection_csv(
                space(), collection(), args["viewId"].as_str(), File::create(backup.with_file_name("records.csv"))?,
                |asset| {
                    let name = format!("{}.png", asset.id);
                    store.copy_attachment(space(), &asset.page_id, &asset.id, File::create(folder.join(&name))?)?;
                    Ok(format!("csv-files/{name}"))
                },
            )?;
            std::fs::write(folder.join("导出报告.txt"), report.limitations.join("\n"))?;
            json!({"rows": report.rows, "attachments": report.attachments.len(), "companionFolder": "csv-files",
                "limitations": report.limitations})
        }
        "import_workspace" | "verify_restore_content" => {
            let recovery = backup.with_file_name("manual.zhiye-recovery.zip");
            let choice = if cmd == "import_workspace" && recovery.exists() { recovery.as_path() } else { backup };
            json!(store.read_workspace_package(File::open(choice)?)?)
        }
        _ => return Err(zhiye_core::Error::new("VERIFY_COMMAND", format!("Unexpected command {cmd}"))),
    })
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let store = Store::open(&args[1]).unwrap();
    if store.workspaces().unwrap().is_empty() {
        store.create_workspace("界面验收").unwrap();
    }
    let mut output = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let request: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let response = dispatch(
            &store,
            Path::new(&args[2]),
            args.get(3).map(Path::new),
            text(&request, "cmd"),
            &request["args"],
        );
        let response = match response {
            Ok(value) => json!({"id": request["id"], "result": value}),
            Err(error) => json!({"id": request["id"], "error": error}),
        };
        writeln!(output, "{response}").unwrap();
        output.flush().unwrap();
    }
}
