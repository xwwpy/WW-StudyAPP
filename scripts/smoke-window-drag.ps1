#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string]$DataDirectory,
    [string]$BinaryPath = (Join-Path $PSScriptRoot '../target/release/studyapp.exe')
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$dragBinary = (Resolve-Path -LiteralPath $BinaryPath).Path
$dragData = [IO.Path]::GetFullPath($DataDirectory)
if (Test-Path -LiteralPath $dragData) { throw 'Window drag smoke requires a fresh isolated directory' }
$dragPriorScenario = $env:WW_STUDYAPP_SMOKE_SCENARIO
$dragPriorTrace = $env:WW_STUDY_WINDOW_DRAG_TRACE
try {
    $env:WW_STUDYAPP_SMOKE_SCENARIO = 'window-drag'
    $env:WW_STUDY_WINDOW_DRAG_TRACE = $dragData + '.jsonl'
    $dragProcess = Start-Process -FilePath $dragBinary -ArgumentList @('--smoke-test', '--data-dir', ('"' + $dragData + '"')) -PassThru -WindowStyle Hidden -RedirectStandardOutput ($dragData + '.stdout.log') -RedirectStandardError ($dragData + '.stderr.log')
    Write-Output ('Isolated window drag PID: ' + $dragProcess.Id)
    if (-not $dragProcess.WaitForExit(60000)) {
        # This is the exact process launched above, never an enumerated user app.
        if (-not $dragProcess.HasExited) { $dragProcess.Kill() }
        throw ('Window drag smoke exceeded 60 seconds; stopped isolated process PID: ' + $dragProcess.Id)
    }
    if ($dragProcess.ExitCode -ne 0) {
        Get-Content -LiteralPath ($dragData + '.stderr.log') -Tail 30
        throw ('Window drag smoke exit code: ' + $dragProcess.ExitCode)
    }
    $dragReport = Get-Content -LiteralPath (Join-Path $dragData 'window-drag-result.txt') -Raw
    if ($dragReport -notmatch '^PASS:') { throw 'Window drag smoke report did not pass' }
    # Caption double-clicks can enter a zero-move modal loop. Keep those raw
    # records but compare only the three deliberately sampled trajectories.
    $dragTrace = @(Get-Content -LiteralPath ($dragData + '.jsonl') | ForEach-Object { $_ | ConvertFrom-Json } | Where-Object { $_.moves -ge 30 })
    if ($dragTrace.Count -ne 3) { throw 'Expected one actual native modal loop per page' }
    $dragControls = Get-Content -LiteralPath (Join-Path $dragData 'window-controls-result.txt') -Raw
    if ($dragControls -notmatch '^PASS:') { throw 'Custom titlebar control smoke report did not pass' }
    foreach ($dragPage in @('today', 'notes', 'settings')) {
        $dragEnvironment = Get-Content -LiteralPath (Join-Path $dragData ('window-drag-' + $dragPage + '-samples.environment.json')) -Raw | ConvertFrom-Json
        if ($dragEnvironment.effectiveDragFullWindows -ne $true -or $dragEnvironment.restored -ne $true -or $dragEnvironment.restoredDragFullWindows -ne $dragEnvironment.originalDragFullWindows) {
            throw ('Full-window drag fixture did not enable and restore the desktop option: ' + $dragPage)
        }
        Write-Output ($dragPage + ' original DragFullWindows=' + $dragEnvironment.originalDragFullWindows + '; restored=' + $dragEnvironment.restored)
    }
    Write-Output $dragReport.Trim()
    $dragTrace | Select-Object moves, renderedFrames, elapsedMicros, drawMicros, drawP95Micros, drawMaxMicros
    Write-Output ('Evidence: ' + $dragData)
} finally {
    $env:WW_STUDYAPP_SMOKE_SCENARIO = $dragPriorScenario
    $env:WW_STUDY_WINDOW_DRAG_TRACE = $dragPriorTrace
}
