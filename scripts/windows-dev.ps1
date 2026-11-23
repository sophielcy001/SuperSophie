param(
    [ValidateSet('test-core', 'test', 'build', 'installer', 'offline-installer', 'dev', 'fetch', 'format', 'perf-build')]
    [string]$Action = 'build',
    [string]$WebView2InstallerPath,
    [switch]$UseLocalNsisTools
)

$ErrorActionPreference = 'Stop'
$project = Split-Path -Parent $PSScriptRoot
if ($UseLocalNsisTools -and $Action -notin 'installer', 'offline-installer') {
    throw '-UseLocalNsisTools 只用于 installer 或 offline-installer。'
}
$runtime = $null
if ($Action -eq 'offline-installer') {
    if (-not $WebView2InstallerPath -or -not (Test-Path -LiteralPath $WebView2InstallerPath -PathType Leaf)) {
        throw '请用 -WebView2InstallerPath 指定已准备好的 WebView2 x64 离线安装程序。'
    }
    $runtime = (Resolve-Path -LiteralPath $WebView2InstallerPath).ProviderPath
}

function Get-CargoTargetDirectory {
    # Tauri reads Cargo metadata from the desktop crate directory.
    $encoding = [Console]::OutputEncoding
    Push-Location -LiteralPath (Join-Path $project 'apps/desktop/src-tauri')
    try {
        [Console]::OutputEncoding = [Text.Encoding]::UTF8
        $metadata = & cargo.exe metadata --no-deps --format-version 1
        if ($LASTEXITCODE -ne 0) { throw '无法读取 Cargo 的构建目标目录。' }
        $directory = ($metadata | ConvertFrom-Json).target_directory
        if (-not $directory -or -not [IO.Path]::IsPathRooted($directory)) {
            throw 'Cargo 未报告有效的构建目标目录。'
        }
        $directory
    } finally {
        [Console]::OutputEncoding = $encoding
        Pop-Location
    }
}

function Assert-LocalNsisTools([string]$Directory) {
    # Match the Tauri CLI version locked in package-lock.json.
    $plugin = 'Plugins/x86-unicode/additional/nsis_tauri_utils.dll'
    $required = @(
        'makensis.exe', 'Bin/makensis.exe', 'Stubs/lzma-x86-unicode', 'Stubs/lzma_solid-x86-unicode',
        'Include/MUI2.nsh', 'Include/FileFunc.nsh', 'Include/x64.nsh', 'Include/nsDialogs.nsh',
        'Include/WinMessages.nsh', 'Include/Win/COM.nsh', 'Include/Win/Propkey.nsh',
        'Include/Win/RestartManager.nsh', 'NSIS.exe', $plugin
    )
    foreach ($name in $required) {
        if (!(Test-Path -LiteralPath (Join-Path $Directory $name) -PathType Leaf)) {
            throw "本地 NSIS 工具缺少 $name。请在 $Directory 准备与 Tauri 版本匹配的工具。"
        }
    }
    $hashes = @{
        'NSIS.exe' = '6d45d4e8ccf18fe1fdac9164058f356102a2ecf2'
        $plugin = '75197fee3c6a814fe035788d1c34ead39349b860'
    }
    foreach ($name in $hashes.Keys) {
        if ((Get-FileHash -LiteralPath (Join-Path $Directory $name) -Algorithm SHA1).Hash -ne $hashes[$name]) {
            throw "本地 NSIS 工具的 $name 指纹不符。请在 $Directory 准备 NSIS 3.13 与 Tauri NSIS 插件 0.5.3。"
        }
    }
}

function Invoke-DesktopInstaller {
    param([string]$RuntimePath)

    $previousMode = $env:ZHIYE_WEBVIEW2_INSTALLER_MODE
    $previousRuntime = $env:ZHIYE_OFFLINE_WEBVIEW2_PATH
    $previousTarget = $env:CARGO_TARGET_DIR
    $staging = $null
    $stagedRuntime = $null
    $localConfig = $null
    try {
        $env:ZHIYE_WEBVIEW2_INSTALLER_MODE = $null
        $env:ZHIYE_OFFLINE_WEBVIEW2_PATH = $null
        if ($RuntimePath -or $UseLocalNsisTools) {
            $targetRoot = if ($previousTarget) {
                $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($previousTarget)
            } else {
                Get-CargoTargetDirectory
            }
            $env:CARGO_TARGET_DIR = if ($RuntimePath) { Join-Path $targetRoot 'offline-installer' } else { $targetRoot }
        }
        if ($UseLocalNsisTools) {
            Assert-LocalNsisTools (Join-Path $env:CARGO_TARGET_DIR '.tauri/NSIS')
        }
        if ($RuntimePath -or $UseLocalNsisTools) {
            $prefix = if ($RuntimePath) { 'zhiye-webview2' } else { 'zhiye-installer' }
            $temporaryPath = Join-Path ([IO.Path]::GetTempPath()) "$prefix-$([Guid]::NewGuid())"
            $staging = [IO.Directory]::CreateDirectory($temporaryPath).FullName
        }
        if ($RuntimePath) {
            $stagedRuntime = Join-Path $staging 'MicrosoftEdgeWebView2RuntimeInstaller.exe'
            Copy-Item -LiteralPath $RuntimePath -Destination $stagedRuntime
            $expectedHash = 'd3cbe8fedb104282d745fff106993efead51ae94f5595a4e75804446a974f9c3'
            if ((Get-FileHash -Algorithm SHA256 -LiteralPath $stagedRuntime).Hash -ne $expectedHash) {
                throw "WebView2 x64 离线安装程序的 SHA-256 不符。请使用验收包内的版本（$expectedHash）。"
            }

            $env:ZHIYE_OFFLINE_WEBVIEW2_PATH = $stagedRuntime
            $env:ZHIYE_WEBVIEW2_INSTALLER_MODE = 'offline'
        }
        if ($UseLocalNsisTools) {
            $localConfig = Join-Path $staging 'tauri-local-tools.json'
            [IO.File]::WriteAllText($localConfig, '{"bundle":{"useLocalToolsDir":true}}')
            $cli = Join-Path $project 'node_modules/@tauri-apps/cli/tauri.js'
            Push-Location -LiteralPath (Join-Path $project 'apps/desktop')
            try {
                & node $cli build --bundles nsis --config $localConfig -- --locked
            } finally {
                Pop-Location
            }
        } else {
            & npm.cmd run desktop:installer --workspace @zhiye/desktop
        }
    } finally {
        $env:ZHIYE_WEBVIEW2_INSTALLER_MODE = $previousMode
        $env:ZHIYE_OFFLINE_WEBVIEW2_PATH = $previousRuntime
        $env:CARGO_TARGET_DIR = $previousTarget
        if ($staging) {
            foreach ($temporaryFile in $stagedRuntime, $localConfig) {
                if ($temporaryFile -and (Test-Path -LiteralPath $temporaryFile)) {
                    Remove-Item -LiteralPath $temporaryFile -Force
                }
            }
            [IO.Directory]::Delete($staging)
        }
    }
}

if (-not (Get-Command cargo.exe -ErrorAction SilentlyContinue)) {
    $localToolchain = Join-Path $env:LOCALAPPDATA 'ZhiyeDevToolchain'
    $localBin = Join-Path $localToolchain 'cargo\bin'
    if (-not (Test-Path -LiteralPath (Join-Path $localBin 'cargo.exe'))) {
        throw '请先按 README 安装 Rust MSVC 工具链。'
    }
    if (-not $env:CARGO_HOME) {
        $env:CARGO_HOME = Join-Path $localToolchain 'cargo'
    }
    if (-not $env:RUSTUP_HOME) {
        $env:RUSTUP_HOME = Join-Path $localToolchain 'rustup'
    }
    $env:Path = "$localBin;$env:Path"
}

# Some Cargo build scripts need the MSVC linker even when launched in a plain shell.
if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
    $finder = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $finder)) { throw '未找到 Visual Studio C++ 构建工具。' }
    $install = & $finder -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if (-not $install) { throw '未找到 Visual Studio C++ 构建工具。' }
    $environmentScript = Join-Path $install 'VC\Auxiliary\Build\vcvars64.bat'
    $variables = cmd.exe /d /c "`"$environmentScript`" >nul && set"
    if ($LASTEXITCODE -ne 0) { throw '初始化 MSVC 环境失败。' }
    foreach ($line in $variables) {
        if ($line -match '^([^=]+)=(.*)$') {
            [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process')
        }
    }
}

Push-Location -LiteralPath $project
try {
    switch ($Action) {
        'test-core' { & cargo.exe test --locked -p zhiye-core }
        'fetch' { & cargo.exe fetch --locked --target x86_64-pc-windows-msvc }
        'format' { & cargo.exe fmt --all -- --check }
        'test' {
            & npm.cmd test
            if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
            & cargo.exe test --locked -p zhiye-core
        }
        'perf-build' {
            & npm.cmd run build:ui
            if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
            & cargo.exe build --locked -p zhiye-core --examples
            if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
            & cargo.exe build --locked -p zhiye-desktop --features custom-protocol,performance-probe
        }
        'build' {
            $programs = @()
            $previousEncoding = [Console]::OutputEncoding
            try {
                [Console]::OutputEncoding = [Text.Encoding]::UTF8
                & npm.cmd run desktop:build --workspace @zhiye/desktop -- --message-format=json-render-diagnostics |
                    ForEach-Object {
                        if ($_ -match '^\{"reason":') {
                            $message = $_ | ConvertFrom-Json
                            if ($message.reason -eq 'compiler-artifact' -and $message.target.name -eq 'zhiye-desktop' -and
                                $message.target.kind -contains 'bin' -and $message.executable) {
                                $programs += $message.executable
                            }
                        } else {
                            Write-Output $_
                        }
                    }
                $buildExit = $LASTEXITCODE
            } finally {
                [Console]::OutputEncoding = $previousEncoding
            }
            if ($buildExit -ne 0) { exit $buildExit }
            if ($programs.Count -ne 1 -or [IO.Path]::GetExtension($programs[0]) -ine '.exe' -or
                -not (Test-Path -LiteralPath $programs[0] -PathType Leaf)) {
                throw 'Cargo 未报告唯一的知页 Windows 可执行文件。'
            }
            $program = $programs[0]
            $config = Join-Path $project 'apps\desktop\src-tauri\tauri.conf.json'
            $version = (Get-Content -LiteralPath $config -Raw -Encoding UTF8 | ConvertFrom-Json).version
            $artifacts = Join-Path $project 'artifacts'
            New-Item -Path $artifacts -ItemType Directory -Force | Out-Null
            Copy-Item -LiteralPath $program -Destination (Join-Path $artifacts 'Zhiye-Demo-win-x64.exe') -Force
            Copy-Item -LiteralPath $program -Destination (Join-Path $artifacts "Zhiye-$version-win-x64.exe") -Force
        }
        'installer' { Invoke-DesktopInstaller }
        'offline-installer' { Invoke-DesktopInstaller -RuntimePath $runtime }
        'dev' { & npm.cmd run dev }
    }
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
    Pop-Location
}
