#requires -Version 7.0
param([string]$FixtureDirectory=(Join-Path $PSScriptRoot '../.scratch/installation-20261026'))
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'installer-policy.ps1')
$root=Join-Path ([IO.Path]::GetFullPath($FixtureDirectory)) ('rollback-snapshot-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory((Join-Path $root 'support')) | Out-Null
$tables=@{File=@();Component=@()}
foreach($resource in @(Get-InstallerPayload)) {
    $tables.File+=@{File=$resource.Id;Component_=$resource.Id;FileName=$resource.Name}
    $tables.Component+=@{Component=$resource.Id;Directory_=$resource.Directory}
}
$tables.File+=@{File='RetiredFixture';Component_='RetiredFixture';FileName='retired-runtime.txt'}
$tables.Component+=@{Component='RetiredFixture';Directory_='INSTALLFOLDER'}
$paths=@(Get-InstallerOwnedPaths -Tables $tables)
if($paths.Count -ne 8 -or 'support/manifest.json' -cnotin $paths -or 'WW-StudyApp.exe' -cnotin $paths -or 'retired-runtime.txt' -cnotin $paths) { throw 'Snapshot did not include the complete owned payload' }
foreach($relative in $paths) { [IO.File]::WriteAllText((Join-Path $root $relative),"old-$relative") }
$registration=@(@{ProductCode='{11111111-1111-4111-8111-111111111111}';Scope=$true;Version='0.0.1';InstallLocation=$root})
$before=Get-InstallerRollbackSnapshot -Root $root -OwnedPaths $paths -AbsentPaths @('new-owned.dll') -Registration $registration
$after=Get-InstallerRollbackSnapshot -Root $root -OwnedPaths $paths -AbsentPaths @('new-owned.dll') -Registration $registration
Assert-InstallerRollbackState -Before $before -After $after
$runtime=Join-Path $root 'vcruntime140.dll'
[IO.File]::WriteAllText($runtime,'failed-upgrade-runtime')
$after=Get-InstallerRollbackSnapshot -Root $root -OwnedPaths $paths -AbsentPaths @('new-owned.dll') -Registration $registration
$rejected=$false
try { Assert-InstallerRollbackState -Before $before -After $after } catch { $rejected=$true }
if(-not $rejected) { throw 'Real snapshot did not detect replaced runtime bytes' }
[IO.File]::WriteAllText($runtime,'old-vcruntime140.dll')
[IO.File]::WriteAllText((Join-Path $root 'new-owned.dll'),'failed-upgrade-leftover')
$after=Get-InstallerRollbackSnapshot -Root $root -OwnedPaths $paths -AbsentPaths @('new-owned.dll') -Registration $registration
$rejected=$false
try { Assert-InstallerRollbackState -Before $before -After $after } catch { $rejected=$true }
if(-not $rejected) { throw 'Real snapshot did not detect an incoming leftover file' }
$rejected=$false
try { Get-InstallerRollbackSnapshot -Root $root -OwnedPaths @('missing-owned.exe') -Registration $registration | Out-Null } catch { $rejected=$true }
if(-not $rejected) { throw 'Snapshot accepted a missing owned payload file' }
Write-Output "Rollback snapshot: 5 focused checks passed; retained fixture $root"
