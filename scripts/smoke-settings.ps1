#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$DataDirectory,
    [string]$BinaryPath = (Join-Path $PSScriptRoot '../target/debug/studyapp.exe')
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
# This uses real native input and is deliberately restricted to the isolated CI
# desktop. Local validation must not move the user's pointer or steal focus.
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'Native settings performance verification runs only on the isolated GitHub CI desktop' }
$settingsBinary = (Resolve-Path -LiteralPath $BinaryPath).Path
$settingsData = [IO.Path]::GetFullPath($DataDirectory)
$settingsStdout = "$settingsData.stdout.log"
$settingsStderr = "$settingsData.stderr.log"
foreach ($settingsPath in @($settingsData, $settingsStdout, $settingsStderr)) {
    if (Test-Path -LiteralPath $settingsPath) { throw 'Settings smoke requires fresh data and log paths' }
}
$settingsOldScenario = [Environment]::GetEnvironmentVariable('WW_STUDYAPP_SMOKE_SCENARIO', 'Process')
try {
    $env:WW_STUDYAPP_SMOKE_SCENARIO = 'settings'
    $settingsProcess = Start-Process -FilePath $settingsBinary -ArgumentList @('--smoke-test', '--data-dir', ('"' + $settingsData + '"')) -PassThru -WindowStyle Hidden -RedirectStandardOutput $settingsStdout -RedirectStandardError $settingsStderr
    if (-not $settingsProcess.WaitForExit(240000)) {
        $settingsProcess.Kill()
        throw 'Isolated settings smoke exceeded 240 seconds'
    }
    if ($settingsProcess.ExitCode -ne 0) {
        Get-Content -LiteralPath $settingsStderr -Tail 60
        throw "Isolated settings smoke exit code: $($settingsProcess.ExitCode)"
    }
    if ((Get-Content -LiteralPath (Join-Path $settingsData 'smoke-input-mode.txt') -Raw).Trim() -cne 'WIN32_SENDINPUT') { throw 'Native settings test did not verify Windows keyboard input' }
    if ((Get-Content -LiteralPath (Join-Path $settingsData 'settings-capture-smoke-result.txt') -Raw) -notmatch '^PASS:') { throw 'Settings category and shortcut regression report missing' }
    $null = Get-Content -LiteralPath (Join-Path $settingsData 'settings-render-timing.json') -Raw | ConvertFrom-Json
    Write-Output "Native settings checks passed; performance evidence retained at $settingsData"
}
finally {
    [Environment]::SetEnvironmentVariable('WW_STUDYAPP_SMOKE_SCENARIO', $settingsOldScenario, 'Process')
    if (Test-Path -LiteralPath $settingsData -PathType Container) {
        if (Test-Path -LiteralPath $settingsStdout) { Copy-Item -LiteralPath $settingsStdout -Destination (Join-Path $settingsData 'smoke.stdout.log') }
        if (Test-Path -LiteralPath $settingsStderr) { Copy-Item -LiteralPath $settingsStderr -Destination (Join-Path $settingsData 'smoke.stderr.log') }
    }
}
