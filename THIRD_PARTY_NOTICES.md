# 第三方组件说明

以下是桌面程序的主要组件及锁定版本。完整版本表见 `Cargo.lock` 和 `package-lock.json`。npm 包中附带各包的许可文本，Rust 依赖的许可标识可用 `cargo metadata --locked --format-version 1` 核对。

安装包附带本声明、项目 [LICENSE](LICENSE) 和 `licenses` 目录。Windows 安装后可在安装目录查阅。macOS 的对应文件位于应用包的 `Contents/Resources`。依赖清单生成命令在解包后的源码目录运行。

## 逐项依赖清单

按目标平台解析 Cargo 后，用脚本从 `package-lock.json` 和 Cargo 元数据生成 JSON。输出记录锁文件哈希、固定版本、来源和许可。npm 条目保留生产、开发和可选标记。Cargo 条目保留使用方、依赖类别及声明的可选性。构建与运行环境中的系统 WebView 见离线指南。[锁定依赖清单脚本](scripts/dependency-inventory.mjs) [离线构建与验收](docs/离线构建与验收.md)

```sh
cargo metadata --locked --offline --format-version 1 --filter-platform aarch64-apple-darwin > cargo-macos.json
cargo metadata --locked --offline --format-version 1 --filter-platform x86_64-pc-windows-msvc > cargo-windows.json
node scripts/dependency-inventory.mjs aarch64-apple-darwin=cargo-macos.json x86_64-pc-windows-msvc=cargo-windows.json > dependencies.json
```

先为要列出的目标准备 Cargo 缓存。输出中 Cargo 的 `normal`、`build` 和 `dev` 对应依赖边的用途，产品运行所需的依赖由目标构建确定。

## 主要组件

| 组件 | 版本 | 许可证或声明 |
|---|---|---|
| Tauri Rust | 2.12.5 | MIT OR Apache-2.0 |
| Tauri JS API、CLI | 2.12.4 | MIT OR Apache-2.0 |
| Tauri NSIS 安装模板 | CLI 2.12.4 | MIT，许可见 `licenses/Tauri-NSIS-MIT.txt` |
| Tauri Dialog Rust | 2.8.4 | MIT OR Apache-2.0 |
| Tauri Opener Rust／open | 2.7.3／5.4.5 | 分别为 MIT OR Apache-2.0、MIT |
| Tauri Deep Link／Single Instance | 2.6.3／2.5.5 | MIT OR Apache-2.0 |
| React、React DOM | 19.3.1 | MIT |
| Tiptap（含 TableKit）和 ProseMirror | Tiptap 3.33.1，ProseMirror 见锁文件 | MIT |
| KaTeX | 0.19.1 | [MIT](licenses/KaTeX-MIT.txt) |
| KaTeX 随附字体 | 随 KaTeX 0.19.1 | [OFL-1.1](licenses/KaTeX-Fonts-OFL.txt) |
| rusqlite／SQLite | rusqlite 0.42.0，内置 SQLite | rusqlite MIT，SQLite 公有领域 |
| getrandom | 0.4.3 | MIT OR Apache-2.0 |
| hmac | 0.13.0 | MIT OR Apache-2.0 |
| chacha20poly1305 | 0.10.1 | MIT OR Apache-2.0 |
| base64 | 0.23.1 | MIT OR Apache-2.0 |
| Hyper／hyper-util／http-body-util | 1.12.0／0.1.21／0.1.5 | MIT |
| Tokio | 1.54.1 | MIT |
| url | 2.5.8 | MIT OR Apache-2.0 |
| reqwest | 0.13.5 | MIT OR Apache-2.0。同步客户端仅启用 HTTP/1.1 和系统 TLS。 |
| native-tls／hyper-tls／tokio-native-tls | 0.2.18／0.6.0／0.3.1 | 分别为 MIT OR Apache-2.0、MIT OR Apache-2.0、MIT |
| security-framework | 3.7.0 | MIT OR Apache-2.0。macOS 的 TLS 与钥匙串。 |
| schannel／windows-sys | 0.1.30／0.61.2 | 分别为 MIT、MIT OR Apache-2.0。Windows 的 TLS 与凭据管理器。 |
| rustls-pki-types | 1.14.1 | MIT OR Apache-2.0 |
| percent-encoding | 2.3.2 | MIT OR Apache-2.0 |
| pulldown-cmark／pulldown-cmark-escape | 0.13.6／0.11.0 | [MIT](licenses/Pulldown-CMark-MIT.txt) |
| csv／csv-core | 1.4.0／0.1.13 | Unlicense OR MIT |
| zip／flate2／zlib-rs | 9.5.1／1.1.11／0.6.8 | 分别为 MIT、MIT OR Apache-2.0、Zlib |
| rustix | 1.1.6 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT。macOS 备份使用 POSIX 刷盘接口。 |
| lucide-react | 1.72.0 | ISC |
| Inter Variable | 5.3.0 | OFL-1.1，见 [Inter 许可](licenses/Inter-OFL.txt)。自 2026-11-24 起界面改用系统字体，字体文件不再打包进前端产物，npm 依赖暂留。 |
| PDF.js | 6.5.438 | [Apache-2.0](licenses/PDFjs-Apache-2.0.txt) |
| PDF.js 随附字体与 CMaps | 随 PDF.js 6.5.438 | [Liberation](licenses/PDFjs-Liberation-Fonts.txt)、[Foxit](licenses/PDFjs-Foxit-Fonts.txt)、[Adobe CMaps](licenses/PDFjs-Adobe-CMaps.txt) |
| Vite、Vitest、TypeScript | 8.3.8、5.1.0、5.9.3 | 开发依赖，分别为 MIT、MIT、Apache-2.0 |
| MCP TypeScript SDK | 1.32.2 | MIT |
| Ajv | 8.20.0 | MIT |
| openapi-typescript、yaml | 7.13.0、2.9.1 | 契约生成依赖，分别为 MIT、ISC |

## 自托管同步服务

以下组件只用于同步服务 `zhiye-sync-server`，不包含在桌面程序中：

| 组件 | 版本 | 许可证或声明 |
|---|---|---|
| postgres／tokio-postgres／postgres-protocol／postgres-types | 0.19.14／0.7.18／0.6.12／0.2.14 | MIT OR Apache-2.0 |
| fallible-iterator、md-5、stringprep、chacha20 | 0.2.0、0.11.0、0.1.5、0.9.1 | MIT OR Apache-2.0 |
| unicode-normalization、unicode-properties、unicode-bidi | 0.1.26、0.1.5、0.3.18 | 前两项为 (MIT OR Apache-2.0) AND Unicode-3.0，unicode-bidi 为 MIT OR Apache-2.0 |
| whoami、wasite | 2.1.3、1.0.2 | Apache-2.0 OR BSL-1.0 OR MIT |
| objc2-system-configuration | 0.3.2 | Zlib OR Apache-2.0 OR MIT（仅 macOS 构建） |

Docker 参考部署另外使用官方镜像 `postgres:17-alpine`（PostgreSQL License）、`caddy:2-alpine`（Apache-2.0）、构建镜像 `rust:1.100.0-slim-bookworm` 和运行镜像 `debian:bookworm-slim`，各镜像中的系统软件按其自身许可分发。

Cargo 锁文件还包含 `openssl`、`openssl-sys`、`openssl-macros`、`openssl-probe` 和 `foreign-types`，它们只在 Linux 目标上作为系统 TLS 使用，Windows 和 macOS 构建不编译。

本机 x64 构建的 Rust 依赖中包含 `cssparser`、`cssparser-macros`、`dtoa-short`、`option-ext` 和 `selectors` 的 MPL-2.0 代码。当前工程通过 Cargo 依赖使用这些组件，其原文件和许可证随对应版本的公开 crate 发布。

Windows 桌面端使用 Microsoft WebView2 Runtime。离线候选包随附微软发布的 x64 Evergreen Standalone Installer，具体下载来源和签名校验结果随包记录。[微软分发说明](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution#offline-deployment) [运行时签名校验](.verify/windows-offline/runtime-authenticode-primary.log)

## Windows 验收包中的构建工具

验收包的 `dependencies/nsis` 保存 Tauri 使用的 NSIS 3.13 二进制归档，以及 NSIS 3.13 的源代码归档。NSIS 的 `COPYING` 随两个归档提供，包含 zlib／libpng、bzip2 和 LZMA 使用的 Common Public License 1.0。对应源代码也可从 NSIS 发布目录获取。[Tauri 的 NSIS 归档](https://github.com/tauri-apps/binary-releases/releases/tag/nsis-3.13) [NSIS 3.13 官方发布](https://sourceforge.net/projects/nsis/files/NSIS%203/3.13/) [NSIS 许可](https://nsis.sourceforge.io/License)

同目录的 Tauri NSIS 插件为 0.5.3。插件 DLL 和对应版本源码随包提供，源码归档包含上游 MIT 和 Apache-2.0 许可文本。可通过验收包清单核对四份文件。应用安装目录的许可入口见本页开头。[Tauri NSIS 插件发布](https://github.com/tauri-apps/nsis-tauri-utils/releases/tag/nsis_tauri_utils-v0.5.3)
