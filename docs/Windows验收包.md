# 知页 0.50.14 Windows 验收

把验收 ZIP 复制到 Windows x64 测试机的本地 SSD，按较短的路径解压，例如 `C:\zhiye-test`。进入解压得到的 `Zhiye-0.50.14-windows-acceptance` 目录，使用本包里的 [Windows 验收步骤](Windows验收.md)。

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

- [0.50.14 常规安装包](../artifacts/Zhiye-0.50.14-win-x64-setup.exe)及[内置 WebView2 的离线安装包](../artifacts/Zhiye-0.50.14-win-x64-offline-setup.exe)用于当前候选版验收。
- [A06 附件素材](../.verify/a06-current/a06-synthetic-inputs.zip)和[A15 迁移素材](../.verify/a15-current/a15-synthetic-inputs.zip)在独立测试资料库使用。
- [0.31.0 演练安装包](../artifacts/Zhiye-0.31.0-win-x64-setup.exe)及[合成整库包](../artifacts/Zhiye-0.31.0-A16-synthetic.zhiye-recovery.zip)用于 A16 覆盖升级、故障重试与恢复。

## 以缓存从源码构建

在 Windows x64 安装 Node.js 24、Rust 1.100.0 MSVC 工具链、Visual Studio C++ 构建工具和 Windows SDK。桌面运行还使用 WebView2。验收 ZIP 带有生成 NSIS 安装包所需的工具和模板。[构建前置条件](离线构建与验收.md#windows-x64-离线源码构建)

验收包内的 `dependencies` 目录携带 Windows x64 的 npm 下载缓存和 `x86_64-pc-windows-msvc` 的 Cargo 注册表缓存。以下命令从验收包顶层执行，在单独目录展开 [0.50.14 源码 ZIP](../artifacts/Zhiye-0.50.14-source.zip)。运行测试时保留 `127.0.0.1`。

```powershell
$bundle = (Get-Location).Path
$env:NPM_CONFIG_CACHE = Join-Path $bundle 'dependencies\npm'
$env:CARGO_HOME = Join-Path $bundle 'dependencies\cargo'
$env:NPM_CONFIG_OFFLINE = 'true'
$env:CARGO_NET_OFFLINE = 'true'
Expand-Archive -LiteralPath '.\artifacts\Zhiye-0.50.14-source.zip' -DestinationPath '.\source'
Set-Location -LiteralPath '.\source\Zhiye-0.50.14-source'
npm.cmd ci --offline --no-audit --no-fund
.\scripts\windows-dev.ps1 test
.\scripts\windows-dev.ps1 build
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
```

离线入口校验暂存的运行时 SHA-256，预期为 `d3cbe8fedb104282d745fff106993efead51ae94f5595a4e75804446a974f9c3`。默认分别从 `target\release\bundle\nsis` 和 `target\offline-installer\release\bundle\nsis` 取常规包和离线包。离线入口在指定的 `CARGO_TARGET_DIR` 下使用 `offline-installer` 子目录，相对路径从源码根目录解析。未设置该变量时，当前源码从 Cargo 读取目标目录，支持 `CARGO_BUILD_TARGET_DIR` 和 `.cargo/config.toml` 中的 `build.target-dir`。[NSIS 构建前置条件](离线构建与验收.md#windows-x64-离线源码构建)

## 保存实机证据

在测试目录记录 Windows、CPU、内存、本机磁盘、WebView2、安装包哈希和断网方式。保留安装退出码、失败截图、原生界面验证报告和 `windows-performance.json`。每项注明对应场景以及通过、失败或待执行的状态。[验收步骤及输入](Windows验收.md) [A18 性能口径](性能验收.md)

0.50.1 的验收工具补充包可在解包后的验收源码副本中使用。它提供安装前后只读状态采集，以及指定本机暂存目录的桌面验证入口。[工具补充包](../artifacts/Zhiye-0.50.1-windows-install-tools.zip) [SHA-256](../artifacts/Zhiye-0.50.1-windows-install-tools.zip.sha256) [用法](../.verify/windows-install-tools/随包使用.md)
