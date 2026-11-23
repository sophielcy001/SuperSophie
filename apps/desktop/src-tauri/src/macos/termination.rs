// SPDX-License-Identifier: AGPL-3.0-or-later

use objc2::{
    MainThreadMarker, ffi,
    runtime::{AnyObject, Imp, Sel},
    sel,
};
use objc2_app_kit::{NSApplication, NSApplicationTerminateReply as Reply};
use std::{cell::Cell, sync::OnceLock, time::Duration};
use tauri::{AppHandle, Emitter, EventTarget, Manager};
use uuid::Uuid;

static APP: OnceLock<AppHandle> = OnceLock::new();

thread_local! {
    static PENDING: Cell<Option<Uuid>> = const { Cell::new(None) };
}

pub(crate) fn install(app: &AppHandle) -> Result<(), &'static str> {
    let main = MainThreadMarker::new().ok_or("macOS 退出检查需要在主线程安装。")?;
    let application = NSApplication::sharedApplication(main);
    let delegate = application.delegate().ok_or("找不到 macOS 应用代理。")?;
    let object: &AnyObject = (*delegate).as_ref();
    let class = object.class();
    let selector = sel!(applicationShouldTerminate:);
    if class.instance_method(selector).is_some() {
        return Err("macOS 应用代理已包含退出检查，请复核运行库版本。");
    }
    APP.set(app.clone())
        .map_err(|_| "macOS 退出检查已经安装。")?;
    // Tao 0.37 only implements applicationWillTerminate. Add the missing
    // optional method without replacing its delegate or other selectors.
    type ShouldTerminate = extern "C-unwind" fn(&AnyObject, Sel, &NSApplication) -> Reply;
    // SAFETY: the ABI matches AppKit's NSUInteger return and object argument
    // on both supported 64-bit Mac architectures. We only add an absent method.
    let installed = unsafe {
        let method = std::mem::transmute::<ShouldTerminate, Imp>(should_terminate);
        ffi::class_addMethod(
            std::ptr::from_ref(class).cast_mut(),
            selector,
            method,
            c"Q@:@".as_ptr(),
        )
    };
    if installed.as_bool() {
        Ok(())
    } else {
        Err("无法安装 macOS 退出检查。")
    }
}

extern "C-unwind" fn should_terminate(
    _delegate: &AnyObject,
    _selector: Sel,
    _application: &NSApplication,
) -> Reply {
    let Some(app) = APP.get() else {
        return Reply::TerminateCancel;
    };
    if app.get_webview_window("main").is_none() {
        return Reply::TerminateNow;
    }
    if PENDING.with(|pending| pending.get().is_some()) {
        return Reply::TerminateLater;
    }

    let request = Uuid::new_v4();
    PENDING.with(|pending| pending.set(Some(request)));
    if app
        .emit_to(
            EventTarget::webview_window("main"),
            "system-quit-requested",
            request.to_string(),
        )
        .is_err()
    {
        PENDING.with(|pending| pending.set(None));
        return Reply::TerminateCancel;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(15)).await;
        let _ = app.run_on_main_thread(move || reply(request, false));
    });
    Reply::TerminateLater
}

fn reply(request: Uuid, saved: bool) {
    let Some(main) = MainThreadMarker::new() else {
        return;
    };
    let matches = PENDING.with(|pending| {
        if pending.get() != Some(request) {
            return false;
        }
        pending.set(None);
        true
    });
    if matches {
        NSApplication::sharedApplication(main).replyToApplicationShouldTerminate(saved);
    }
}

#[tauri::command]
pub(crate) fn finish_macos_quit(
    app: AppHandle,
    request: String,
    saved: bool,
) -> Result<(), String> {
    let request = Uuid::parse_str(&request).map_err(|_| "退出请求无效。".to_string())?;
    app.run_on_main_thread(move || reply(request, saved))
        .map_err(|error| error.to_string())
}
