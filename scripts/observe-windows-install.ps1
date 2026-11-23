# SPDX-License-Identifier: AGPL-3.0-or-later

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Installer,
    [string]$ExpectedVersion,
    [ValidatePattern('^[A-Fa-f0-9]{64}$')][string]$ExpectedApplicationSha256,
    [Nullable[int]]$InstallerExitCode
)

$ErrorActionPreference = 'Stop'
if (!$IsWindows -and $env:OS -ne 'Windows_NT') { throw 'Windows is required.' }
if (![Environment]::Is64BitProcess) { throw 'Use a 64-bit PowerShell process to read both registry views.' }

function Read-DefaultValue([string]$Path) {
    $key = Get-Item -LiteralPath $Path -ErrorAction SilentlyContinue
    if ($key) { return $key.GetValue('') }
    return $null
}

$file = Get-Item -LiteralPath $Installer
if ($file.PSIsContainer) { throw 'Installer must be a file.' }
$checksum = Get-Content -LiteralPath ($file.FullName + '.sha256') -TotalCount 1 -Encoding UTF8
$expectedPackageHash = ($checksum -split '\s+')[0]
if ($expectedPackageHash -notmatch '^[a-f0-9]{64}$') { throw 'Invalid installer SHA-256 sidecar.' }
$actualPackageHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $file.FullName).Hash.ToLowerInvariant()

$webView2 = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
$registrations = foreach ($path in @(
    "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\$webView2"
    "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$webView2"
    "HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$webView2"
)) {
    $entry = Get-ItemProperty -LiteralPath $path -ErrorAction SilentlyContinue
    $parsed = $null
    $valid = [Version]::TryParse([string]$entry.pv, [ref]$parsed) -and $parsed -gt [Version]'0.0.0.0'
    [ordered]@{ registry = $path; version = $entry.pv; valid = $valid }
}

$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\知页'
$uninstall = Get-ItemProperty -LiteralPath $uninstallKey -ErrorAction SilentlyContinue
$directory = if ($uninstall.InstallLocation) { ([string]$uninstall.InstallLocation).Trim('"') } else { $null }
$application = if ($directory) { Join-Path $directory 'zhiye-desktop.exe' } else { $null }
$appFile = if ($application) { Get-Item -LiteralPath $application -ErrorAction SilentlyContinue }
$appVersion = if ($appFile -and !$appFile.PSIsContainer) { $appFile.VersionInfo.ProductVersion } else { $null }
$appHash = if ($appFile -and !$appFile.PSIsContainer) {
    (Get-FileHash -Algorithm SHA256 -LiteralPath $appFile.FullName).Hash.ToLowerInvariant()
} else { $null }
$protocolRoot = 'HKCU:\Software\Classes\zhiye'
$protocol = Get-Item -LiteralPath $protocolRoot -ErrorAction SilentlyContinue
$registeredCommand = Read-DefaultValue "$protocolRoot\shell\open\command"
$effectiveCommand = Read-DefaultValue 'Registry::HKEY_CLASSES_ROOT\zhiye\shell\open\command'
$expectedCommand = if ($application) { '"' + $application + '" "%1"' } else { $null }
$startMenu = [Environment]::GetFolderPath('Programs')
$shortcuts = @()
if ($directory -and $startMenu -and (Test-Path -LiteralPath $startMenu -PathType Container)) {
    $shell = New-Object -ComObject WScript.Shell
    $shortcuts = @(Get-ChildItem -LiteralPath $startMenu -Filter '知页.lnk' -File -Recurse | ForEach-Object {
        [ordered]@{ path = $_.FullName; target = $shell.CreateShortcut($_.FullName).TargetPath }
    })
}
$licenses = @()
if ($directory -and (Test-Path -LiteralPath $directory -PathType Container)) {
    $licenses = @(foreach ($name in @('LICENSE', 'THIRD_PARTY_NOTICES.md')) {
        if (Test-Path -LiteralPath (Join-Path $directory $name) -PathType Leaf) { $name }
    })
    $licenseDirectory = Join-Path $directory 'licenses'
    if (Test-Path -LiteralPath $licenseDirectory -PathType Container) {
        $licenses += @(Get-ChildItem -LiteralPath $licenseDirectory -File -Name | Sort-Object | ForEach-Object { "licenses/$_" })
    }
}

$checks = [ordered]@{ packageHash = $expectedPackageHash -eq $actualPackageHash }
if ($PSBoundParameters.ContainsKey('InstallerExitCode')) {
    $checks.exitCode = $InstallerExitCode -eq 0
}
if ($ExpectedVersion) {
    $checks.version = $uninstall.DisplayVersion -eq $ExpectedVersion -and $appVersion -eq $ExpectedVersion
    $checks.runtime = @($registrations | Where-Object { $_.valid }).Count -gt 0
    $checks.protocol = $null -ne $protocol -and 'URL Protocol' -in $protocol.GetValueNames() -and
        $null -ne $expectedCommand -and $registeredCommand -eq $expectedCommand -and $effectiveCommand -eq $expectedCommand
    $checks.startMenu = $null -ne $application -and @($shortcuts | Where-Object { $_.target -eq $application }).Count -gt 0
}
if ($ExpectedApplicationSha256) { $checks.applicationHash = $appHash -eq $ExpectedApplicationSha256 }
$failures = @($checks.GetEnumerator() | Where-Object { !$_.Value } | ForEach-Object Key)

[ordered]@{
    recordedAt = [DateTime]::UtcNow.ToString('o')
    installer = [ordered]@{
        path = $file.FullName
        sha256 = $actualPackageHash
        expectedSha256 = $expectedPackageHash
        exitCode = $InstallerExitCode
    }
    webView2 = @($registrations)
    installed = [ordered]@{
        registryVersion = $uninstall.DisplayVersion
        directory = $directory
        application = $application
        applicationVersion = $appVersion
        applicationSha256 = $appHash
        uninstallCommand = $uninstall.UninstallString
        userProtocolCommand = $registeredCommand
        effectiveProtocolCommand = $effectiveCommand
        startMenuShortcuts = $shortcuts
        licenses = $licenses
    }
    checks = $checks
    failedChecks = $failures
} | ConvertTo-Json -Depth 6
if ($failures.Count) { throw "Windows installation observation failed: $($failures -join ', ')." }
