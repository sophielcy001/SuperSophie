# Windows 1.0 验收

2026-11-19 起，开发和交付优先推进 Windows x64。0.50.14 Beta 的两个 NSIS 候选用于本轮测试。使用本节安装包及其校验文件，A16 使用下方 0.31.0 合成基线，逐项记录所测版本。[平台优先级](adr/0003-Windows优先交付.md) [开发进度](../开发进度.md)

- [0.50.14 常规安装候选](../artifacts/Zhiye-0.50.14-win-x64-setup.exe) [SHA-256](../artifacts/Zhiye-0.50.14-win-x64-setup.exe.sha256)
- [0.50.14 离线安装候选](../artifacts/Zhiye-0.50.14-win-x64-offline-setup.exe) [SHA-256](../artifacts/Zhiye-0.50.14-win-x64-offline-setup.exe.sha256)

使用可交互的 Windows x64 测试账户和合成资料。A18 的参考设备为 16 GiB 内存和本机 SSD。把安装包、测试应用和验收数据放到本机磁盘，记录实际 Windows 版本、CPU、内存、磁盘和 WebView2 版本。验收证据与版本决策按原计划第 9 章保存。[首版验收场景](../个人资源管理系统_完整开发Plan_v2.0.md#91-首版必须完成的场景)

整套文件和逐项校验清单可随验收 ZIP 转移到测试机。[0.50.14 Windows 验收包](../artifacts/Zhiye-0.50.14-windows-acceptance.zip) [解包与校验](Windows验收包.md)

## 0.50.14 Beta 候选安装包

0.50.14 在 Windows 加载阶段短暂聚焦空白输入框，以初始化默认新版微软拼音。常规包已通过实际标题和正文输入，离线包通过 27 项桌面检查、引用切页专项和缺 WebView2 的断网首装。两个安装器从同一冻结源码构建。[拼音复验](../.verify/windows-0514/复验.md) [离线复验](../.verify/windows-offline-0514/复验.md)

0.50.13 修复选择页面引用后立刻切页时写错页的问题。常规及离线包在已有 WebView2 的断网 Windows 客机通过实装、引用切页回归与卸载。宿主用两包解出的程序各通过 27 项含键盘的 WebView2 检查。[同版复验](../.verify/windows-0513/复验.md)

0.50.12 修复备份面板的 Windows 路径显示。使用「最近成功」显示的共享路径，可以直接在系统文件窗口选择恢复包。

0.50.11 使用紧凑页面清单，复验启动时分别记录原始窗口和追踪刷新。在十万页测试中核对原始编辑器、初始行数及从搜索打开末页。

0.50.10 的 A16 复验补充旧备份清理重试。用共享读句柄占用到期旧 ZIP，核对告警和两份备份；释放句柄后复用当天备份并清理到期旧包。

覆盖时主程序文件无法写入，安装器会停止并返回非零退出码。解除文件占用后重跑安装包，核对程序版本、卸载登记和旧加载器。

0.50.9 的 A16 复验覆盖状态日志写入失败、新包登记和旧包保留。在隔离测试资料中验证解除故障后可继续恢复，并核对既有页面修订。

0.50.8 纳入容量错误提示与自动备份告警。A03 核对草稿另存和重试，A16 核对备份状态日志不可写时的告警、旧备份保留及修复后重试。

两份候选包在 Windows x64 测试机使用 MSVC 构建，目标为 Windows 10 及以上 x64，安装范围为当前用户。两包使用同版 x64 源码构建，应用文件各有独立指纹。WebView2 加载器静态链接在应用内。两份知页安装程序及应用均未签名。0.50.1 的安装、覆盖及协议记录可用于与本版逐项对照。[本版应用指纹](../.verify/windows-0513/candidate-audit.log) [历史安装与系统启动复验](../.verify/windows-install-0501/复验.md)

0.50.2 修复 0.50.1 覆盖升级的旧加载器遗留。安装器写入新版应用后，按既有安装登记和完整 SHA-256 清理旧 DLL。若旧 DLL 正被占用，可释放文件后重跑安装器。[修复原理和回归场景](../.verify/windows-loader-cleanup/复验.md)

0.50.6 优化 PDF 索引状态与全库文本搜索。按 A05 核对命中顺序、属性筛选、PDF 第 2 页和重建索引，使用固定夹具核对十万页末页定位。[搜索及索引复验](../.verify/windows-search-scale/复验.md)

自 0.50.7 起，验收包携带与锁定 Tauri CLI 匹配的 NSIS 及插件，连同二者的源代码和许可。构建入口支持按目标目录带入工具。原生 WebView2 探针使用 Node HTTP 读取回环目标，超时会记录端点与最近的发现状态。[从包内工具重建](Windows验收包.md#从源码重建两种安装包)

0.50.5 启用本机 API 时会跳过 Fetch 禁用端口。记录界面显示的监听地址，用 Node SDK 实际读取合成资料，并在关闭 API 后核对端口停止服务。[Fetch 端口复验](../.verify/windows-fetch-port/复验.md)

0.50.4 在覆盖时有限重试读取新版应用的版本及访问旧 WebView2 加载器。Restart Manager 返回 350 后，通过新会话确认目标文件已空闲才继续。卸载时主程序无法删除会保留卸载入口，文件释放后可重试。[旧加载器复验](../.verify/windows-loader-version/复验.md) [卸载复验](../.verify/windows-uninstall-sharing/复验.md) [运行检查复验](../.verify/windows-restart-manager/复验.md)

0.50.3 纳入保存失败告警的辅助功能修复。通过 UI Automation 核对错误正文可读取，重试、草稿导出及其他操作独立可访问。实际讲述人按系统输入与恢复步骤验收。[告警回归](../.verify/a02-windows-0502/复验.md#保存失败告警)

- 机器缺少 WebView2 Runtime 时，常规安装程序联网获取微软引导程序。[常规包](../artifacts/Zhiye-0.50.14-win-x64-setup.exe) [SHA-256](../artifacts/Zhiye-0.50.14-win-x64-setup.exe.sha256)
- 机器缺少 WebView2 Runtime 时，离线安装程序运行随包的微软 x64 Evergreen 独立安装程序。包内运行时哈希与已验证微软主签名及时间戳的文件一致。[离线包](../artifacts/Zhiye-0.50.14-win-x64-offline-setup.exe) [SHA-256](../artifacts/Zhiye-0.50.14-win-x64-offline-setup.exe.sha256) [微软离线部署](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution#offline-deployment)

在项目根目录用 PowerShell 核对安装包。验证常规包时替换 `$candidate` 的路径，保留对应的校验文件。

```powershell
$candidate = '.\artifacts\Zhiye-0.50.14-win-x64-offline-setup.exe'
$expected = ((Get-Content -LiteralPath "$candidate.sha256") -split '\s+')[0]
$actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $candidate).Hash
if ($actual -ne $expected) { throw '安装包 SHA-256 不匹配。' }
```

## A17 断网安装

用专用 Windows x64 测试账户或快照准备两种初始状态。一份缺少 WebView2 Runtime，另一份已有可用版本。将离线包和校验文件复制到本机磁盘，核对哈希，再断开外网并保留本机回环。

在安装前后分别运行下面的命令，保存机器级和当前用户的 Runtime 版本。启动安装器并记录退出码、应用程序路径和文件哈希。在缺少 Runtime 的快照中，预期首次安装后出现有效版本；已有 Runtime 的快照中，预期复用现有版本。两次都保持断网，用开始菜单启动知页，进入下文的安装后离线验收。

只有有效且大于 `0.0.0.0` 的 `pv` 值表示已安装。机器级值为空或为零版本时继续检查当前用户。如果运行时安装出现错误，记录安装器详情、退出码和两处注册表读数。出现启动失败、非零返回码或安装后仍无有效版本时，应在复制知页应用文件前停止。[微软的运行时检测规则](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution#detect-if-a-webview2-runtime-is-already-installed)

```powershell
$webView2Guid = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
$runtimeKeys = @(
  "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\$webView2Guid"
  "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$webView2Guid"
  "HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$webView2Guid"
)
Get-ItemProperty -Path $runtimeKeys -Name pv -ErrorAction SilentlyContinue |
  Select-Object PSPath, pv
```

## 干净构建与安装

从源码 ZIP 解包到新目录。将根目录的 `Cargo.lock` 和 `package-lock.json` 与 `SOURCE-MANIFEST.json` 中的 SHA-256 核对。按离线构建指南准备同架构工具链和锁定缓存。离线源码构建时保留本机回环，以便运行接口用例。

```powershell
npm.cmd ci --offline --no-audit --no-fund
.\scripts\windows-dev.ps1 test
.\scripts\windows-dev.ps1 build
```

按交接步骤在 Windows 从同一份归档源码构建常规和离线 NSIS 安装包。记录两次打包的退出状态、输出路径和 SHA-256。提前预热同版本 Tauri 所需的 NSIS 工具和插件，并将已核验的 WebView2 独立运行时提供给离线构建入口。[两种安装包的重建步骤](Windows验收包.md#从源码重建两种安装包) [离线构建与缓存](离线构建与验收.md)

在干净的测试账户或快照中安装候选包，留存安装结果、安装位置和应用文件哈希。在安装目录核对 `LICENSE`、`THIRD_PARTY_NOTICES.md` 和 `licenses` 下的八份许可文件。通过开始菜单启动，确认产品和版本。下列命令读取注册的协议处理程序。使用测试账户的默认数据目录运行以下两次协议启动，以启用 Windows 单实例转发。[第三方组件清单](../THIRD_PARTY_NOTICES.md) [定位链接操作](定位链接.md)

```powershell
(Get-Item -LiteralPath 'Registry::HKEY_CLASSES_ROOT\zhiye\shell\open\command').GetValue('')
```

从「页面操作 → 页面链接」复制一个已有测试页的 `zhiye://` 地址。先完全退出应用，在 PowerShell 执行以下命令，核对启动后进入目标页。随后保留此窗口并打开另一个页面，再执行 `Start-Process -FilePath $location`，核对已有进程把窗口切回目标页。

```powershell
$location = [string](Get-Clipboard -Raw)
$location = $location.Trim()
if ($location -notmatch '^zhiye://') { throw '请先复制测试页的知页链接。' }
Start-Process -FilePath $location
```

把 `$installed` 设置为实际安装目录中的 `zhiye-desktop.exe`。用原安装文件启动隔离的验收资料，执行 WebView2 编辑、重启、附件和键盘场景。更新的验收脚本支持通过 `--scratch` 把报告、截图、WebView2 配置和合成资料集中到本机目录。运行报告的 `testDirectory` 给出实际位置。省略该选项时使用当前目录下的 `.verify\desktop-*`。[0.50.1 验收工具补充包](../artifacts/Zhiye-0.50.1-windows-install-tools.zip)

```powershell
Get-FileHash -Algorithm SHA256 -LiteralPath $installed
node .\scripts\verify-desktop.mjs $installed --installed --keyboard --scratch 'C:\zhiye-test\desktop results'
```

安装前后可运行只读取证脚本，记录安装包哈希、WebView2 注册版本、应用路径和版本、开始菜单快捷方式、协议命令及许可清单。采集安装后状态时可以传入实际安装退出码和候选应用指纹。覆盖升级时用两份报告核对路径，断网场景仍按 A17 记录实际隔离方式。[工具用法](../.verify/windows-install-tools/随包使用.md) [安装状态采集脚本](../scripts/observe-windows-install.ps1)

在测试页中用「页面操作 → 页面外观」改图标、导入本地 PNG 并保存封面，退出再打开。随后选择「无封面」，在附件列表核对原件，再从历史恢复有封面的修订。导出 Markdown 并核对相对图片路径。旧库升级时一并核对页面 ID、原有正文和升级前的恢复包。[页面数据格式](数据格式.md#工作空间)

用包含折叠、公式、页面引用和附件的测试页导出 Markdown。核对提示中的配套目录、报告中的源页修订及块 ID，按相对路径读取附件并用 `Get-FileHash -Algorithm SHA256` 对照报告。打开报告里的源链接，确认回到对应页或块。在记录页从两张图片中手选封面，随后打开默认画廊和指定附件属性的画廊，核对各自显示的图片。[通用导出](数据格式.md#通用导出) [画廊](数据库视图.md#画廊)

对同一页另存 HTML，把正文文件和配套目录一起移到独立目录，断网从浏览器打开。核对表格、封面、本地原件和源引用。在「备份与恢复」分别勾选与取消「内容包包含修订历史」，导出后把两个包恢复到独立空间。比较两份恢复的当前正文与原件，并核对选中的历史版本。[内容包格式](数据格式.md#原生工作空间包)

从包含同一批页面的已恢复空间运行「合并内容包」。核对新分支、原记录与副本的独立 ID，并从副本打开页链接、块引用和附件。再合并一份校验失败的测试包，核对分支和历史数量，重开窗口后展开最近合并查看批次结果。[合并操作](故障恢复.md#合并到当前工作空间)

## 系统输入与恢复

0.50.12 常规包已在无网卡 Windows 10 客机使用微软拼音兼容模式完成标题、正文的原生取消与确认后立即切页。0.50.13 补测默认新版，记事本和空白 WebView2 文本框可用空格提交，现有标题和预填文本框的组字仍待修复。0.50.12 系统讲述人读出页面树、搜索和任务控件及保存失败告警，录音和实际修订均已归档。[兼容模式](../.verify/a02-windows-ime/复验.md) [默认模式](../.verify/a02-windows-0513/复验.md) [讲述人取证](../.verify/a02-windows-narrator/复验.md)

0.50.14 常规专项候选在同一客机用默认新版微软拼音通过 Windows 原生点击后输入。取消保留原修订，空格确认后立即切页分别提交标题第 3 版和正文第 4 版，重开保留内容和块 ID。[常规专项安装包](../artifacts/Zhiye-0.50.14-win-x64-setup.exe) [SHA-256](../artifacts/Zhiye-0.50.14-win-x64-setup.exe.sha256) [同版复验](../.verify/windows-0514/复验.md)

- 用 Windows 中文输入法在标题和正文中分别组字并停留一秒。核对「输入法组字中」以及历史修订。各取消一次和确认一次，确认后立即切页，再检查正文、块 ID、焦点及新增修订。随后测试混排粘贴、移动、撤销和重开。使用 Windows「讲述人」检查树、搜索结果、模态窗口和保存错误的播报。[键盘复验](键盘与外观.md#复验)
- 在任务清单中创建一个任务和嵌套子项，再添加一项首段仅含页面引用的任务。用讲述人核对各复选框的中文名称和勾选状态。给子项改名后确认只更新该子项的名称，按空格切换并重开检查保存结果。[任务清单键盘行为](键盘与外观.md)
- 用独立测试库核对进程中断重开、保存失败提示和草稿导出。可用 Core 的 `sqlite_full_rolls_back_body_history_and_pending_index_then_allows_retry` 在测试数据库中施加 SQLite 的容量上限，核对已提交正文、历史、索引和重试结果。[保存故障处理](故障恢复.md) [容量故障用例](../crates/core/src/store/recovery.rs)
- 在独立测试库归档带子页和附件的分支。用键盘验证清空确认的初始焦点和 Esc 取消，再确认清空，核对保留页面、源块不可用提示及旧备份可恢复。用备有已满七天标记的合成隔离库测试清理，检查原件移除及共享原件保留。[清空回收站](故障恢复.md#清空回收站与清理附件原件)

## A03 保存失败与草稿恢复

在独立小容量 NTFS 测试卷使用合成资料库，另备一个有足够空间的草稿输出目录。创建含正文、页面引用的测试页，先保存并记下修订号。只在测试卷放置占位文件，直到 Windows 报告空间不足。在原编辑器修改标题和正文，按 `Ctrl+S`，核对保存失败、原修订与历史保持，以及 `Alt+F4` 后草稿仍在。

0.50.12 常规包在断网 Windows 10 客机的独立 NTFS 虚拟卷分别验证卷实际写满和 64 MiB 用户硬配额耗尽。两种条件均完成系统草稿另存、重试、强制重开和明确放弃。配额场景应分别记录调用测试账户的可用配额和卷的实际剩余空间。[卷写满](../.verify/windows-ntfs-capacity/复验.md#a03-保存失败与草稿恢复) [用户配额](../.verify/windows-ntfs-capacity/复验.md#a03-用户磁盘配额)

通过「导出草稿」另存到另一张卷，核对未提交的标题、正文、引用和基准修订。移除测试占位文件后点击「重试」，确认只新增一条修订。保存成功后结束对应测试进程，再打开同一资料库核对正文与历史。0.50.12 A03 补充包包含独立卷操作、候选指纹和只读 SQLite 取证脚本。[0.50.12 A03 补充包](../artifacts/Zhiye-0.50.12-windows-A03.zip) [SHA-256](../artifacts/Zhiye-0.50.12-windows-A03.zip.sha256) [解包核对](../.verify/a03-windows-0512/复验.md)

## A06 附件原件与复制失败

0.50.12 常规包在断网 Windows 10 客机的独立 NTFS 虚拟卷完成第 4 步。真实空间不足时保留原页及原件，释放容量后重试、另存逐文件核对及重开均通过。目标路径占用场景见历史复验。[0.50.12 独立卷复验](../.verify/windows-ntfs-capacity/复验.md#a06-附件复制与重试) [路径占用复验](../.verify/windows-storage-0502/复验.md#a06-附件与原件)

将合成素材 ZIP 展开到本机测试目录。先按 `SHA256SUMS.txt` 用 `Get-FileHash -Algorithm SHA256` 核对四份原件。在测试账户的知页中新建测试页。[合成素材](../.verify/a06-current/a06-synthetic-inputs.zip) [ZIP SHA-256](../.verify/a06-current/a06-synthetic-inputs.zip.sha256)

1. 点「附件 → 添加附件」后取消，核对列表为空。再选 `original.bin`、`preview.png` 和 `two-pages.pdf`，检查三个条目。重选 `original.bin`，核对「已在此页面」提示和附件数量。
2. 打开图片，再打开 PDF 并翻到第 2 页，核对「赤铜配比」字样。用正文工具栏「插入 → 附件块」引用 `original.bin`，等「已保存」。在附件列表改名为 `renamed.bin`，从正文块打开它，检查「此文件可另存为原件」提示。
3. 用系统另存面板将原件写到工作空间目录之外，核对输出与 `original.bin` 的 SHA-256。退出再重开安装版，重新打开正文块与附件列表。[0.33.0 原生验证结果](../.verify/a06-current/native-summary.json)
4. 在独立的容量受限本机测试卷上，通过进程的 `ZHIYE_DATA_DIR` 指向合成工作空间。把输入原件放在另一个卷。将测试卷的可用空间控制在 4 MiB 左右，选择约 16 MiB 的 `retry-after-space.bin`，记录空间不足提示和原有附件。释放测试卷空间后重选同一个文件，确认附件和正文引用均可重开。将重试得到的原件另存并核对哈希。[本轮容量和状态证据](../.verify/a06-current/native-summary.json)

## A15 迁移与导入报告

0.50.0 常规交付候选已使用下列合成素材复验系统文件、目录选择与另存。四份 TXT 报告及四份原件校验记录可用于复跑对照。[实测记录和复跑入口](../.verify/a15-windows-050/复验.md#系统选择与另存)

将合成 ZIP 展开到本机测试目录，按 `SHA256SUMS.txt` 核对素材。用当前候选的测试工作空间创建「接收分支」和一个已有数值属性「分数」的数据库。保存应用实际生成的报告。[合成素材](../.verify/a15-current/a15-synthetic-inputs.zip) [SHA-256](../.verify/a15-current/a15-synthetic-inputs.zip.sha256)

1. 从「接收分支 → 页面操作 → 导入资料到此处」选 Notion ZIP。先选 `notion-bad\notion-sample.zip`，核对此分支没有新增页面，并记录重开导入窗口显示的失败原因。重试 `notion-good\notion-sample.zip`，应生成一个子分支。核对完整 CSV 与 Markdown 正文合并，以及未转换的 HTML 和源 ZIP 均可从报告打开。
2. 从同一接收分支选「Obsidian 资料库目录」，选中 `obsidian-vault`。核对报告中的同名歧义和缺页两处失配、可解析的页面链接、图片和留存的 `草图.canvas`。保存 TXT 并核对原件哈希。
3. 从数值属性数据库的「页面操作 → 导入 CSV」选择 `csv-bad\records-sample.csv`。保存失败报告，核对未闭合引号和未新增记录。选择 `csv-good\records-sample.csv` 追加，核对两条记录及数值 `12.5`。第 3 行是无效数值，第 4 行是空标题，第 5 行的列数不足。通过报告的「查看原件」另存 CSV，核对 SHA-256。关闭原件预览，确认回到原导入报告，再保存 TXT 并核对附件 ID 和页面 ID。

## A16 覆盖升级与独立恢复

0.50.14 常规包已在断网 Windows 10 客机通过 0.31.0 原位覆盖、两种备份阻塞保全源库、格式 18 到 21 升级及两份独立恢复。0.50.13 常规版保存第 4 版后原位覆盖到 0.50.14 离线版，安装前后 SQLite 指纹一致，重开读回同一修订和原件。[0.50.14 验收记录](../.verify/windows-upgrade-0514/复验.md)

0.50.13 常规包已在断网 Windows 10 客机跑通本节的 0.31.0 覆盖、两种升级备份阻塞、格式 18 到 21 迁移及两份独立恢复。离线包另通过 0.50.12 原位覆盖。[同版验收报告](../.verify/windows-upgrade-0513/复验.md)

在专用 Windows 测试账户中使用同一安装位置覆盖升级，记录两次安装的退出状态、应用路径、文件版本和 SHA-256。本节当前候选为 0.50.14，旧版基线为 0.31.0。安装程序和应用均未签名。覆盖后额外核对安装目录中的旧动态加载器已清理。0.50.1 实测过两处备份故障、格式 18 到 21 的数据升级和系统窗口恢复前后两包。[0.31.0 安装包](../artifacts/Zhiye-0.31.0-win-x64-setup.exe) [0.31.0 SHA-256](../artifacts/Zhiye-0.31.0-win-x64-setup.exe.sha256) [0.50.14 安装包](../artifacts/Zhiye-0.50.14-win-x64-setup.exe) [0.50.14 SHA-256](../artifacts/Zhiye-0.50.14-win-x64-setup.exe.sha256) [0.50.1 安装覆盖](../.verify/windows-install-0501/复验.md#覆盖升级与独立恢复)

### 准备独立的升级资料

在本机 SSD 上创建独立数据根。整个演练在同一个 PowerShell 会话中通过 `Start-Process -FilePath` 启动实际安装目录里的 `zhiye-desktop.exe`，让测试进程继承 `ZHIYE_DATA_DIR`。

```powershell
$a16Case = Join-Path $env:LOCALAPPDATA ('Zhiye-A16-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $a16Case -ErrorAction Stop | Out-Null
$a16Data = Join-Path $a16Case 'workspaces'
$env:ZHIYE_DATA_DIR = $a16Data
```

用 0.31.0 从「备份与恢复 → 从备份恢复」打开合成整库包。核对「演练目录 → 升级前正文」、当前第 2 版和附件「演练原件.txt」。原件 SHA-256 为 `01db3fd1f99e54310e4d8ce00e1830efc310e76f019f1c7fccbce0ade273346e`。为该页创建合成只读授权并验证一次读取。从「页面操作 → 页面链接」复制页面地址，完全退出。把实际安装目录复制到 `$a16Case` 下的 `old-app`，用于稍后的旧版读取检查。[合成整库包](../artifacts/Zhiye-0.31.0-A16-synthetic.zhiye-recovery.zip) [整库包 SHA-256](../artifacts/Zhiye-0.31.0-A16-synthetic.zhiye-recovery.zip.sha256) [只读授权复验](本机API.md#从源码复验)

```powershell
$a16Link = [string](Get-Clipboard -Raw)
if ($a16Link.Trim() -notmatch '^zhiye://open/workspaces/([0-9a-f-]{36})/pages/([0-9a-f-]{36})$') {
    throw '请复制「升级前正文」的页面链接。'
}
$a16Workspace = [guid]::Parse($Matches[1]).ToString()
$a16Page = [guid]::Parse($Matches[2]).ToString()
$a16Db = Join-Path $a16Data "$a16Workspace\workspace.sqlite"
$a16Backups = Join-Path $a16Data 'upgrade-backups'
$a16Before = (Get-FileHash -Algorithm SHA256 -LiteralPath $a16Db).Hash
function Assert-A16SourceHash {
    $wal = "$a16Db-wal"
    if ((Test-Path -LiteralPath $wal) -and ((Get-Item -LiteralPath $wal).Length -gt 0)) {
        throw '仍有 WAL 数据，请确认测试应用已退出。'
    }
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $a16Db).Hash -ne $a16Before) {
        throw '升级故障后源库哈希不同，请保存现场。'
    }
}
Assert-A16SourceHash
```

### 触发故障并在同一源库重试

在首次启动候选版前，先在这套测试资料中创建名为 `upgrade-backups` 的普通文件。按同一账户与安装位置覆盖安装候选版，再用本节 PowerShell 启动。在「无法打开的工作空间」记录 `$a16Workspace`、错误和「备份与恢复」入口。退出应用，运行 `Assert-A16SourceHash`。把该工作空间目录复制到另一处测试数据根，可用 `old-app` 对着副本核对旧版能读到原正文。完成后将 `ZHIYE_DATA_DIR` 设回 `$a16Data`。

```powershell
New-Item -ItemType File -Path $a16Backups -Value 'A16 synthetic directory obstruction' -ErrorAction Stop | Out-Null
```

关闭两个版本的测试进程。把第一处阻塞文件移出数据根，然后在升级备份目录里用一个同名空目录阻塞整库 ZIP。启动候选版，再记录同一工作空间的错误。完全退出后运行 `Assert-A16SourceHash`，核对 `$a16Snapshot` 已生成，`$a16Zip` 仍是演练创建的空目录。

```powershell
Move-Item -LiteralPath $a16Backups -Destination (Join-Path $a16Case 'directory-obstruction.txt') -ErrorAction Stop
New-Item -ItemType Directory -Path $a16Backups -ErrorAction Stop | Out-Null
$a16Zip = Join-Path $a16Backups "$a16Workspace-v18.zhiye-recovery.zip"
$a16Snapshot = Join-Path $a16Backups "$a16Workspace-v18.sqlite"
New-Item -ItemType Directory -Path $a16Zip -ErrorAction Stop | Out-Null
```

移走 ZIP 同名的空目录，保留 SQLite 快照，再启动候选版。确认 `$a16Page` 对应的层级、正文、历史和原件可读，并记录升级前 SQLite 和整库包的 SHA-256。此时 `$a16Zip` 应为可恢复的 ZIP 文件。[Windows 0.50.0 故障与重试](../.verify/a16-windows-050/upgrade-report.json)

```powershell
if (!(Test-Path -LiteralPath $a16Zip -PathType Container) -or (Get-ChildItem -LiteralPath $a16Zip -Force)) {
    throw '需要本轮创建的空目录，保留当前现场并核对路径。'
}
Move-Item -LiteralPath $a16Zip -Destination (Join-Path $a16Case 'zip-obstruction') -ErrorAction Stop
```

### 核对升级后新增内容与旧版保护

1. 把原标题和正文更新为「升级后新增正文」，等「已保存」后在 `$a16Case` 另存新整库包。在候选版分别恢复升级前的整库包和新整库包。两个新空间应分别含第 2 和第 3 版，页、块、历史及原件可逐项核对。重开源空间时仍可读取第 3 版。[恢复操作](故障恢复.md#工作空间升级失败)
2. 用 0.31.0 时创建的令牌读取源空间。两份恢复空间应拒绝同一令牌。记录 HTTP 状态和对象 ID，结束后撤销合成授权。[只读授权复验](本机API.md#从源码复验)
3. 关闭候选版，把 `$a16Data` 内的 `$a16Workspace` 目录复制到 `$a16Case` 下的独立回滚测试根，并记录该副本数据库的 SHA-256。将该 PowerShell 的 `ZHIYE_DATA_DIR` 改为回滚测试根，再启动 `old-app` 中的 0.31.0。预期显示「此版本的知页不能打开该工作空间」。退出后复算副本哈希，核对一致。把 `ZHIYE_DATA_DIR` 设回 `$a16Data`，用候选版再次确认源空间第 3 版。[旧版拒绝及前后恢复证据](../.verify/a16-windows-050/upgrade-report.json)

### 核对当日自动恢复包损坏后的替换

0.50.2 实装版在同一 UTC 日期完成手动和后台替换，两个新恢复包均经系统文件窗口独立恢复并读回第 2 版。[实装证据](../.verify/windows-storage-0502/复验.md#a16-当日备份替换)

在与升级演练分开的合成工作空间中生成当天的自动恢复包。从「备份与恢复 → 最近成功」记录实际路径、SHA-256、文件长度和修改时间。用下列专项测试复现同长度损坏，并保留测试日志。

```powershell
cargo.exe test --offline --locked -p zhiye-core --lib same_length_corrupt_snapshot_does_not_count_as_todays_backup
```

安装版复验时，用隔离数据根重新生成当天的合成恢复包。在知页退出后只改动该测试包内 SQLite 条目的字节，保持文件长度。手动分支的测试包还应恢复原修改时间。重开后执行「立即运行备份」，记录新恢复包的路径和哈希，核对旧文件仍存在，并把新包恢复到独立空间。旧包损坏、最新保留点能恢复当天页面以及较早备份保留的证据与 A16 一起保存。[当日恢复包的校验](故障恢复.md#当日恢复包的校验)

## 安装后离线验收

0.50.2 离线 NSIS 已在有 WebView2 和默认网络路由的测试机复验千页、六视图、内容恢复、HTTP／SDK／MCP 授权及退出停服。独立断网环境按下列步骤复跑。[已安装的联测证据](../.verify/windows-library-0502/复验.md)

退出应用，断开外网并保留 `127.0.0.1`，记录实际隔离方式。重新启动安装版，创建、编辑、重开并搜索页面，核对六视图中的同一条记录，导入原件及预览 PDF 第 2 页。完成整库包的独立恢复，再启用本机 API。用 SDK 和 MCP 读取合成的授权内容并复查拒绝授权范围外的内容。关闭应用后复核监听已停。脚本的 `blockedExternalRequests` 记录 WebView2 在验收中的外部请求观察结果，实际断网结果写入同一轮记录。[离线运行流程](离线构建与验收.md#离线运行验收) [本机接口复验](本机API.md#从源码复验)

## 固定数据与压力复验

在 Windows 参考机用待测版本的锁定源码运行以下命令。把 `C:\zhiye-acceptance-050` 换成本机 SSD 上尚不存在的结果目录。性能流程使用带探针的 Debug 构建。编译完成后单独开始测量，避免构建和桌面交互占用同一台机器。

```powershell
.\scripts\windows-dev.ps1 perf-build
node .\scripts\performance\windows.mjs 'C:\zhiye-acceptance-050'
```

应用进程使用独立的 WebView2 资料目录。脚本保存原生冷启动、回环 HTTP、实际 WebView2 交互和十万页压力的原始样本。`windows-performance.json` 记录锁文件、可执行文件及数据指纹、每步退出状态和五项 p95 阈值结果。按 9.2 记录本机 SSD 及内存，并保存失败或未执行的场景。[测量口径与报告](性能验收.md) [Playwright WebView2 指南](https://playwright.dev/docs/webview2)
