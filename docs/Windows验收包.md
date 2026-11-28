# 知页 0.50.14 Windows 验收

把验收 ZIP 复制到 Windows x64 测试机的本地 SSD，先核对外层 SHA-256。解压到新的本机 NTFS 目录，使用较短的路径，例如 `C:\zhiye-test`。进入解压得到的 `Zhiye-0.50.14-windows-acceptance` 目录，使用本包里的 [Windows 验收步骤](Windows验收.md)。

0.50.14 PDF 搜索分页修复批次的 Windows 验收 ZIP 汇集两种安装器、冻结源码、锁定依赖、实装回执、A18 工具及补测原始样本。解压后可从包顶层复算随包补测，也可按性能指南核对收到的正式样本。[修复批次实测范围](Windows交接验收.md#pdf-搜索分页修复包) [A18 工具 ZIP](../artifacts/0.50.14-windows-PDF-Pagination/Zhiye-0.50.14-windows-a18.zip) [随包 A18 补测复算](性能验收.md#重算随包补测) [A18 回传复核](性能验收.md#复核回传结果)

同批实装的补充回执随独立 ZIP 提供。两份 ZIP 及各自 `.sha256` 可用补充包内的 `verify.ps1` 配对核验。具体步骤和 A01–A18 入口见补充包的 `开始核对.md`。[补充 ZIP](../artifacts/0.50.14-windows-PDF-Pagination/Zhiye-0.50.14-windows-pagination-evidence.zip) [SHA-256](../artifacts/0.50.14-windows-PDF-Pagination/Zhiye-0.50.14-windows-pagination-evidence.zip.sha256) [核对结果](../.verify/windows-pdf-pagination-evidence-0514/复验.md)

分页修复批次的两种安装包均通过 25 页 PDF 分页、关键词回切、原件第 21 页定位和 27 项桌面与键盘检查。正式 A18 待 16 GiB 实体参考机。[分页专项回执](../.verify/windows-pdf-pagination-0514/audit.json)

无网卡、无 WebView2 的 Windows 10 客机通过常规包断网失败回滚、离线包首装和运行时复用，四次启动关闭及两次卸载均通过。[断网首装回执](../.verify/windows-pdf-pagination-runtime-0514/audit.json)

本批在新建客机验证常规包联网首装和运行时签名，也完成 0.31.0 → 常规包、0.50.13 → 离线包的覆盖升级、备份恢复和资料重开。[联网首装回执](../.verify/windows-pdf-pagination-online-0514/audit.json) [升级与恢复回执](../.verify/windows-pdf-pagination-upgrade-0514/audit.json)

## 文件校验

在验收包顶层开启 PowerShell。用随包清单校验安装包、源码、素材、说明和缓存文件。

```powershell
$bundle = (Get-Location).Path
foreach ($entry in Get-Content -LiteralPath '.\SHA256SUMS.txt' -Encoding UTF8) {
    $expected, $relative = $entry -split '  ', 2
    $path = Join-Path $bundle $relative
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $path -ErrorAction Stop).Hash
    if ($actual -ne $expected) { throw "哈希不匹配 $relative" }
}
```

## 安装与样例

- 使用本批常规安装包和内置 WebView2 的离线安装包实装验收。[常规安装包](../artifacts/0.50.14-windows-PDF-Pagination/Zhiye-0.50.14-win-x64-setup.exe) [离线安装包](../artifacts/0.50.14-windows-PDF-Pagination/Zhiye-0.50.14-win-x64-offline-setup.exe)
- [A06 附件素材](../.verify/a06-current/a06-synthetic-inputs.zip)和[A15 迁移素材](../.verify/a15-current/a15-synthetic-inputs.zip)在独立测试资料库使用。
- [0.31.0 演练安装包](../artifacts/Zhiye-0.31.0-win-x64-setup.exe)及[合成整库包](../artifacts/Zhiye-0.31.0-A16-synthetic.zhiye-recovery.zip)用于 A16 覆盖升级、故障重试与恢复。

## 以缓存从源码构建

在 Windows x64 安装 Node.js 24、Rust 1.100.0 MSVC 工具链、Visual Studio C++ 构建工具和 Windows SDK。桌面运行还使用 WebView2。验收 ZIP 带有生成 NSIS 安装包所需的工具和模板。[构建前置条件](离线构建与验收.md#windows-x64-离线源码构建)

验收包内的 `dependencies` 目录携带 Windows x64 的 npm 下载缓存和 `x86_64-pc-windows-msvc` 的 Cargo 注册表缓存。以下命令从验收包顶层执行，在单独目录展开源码，并在测试时保留 `127.0.0.1`。[0.50.14 源码 ZIP](../artifacts/0.50.14-windows-PDF-Pagination/Zhiye-0.50.14-source.zip)

```powershell
$bundle = (Get-Location).Path
$env:NPM_CONFIG_CACHE = Join-Path $bundle 'dependencies\npm'
$env:CARGO_HOME = Join-Path $bundle 'dependencies\cargo'
$env:NPM_CONFIG_OFFLINE = 'true'
$env:CARGO_NET_OFFLINE = 'true'
Expand-Archive -LiteralPath '.\artifacts\Zhiye-0.50.14-source.zip' -DestinationPath '.\source'
Set-Location -LiteralPath '.\source\Zhiye-0.50.14-source'
npm.cmd ci --offline --no-audit --no-fund
if ($LASTEXITCODE -ne 0) { throw '离线安装 npm 依赖失败。' }
.\scripts\windows-dev.ps1 test
if ($LASTEXITCODE -ne 0) { throw '源码测试失败。' }
.\scripts\windows-dev.ps1 build
if ($LASTEXITCODE -ne 0) { throw '桌面构建失败。' }
```

### 从源码重建两种安装包

从上述命令完成后的源码目录继续，保留 `$bundle` 和依赖缓存设置。从验收包的 `dependencies\nsis` 准备两份构建目录缓存。包内同时保存 NSIS 与 Tauri NSIS 插件的对应源码及许可。较早的验收包可按离线指南准备匹配的本地工具。[构建目录内的 NSIS 工具](离线构建与验收.md#使用构建目录内的-nsis-工具)

```powershell
$nsisAssets = Join-Path $bundle 'dependencies\nsis'
$nsisScratch = Join-Path $bundle "nsis-tools-$([Guid]::NewGuid())"
Expand-Archive -LiteralPath (Join-Path $nsisAssets 'nsis-3.13.zip') -DestinationPath $nsisScratch
$nsis = Join-Path $nsisScratch 'nsis-3.13'
$pluginDir = Join-Path $nsis 'Plugins\x86-unicode\additional'
New-Item -ItemType Directory -Path $pluginDir -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $nsisAssets 'nsis_tauri_utils.dll') -Destination $pluginDir

$env:CARGO_TARGET_DIR = Join-Path (Get-Location).Path 'target'
foreach ($tools in "$env:CARGO_TARGET_DIR\.tauri", "$env:CARGO_TARGET_DIR\offline-installer\.tauri") {
    if (Test-Path -LiteralPath (Join-Path $tools 'NSIS')) { throw "请使用新构建目录。已有缓存 $tools\NSIS" }
    New-Item -ItemType Directory -Path $tools -Force | Out-Null
    Copy-Item -LiteralPath $nsis -Destination (Join-Path $tools 'NSIS') -Recurse
}
```

若已经保存验收包对应的微软 x64 独立运行时，将 `$runtime` 指向该文件。也可以用 PATH 上的 `7z.exe` 从验收包自带的离线安装程序提取。

```powershell
$offlinePackage = Join-Path $bundle 'artifacts\Zhiye-0.50.14-win-x64-offline-setup.exe'
$runtimeDir = Join-Path $bundle "webview2-$([Guid]::NewGuid())"
& 7z.exe e "-o$runtimeDir" $offlinePackage '$PLUGINSDIR/MicrosoftEdgeWebView2RuntimeInstaller.exe'
if ($LASTEXITCODE -ne 0) { throw '提取 WebView2 独立运行时失败。' }
$runtime = Join-Path $runtimeDir 'MicrosoftEdgeWebView2RuntimeInstaller.exe'
```

```powershell
.\scripts\windows-dev.ps1 installer -UseLocalNsisTools
if ($LASTEXITCODE -ne 0) { throw '常规安装包构建失败。' }
.\scripts\windows-dev.ps1 offline-installer -WebView2InstallerPath $runtime -UseLocalNsisTools
if ($LASTEXITCODE -ne 0) { throw '离线安装包构建失败。' }
```

离线入口校验暂存的运行时 SHA-256，预期为 `d3cbe8fedb104282d745fff106993efead51ae94f5595a4e75804446a974f9c3`。默认分别从 `target\release\bundle\nsis` 和 `target\offline-installer\release\bundle\nsis` 取常规包和离线包。离线入口在指定的 `CARGO_TARGET_DIR` 下使用 `offline-installer` 子目录，相对路径从源码根目录解析。未设置该变量时，当前源码从 Cargo 读取目标目录，支持 `CARGO_BUILD_TARGET_DIR` 和 `.cargo/config.toml` 中的 `build.target-dir`。[NSIS 构建前置条件](离线构建与验收.md#windows-x64-离线源码构建)

PDF 分页修复批次用本批交接 ZIP 在无网卡 Windows 10 客机完成独立重建。24 项 JavaScript／TypeScript 与 126 项 Rust Core 测试通过。两种重建安装包各通过 27 项实装检查和卸载，另通过 25 页 PDF 分页专项。[分页修复重建件和回执](../.verify/windows-pdf-pagination-rebuild-0514/复验.md)

此前的 PDF 搜索候选同样用对应验收 ZIP 在无网卡客机完成独立重建。[上一批重建回执](../.verify/windows-pdf-rebuild-0514/复验.md)

0.50.14 在无网卡 Windows 10 客机从归档验收 ZIP 展开新的构建工作目录。核对 1,330 份验收文件和 293 份源码，随包缓存支持离线安装依赖并构建 SDK、MCP 与桌面。24 项 JavaScript／TypeScript 测试和 126 项 Rust Core 测试通过。重建常规与离线安装包，各实装通过 27 项桌面和键盘检查，卸载均退出 0。[0.50.14 独立重建复验](../.verify/windows-rebuild-0514/复验.md)

此前的中英文标准验收 ZIP 在无网卡 Windows 10 客机从空目录复跑，核对 1,331 个验收成员、295 份源码和锁定缓存。重建两种安装器，各通过 27 项桌面和键盘实装检查及卸载。[该批交付的离线源码重建](../.verify/windows-l10n-rebuild-0514/复验.md)

当前卸载提示候选在无网卡 Windows 10 客机用原验收 ZIP 从空目录重建，核对 1,331 个验收成员和 295 份源码。两份重建安装器各通过 27 项实装检查，许可和 WebView2 运行时指纹均已核对。当前交接 ZIP 已逐项确认沿用这次构建的随包输入。[原始构建与实装](../.verify/windows-uninstall-rebuild-0514/复验.md) [交接包输入核对](../.verify/windows-current-build-provenance-0514/复验.md)

## 从项目目录组装验收 ZIP

在 SuperSophie 项目根目录使用标准脚本。用 `--release-dir` 选定同一批源码 ZIP、常规和离线安装器及各自的 SHA-256。分页修复批次的输入位于 `artifacts/0.50.14-windows-PDF-Pagination`。

```powershell
$release = 'artifacts/0.50.14-windows-PDF-Pagination'
$cache = '.verify/windows-offline-0514/dependencies'
$kit = Join-Path $release 'Zhiye-0.50.14-windows-a18.zip'
python -B scripts/package-windows-acceptance.py $cache 'Zhiye-0.50.14-windows-acceptance.zip' --release-dir $release --nsis-tools "$cache/nsis" --a18-kit $kit
```

## 保存实机证据

在测试目录记录 Windows、CPU、内存、本机磁盘、WebView2、安装包哈希和断网方式。保留安装退出码、失败截图、原生界面验证报告和 `windows-performance.json`。每项注明对应场景以及通过、失败或待执行的状态。[验收步骤及输入](Windows验收.md) [A18 性能口径](性能验收.md)

0.50.1 的验收工具补充包可在解包后的验收源码副本中使用。它提供安装前后只读状态采集，以及指定本机暂存目录的桌面验证入口。[工具补充包](../artifacts/Zhiye-0.50.1-windows-install-tools.zip) [SHA-256](../artifacts/Zhiye-0.50.1-windows-install-tools.zip.sha256) [用法](../.verify/windows-install-tools/随包使用.md)
