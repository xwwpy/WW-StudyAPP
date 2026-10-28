#requires -Version 7.0
[CmdletBinding()]
param([Parameter(Mandatory)][string]$EvidenceDirectory)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or -not $IsWindows -or -not $env:RUNNER_TEMP) {
    throw 'Pinyin preparation is restricted to the isolated GitHub Windows runner'
}
$imeIdentity = [Security.Principal.WindowsIdentity]::GetCurrent()
$imePrincipal = [Security.Principal.WindowsPrincipal]::new($imeIdentity)
if (-not $imePrincipal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Official Windows capability servicing requires the runner administrator token'
}
$imeRunner = (Resolve-Path -LiteralPath $env:RUNNER_TEMP).Path.TrimEnd('\')
$imeEvidence = [IO.Path]::GetFullPath($EvidenceDirectory)
$imeParent = Split-Path -Parent $imeEvidence
if (-not $imeParent.StartsWith("$imeRunner\", [StringComparison]::OrdinalIgnoreCase) -or
    -not (Test-Path -LiteralPath $imeParent -PathType Container) -or (Test-Path -LiteralPath $imeEvidence)) {
    throw 'Preparation requires a fresh directory below an isolated RUNNER_TEMP child'
}
for ($imeAncestor = Get-Item -LiteralPath $imeParent; $null -ne $imeAncestor; $imeAncestor = $imeAncestor.Parent) {
    if ($imeAncestor.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Reparse points are not allowed in IME evidence paths' }
    if ($imeAncestor.FullName -eq $imeRunner) { break }
}
New-Item -ItemType Directory -Path $imeEvidence | Out-Null
$imeCapability = 'Language.Basic~~~zh-CN~0.0.1.0'
$imeTip = '0804:{81D4E9C9-1D3B-41BC-9E6C-4B40BF79E35E}{FA550B04-5AD7-411F-A5AC-CA038EC515D7}'
$imeWatch = [Diagnostics.Stopwatch]::StartNew()
$imeReport = [ordered]@{
    capability = $imeCapability; tip = $imeTip; source = 'Windows capability servicing with the official Windows Update source'
    os = [Environment]::OSVersion.VersionString; image = $env:ImageVersion
    before = $null; after = $null; install = $null; status = 'incomplete'; error = $null
}
function Test-ImeChineseTag {
    param([string]$Tag)
    return $Tag -in @('zh-CN', 'zh-Hans-CN')
}
function Get-ImeLanguageRecords {
    param($Languages)
    # WinUserLanguageList can be emitted as one non-enumerated pipeline object.
    # foreach explicitly visits each language instead of projecting array properties.
    foreach ($imeLanguage in $Languages) {
        [ordered]@{
            language = [string]$imeLanguage.LanguageTag
            tips = @(foreach ($imeInputTip in $imeLanguage.InputMethodTips) { [string]$imeInputTip })
        }
    }
}
function Test-ImePreservedLanguageOrder {
    param([string[]]$Before, [string[]]$After)
    if ($After.Count -lt $Before.Count) { return $false }
    for ($imeIndex = 0; $imeIndex -lt $Before.Count; $imeIndex++) {
        if ($Before[$imeIndex] -ine $After[$imeIndex] -and
            -not ((Test-ImeChineseTag -Tag $Before[$imeIndex]) -and (Test-ImeChineseTag -Tag $After[$imeIndex]))) {
            return $false
        }
    }
    return $true
}
function Add-ImePinyinProfile {
    param($Languages, [string]$Tip, [scriptblock]$NewLanguage)
    $imeOrder = @(foreach ($imeLanguage in $Languages) { [string]$imeLanguage.LanguageTag })
    $imeChinese = @(foreach ($imeLanguage in $Languages) {
        if (Test-ImeChineseTag -Tag $imeLanguage.LanguageTag) { $imeLanguage }
    })
    $imeModified = $false
    if ($imeChinese.Count -eq 0) {
        $imeCreatedLanguage = & $NewLanguage
        if (-not (Test-ImeChineseTag -Tag $imeCreatedLanguage.LanguageTag)) { throw 'New Chinese language profile has an unexpected tag' }
        $Languages.Add($imeCreatedLanguage)
        $imeChinese = @($imeCreatedLanguage)
        $imeModified = $true
    }
    if ($imeChinese.Count -ne 1) { throw 'Chinese language profile is ambiguous' }
    if ($Tip -notin $imeChinese[0].InputMethodTips) {
        $imeChinese[0].InputMethodTips.Add($Tip)
        $imeModified = $true
    }
    return [pscustomobject]@{ Changed = $imeModified; OriginalOrder = $imeOrder }
}
function Get-ImeSetupInventory {
    $imeState = Get-WindowsCapability -Online -Name $imeCapability -ErrorAction Stop
    if (@($imeState).Count -ne 1 -or $imeState.Name -ne $imeCapability) { throw 'Capability inventory is ambiguous' }
    return [ordered]@{
        state = [string]$imeState.State
        languages = @(Get-ImeLanguageRecords -Languages (Get-WinUserLanguageList))
    }
}
try {
    $imeReport.before = Get-ImeSetupInventory
    $imeReport | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $imeEvidence 'setup-before.json') -Encoding utf8
    if ($imeReport.before.state -ne 'Installed') {
        # Omitting Source and LimitAccess lets Windows select the matching official source.
        # Never change update policy, reset update caches, download a guessed CAB, or reboot.
        $imeInstall = Add-WindowsCapability -Online -Name $imeCapability -LogPath (Join-Path $imeEvidence 'capability-install.log') -ErrorAction Stop
        $imeReport.install = [ordered]@{ restartNeeded = $imeInstall.RestartNeeded; online = $imeInstall.Online }
        if ($imeInstall.RestartNeeded -ne $false) { throw 'Chinese capability installation requires a restart; this run cannot prove live IME behavior' }
    }
    $imeInstalled = Get-WindowsCapability -Online -Name $imeCapability -ErrorAction Stop
    if ($imeInstalled.State -ne 'Installed') { throw "Chinese Basic capability is unavailable: $($imeInstalled.State)" }
    $imeLanguages = Get-WinUserLanguageList
    $imeProfileChange = Add-ImePinyinProfile -Languages $imeLanguages -Tip $imeTip -NewLanguage {
        $imeCreatedList = New-WinUserLanguageList -Language 'zh-CN'
        $imeCreatedList[0]
    }
    $imeOriginalOrder = $imeProfileChange.OriginalOrder
    if ($imeProfileChange.Changed) { Set-WinUserLanguageList -LanguageList $imeLanguages -Force -ErrorAction Stop }
    $imeReport.after = Get-ImeSetupInventory
    $imeAfterOrder = @(foreach ($imeLanguage in $imeReport.after.languages) { $imeLanguage.language })
    if (-not (Test-ImePreservedLanguageOrder -Before $imeOriginalOrder -After $imeAfterOrder)) {
        throw 'Original user language order was not preserved'
    }
    $imeAfterChinese = @(foreach ($imeLanguage in $imeReport.after.languages) {
        if (Test-ImeChineseTag -Tag $imeLanguage.language) { $imeLanguage }
    })
    if ($imeReport.after.state -ne 'Installed' -or $imeAfterChinese.Count -ne 1 -or $imeTip -notin $imeAfterChinese[0].tips) {
        throw 'Official Chinese Basic capability and Microsoft Pinyin TIP were not verified'
    }
    $imeReport.status = 'prepared'
    Write-Host 'Official Pinyin capability and language TIP are prepared; native composition must still pass independently.'
} catch {
    $imeReport.status = 'failed'
    $imeReport.error = $_.Exception.Message
    throw
} finally {
    try { $imeReport.after = Get-ImeSetupInventory } catch { $imeReport['inventoryError'] = $_.Exception.Message }
    $imeReport['elapsedSeconds'] = [math]::Round($imeWatch.Elapsed.TotalSeconds, 2)
    $imeReport | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $imeEvidence 'setup-result.json') -Encoding utf8
}
