#Requires -Version 7.0
<#
.SYNOPSIS
Native acceptance for the NSIS installer: interactive directory selection,
silent reinstall using its saved location, shortcuts, uninstall and data retention.

Runs on a disposable Windows runner; it installs into a unique temporary
directory and removes only what it created.
#>
param(
    [Parameter(Mandatory)][string]$SetupPath,
    [Parameter(Mandatory)][string]$Version
)

$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') {
    throw 'Native installer acceptance may run only on a GitHub Windows runner.'
}
$evidence = Join-Path $PSScriptRoot '../.scratch/nsis-acceptance'
[void][IO.Directory]::CreateDirectory($evidence)
Start-Transcript -Path (Join-Path $evidence 'lifecycle.log') -Force
try {
Add-Type -Path (Join-Path $PSScriptRoot 'shortcut-reader.cs')
$setup = (Resolve-Path -LiteralPath $SetupPath).Path
$root = Join-Path $env:RUNNER_TEMP ("nsis-acceptance-" + [guid]::NewGuid().ToString('N'))
$install = Join-Path $root '自选安装目录 with spaces\WW StudyApp'
$data = Join-Path $env:APPDATA 'StudyApp\StudyApp'
$sentinel = Join-Path $data ('nsis-acceptance-sentinel-' + [guid]::NewGuid().ToString('N') + '.txt')
$sentinelText = 'library survived the NSIS lifecycle ' + [guid]::NewGuid().ToString('N')
$arp = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\WW-StudyApp'
$installKey = 'HKCU:\Software\WW-StudyApp\Installer'
$startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\WW-StudyApp'
$desktopShortcut = Join-Path ([Environment]::GetFolderPath('DesktopDirectory')) 'WW-StudyApp.lnk'

[void][IO.Directory]::CreateDirectory($root)
[void][IO.Directory]::CreateDirectory($data)
[IO.File]::WriteAllText($sentinel, $sentinelText, [Text.UTF8Encoding]::new($false))
$sentinelHash = (Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash

function Assert-True([bool]$condition, [string]$message) {
    if (-not $condition) { throw "ASSERTION FAILED: $message" }
    Write-Output "ok - $message"
}

function Invoke-Installer([string[]]$arguments, [string]$label) {
    $process = Start-Process -FilePath $setup -ArgumentList $arguments -Wait -PassThru -WindowStyle Hidden
    Assert-True ($process.ExitCode -eq 0) "$label exits 0 (got $($process.ExitCode))"
}

Write-Output "NSIS acceptance for WW-StudyApp $Version"
Write-Output "install=$install"

# 1. Select a custom destination in the actual NSIS directory page (no /D).
$compiler = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
$probe = Join-Path $root 'nsis-directory-native.exe'
& $compiler /nologo /target:exe "/out:$probe" (Join-Path $PSScriptRoot 'nsis-directory-native.cs')
if ($LASTEXITCODE -ne 0) { throw 'Native directory helper compilation failed.' }
$interactive = Start-Process -FilePath $probe -ArgumentList @(('"' + $setup + '"'), ('"' + $install + '"')) -WindowStyle Hidden -Wait -PassThru -RedirectStandardOutput (Join-Path $evidence 'directory.stdout.log') -RedirectStandardError (Join-Path $evidence 'directory.stderr.log')
Get-Content -LiteralPath (Join-Path $evidence 'directory.stdout.log')
Get-Content -LiteralPath (Join-Path $evidence 'directory.stderr.log')
Assert-True ($interactive.ExitCode -eq 0) "interactive custom directory install exits 0 (got $($interactive.ExitCode))"
foreach ($file in @('WW-StudyApp.exe', 'Uninstall.exe', 'vcruntime140.dll', 'vcruntime140_1.dll', 'README.md',
        'LICENSE', 'RELEASE-NOTES.md', 'THIRD-PARTY-NOTICES.txt')) {
    Assert-True (Test-Path -LiteralPath (Join-Path $install $file)) "installed $file"
}
$entry = Get-ItemProperty -LiteralPath $arp -ErrorAction SilentlyContinue
Assert-True ($null -ne $entry) 'uninstall entry exists'
Assert-True ($entry.DisplayVersion -eq $Version) "uninstall entry reports $Version"
Assert-True ($entry.InstallLocation.TrimEnd('\') -eq $install.TrimEnd('\')) 'uninstall entry points at the install directory'
Assert-True ((Get-ItemProperty -LiteralPath $installKey).InstallLocation.TrimEnd('\') -eq $install.TrimEnd('\')) 'installer saves selected path for updates'
Assert-True ($entry.DisplayIcon -eq (Join-Path $install 'WW-StudyApp.exe')) 'uninstall icon uses selected application executable'
Assert-True (Test-Path -LiteralPath (Join-Path $startMenu 'WW-StudyApp.lnk')) 'start menu shortcut exists'
Assert-True (Test-Path -LiteralPath (Join-Path $startMenu '卸载 WW-StudyApp.lnk')) 'uninstall shortcut exists'
function Assert-ApplicationShortcut([string]$path) {
    Assert-True (Test-Path -LiteralPath $path) "shortcut exists: $path"
    Copy-Item -LiteralPath $path -Destination (Join-Path $evidence ([IO.Path]::GetFileName($path))) -Force
    $link = [ShortcutReader]::Read($path)
    Write-Output "shortcut target=[$($link.TargetPath)] expected=[$(Join-Path $install 'WW-StudyApp.exe')] icon=[$($link.IconLocation)] index=[$($link.IconIndex)]"
    Assert-True ($link.TargetPath -eq (Join-Path $install 'WW-StudyApp.exe')) "shortcut targets selected path: $path"
    Assert-True ($link.IconLocation -eq (Join-Path $install 'WW-StudyApp.exe')) "shortcut uses application icon: $path"
    Assert-True ($link.IconIndex -eq 0) "shortcut uses first application icon: $path"
}
Assert-ApplicationShortcut $desktopShortcut
Assert-ApplicationShortcut (Join-Path $startMenu 'WW-StudyApp.lnk')
$uninstallShortcut = Join-Path $startMenu '卸载 WW-StudyApp.lnk'
Copy-Item -LiteralPath $uninstallShortcut -Destination (Join-Path $evidence '卸载 WW-StudyApp.lnk') -Force
$uninstallLink = [ShortcutReader]::Read($uninstallShortcut)
Write-Output "uninstall shortcut target=[$($uninstallLink.TargetPath)]"
Assert-True ($uninstallLink.TargetPath -eq (Join-Path $install 'Uninstall.exe')) 'uninstall shortcut targets selected directory'

# 2. Neither UI nor silent /D may leave an old uninstaller behind in another directory.
$relocated = Join-Path $root '禁止迁移 rejected relocation\WW StudyApp'
function Get-RegistrationSnapshot {
    $entries = foreach ($key in @($arp, $installKey)) {
        $values = [ordered]@{}
        (Get-ItemProperty -LiteralPath $key).PSObject.Properties |
            Where-Object Name -NotMatch '^PS(Path|ParentPath|ChildName|Drive|Provider)$' |
            Sort-Object Name | ForEach-Object { $values[$_.Name] = $_.Value }
        $values
    }
    ConvertTo-Json -InputObject @($entries) -Depth 4 -Compress
}
$registrationBefore = Get-RegistrationSnapshot
$protectedFiles = @((Join-Path $install 'WW-StudyApp.exe'), (Join-Path $install 'Uninstall.exe'),
    $desktopShortcut, (Join-Path $startMenu 'WW-StudyApp.lnk'), (Join-Path $startMenu '卸载 WW-StudyApp.lnk'))
$hashesBefore = @($protectedFiles | ForEach-Object { (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash })
function Assert-RejectedRelocationPreservesInstall([string]$label) {
    Assert-True (-not (Test-Path -LiteralPath $relocated)) "$label creates no new installation directory"
    $registrationAfter = Get-RegistrationSnapshot
    Assert-True ($registrationAfter -eq $registrationBefore) "$label preserves both registry entries exactly"
    $hashesAfter = @($protectedFiles | ForEach-Object { (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash })
    Assert-True (($hashesAfter -join ',') -eq ($hashesBefore -join ',')) "$label preserves application, uninstaller and shortcut bytes"
    Assert-ApplicationShortcut $desktopShortcut
    Assert-ApplicationShortcut (Join-Path $startMenu 'WW-StudyApp.lnk')
    Assert-True ((Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash -eq $sentinelHash) "$label preserves library bytes"
    Assert-True (@(Get-Process -Name 'WW-StudyApp' -ErrorAction SilentlyContinue).Count -eq 0) "$label leaves application closed"
}
$rejected = Start-Process -FilePath $probe -ArgumentList @(('"' + $setup + '"'), ('"' + $relocated + '"'), 'reject') -WindowStyle Hidden -Wait -PassThru -RedirectStandardOutput (Join-Path $evidence 'relocation.stdout.log') -RedirectStandardError (Join-Path $evidence 'relocation.stderr.log')
Get-Content -LiteralPath (Join-Path $evidence 'relocation.stdout.log')
Get-Content -LiteralPath (Join-Path $evidence 'relocation.stderr.log')
Assert-True ($rejected.ExitCode -eq 0) "interactive relocation rejection passes (got $($rejected.ExitCode))"
Assert-RejectedRelocationPreservesInstall 'interactive rejection'
$rejected = Start-Process -FilePath $setup -ArgumentList @('/S', ('/D=' + $relocated)) -WindowStyle Hidden -Wait -PassThru
Assert-True ($rejected.ExitCode -eq 6) "silent explicit /D relocation exits 6 (got $($rejected.ExitCode))"
Assert-RejectedRelocationPreservesInstall 'silent rejection'

# 3. Same-version reinstall behaves as an in-place upgrade and keeps working.
Invoke-Installer @('/S') 'reinstall without /D uses the selected location'
# Case, trailing separator and dot segments still name the same Windows directory.
$equivalentInstall = $install.ToUpperInvariant() + '\.\'
Invoke-Installer @('/S', ('/D=' + $equivalentInstall)) 'normalized equivalent explicit /D remains an in-place install'
Assert-True (Test-Path -LiteralPath (Join-Path $install 'WW-StudyApp.exe')) 'application still installed after reinstall'
Assert-True ((Get-ItemProperty -LiteralPath $arp).DisplayVersion -eq $Version) 'version survives reinstall'
Assert-True ((Get-ItemProperty -LiteralPath $arp).InstallLocation.TrimEnd('\') -eq $install.TrimEnd('\')) 'reinstall retains selected location'
Assert-ApplicationShortcut $desktopShortcut
Assert-ApplicationShortcut (Join-Path $startMenu 'WW-StudyApp.lnk')
Assert-True (@(Get-Process -Name 'WW-StudyApp' -ErrorAction SilentlyContinue).Count -eq 0) 'plain silent reinstall leaves application closed'
Assert-True ([IO.File]::ReadAllText($sentinel) -eq $sentinelText) 'reinstall retains library data'
Assert-True ((Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash -eq $sentinelHash) 'reinstall retains exact library bytes'

# 4. Uninstall removes the program, shortcuts and registration, but never data.
$uninstaller = Join-Path $install 'Uninstall.exe'
$process = Start-Process -FilePath $uninstaller -ArgumentList '/S' -WindowStyle Hidden -Wait -PassThru
Assert-True ($process.ExitCode -eq 0) "uninstall exits 0 (got $($process.ExitCode))"
# NSIS can return from the original uninstaller before its temporary copy exits.
$deadline = [DateTime]::UtcNow.AddSeconds(30)
while ([DateTime]::UtcNow -lt $deadline -and (
    (Test-Path -LiteralPath $uninstaller) -or
    (Test-Path -LiteralPath $arp) -or
    (Test-Path -LiteralPath $installKey) -or
    (Test-Path -LiteralPath $desktopShortcut) -or
    (Test-Path -LiteralPath (Join-Path $startMenu 'WW-StudyApp.lnk')) -or
    (Test-Path -LiteralPath (Join-Path $startMenu '卸载 WW-StudyApp.lnk')))) {
    Start-Sleep -Milliseconds 100
}
Assert-True (-not (Test-Path -LiteralPath (Join-Path $install 'WW-StudyApp.exe'))) 'application binary removed'
Assert-True (-not (Test-Path -LiteralPath (Join-Path $install 'Uninstall.exe'))) 'uninstaller removed'
Assert-True ($null -eq (Get-ItemProperty -LiteralPath $arp -ErrorAction SilentlyContinue)) 'uninstall entry removed'
Assert-True ($null -eq (Get-ItemProperty -LiteralPath $installKey -ErrorAction SilentlyContinue)) 'saved installer location removed'
Assert-True (-not (Test-Path -LiteralPath (Join-Path $startMenu 'WW-StudyApp.lnk'))) 'start menu shortcut removed'
Assert-True (-not (Test-Path -LiteralPath (Join-Path $startMenu '卸载 WW-StudyApp.lnk'))) 'uninstall shortcut removed'
Assert-True (-not (Test-Path -LiteralPath $desktopShortcut)) 'desktop shortcut removed'
Assert-True (Test-Path -LiteralPath $sentinel) 'library sentinel still exists'
Assert-True ([IO.File]::ReadAllText($sentinel) -eq $sentinelText) 'library data is byte-identical'
Assert-True ((Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash -eq $sentinelHash) 'uninstall retains exact library bytes'
Assert-True (Test-Path -LiteralPath $data) 'library folder retained'

Write-Output 'NSIS acceptance PASSED'
} finally {
    Stop-Transcript
}
