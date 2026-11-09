#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidatePattern('^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$')]
    [string]$Version,
    [string]$BinaryPath,
    [string]$OutputDirectory,
    [switch]$IncludeModern,
    [switch]$IncludeInstallers,
    [switch]$BuildLifecycleFixtures,
    [string]$UiTestBinaryPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($IncludeInstallers -and ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows')) { throw 'Installer packaging runs only on the Windows CI runner' }
if ($BuildLifecycleFixtures -and -not $IncludeInstallers) { throw 'Lifecycle fixtures require -IncludeInstallers' }
if (($IncludeModern -or $IncludeInstallers) -and $Version -notmatch '^\d+\.\d+\.\d+$') { throw 'Modern packages require a three-field release version' }
$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if (-not $BinaryPath) { $BinaryPath = Join-Path $repoRoot 'target/release/studyapp.exe' }
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $repoRoot 'dist' }
$binary = (Resolve-Path -LiteralPath $BinaryPath).Path
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw 'BinaryPath must name one existing file' }
$output = [IO.Path]::GetFullPath($OutputDirectory)
$name = "WW-StudyApp-$Version-windows-x86_64"
$staging = Join-Path $output $name
$archive = Join-Path $output "$name.zip"
$checksum = "$archive.sha256"
foreach ($path in @($staging, $archive, $checksum)) {
    if (Test-Path -LiteralPath $path) { throw "Output already exists; choose a fresh OutputDirectory: $path" }
}

function Initialize-PackageDependencyCache {
    # Notices include optional dependencies that a prior application build may not cache.
    $target = 'x86_64-pc-windows-msvc'
    $PSNativeCommandUseErrorActionPreference = $false
    $null = cargo fetch --locked --offline --target $target 2>&1
    if ($LASTEXITCODE -eq 0) { return }
    cargo fetch --locked --target $target
    if ($LASTEXITCODE -ne 0) { throw 'Locked Cargo dependency fetch failed; packaging requires the Windows dependency sources' }
}

Push-Location -LiteralPath $repoRoot
try {
    Initialize-PackageDependencyCache
    # Filter to the supported binary target, including all features for a conservative notice set.
    $metadata = cargo metadata --locked --offline --all-features --filter-platform x86_64-pc-windows-msvc --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Locked Cargo metadata failed; build the Windows application first' }
    $workspace = @($metadata.packages | Where-Object { $_.id -in $metadata.workspace_members })
    if (@($workspace | Where-Object { $_.version -cne $Version }).Count -gt 0) { throw 'Version differs from workspace package versions' }
    [IO.Directory]::CreateDirectory($staging) | Out-Null
    Copy-Item -LiteralPath $binary -Destination (Join-Path $staging 'WW-StudyApp.exe')
    foreach ($file in @('LICENSE')) {
        Copy-Item -LiteralPath (Join-Path $repoRoot $file) -Destination (Join-Path $staging $file)
    }
    Copy-Item -LiteralPath (Join-Path $repoRoot 'docs/release-notes.md') -Destination (Join-Path $staging 'RELEASE-NOTES.md')
    . (Join-Path $PSScriptRoot 'windows-runtime.ps1')
    $runtime = Get-WindowsRuntime
    $imports = @(Assert-WindowsRuntimeImports $binary $runtime)
    if ('vcruntime140.dll' -notin $imports) { throw 'Expected a dynamically linked Visual C++ runtime' }
    foreach ($dll in $runtime.Files) {
        $source = Join-Path $runtime.Directory $dll
        $null = Assert-WindowsRuntimeImports $source $runtime
        Copy-Item -LiteralPath $source -Destination (Join-Path $staging $dll)
    }
    $readme = @"
# WW-StudyApp $Version

解压整个压缩包，双击 WW-StudyApp.exe 启动。请将 DLL 文件保留在程序旁边。

支持 Windows 10/11 x64。Markdown 编辑器需要 Microsoft Edge WebView2 Runtime。

使用说明：https://github.com/xwwpy/WW-StudyAPP/blob/v$Version/docs/user-guide.md
版本更新：https://github.com/xwwpy/WW-StudyAPP/releases
WebView2：https://developer.microsoft.com/microsoft-edge/webview2/

更新程序不会改变你选择的资料库位置。
"@
    [IO.File]::WriteAllText((Join-Path $staging 'README.md'), $readme, [Text.UTF8Encoding]::new($false))
    $noticePath = Join-Path $staging 'THIRD-PARTY-NOTICES.txt'
    python (Join-Path $PSScriptRoot 'third-party-notices.py') --target x86_64-pc-windows-msvc --output $noticePath
    if ($LASTEXITCODE -ne 0) { throw 'Third-party license notice collection failed' }
    $runtimeNotice = "Microsoft Visual C++ Runtime`nCopyright (c) Microsoft Corporation. All rights reserved.`nRuntime files: $($runtime.Files -join ', ')`nLicense terms: https://visualstudio.microsoft.com/license-terms/`nRedistribution: https://learn.microsoft.com/cpp/windows/redistributing-visual-cpp-files`n`n"
    [IO.File]::WriteAllText($noticePath, $runtimeNotice + [IO.File]::ReadAllText($noticePath), [Text.UTF8Encoding]::new($false))
    # ZipFile accepts literal directory names; it does not interpret wildcard paths.
    [IO.Compression.ZipFile]::CreateFromDirectory($staging, $archive, [IO.Compression.CompressionLevel]::Optimal, $true)
    & (Join-Path $PSScriptRoot 'check-package.ps1') -ArchivePath $archive -Version $Version
    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText($checksum, "$hash  $name.zip`n", [Text.UTF8Encoding]::new($false))
    Write-Output "ZIP: $archive"
    Write-Output "SHA256: $checksum"
    if ($IncludeModern -or $IncludeInstallers) {
        . (Join-Path $PSScriptRoot 'modern-package.ps1')
        $modern = New-ModernPortablePackage -LegacyStaging $staging -Version $Version -OutputDirectory $output
        if ((Get-Item -LiteralPath $archive).Length -gt 256MB -or (Get-Item -LiteralPath $modern.Archive).Length -gt 256MB) { throw 'Portable package exceeds the updater size bound' }
        Write-Output "Modern ZIP: $($modern.Archive)"
        if ($IncludeInstallers) {
            & (Join-Path $PSScriptRoot 'build-installers.ps1') -Version $Version -PayloadDirectory $modern.Staging -OutputDirectory $output -BuildLifecycleFixtures:$BuildLifecycleFixtures -UiTestBinaryPath $UiTestBinaryPath
        }
    }
} finally {
    Pop-Location
}
