# Mac 正常退出时的保存检查

按原设计第 5.1 节与 A03 检查当前页面草稿。0.49.0 的应用菜单退出走窗口关闭检查。系统普通退出走 AppKit，现有 Tauri 和 Tao 在该路径上只收到结束通知，来不及阻止退出。实测活动监视器的普通「退出」结束了仍持有保存失败草稿的应用。

复用已锁定的 Rust AppKit 绑定，为当前代理补上 `applicationShouldTerminate:`，保留原有代理和链接事件。使用 Apple 的延迟退出协议。应用菜单和系统普通退出进入同一检查，窗口关闭沿用现有入口，三种操作都先提交页面编辑器。保存失败、已有操作进行中或 WebView 未答复时取消退出。回执仅能完成对应的一次请求。

- [x] 从 0.49.0 DMG 复制应用，在独立 APFS 合成库重现真实写满、保存失败及系统普通退出丢失草稿。
- [x] 在 `apps/desktop/src-tauri/src/macos/termination.rs` 注册退出回调。主线程处理答复并取消超时请求。AppKit 原生运行待成品验收。
- [x] 在 `apps/desktop/src/App.tsx` 和 `desktop.ts` 共用现有页面提交逻辑。真实 Core 浏览器联调覆盖成功提交、保存中重复请求和失败保稿。真实写满后的重试留给下方原生验收。[联调结果](../../../.verify/macos-050/windows-2026-11-19/browser-report.json)
- [ ] 生成 0.50.0 Apple Silicon DMG 与源码及校验文件。从镜像复制应用后验证系统普通退出、应用快捷键、草稿导出、写满恢复和重新打开，并记录验证范围。

依据 [开发计划 v2.0](../../../个人资源管理系统_完整开发Plan_v2.0.md#9-验收与测试策略) 与 [Apple AppKit 文档](https://developer.apple.com/documentation/appkit/nsapplicationdelegate/applicationshouldterminate%28_%3A%29?changes=_1_7&language=objc)。
