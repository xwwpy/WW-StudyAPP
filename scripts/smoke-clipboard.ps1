#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$DataDirectory,
    [string]$BinaryPath = (Join-Path $PSScriptRoot '../target/debug/studyapp.exe')
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$clipboardBinary = (Resolve-Path -LiteralPath $BinaryPath).Path
$clipboardData = [IO.Path]::GetFullPath($DataDirectory)
if (Test-Path -LiteralPath $clipboardData) { throw 'Clipboard smoke requires a fresh directory; existing files are never changed' }
$clipboardStdout = "$clipboardData.stdout.log"
$clipboardStderr = "$clipboardData.stderr.log"
if ((Test-Path -LiteralPath $clipboardStdout) -or (Test-Path -LiteralPath $clipboardStderr)) { throw 'Clipboard smoke log paths must also be fresh' }
$clipboardOldScenario = [Environment]::GetEnvironmentVariable('WW_STUDYAPP_SMOKE_SCENARIO', 'Process')
$clipboardProcess = $null
try {
    $env:WW_STUDYAPP_SMOKE_SCENARIO = 'clipboard'
    $clipboardProcess = Start-Process -FilePath $clipboardBinary -ArgumentList @('--smoke-test', '--data-dir', ('"' + $clipboardData + '"')) -PassThru -WindowStyle Hidden -RedirectStandardOutput $clipboardStdout -RedirectStandardError $clipboardStderr
    $clipboardDeadline = [DateTime]::UtcNow.AddSeconds(180)
    while (-not $clipboardProcess.WaitForExit(1000)) {
        if ([DateTime]::UtcNow -ge $clipboardDeadline) {
            # This exact process was created above; never stop a process by name.
            $clipboardProcess.Kill()
            throw 'Isolated clipboard smoke exceeded 180 seconds'
        }
    }
    if ($clipboardProcess.ExitCode -ne 0) {
        Get-Content -LiteralPath $clipboardStderr -Tail 40
        throw "Isolated clipboard smoke exit code: $($clipboardProcess.ExitCode)"
    }
    $clipboardEvidence = Get-Content -LiteralPath (Join-Path $clipboardData 'clipboard-evidence.json') -Raw | ConvertFrom-Json
    if ($clipboardEvidence.input -cne 'WIN32_SENDINPUT_CTRL_V' -or -not $clipboardEvidence.plainText -or -not $clipboardEvidence.nativeRendered) { throw 'Real native clipboard input/rendering proof missing' }
    if ($clipboardEvidence.images.Count -ne 3) { throw 'Expected PNG, DIB and DIBV5 evidence' }
    foreach ($clipboardFormat in @('PNG', 'CF_DIB', 'CF_DIBV5')) {
        $clipboardEntry = @($clipboardEvidence.images | Where-Object { $_.source -ceq $clipboardFormat })
        if ($clipboardEntry.Count -ne 1 -or -not $clipboardEntry[0].trustedPaste -or -not $clipboardEntry[0].durableAssets -or -not $clipboardEntry[0].nativeFormats.$clipboardFormat) { throw "Native $clipboardFormat proof missing" }
    }
    $clipboardScreenshot = Join-Path $clipboardData 'native-clipboard.bmp'
    if (-not (Test-Path -LiteralPath $clipboardScreenshot) -or (Get-Item -LiteralPath $clipboardScreenshot).Length -lt 1000) { throw 'Native clipboard screenshot missing or empty' }
    $clipboardSavedNote = Get-Content -LiteralPath $clipboardEvidence.savedNote -Raw
    if ([regex]::Matches($clipboardSavedNote, 'studyapp-media:').Count -ne 3 -or -not $clipboardSavedNote.Contains('剪贴板纯文本 fixture')) { throw 'Saved clipboard note proof missing' }
    Write-Output "Native PNG/DIB/DIBV5 clipboard smoke passed; evidence retained at $clipboardData"
}
finally {
    [Environment]::SetEnvironmentVariable('WW_STUDYAPP_SMOKE_SCENARIO', $clipboardOldScenario, 'Process')
    if (Test-Path -LiteralPath $clipboardData -PathType Container) {
        if (Test-Path -LiteralPath $clipboardStdout) { Copy-Item -LiteralPath $clipboardStdout -Destination (Join-Path $clipboardData 'smoke.stdout.log') }
        if (Test-Path -LiteralPath $clipboardStderr) { Copy-Item -LiteralPath $clipboardStderr -Destination (Join-Path $clipboardData 'smoke.stderr.log') }
    }
}
