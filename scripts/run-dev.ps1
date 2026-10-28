#requires -Version 5.1
[CmdletBinding()]
param(
    [switch]$Release,
    [switch]$BuildOnly,
    [switch]$CheckOnly,
    [string]$TargetDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if (-not $env:LOCALAPPDATA) { throw '无法定位当前用户的本地应用目录。' }

# The process and the PE file can each lower the integrity of a child process.
# Keep the workspace policy intact; never relabel it or grant the app extra rights.
$tokenGroups = & (Join-Path $env:SystemRoot 'System32/whoami.exe') /groups /fo csv /nh
if ($LASTEXITCODE -ne 0) { throw '无法检查当前终端的运行权限。请使用桌面上的 PowerShell。' }
$integrity = [regex]::Match(($tokenGroups -join "`n"), 'S-1-16-(\d+)')
if (-not $integrity.Success) { throw '无法确认当前终端的运行权限。请使用桌面上的 PowerShell。' }
if ([int]$integrity.Groups[1].Value -lt 8192) {
    throw '当前终端受限，启动的应用也会受限。请从桌面打开 PowerShell，再运行 scripts/run-dev.ps1；无需修改资料库权限。'
}

if (-not $TargetDirectory) {
    $TargetDirectory = Join-Path $env:LOCALAPPDATA 'WW-StudyApp\dev-target'
}
$target = [IO.Path]::GetFullPath($TargetDirectory)
$repoPrefix = $repoRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
if ($target.Equals($repoRoot, [StringComparison]::OrdinalIgnoreCase) -or $target.StartsWith($repoPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw '构建目录必须位于源码工作区之外，避免生成的 EXE 再次继承受限完整性标记。'
}
if (Test-Path -LiteralPath $target -PathType Leaf) { throw '构建位置必须是目录。' }
$cargo = Get-Command cargo -CommandType Application -ErrorAction Stop
$cargoArguments = @($(if ($BuildOnly) { 'build' } else { 'run' }), '--locked', '-p', 'studyapp-desktop', '--target-dir', $target)
if ($Release) { $cargoArguments += '--release' }
Write-Output "构建位置：$target"
Write-Output '学习资料仍使用应用中已选择的资料库位置。'
if ($CheckOnly) {
    Write-Output '运行环境检查通过；未构建、未启动应用。'
    return
}
Push-Location -LiteralPath $repoRoot
try {
    & $cargo.Source @cargoArguments
    if ($LASTEXITCODE -ne 0) { throw "Cargo 运行失败（退出代码 $LASTEXITCODE）。" }
} finally {
    Pop-Location
}
