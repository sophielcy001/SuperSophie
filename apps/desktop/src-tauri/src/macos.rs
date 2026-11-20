// SPDX-License-Identifier: AGPL-3.0-or-later

pub(crate) mod termination;

use tauri::{
    AppHandle, Emitter, Wry,
    menu::{
        AboutMetadata, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu, WINDOW_SUBMENU_ID,
    },
};

const COMMAND_PREFIX: &str = "zhiye-command-";

fn command(
    app: &AppHandle,
    id: &str,
    title: &str,
    accelerator: Option<&str>,
) -> tauri::Result<MenuItem<Wry>> {
    MenuItem::with_id(
        app,
        format!("{COMMAND_PREFIX}{id}"),
        title,
        true,
        accelerator,
    )
}

pub(crate) fn menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let application = Submenu::with_items(
        app,
        "知页",
        true,
        &[
            &PredefinedMenuItem::about(
                app,
                Some("关于知页"),
                Some(AboutMetadata {
                    name: Some("知页".into()),
                    version: Some(app.package_info().version.to_string()),
                    ..Default::default()
                }),
            )?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, Some("服务"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, Some("隐藏知页"))?,
            &PredefinedMenuItem::hide_others(app, Some("隐藏其他"))?,
            &PredefinedMenuItem::show_all(app, Some("全部显示"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, Some("退出知页"))?,
        ],
    )?;
    let file = Submenu::with_items(
        app,
        "文件",
        true,
        &[
            &command(app, "new-page", "新建页面", Some("Cmd+N"))?,
            &command(app, "new-database", "新建数据库", Some("Cmd+Shift+N"))?,
            &PredefinedMenuItem::separator(app)?,
            &command(app, "save", "保存", Some("Cmd+S"))?,
            &command(app, "import", "导入资料…", None)?,
            &command(app, "backup", "备份与恢复…", None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, Some("关闭窗口"))?,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        "编辑",
        true,
        &[
            &PredefinedMenuItem::undo(app, Some("撤销"))?,
            &PredefinedMenuItem::redo(app, Some("重做"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, Some("剪切"))?,
            &PredefinedMenuItem::copy(app, Some("复制"))?,
            &PredefinedMenuItem::paste(app, Some("粘贴"))?,
            &PredefinedMenuItem::select_all(app, Some("全选"))?,
        ],
    )?;
    let view = Submenu::with_items(
        app,
        "显示",
        true,
        &[
            &command(app, "search", "搜索页面…", Some("Cmd+K"))?,
            &command(app, "commands", "命令面板…", Some("Cmd+Shift+P"))?,
            &command(app, "focus", "切换专注模式", Some("Cmd+Shift+F"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::fullscreen(app, Some("进入全屏幕"))?,
        ],
    )?;
    let window = Submenu::with_id_and_items(
        app,
        WINDOW_SUBMENU_ID,
        "窗口",
        true,
        &[
            &PredefinedMenuItem::minimize(app, Some("最小化"))?,
            &PredefinedMenuItem::maximize(app, Some("缩放"))?,
        ],
    )?;
    Menu::with_items(app, &[&application, &file, &edit, &view, &window])
}

pub(crate) fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref();
    if let Some(command) = id.strip_prefix(COMMAND_PREFIX) {
        let _ = app.emit("native-command", command);
    }
}
