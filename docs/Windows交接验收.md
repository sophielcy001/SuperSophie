# Windows 0.50.14 交接与实测记录

2026-11-21。当前验收入口为 [PDF 搜索分页修复包](#pdf-搜索分页修复包)。验收包提供两种 Windows x64 安装器、同批冻结源码、锁定缓存、合成素材和 A18 工具。各项回执按随包清单校验。[验收步骤及文件校验](Windows验收包.md)

## WebView2 HTTPS 修复批次

| 场景 | HTTPS 修复批次的验证范围 | 验收回执 |
| --- | --- | --- |
| K23、A17 安装及运行时 | 无 WebView2 的 Windows 10 客机完成常规包联网首装，运行时通过微软签名校验。断网客机上常规包退出 2，离线包通过首装、重装复用及卸载。 | [联网首装审计](../.verify/windows-current-online-runtime-0514/audit.json) [客机原始回执](../.verify/windows-current-online-runtime-0514/guest-result.json) [断网首装与接续](../.verify/windows-current-runtime-fallback-0514/audit.json) |
| K23、A16 自定义安装路径 | 常规包静默安装到含中文和空格的路径，离线包原位覆盖。系统定位链接、重开正文和卸载后的资料保留通过。 | [实装与原始回执](../.verify/windows-unicode-install-0514/复验.md) |
| K24、A17 断网源码重建 | 交接 ZIP 的锁定缓存支持从空目录重建 SDK、MCP、桌面和两种 NSIS。24 项 JavaScript／TypeScript 与 126 项 Rust Core 测试通过。常规和离线安装器各通过 27 项实装检查，两次卸载均成功。 | [断网重建审计](../.verify/windows-https-rebuild-0514/audit-3.json) [客机原始回执](../.verify/windows-https-rebuild-0514/serial-3.json) |
| A18 性能补测 | 32 GiB Windows 11 实体机在本机 NVMe SSD 的五项 p95 均达到门槛。226 次回环 HTTP 无错误，十万页库完成末页定位与重开。正式参考机样本待采集。 | [同批补测复核](../.verify/windows-https-handoff-0514/replay-supplemental.json) [随包重算](性能验收.md#重算随包补测) |

## PDF 搜索候选批次

0.50.14 PDF 搜索候选包含 PDF 搜索分页优化。常规包和离线包在已有 WebView2 的 Windows 11 x64 本机各通过 27 项实装检查和卸载，常规包额外通过 10 项 A05 搜索检查。两种安装器、冻结源码、A18 工具及校验文件一同归档。采样时使用该批次随附的 A18 工具。[PDF 候选文件与实装](../.verify/windows-pdf-candidate-0514/复验.md)

交接 ZIP 汇集同批安装包、冻结源码、锁定缓存、实装回执和 A18 原始样本。Windows 解包核对了 1,372 个文件并通过随包样本复算。[PDF 候选交接 ZIP](../artifacts/0.50.14-windows-PDF/Zhiye-0.50.14-windows-acceptance-handoff-pdf-verified.zip) [SHA-256](../artifacts/0.50.14-windows-PDF/Zhiye-0.50.14-windows-acceptance-handoff-pdf-verified.zip.sha256) [组包与解包校验](../.verify/windows-pdf-verified-handoff-0514/复验.md)

本批在 32 GiB 实体机补测五项 p95 达标，已独立复算 14 份原始结果。正式采样需使用 16 GiB 实体参考机。[补测结果与现场问题](../.verify/windows-pdf-candidate-0514/复验.md#a18-性能补测)

无网卡、无 WebView2 的 Windows 10 客机验证本批常规包失败后无残留。离线包完成 WebView2 首装与重装复用，两次卸载成功。[PDF 候选断网首装与回执](../.verify/windows-pdf-runtime-0514/复验.md)

使用同批验收 ZIP 核对两种原安装器，在无网卡客机完成 0.31.0 和 0.50.13 的原位覆盖升级。两次模拟备份故障均保全旧库，解除后完成迁移、独立恢复与重开。[PDF 候选覆盖升级与恢复](../.verify/windows-pdf-upgrade-0514/复验.md)

PDF 搜索候选的验收 ZIP 在断网客机完成冻结源码重建。24 项 JavaScript／TypeScript、126 项 Rust Core 测试及两种重建包各 27 项实装检查通过。[同批断网源码重建](../.verify/windows-pdf-rebuild-0514/复验.md)

常规包在全新 Windows 10 客机联网首装并于重装时复用 WebView2 157.0.4341.60，运行时微软签名有效。[同批联网首装](../.verify/windows-pdf-online-0514/复验.md)

PDF 搜索候选批次的常规包在断网客机通过 Windows Shell PDF 第 2 页链接，离线包通过四组断网资料库流程。专项回执已按本节交接 ZIP 的指纹归档。[PDF 系统链接](../.verify/windows-pdf-shell-0514/复验.md) [千页、六视图、搜索与迁移](../.verify/windows-pdf-offline-library-0514/复验.md)

## PDF 搜索分页修复包

0.50.14 分页修复包在已有 WebView2 的 Windows 11 上实装，两包各通过 PDF 专项、27 项桌面与键盘检查及卸载。此批次冻结实际构建的源码，绑定同批 A18 工具和 32 GiB 实体机补测。五项 p95 均达门槛。[修复安装包与验证范围](../.verify/windows-pdf-pagination-0514/复验.md#修复安装包实装) [源码与 A18 工具](../.verify/windows-pdf-pagination-handoff-0514/复验.md)

分页修复批次已通过断网首装、联网首装、空目录重建和跨版本覆盖升级。两份重建包分别通过桌面和 PDF 分页专项。附带 14 份同批 A18 补测数据，16 GiB 实体参考机正式样本待采集。[分页修复交接 ZIP](../artifacts/0.50.14-windows-PDF-Pagination/Zhiye-0.50.14-windows-acceptance-handoff-pdf-pagination-verified.zip) [SHA-256](../artifacts/0.50.14-windows-PDF-Pagination/Zhiye-0.50.14-windows-acceptance-handoff-pdf-pagination-verified.zip.sha256) [实测输入包](../artifacts/0.50.14-windows-PDF-Pagination/Zhiye-0.50.14-windows-acceptance-handoff-pdf-pagination.zip)

本批另提供独立补充 ZIP，集中 17 组实装审计和 A01–A18 索引。保留两份 ZIP 和各自的 `.sha256`，解开补充包后按 `开始核对.md` 复验。[补充回执 ZIP](../artifacts/0.50.14-windows-PDF-Pagination/Zhiye-0.50.14-windows-pagination-evidence.zip) [SHA-256](../artifacts/0.50.14-windows-PDF-Pagination/Zhiye-0.50.14-windows-pagination-evidence.zip.sha256) [核对结果](../.verify/windows-pdf-pagination-evidence-0514/复验.md)

常规包在无 WebView2 的 Windows 10 客机联网安装了微软签名有效的运行时。离线包在无网卡客机完成 WebView2 首装和复用。覆盖升级验证从格式 18 迁移到 21，两个故障场景均保全旧库，恢复独立备份读回第 2、3 版，0.50.13 升级后重开读回第 4 版。[联网首装](../.verify/windows-pdf-pagination-online-0514/audit.json) [断网首装](../.verify/windows-pdf-pagination-runtime-0514/audit.json) [独立重建](../.verify/windows-pdf-pagination-rebuild-0514/audit-3.json) [升级与恢复](../.verify/windows-pdf-pagination-upgrade-0514/audit.json)

离线包在无网卡 Windows 10 客机通过千页编辑、独立恢复、六视图、内容包、搜索及迁移重试，18 份回传文件已按同批交接 ZIP 校验。[同批断网资料库](../.verify/windows-pdf-pagination-offline-library-0514/复验.md)

同批常规包通过第二页搜索结果的 Windows Shell 定位链接。原件第 21 页在搜索、重建索引后搜索、冷启动及窗口转发中渲染一致，失效的原件指纹被拒绝。[同批 PDF 系统链接](../.verify/windows-pdf-pagination-shell-0514/复验.md)

同批离线包完成无网卡客机 A08–A14。常规和离线包另在 Windows 11 上从 UNC 共享目录恢复第 2、3、4 版，核对正文、历史、原件及已禁用的历史授权。[断网集成](../.verify/windows-pdf-pagination-integrations-0514/复验.md) [NAS 备份与跨包恢复](../.verify/windows-pdf-pagination-nas-0514/复验.md)

同批常规包通过 NTFS 整卷写满下的草稿保护、释放空间后重试、强制重开和附件原件保全。在 64 MiB 用户硬配额触顶时，另存和重试均能保全资料。[A03、A06 整卷写满](../.verify/windows-pdf-pagination-ntfs-0514/复验.md) [A03 用户配额](../.verify/windows-pdf-pagination-quota-0514/复验.md)

同批常规包在无网卡 Windows 10 客机通过默认微软拼音选词后切页，以及讲述人实际播报、任务键盘操作和保存错误复验。[微软拼音](../.verify/windows-pdf-pagination-ime-0514/复验.md) [讲述人与录音](../.verify/windows-pdf-pagination-narrator-0514/复验.md)

同批常规包通过 Windows 系统粘贴、撤销、重做、键盘移块、顶层及嵌套拖动。保存请求完成后才切页，重开读回已提交内容与块 ID。[正文编辑](../.verify/windows-pdf-pagination-editor-0514/复验.md)

同批离线包通过卸载删除权限故障保全。0.31.0 安全拒读格式 21 的独立副本，仍能独立恢复格式 18。同批常规包重装后，两种恢复路径均读回第 4 版和两份原件。[卸载故障与回滚](../.verify/windows-pdf-pagination-rollback-0514/复验.md)

同批常规包在无网卡 Windows 10 客机安装至含中文和空格的路径，离线包按已有登记原位覆盖。两轮的快捷方式、系统协议与定位均通过，卸载保留资料库和邻近文件。[中文安装路径](../.verify/windows-pdf-pagination-unicode-0514/复验.md)

同批常规包通过页面树跨目录移动、循环拒绝和图标封面重开。恢复移动前的外观后，页面仍在新目录，导出 Markdown 的封面可按相对路径读回。[页面树、外观与导出](../.verify/windows-pdf-pagination-appearance-0514/复验.md)

同批常规包在断网客机通过页面链接与块引用往返、源块编辑移除和历史恢复。回环 API 对三项未授权访问返回 404，增加源页授权后仅显示可见反向链接。[页面与块引用](../.verify/windows-pdf-pagination-references-0514/复验.md)

同批常规包通过取消与清空回收站、历史恢复和清空前整库恢复。在七天保留期前后检查确认按钮与候选原件，清理后保留历史引用。从备份独立恢复两份原件，重开读回父子页第 4、5 版。[历史与回收站实装](../.verify/windows-pdf-pagination-trash-0514/复验.md)

## 首屏性能候选批次

0.50.14 首屏性能候选的交接 ZIP 收录两种安装器、同批冻结源码、锁定缓存和 A18 工具。32 GiB 实体机补测五项达标，筛选数据库首屏 p95 为 216.6 毫秒。无网卡 Windows 10 客机通过覆盖升级、备份恢复和从空目录重建后的两次实装检查。正式参考机按该批次独立采样。[首屏性能候选完整交接 ZIP](../artifacts/0.50.14-windows-Performance/Zhiye-0.50.14-windows-acceptance-handoff-performance-verified.zip) [SHA-256](../artifacts/0.50.14-windows-Performance/Zhiye-0.50.14-windows-acceptance-handoff-performance-verified.zip.sha256) [打包与复算](../.verify/windows-performance-handoff-0514/复验.md) [A18 工具](../artifacts/0.50.14-windows-Performance/Zhiye-0.50.14-windows-a18.zip) [A18 工具 SHA-256](../artifacts/0.50.14-windows-Performance/Zhiye-0.50.14-windows-a18.zip.sha256) [升级恢复](../.verify/windows-performance-upgrade-0514/复验.md) [断网重建](../.verify/windows-performance-rebuild-0514/复验.md)

无网卡、无 WebView2 的 Windows 10 客机使用首屏性能候选原始安装包。常规包退出 2，系统无半安装残留。离线包完成 WebView2 首装，重装时复用相同运行时。四次开始菜单启动和关闭、两次卸载均成功。[断网首装](../.verify/windows-performance-runtime-0514/复验.md)

常规包也已在全新 Windows 10 客机联网首装和重装，安装了签名有效的 WebView2 157.0.4341.60。同批客机原始回执可以核对联网和断网首装、断网重建与覆盖升级。[联网首装](../.verify/windows-performance-online-0514/audit.json) [断网首装](../.verify/windows-performance-runtime-0514/audit.json) [断网重建](../.verify/windows-performance-rebuild-0514/audit-3.json) [升级恢复](../.verify/windows-performance-upgrade-0514/audit.json)

## 卸载提示批次实测

[卸载提示批次交接记录](../.verify/windows-handoff-replay-0514/复验.md)保存了以下实装和重建的原始批次指纹。

| 场景 | 卸载提示批次的验证范围 | 随包回执 |
| --- | --- | --- |
| A16 NAS 与跨安装包恢复 | Windows 11 常规包实测 UNC 手动备份、目标故障重试、当天复用和重启后计划备份。常规与离线包均从共享恢复并重开第 2、3、4 版，核对三份恢复 ZIP、七份 SQLite 和十次系统文件窗口。 | [NAS 审计](../.verify/windows-current-nas-0514/audit.json) |
| A16 卸载故障与回滚 | 本批离线包保存第 4 版后触发 NTFS 删除权限错误，卸载引擎退出 2，资料指纹保持。0.31.0 独立恢复格式 18，0.50.14 常规包重开原空间和恢复空间验证第 4 版。 | [回滚审计](../.verify/windows-uninstall-rollback-0514/audit.json) |
| A17 离线安装 | Windows 10 LTSC 断网客机完成 WebView2 首装与同文件复用，另完成 27 项桌面检查及卸载。 | [运行时审计](../.verify/windows-uninstall-runtime-0514/runtime-audit.json) · [桌面审计](../.verify/windows-uninstall-runtime-0514/desktop-audit.json) |
| A17 冻结源码重建 | 断网重建 SDK、MCP、桌面及两种 NSIS。24 项 JavaScript／TypeScript 检查、126 项 Rust Core 检查、两种重建安装器各 27 项原生检查通过。 | [重建审计](../.verify/windows-uninstall-rebuild-0514/audit-3.json) [交接包输入核对](../.verify/windows-current-build-provenance-0514/复验.md) |

本批离线安装器在无网卡 Windows 10 跑通千页编辑及恢复、六视图、全文与 PDF 和迁移中断重试。[断网资料库复验](../.verify/windows-current-offline-library-0514/复验.md)

同批离线安装器在无网卡 Windows 10 另通过 A08–A14。随包冻结源码和 npm 缓存重建 SDK 与 MCP 后，用实装程序复验授权、冲突草稿与建议、幂等重试、回环停服和游标续读。[断网集成复验](../.verify/windows-current-offline-integrations-0514/复验.md)

## 16 GiB 参考机性能验收

16 GiB 物理内存参考机的 A18 正式样本待采集。使用已有 WebView2 的 Windows x64 实机，准备专用交互测试账户和本机 SSD。账户应处于无知页安装、无运行中知页窗口的状态。从 PDF 分页修复批次验收包取得同批 A18 工具，或校验其单独归档的 ZIP 及 SHA-256。测试较早候选时使用对应验收包的 A18 工具。解压后打开包内的「开始验收.md」，从 A18 解包目录运行以下命令，结果路径使用尚未创建的目录。[分页修复批次 A18 ZIP](../artifacts/0.50.14-windows-PDF-Pagination/Zhiye-0.50.14-windows-a18.zip) [SHA-256](../artifacts/0.50.14-windows-PDF-Pagination/Zhiye-0.50.14-windows-a18.zip.sha256)

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\Run-A18.ps1 -OutputDirectory 'C:\A18\results-reference'
```

默认使用当前账户的本机临时目录。如需选择另一块本机 SSD，追加 `-ScratchRoot 'D:\A18 工作目录'`，并提前创建该目录。将完整结果目录交回 SuperSophie。复核可从解压后的交接包顶层运行，指定原交接 ZIP 及其内嵌的同批 A18 工具包。随包入口核对冻结源码和安装器指纹，按设备回执检查物理内存与 SSD，再从原始样本重算五项门槛。[回传复核命令](性能验收.md#复核回传结果) [五项门槛与采样规则](性能验收.md)
