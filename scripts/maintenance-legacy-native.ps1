# Windows PowerShell 5.1: exercise the actual unchanged schema-1 helper.
param([Parameter(Mandatory)][string]$HelperPath,[Parameter(Mandatory)][string]$LegacyPackage,[Parameter(Mandatory)][string]$ModernPackage,[Parameter(Mandatory)][string]$Version,[Parameter(Mandatory)][string]$OutputDirectory)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
if($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') { throw 'Legacy native acceptance runs only on the Windows CI runner' }
$root=[IO.Path]::GetFullPath($OutputDirectory)
$runner=[IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\')+'\'
if(-not $root.StartsWith($runner,[StringComparison]::OrdinalIgnoreCase) -or (Test-Path -LiteralPath $root)) { throw 'Legacy helper requires a fresh RUNNER_TEMP child' }
. $HelperPath
[IO.Directory]::CreateDirectory($root) | Out-Null
$install=Join-Path $root 'install'
$stage=Join-Path $root 'stage'
[IO.Directory]::CreateDirectory($install) | Out-Null
[IO.Directory]::CreateDirectory($stage) | Out-Null
$target=Join-Path $install 'WW-StudyApp.exe'
[IO.File]::WriteAllText($target,'synthetic old 0.0.1 executable fixture')
$original=Join-Path $install 'original-user-note.md'
[IO.File]::WriteAllText($original,'Original user note must survive old updater and repair')
$originalHash=Get-Sha256 $original
$archivePath=Join-Path $stage 'package.zip'
[IO.File]::Copy([IO.Path]::GetFullPath($LegacyPackage),$archivePath,$false)
$legacyRoot="WW-StudyApp-$Version-windows-x86_64"
$archive=[IO.Compression.ZipFile]::OpenRead($archivePath)
try { $legacyEntries=@(Read-ValidatedEntries ([pscustomobject]@{package_root=$legacyRoot}) $archive);if($legacyEntries.Count -ne 7) { throw 'Actual compatibility package lost its seven-file shape' } } finally { $archive.Dispose() }

# Normalize only the root to isolate the second-EXE policy, rather than passing
# due to the modern archive's intentionally different root name.
$rebasedPath=Join-Path $root 'modern-with-legacy-root.zip'
$modern=[IO.Compression.ZipFile]::OpenRead([IO.Path]::GetFullPath($ModernPackage))
$rebased=[IO.Compression.ZipFile]::Open($rebasedPath,[IO.Compression.ZipArchiveMode]::Create)
try {
    $prefix="WW-StudyApp-$Version-windows-x86_64-portable/"
    foreach($entry in $modern.Entries) {
        if(-not $entry.FullName.StartsWith($prefix,[StringComparison]::Ordinal)) { throw 'Modern root differs from exact package version' }
        $copy=$rebased.CreateEntry($legacyRoot+'/'+$entry.FullName.Substring($prefix.Length))
        $input=$entry.Open();$output=$copy.Open()
        try { $input.CopyTo($output) } finally { $input.Dispose();$output.Dispose() }
    }
} finally { $modern.Dispose();$rebased.Dispose() }
$probe=[IO.Compression.ZipFile]::OpenRead($rebasedPath)
$rejection=''
try { try { $null=Read-ValidatedEntries ([pscustomobject]@{package_root=$legacyRoot}) $probe } catch { $rejection=$_.Exception.Message } } finally { $probe.Dispose() }
if($rejection -cne 'unexpected_executable') { throw "Actual schema1 helper did not reject the additional executable: $rejection" }
$file=Get-Item -LiteralPath $target
$plan=[pscustomobject]@{schema=1;id=[guid]::NewGuid().ToString('N');version=$Version;stage_dir=$stage;archive_path=$archivePath;archive_sha256=(Get-Sha256 $archivePath);archive_size=(Get-Item -LiteralPath $archivePath).Length;package_root=$legacyRoot;target_exe=$target;old_sha256=(Get-Sha256 $target);old_size=$file.Length;old_modified_ticks=$file.LastWriteTimeUtc.Ticks.ToString();parent_pid=$PID;restart_args=@();status_path=(Join-Path $stage 'status.json')}
Prepare-Update $plan
Invoke-UpdateReplacement $plan {param($ignoredPlan)}
$status=Get-Content -LiteralPath $plan.status_path -Raw | ConvertFrom-Json
if($status.state -cne 'succeeded' -or $status.version -cne $Version -or (Get-Sha256 $original) -cne $originalHash -or (Test-Path -LiteralPath (Join-Path $install 'Uninstall.exe'))) { throw 'Schema1 skip-release replacement did not preserve compatibility/unknown files' }
[ordered]@{schema=1;legacy_members=$legacyEntries.Count;modern_rejection=$rejection;helper_sha256=(Get-Sha256 $HelperPath);install_root=$install;installed_main_sha256=(Get-Sha256 $target);original_path=$original;original_sha256=$originalHash;status=$status;restart_mode='injected-callback-not-health-evidence'} | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $root 'legacy-acceptance.json') -Encoding utf8
