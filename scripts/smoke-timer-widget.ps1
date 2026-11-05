#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string]$DataDirectory,
    [string]$BinaryPath = (Join-Path $PSScriptRoot '../target/debug/studyapp.exe')
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'Native timer widget verification runs only on GitHub Windows runners' }
$binary = (Resolve-Path -LiteralPath $BinaryPath).Path
$data = [IO.Path]::GetFullPath($DataDirectory)
if (Test-Path -LiteralPath $data) { throw 'Timer widget smoke requires a fresh isolated directory' }
$prior = $env:WW_STUDYAPP_SMOKE_SCENARIO
try {
    $env:WW_STUDYAPP_SMOKE_SCENARIO = 'timer-widget'
    $process = Start-Process -FilePath $binary -ArgumentList @('--smoke-test', '--data-dir', ('"' + $data + '"')) -PassThru -NoNewWindow -RedirectStandardOutput ($data + '.stdout.log') -RedirectStandardError ($data + '.stderr.log')
    if (-not $process.WaitForExit(90000)) {
        if (-not $process.HasExited) { $process.Kill() }
        throw ('Isolated timer widget smoke timed out; PID: ' + $process.Id)
    }
    if ($process.ExitCode -ne 0) { throw ('Timer widget smoke failed; exit code: ' + $process.ExitCode) }
    $report = Get-Content -LiteralPath (Join-Path $data 'timer-widget-result.json') -Raw | ConvertFrom-Json
    if ($report.pass -ne $true) { throw 'Timer widget native assertions did not pass' }
    Write-Output ('PASS: native timer widget, shared state, drag/resize, transparency, pinning and device geometry; ' + $data)
} finally {
    $env:WW_STUDYAPP_SMOKE_SCENARIO = $prior
}
