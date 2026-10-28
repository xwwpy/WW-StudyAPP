#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$DataDirectory,
    [Parameter(Mandatory)][string]$BinaryPath
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or -not $IsWindows -or -not $env:RUNNER_TEMP) {
    throw 'Real Pinyin tests run only on the isolated GitHub Windows desktop'
}
$imeRunner = (Resolve-Path -LiteralPath $env:RUNNER_TEMP).Path.TrimEnd('\')
$imeData = [IO.Path]::GetFullPath($DataDirectory)
$imeBinary = (Resolve-Path -LiteralPath $BinaryPath).Path
$imeParent = Split-Path -Parent $imeData
foreach ($imePath in @($imeParent, (Split-Path -Parent $imeBinary))) {
    if (-not $imePath.StartsWith("$imeRunner\", [StringComparison]::OrdinalIgnoreCase)) { throw 'IME binary and data must be inside the isolated runner temporary directory' }
    for ($imeAncestor = Get-Item -LiteralPath $imePath; $null -ne $imeAncestor; $imeAncestor = $imeAncestor.Parent) {
        if ($imeAncestor.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Reparse points are not allowed in IME paths' }
        if ($imeAncestor.FullName -eq $imeRunner) { break }
    }
}
if ((Get-Item -LiteralPath $imeBinary).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'IME binary cannot be a reparse point' }
$imeStdout = "$imeData.stdout.log"
$imeStderr = "$imeData.stderr.log"
foreach ($imeFreshPath in @($imeData, $imeStdout, $imeStderr)) {
    if (Test-Path -LiteralPath $imeFreshPath) { throw 'IME diagnostics require fresh data and logs' }
}
$imeMetadata = Get-Content -Raw -LiteralPath (Join-Path (Split-Path -Parent $imeBinary) 'binary-metadata.json') | ConvertFrom-Json
if ($imeMetadata.test_sha -cne $env:GITHUB_SHA -or $imeMetadata.sha -cnotmatch '^[0-9a-f]{40}$' -or
    $imeMetadata.test_sha -cnotmatch '^[0-9a-f]{40}$' -or $imeMetadata.input_fingerprint -cnotmatch '^[0-9a-f]{64}$' -or
    (Get-FileHash -LiteralPath $imeBinary -Algorithm SHA256).Hash.ToLowerInvariant() -cne $imeMetadata.sha256) {
    throw 'Native Pinyin binary does not match the verified executable and current test checkout'
}
$imeOldScenario = [Environment]::GetEnvironmentVariable('WW_STUDYAPP_SMOKE_SCENARIO', 'Process')
$imeOldCdp = [Environment]::GetEnvironmentVariable('WW_STUDYAPP_SMOKE_CDP_INPUT', 'Process')
$imeProcess = $null
try {
    $env:WW_STUDYAPP_SMOKE_SCENARIO = 'inputs-ime'
    [Environment]::SetEnvironmentVariable('WW_STUDYAPP_SMOKE_CDP_INPUT', $null, 'Process')
    $imeProcess = Start-Process -FilePath $imeBinary -ArgumentList @('--smoke-test', '--data-dir', ('"{0}"' -f $imeData)) -PassThru -WindowStyle Hidden -RedirectStandardOutput $imeStdout -RedirectStandardError $imeStderr
    if (-not $imeProcess.WaitForExit(180000)) { throw 'Real Pinyin composition exceeded the 180 second native test deadline' }
    $imeProcess.Refresh()
    if ($imeProcess.ExitCode -ne 0) { throw "Native Pinyin process failed with exit code $($imeProcess.ExitCode)" }
    & python (Join-Path $PSScriptRoot 'ime-ci.py') validate-result --directory $imeData
    if ($LASTEXITCODE -ne 0) { throw 'Real Pinyin evidence failed validation' }
} catch {
    if (Test-Path -LiteralPath $imeStderr) { Get-Content -LiteralPath $imeStderr -Tail 40 | Write-Host }
    throw
} finally {
    if ($null -ne $imeProcess -and -not $imeProcess.HasExited) { $imeProcess.Kill(); $imeProcess.WaitForExit(10000) | Out-Null }
    [Environment]::SetEnvironmentVariable('WW_STUDYAPP_SMOKE_SCENARIO', $imeOldScenario, 'Process')
    [Environment]::SetEnvironmentVariable('WW_STUDYAPP_SMOKE_CDP_INPUT', $imeOldCdp, 'Process')
}
