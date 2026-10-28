#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$DataDirectory,
    [string]$BinaryPath = (Join-Path $PSScriptRoot '../target/debug/studyapp.exe')
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'Native updater checks run only on the isolated GitHub CI desktop' }
$binary = (Resolve-Path -LiteralPath $BinaryPath).Path
$data = [IO.Path]::GetFullPath($DataDirectory)
$stdout = "$data.stdout.log"
$stderr = "$data.stderr.log"
foreach ($path in @($data, $stdout, $stderr)) {
    if (Test-Path -LiteralPath $path) { throw 'Updater checks require fresh data and log paths' }
}
$previousScenario = [Environment]::GetEnvironmentVariable('WW_STUDYAPP_SMOKE_SCENARIO', 'Process')
try {
    $env:WW_STUDYAPP_SMOKE_SCENARIO = 'updates'
    $process = Start-Process -FilePath $binary -ArgumentList @('--smoke-test', '--data-dir', ('"' + $data + '"')) -PassThru -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    if (-not $process.WaitForExit(120000)) {
        $process.Kill()
        throw 'Isolated updater checks exceeded 120 seconds'
    }
    if ($process.ExitCode -ne 0) {
        Get-Content -LiteralPath $stderr -Tail 80
        throw "Updater checks exit code: $($process.ExitCode)"
    }
    $result = Get-Content -LiteralPath (Join-Path $data 'update-result.json') -Raw | ConvertFrom-Json
    if ($result.startup_checks -ne 1 -or $result.manual_checks -ne 3 -or $result.download_requests -ne 0 -or
        -not $result.quiet_startup_error -or -not $result.available_card_above_version -or
        -not $result.no_idle_checking_error_or_current_card) {
        throw 'Updater checks did not verify the expected interaction contract'
    }
    if ((Get-Item -LiteralPath (Join-Path $data 'update-available.png')).Length -lt 1000) {
        throw 'Updater availability screenshot is missing or empty'
    }
    Write-Output "Native updater checks passed; evidence retained at $data"
} finally {
    [Environment]::SetEnvironmentVariable('WW_STUDYAPP_SMOKE_SCENARIO', $previousScenario, 'Process')
    if (Test-Path -LiteralPath $data -PathType Container) {
        if (Test-Path -LiteralPath $stdout) { Copy-Item -LiteralPath $stdout -Destination (Join-Path $data 'smoke.stdout.log') }
        if (Test-Path -LiteralPath $stderr) { Copy-Item -LiteralPath $stderr -Destination (Join-Path $data 'smoke.stderr.log') }
    }
}
