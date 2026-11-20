// SPDX-License-Identifier: AGPL-3.0-or-later

use std::io::{self, Write};
use tauri::{
    AppHandle, Webview,
    webview::{PageLoadEvent, PageLoadPayload},
};
use zhiye_core::{Error, Result};

fn enabled() -> bool {
    std::env::var_os("ZHIYE_PERFORMANCE_PROBE").is_some()
        && std::env::var_os("ZHIYE_DATA_DIR").is_some()
}

pub(crate) fn page_loaded(webview: &Webview, payload: &PageLoadPayload<'_>) {
    if !enabled() || webview.label() != "main" || payload.event() != PageLoadEvent::Finished {
        return;
    }
    if let Err(error) = webview.eval(
        r#"
        (() => {
          const isEditable = () => {
            const node = document.querySelector('[aria-label="页面正文"][contenteditable="true"]');
            return node && node.getBoundingClientRect().height > 0;
          };
          const observer = new MutationObserver(check);
          function check() {
            if (!isEditable()) return;
            observer.disconnect();
            requestAnimationFrame(() => requestAnimationFrame(() => {
              if (isEditable()) void window.__TAURI_INTERNALS__.invoke('record_performance_ready');
            }));
          }
          observer.observe(document.documentElement, {
            childList: true, subtree: true, attributes: true, attributeFilter: ['contenteditable']
          });
          check();
        })();
    "#,
    ) {
        eprintln!("M5_PERFORMANCE_ERROR {error}");
    }
}

#[tauri::command]
pub(crate) async fn record_performance_ready(app: AppHandle) -> Result<()> {
    if !enabled() {
        return Err(Error::new("PERFORMANCE_PROBE_DISABLED", "性能测量未启用。"));
    }
    writeln!(
        io::stdout(),
        "M5_PERFORMANCE_READY {}",
        env!("CARGO_PKG_VERSION")
    )?;
    if std::env::var_os("ZHIYE_PERFORMANCE_PROBE_HTTP").is_some()
        && let Some(address) = crate::integrations::set_local_api(app, true).await?.address
    {
        writeln!(io::stdout(), "M5_PERFORMANCE_HTTP {address}")?;
    }
    Ok(())
}
