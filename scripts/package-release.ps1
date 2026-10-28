#Requires -Version 7.0
<#
.SYNOPSIS
Builds the cargo-sweep style Windows release assets for WW-StudyApp:
installer wrapper ZIP, portable ZIP and SHA256SUMS.

The NSIS setup.exe is produced separately by makensis (installer/studyapp.nsi);
this script only wraps it and the portable payload exactly like the reference
project does, and never deletes anything recursively.
#>
param(
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][string]$SetupPath,
    [Parameter(Mandatory)][string]$PayloadDirectory,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [string]$RepositoryRoot = (Split-Path -Parent $PSScriptRoot)
)

$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath($RepositoryRoot).TrimEnd('\')
$setup = [IO.Path]::GetFullPath($SetupPath)
$payload = [IO.Path]::GetFullPath($PayloadDirectory)
$release = [IO.Path]::GetFullPath($OutputDirectory)
$installerReadme = Join-Path $root 'packaging/README-installer.txt'
$portableReadme = Join-Path $root 'packaging/README-portable.txt'

foreach ($required in @($setup, $installerReadme, $portableReadme)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) { throw "Missing packaging input: $required" }
}
if (-not (Test-Path -LiteralPath $payload -PathType Container)) { throw "Missing payload directory: $payload" }
if (-not (Test-Path -LiteralPath (Join-Path $payload 'WW-StudyApp.exe') -PathType Leaf)) { throw 'Payload does not contain WW-StudyApp.exe' }

if (Test-Path -LiteralPath $release) {
    if (-not (Test-Path -LiteralPath $release -PathType Container) -or
        @(Get-ChildItem -LiteralPath $release -Force).Count -ne 0) {
        throw "Output directory must be empty: $release"
    }
}
[void][IO.Directory]::CreateDirectory($release)

$stage = Join-Path $root ('.scratch/nsis-package-' + [guid]::NewGuid().ToString('N'))
$installerStage = Join-Path $stage 'installer'
$portableStage = Join-Path $stage 'portable/WW-StudyApp'
[void][IO.Directory]::CreateDirectory($installerStage)
[void][IO.Directory]::CreateDirectory($portableStage)

$installCmd = @'
@echo off
setlocal
title WW-StudyApp Installer
set "ROOT=%~dp0"
set "TEMP=%ROOT%temp"
set "TMP=%ROOT%temp"
if not exist "%TEMP%" mkdir "%TEMP%"
echo.
echo WW-StudyApp installer
echo Temporary folder: %TEMP%
echo.
for %%F in ("%ROOT%*Setup.exe") do (
  "%%~fF" %*
  set "RC=!errorlevel!"
)
if not "%RC%"=="0" (
  echo.
  echo The installer exited with code %RC%.
  pause
)
exit /b %RC%
'@
$installCmd = $installCmd -replace 'setlocal', 'setlocal EnableDelayedExpansion'
Set-Content -LiteralPath (Join-Path $installerStage 'INSTALL-WW-STUDYAPP.cmd') -Value $installCmd -Encoding ascii

$startCmd = @'
@echo off
setlocal
set "ROOT=%~dp0"
set "TEMP=%ROOT%temp"
set "TMP=%ROOT%temp"
if not exist "%TEMP%" mkdir "%TEMP%"
if not exist "%ROOT%webview-data" mkdir "%ROOT%webview-data"
set "WEBVIEW2_USER_DATA_FOLDER=%ROOT%webview-data"
start "" "%ROOT%WW-StudyApp.exe" %*
timeout /t 2 /nobreak >nul
tasklist /fi "imagename eq WW-StudyApp.exe" | find /i "WW-StudyApp.exe" >nul
if errorlevel 1 (
  echo.
  echo WW-StudyApp did not stay running.
  echo Check that this folder is writable and that WebView2 Runtime is installed.
  pause
)
'@
Set-Content -LiteralPath (Join-Path $portableStage 'START-WW-STUDYAPP.cmd') -Value $startCmd -Encoding ascii

Copy-Item -LiteralPath $setup -Destination $installerStage -Force
Copy-Item -LiteralPath $installerReadme -Destination (Join-Path $installerStage 'README-zh.txt') -Force
Copy-Item -LiteralPath $portableReadme -Destination (Join-Path $portableStage 'README-zh.txt') -Force
foreach ($file in Get-ChildItem -LiteralPath $payload -File) {
    Copy-Item -LiteralPath $file.FullName -Destination (Join-Path $portableStage $file.Name) -Force
}

$setupName = Split-Path -Leaf $setup
$installerZip = Join-Path $release "WW-StudyApp-$Version-installer-x64.zip"
$portableZip = Join-Path $release "WW-StudyApp-$Version-portable-x64.zip"
Compress-Archive -Path (Join-Path $installerStage '*') -DestinationPath $installerZip -CompressionLevel Optimal
Compress-Archive -Path (Join-Path $stage 'portable/WW-StudyApp') -DestinationPath $portableZip -CompressionLevel Optimal

$checksumLines = foreach ($asset in @($setup, $installerZip, $portableZip)) {
    $hash = (Get-FileHash -LiteralPath $asset -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $(Split-Path -Leaf $asset)"
}
[IO.File]::WriteAllLines((Join-Path $release 'SHA256SUMS.txt'), $checksumLines, [Text.Encoding]::ASCII)

# The staging directory lives under the git-ignored .scratch tree; CI runners are
# ephemeral, so it is left for inspection instead of being deleted recursively.

Write-Output "Packaged WW-StudyApp $Version ($setupName)"
Get-ChildItem -LiteralPath $release | Select-Object Name, Length | Format-Table -AutoSize
