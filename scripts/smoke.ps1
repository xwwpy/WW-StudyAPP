#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$DataDirectory,
    [string]$BinaryPath = (Join-Path $PSScriptRoot '../target/debug/studyapp.exe')
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$binary = (Resolve-Path -LiteralPath $BinaryPath).Path
$data = [IO.Path]::GetFullPath($DataDirectory)
if (Test-Path -LiteralPath $data) { throw 'Smoke directory must be fresh; no existing fixtures are changed' }
$stdout = "$data.stdout.log"
$stderr = "$data.stderr.log"
$process = Start-Process -FilePath $binary -ArgumentList @('--smoke-test', '--data-dir', ('"' + $data + '"')) -PassThru -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr
# Includes real input, all settings categories, independent drafts and save-view
# preservation. Keep a bounded deadline for the expanded sequential scenario.
if (-not $process.WaitForExit(480000)) { $process.Kill(); throw 'Native smoke exceeded 480 seconds' }
if (Test-Path -LiteralPath $data -PathType Container) {
    Copy-Item -LiteralPath $stdout -Destination (Join-Path $data 'smoke.stdout.log')
    Copy-Item -LiteralPath $stderr -Destination (Join-Path $data 'smoke.stderr.log')
}
if ($process.ExitCode -ne 0) {
    Get-Content -LiteralPath $stderr -Tail 50
    throw "Native smoke exit code: $($process.ExitCode)"
}
$inputMode = (Get-Content -LiteralPath (Join-Path $data 'smoke-input-mode.txt') -Raw).Trim()
if ($env:GITHUB_ACTIONS -eq 'true' -and $inputMode -ne 'WIN32_SENDINPUT') { throw 'CI must verify actual Windows keyboard input' }
$report = Join-Path $data 'smoke-result.txt'
if (-not (Test-Path -LiteralPath $report)) { throw 'Smoke report missing' }
if ((Get-Content -LiteralPath $report -Raw) -notmatch '^PASS:') { throw 'Smoke report did not pass' }
$libraryReport = Join-Path $data 'library-smoke-result.txt'
if ((Get-Content -LiteralPath $libraryReport -Raw) -notmatch '^PASS:') { throw 'Library and history smoke report did not pass' }
$interactionsReport = Join-Path $data 'interactions-smoke-result.txt'
if ((Get-Content -LiteralPath $interactionsReport -Raw) -notmatch '^PASS:') { throw 'Note interactions and scroll smoke report did not pass' }
$workspaceReport = Join-Path $data 'workspace-smoke-result.txt'
if ((Get-Content -LiteralPath $workspaceReport -Raw) -notmatch '^PASS:') { throw 'Adjustable workspace smoke report did not pass' }
$imageReport = Join-Path $data 'native-image-smoke-result.txt'
if ((Get-Content -LiteralPath $imageReport -Raw) -notmatch '^PASS:') { throw 'Managed image smoke report did not pass' }
$settingsReport = Join-Path $data 'settings-capture-smoke-result.txt'
if ((Get-Content -LiteralPath $settingsReport -Raw) -notmatch '^PASS:') { throw 'Settings categories and shortcut recording smoke report did not pass' }
$draftsReport = Join-Path $data 'native-multiple-drafts-result.txt'
if ((Get-Content -LiteralPath $draftsReport -Raw) -notmatch '^PASS:') { throw 'Multiple independent drafts smoke report did not pass' }
$saveViewReport = Join-Path $data 'native-save-view-result.txt'
if ((Get-Content -LiteralPath $saveViewReport -Raw) -notmatch '^PASS:') { throw 'Saved note viewport smoke report did not pass' }
foreach ($nativeImage in @('native-first-reader.bmp', 'native-first-editor.bmp', 'native-after-history.bmp', 'native-after-menu.bmp', 'native-after-settings.bmp', 'native-after-resize.bmp', 'multiple-drafts-native.bmp')) {
    $nativePath = Join-Path $data $nativeImage
    if (-not (Test-Path -LiteralPath $nativePath) -or (Get-Item -LiteralPath $nativePath).Length -lt 1000) {
        throw "Native screen capture missing or empty: $nativeImage"
    }
}
$closeDraftId = (Get-Content -LiteralPath (Join-Path $data 'native-close-draft-id.txt') -Raw).Trim()
if ($closeDraftId -cnotmatch '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$') { throw 'Close draft identity is invalid' }
$draft = Get-Content -LiteralPath (Join-Path $data ('drafts/' + $closeDraftId + '.json')) -Raw | ConvertFrom-Json
if ($draft.schema -ne 2 -or $draft.id -cne $closeDraftId -or $draft.status.kind -cne 'active' -or $draft.revision -lt 1) { throw 'Normal window close did not retain the exact active draft record' }
if ($draft.draft.content -cne '关闭窗口前的末次输入 🧠') { throw 'Normal window close did not persist the last editor input' }
foreach ($index in 0..4) {
    $image = Join-Path $data "page-$index.png"
    if (-not (Test-Path -LiteralPath $image)) { throw "Screenshot missing: $image" }
    if ((Get-Item -LiteralPath $image).Length -lt 1000) { throw "Screenshot is empty: $image" }
}
Write-Output "Native smoke passed; evidence retained at $data"
