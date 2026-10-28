#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$DataDirectory,
    [string]$BinaryPath = (Join-Path $PSScriptRoot '../target/debug/studyapp.exe')
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'Native rename regression runs only on the isolated GitHub CI desktop' }
$renameBinary = (Resolve-Path -LiteralPath $BinaryPath).Path
$renameData = [IO.Path]::GetFullPath($DataDirectory)
$renameStdout = "$renameData.stdout.log"
$renameStderr = "$renameData.stderr.log"
foreach ($renamePath in @($renameData, $renameStdout, $renameStderr)) {
    if (Test-Path -LiteralPath $renamePath) { throw 'Rename regression requires fresh data and log paths' }
}
$renameOldScenario = [Environment]::GetEnvironmentVariable('WW_STUDYAPP_SMOKE_SCENARIO', 'Process')
try {
    $env:WW_STUDYAPP_SMOKE_SCENARIO = 'note-rename'
    $renameProcess = Start-Process -FilePath $renameBinary -ArgumentList @('--smoke-test', '--data-dir', ('"' + $renameData + '"')) -PassThru -WindowStyle Hidden -RedirectStandardOutput $renameStdout -RedirectStandardError $renameStderr
    if (-not $renameProcess.WaitForExit(120000)) {
        $renameProcess.Kill()
        throw 'Isolated rename regression exceeded 120 seconds'
    }
    if ($renameProcess.ExitCode -ne 0) {
        Get-Content -LiteralPath $renameStderr -Tail 80
        throw "Rename regression exit code: $($renameProcess.ExitCode)"
    }
    $renameEvidence = Get-Content -LiteralPath (Join-Path $renameData 'native-note-rename-evidence.json') -Raw | ConvertFrom-Json
    foreach ($renameAssertion in @('prefilled', 'escapeCancelled', 'enterPersisted', 'historyAndMarkdownPreserved', 'busyRejected', 'staleLibraryAndRevisionRejected', 'liveDraftPreserved')) {
        if ($renameEvidence.$renameAssertion -cne $true) { throw "Rename assertion failed: $renameAssertion" }
    }
    if (-not $renameEvidence.targetNoteId -or -not $renameEvidence.selectedNoteId -or $renameEvidence.targetNoteId -eq $renameEvidence.selectedNoteId) {
        throw 'Rename regression did not exercise a distinct target and selection'
    }
    Write-Output "Native note rename passed; evidence retained at $renameData"
}
finally {
    [Environment]::SetEnvironmentVariable('WW_STUDYAPP_SMOKE_SCENARIO', $renameOldScenario, 'Process')
    if (Test-Path -LiteralPath $renameData -PathType Container) {
        if (Test-Path -LiteralPath $renameStdout) { Copy-Item -LiteralPath $renameStdout -Destination (Join-Path $renameData 'smoke.stdout.log') }
        if (Test-Path -LiteralPath $renameStderr) { Copy-Item -LiteralPath $renameStderr -Destination (Join-Path $renameData 'smoke.stderr.log') }
    }
}
