# Trusted, embedded helper. It never evaluates downloaded scripts and never deletes files.
# Compatible with the Windows PowerShell 5.1 supplied by Windows.
param([ValidateSet('Prepare', 'Apply')][string]$Mode, [string]$PlanPath)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

function Assert-PlainPath([string]$Path) {
    if (-not [IO.Path]::IsPathRooted($Path) -or $Path.StartsWith('\\')) { throw 'unsupported_path' }
    $full = [IO.Path]::GetFullPath($Path)
    if ($full.Substring([IO.Path]::GetPathRoot($full).Length).Contains(':')) { throw 'unsupported_path' }
    $cursor = $full
    while ($cursor) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -Force -LiteralPath $cursor
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'reparse_path' }
        }
        $parent = [IO.Path]::GetDirectoryName($cursor)
        if ($parent -eq $cursor) { break }
        $cursor = $parent
    }
    return $full
}

function Get-Sha256([string]$Path) { return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }

function Get-EntrySha256($Entry) {
    $stream = $Entry.Open()
    $algorithm = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($algorithm.ComputeHash($stream))).Replace('-', '').ToLowerInvariant() }
    finally { $algorithm.Dispose(); $stream.Dispose() }
}

function Write-NewJson([string]$Path, $Value) {
    $null = Assert-PlainPath $Path
    $bytes = [Text.Encoding]::UTF8.GetBytes(($Value | ConvertTo-Json -Depth 8 -Compress))
    $stream = [IO.File]::Open($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
    try { $stream.Write($bytes, 0, $bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
}

function Write-UpdateStatus($Plan, [string]$State, [string]$Code, [string]$Backup = '', [string]$Distribution = '') {
    $value = [ordered]@{ schema = 1; id = $Plan.id; state = $State; code = $Code; version = $Plan.version;
        target_exe = $Plan.target_exe; backup_path = $Backup; distribution_path = $Distribution;
        timestamp = [DateTime]::UtcNow.ToString('o') }
    $status = Assert-PlainPath $Plan.status_path
    $temporary = $status + '.' + [guid]::NewGuid().ToString('N') + '.json'
    Write-NewJson $temporary $value
    if ([IO.File]::Exists($status)) {
        $null = Assert-PlainPath $status
        [IO.File]::Replace($temporary, $status, ($status + '.previous-' + [guid]::NewGuid().ToString('N')), $false)
    } else { [IO.File]::Move($temporary, $status) }
}

function Assert-Plan($Plan) {
    if ($Plan.schema -ne 1 -or $Plan.id -notmatch '^[a-f0-9]{32}$' -or $Plan.version -notmatch '^\d+\.\d+\.\d+$') { throw 'invalid_plan' }
    if ($Plan.old_sha256 -notmatch '^[a-f0-9]{64}$' -or $Plan.archive_sha256 -notmatch '^[a-f0-9]{64}$') { throw 'invalid_plan' }
    $stage = Assert-PlainPath $Plan.stage_dir
    $archive = Assert-PlainPath $Plan.archive_path
    $target = Assert-PlainPath $Plan.target_exe
    $null = Assert-PlainPath $Plan.status_path
    if ($archive -ne (Join-Path $stage 'package.zip') -or -not [IO.Directory]::Exists($stage)) { throw 'invalid_plan' }
    if ($Plan.package_root -cne "WW-StudyApp-$($Plan.version)-windows-x86_64") { throw 'invalid_plan' }
    if ([IO.Path]::GetExtension($target) -ine '.exe' -or $target.StartsWith($stage + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'invalid_plan' }
    $argsList = @($Plan.restart_args)
    if ($argsList.Count -ne 0 -and ($argsList.Count -ne 2 -or $argsList[0] -cne '--data-dir' -or [string]::IsNullOrWhiteSpace($argsList[1]) -or $argsList[1].Contains([char]0))) { throw 'invalid_plan' }
}

function Assert-OldExecutable($Plan) {
    $target = Assert-PlainPath $Plan.target_exe
    $file = Get-Item -Force -LiteralPath $target
    if ($file.PSIsContainer -or $file.Length -ne $Plan.old_size -or $file.LastWriteTimeUtc.Ticks.ToString() -ne $Plan.old_modified_ticks -or (Get-Sha256 $target) -cne $Plan.old_sha256) { throw 'installation_changed' }
}

function Read-ValidatedEntries($Plan, $Archive) {
    if ($Archive.Entries.Count -lt 2 -or $Archive.Entries.Count -gt 8192) { throw 'archive_limits' }
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $entries = [Collections.Generic.List[object]]::new()
    [long]$expanded = 0
    $exeCount = 0
    $hasLicense = $false
    foreach ($entry in $Archive.Entries) {
        $name = $entry.FullName
        if ([string]::IsNullOrWhiteSpace($name) -or $name.Length -gt 1024 -or $name.Contains('\') -or $name.Contains(':') -or $name.StartsWith('/') -or $name.Contains([char]0)) { throw 'unsafe_archive_path' }
        $directory = $name.EndsWith('/')
        $parts = $name.TrimEnd('/').Split('/')
        if ($parts[0] -cne $Plan.package_root) { throw 'unexpected_archive_root' }
        foreach ($part in $parts) {
            if ([string]::IsNullOrEmpty($part) -or $part -eq '.' -or $part -eq '..' -or $part -match '[<>"|?*\x00-\x1f]' -or $part.EndsWith('.') -or $part.EndsWith(' ') -or $part -match '^(CON|PRN|AUX|NUL|CONIN\$|CONOUT\$|COM[1-9\u00b9\u00b2\u00b3]|LPT[1-9\u00b9\u00b2\u00b3])(?:\.|$)') { throw 'unsafe_archive_path' }
        }
        if (-not $seen.Add($name.TrimEnd('/'))) { throw 'duplicate_archive_path' }
        $kind = ($entry.ExternalAttributes -shr 16) -band 0xF000
        if ($kind -notin @(0, 0x8000, 0x4000) -or ($entry.ExternalAttributes -band 0x400) -ne 0 -or ($kind -eq 0x4000 -and -not $directory)) { throw 'archive_link' }
        if (-not $directory -and $parts.Length -lt 2) { throw 'unsafe_archive_path' }
        if ($entry.Length -lt 0 -or $entry.Length -gt 512MB -or $entry.CompressedLength -lt 0 -or ($entry.Length -gt 1MB -and $entry.Length -gt ([Math]::Max(1, $entry.CompressedLength) * 200))) { throw 'archive_limits' }
        $expanded += $entry.Length
        if ($expanded -gt 768MB) { throw 'archive_limits' }
        if ([IO.Path]::GetExtension($name) -ieq '.exe') {
            if ($directory -or $name -cne ($Plan.package_root + '/WW-StudyApp.exe')) { throw 'unexpected_executable' }
            $exeCount++
        }
        if ([IO.Path]::GetExtension($name) -ieq '.dll' -and $name -cnotin @(
            ($Plan.package_root + '/vcruntime140.dll'), ($Plan.package_root + '/vcruntime140_1.dll')
        )) { throw 'unexpected_runtime' }
        if ($name -ceq ($Plan.package_root + '/LICENSE') -and -not $directory) { $hasLicense = $true }
        $entries.Add([pscustomobject]@{ entry = $entry; name = $name; directory = $directory })
    }
    if ($exeCount -ne 1 -or -not $hasLicense) { throw 'missing_distribution_file' }
    # Also reject a file that another member treats as a directory.
    $files = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($record in $entries) { if (-not $record.directory) { $null = $files.Add($record.name) } }
    foreach ($record in $entries) {
        $parts = $record.name.TrimEnd('/').Split('/')
        $prefix = $parts[0]
        for ($index = 1; $index -lt $parts.Length; $index++) {
            if ($files.Contains($prefix)) { throw 'archive_path_collision' }
            $prefix += '/' + $parts[$index]
        }
    }
    return $entries
}

function Prepare-Update($Plan) {
    Assert-Plan $Plan
    Assert-OldExecutable $Plan
    if (((Get-Item -LiteralPath $Plan.target_exe).Attributes -band [IO.FileAttributes]::ReadOnly) -ne 0) { throw 'manual_update_required' }
    if ((Get-Item -LiteralPath $Plan.archive_path).Length -ne $Plan.archive_size -or $Plan.archive_size -le 0 -or $Plan.archive_size -gt 256MB -or (Get-Sha256 $Plan.archive_path) -cne $Plan.archive_sha256) { throw 'archive_hash_mismatch' }
    $extract = Assert-PlainPath (Join-Path $Plan.stage_dir 'extracted')
    if (Test-Path -LiteralPath $extract) { throw 'stage_already_exists' }
    $archive = [IO.Compression.ZipFile]::OpenRead($Plan.archive_path)
    try {
        $entries = Read-ValidatedEntries $Plan $archive
        [IO.Directory]::CreateDirectory($extract) | Out-Null
        foreach ($record in $entries) {
            $destination = Assert-PlainPath (Join-Path $extract $record.name)
            if (-not $destination.StartsWith($extract + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'unsafe_archive_path' }
            if ($record.directory) { [IO.Directory]::CreateDirectory($destination) | Out-Null; continue }
            [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destination)) | Out-Null
            $null = Assert-PlainPath $destination
            $inputStream = $record.entry.Open()
            try {
                $outputStream = [IO.File]::Open($destination, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
                try {
                    $buffer = [byte[]]::new(65536)
                    [long]$written = 0
                    while (($read = $inputStream.Read($buffer, 0, $buffer.Length)) -gt 0) {
                        $written += $read
                        if ($written -gt $record.entry.Length) { throw 'archive_limits' }
                        $outputStream.Write($buffer, 0, $read)
                    }
                    if ($written -ne $record.entry.Length) { throw 'archive_size_mismatch' }
                    $outputStream.Flush($true)
                } finally { $outputStream.Dispose() }
            } finally { $inputStream.Dispose() }
        }
    } finally { $archive.Dispose() }
    $candidate = Join-Path $extract ($Plan.package_root + '/WW-StudyApp.exe')
    $manifest = [Collections.Generic.List[object]]::new()
    foreach ($record in $entries) {
        if (-not $record.directory) {
            $path = Join-Path $extract $record.name
            $manifest.Add([pscustomobject]@{ relative = $record.name; sha256 = (Get-Sha256 $path) })
        }
    }
    $parentProcess = [Diagnostics.Process]::GetProcessById([int]$Plan.parent_pid)
    try { $parentStartTicks = $parentProcess.StartTime.ToUniversalTime().Ticks.ToString() } finally { $parentProcess.Dispose() }
    $runtimes = @($entries | Where-Object { -not $_.directory -and [IO.Path]::GetExtension($_.name) -ieq '.dll' } | ForEach-Object {
        Get-RuntimeIdentity (Join-Path ([IO.Path]::GetDirectoryName($Plan.target_exe)) ([IO.Path]::GetFileName($_.name)))
    })
    Write-NewJson (Join-Path $Plan.stage_dir 'prepared.json') ([ordered]@{ candidate_sha256 = (Get-Sha256 $candidate); parent_start_ticks = $parentStartTicks; files = @($manifest.ToArray()); runtimes = $runtimes })
    # Probe one unique sibling file. Retain it, as no cleanup/delete is needed for correctness.
    try {
        $probe = Join-Path ([IO.Path]::GetDirectoryName($Plan.target_exe)) ('.ww-studyapp-write-probe-' + $Plan.id)
        $null = Assert-PlainPath $probe
        $stream = [IO.File]::Open($probe, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        $stream.Dispose()
    } catch { throw 'manual_update_required' }
    Write-UpdateStatus $Plan 'prepared' 'ready'
}

function Quote-WindowsArgument([string]$Value) {
    # CommandLineToArgvW / CRT rules; ProcessStartInfo invokes an exe directly, without a shell.
    $result = '"'
    $slashes = 0
    foreach ($char in $Value.ToCharArray()) {
        if ($char -eq '\') { $slashes++; continue }
        if ($char -eq '"') { $result += ('\' * ($slashes * 2 + 1)) + '"' }
        else { $result += ('\' * $slashes) + $char }
        $slashes = 0
    }
    return $result + ('\' * ($slashes * 2)) + '"'
}

function Restart-UpdatedApp($Plan) {
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $Plan.target_exe
    $info.UseShellExecute = $false
    $info.WorkingDirectory = [IO.Path]::GetDirectoryName($Plan.target_exe)
    $info.Arguments = (@($Plan.restart_args | ForEach-Object { Quote-WindowsArgument $_ }) -join ' ')
    $process = [Diagnostics.Process]::Start($info)
    if ($null -eq $process) { throw 'restart_failed' }
    $process.Dispose()
}

function Wait-ParentExit($Plan) {
    $preparedPath = Assert-PlainPath (Join-Path $Plan.stage_dir 'prepared.json')
    if ((Get-Item -LiteralPath $preparedPath).Length -gt 4MB) { throw 'invalid_preparation' }
    $prepared = [IO.File]::ReadAllText($preparedPath) | ConvertFrom-Json
    try { $parentProcess = [Diagnostics.Process]::GetProcessById([int]$Plan.parent_pid) }
    catch [ArgumentException] { return }
    try {
        if ($parentProcess.StartTime.ToUniversalTime().Ticks.ToString() -ne $prepared.parent_start_ticks) { return }
        if (-not $parentProcess.WaitForExit(300000)) { throw 'parent_still_running' }
    } finally { $parentProcess.Dispose() }
}

function Get-RuntimeIdentity([string]$Path) {
    $path = Assert-PlainPath $Path
    $hash = ''
    if (Test-Path -LiteralPath $path) {
        $item = Get-Item -Force -LiteralPath $path
        if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReadOnly) -ne 0) { throw 'manual_update_required' }
        $hash = Get-Sha256 $path
    }
    return [pscustomobject]@{ name = [IO.Path]::GetFileName($path); sha256 = $hash }
}

function Restore-UpdatedRuntimes($Runtimes) {
    for ($index = $Runtimes.Count - 1; $index -ge 0; $index--) {
        $runtime = $Runtimes[$index]
        $null = Assert-PlainPath $runtime.target
        if ((Get-Sha256 $runtime.target) -cne $runtime.new_hash) { throw 'rollback_identity_changed' }
        $null = Assert-PlainPath $runtime.failed
        if ($runtime.old_hash) {
            $null = Assert-PlainPath $runtime.backup
            if ((Get-Sha256 $runtime.backup) -cne $runtime.old_hash) { throw 'rollback_identity_changed' }
            $restore = $runtime.backup + '.restore'
            $null = Assert-PlainPath $restore
            [IO.File]::Copy($runtime.backup, $restore, $false)
            [IO.File]::Replace($restore, $runtime.target, $runtime.failed, $false)
        } else {
            # Retain the failed DLL under a non-loadable name; no file is deleted.
            [IO.File]::Move($runtime.target, $runtime.failed)
        }
    }
}

function Invoke-UpdateReplacement($Plan, [scriptblock]$Restart) {
    Assert-Plan $Plan
    Assert-OldExecutable $Plan
    $preparedPath = Assert-PlainPath (Join-Path $Plan.stage_dir 'prepared.json')
    if ((Get-Item -LiteralPath $preparedPath).Length -gt 4MB) { throw 'invalid_preparation' }
    $prepared = [IO.File]::ReadAllText($preparedPath) | ConvertFrom-Json
    $extract = Join-Path $Plan.stage_dir 'extracted'
    $install = [IO.Path]::GetDirectoryName($Plan.target_exe)
    $distribution = Join-Path $install ('.ww-studyapp-distribution-' + $Plan.version + '-' + $Plan.id)
    $candidate = Join-Path $extract ($Plan.package_root + '/WW-StudyApp.exe')
    $sibling = Join-Path $install ('.ww-studyapp-new-' + $Plan.id + '.exe')
    $backup = Join-Path $install ('.ww-studyapp-backup-' + $Plan.id + '.exe')
    foreach ($path in @($distribution, $sibling, $backup)) {
        $null = Assert-PlainPath $path
        if (Test-Path -LiteralPath $path) { throw 'transaction_path_exists' }
    }
    # Revalidate the archive and preparation on handoff, including every retained license/source file.
    if ((Get-Sha256 $Plan.archive_path) -cne $Plan.archive_sha256) { throw 'archive_hash_mismatch' }
    $archive = [IO.Compression.ZipFile]::OpenRead($Plan.archive_path)
    try {
        $entries = @(Read-ValidatedEntries $Plan $archive | Where-Object { -not $_.directory })
        if ($entries.Count -ne @($prepared.files).Count) { throw 'invalid_preparation' }
        $recorded = @{}
        foreach ($file in $prepared.files) {
            if ($recorded.ContainsKey($file.relative)) { throw 'invalid_preparation' }
            $recorded[$file.relative] = $file.sha256
        }
        foreach ($entry in $entries) {
            $source = Assert-PlainPath (Join-Path $extract $entry.name)
            $archiveHash = Get-EntrySha256 $entry.entry
            if (-not $recorded.ContainsKey($entry.name) -or (Get-Sha256 $source) -cne $archiveHash -or $archiveHash -cne $recorded[$entry.name]) { throw 'candidate_changed' }
        }
    } finally { $archive.Dispose() }
    $null = Assert-PlainPath $candidate
    if ((Get-Sha256 $candidate) -cne $prepared.candidate_sha256) { throw 'candidate_changed' }
    $runtimeEntries = @($entries | Where-Object { [IO.Path]::GetExtension($_.name) -ieq '.dll' })
    if (@($prepared.runtimes).Count -ne $runtimeEntries.Count) { throw 'invalid_preparation' }
    $runtimePlans = [Collections.Generic.List[object]]::new()
    foreach ($entry in $runtimeEntries) {
        $name = [IO.Path]::GetFileName($entry.name)
        $identity = @($prepared.runtimes | Where-Object name -CEQ $name)
        $target = Join-Path $install $name
        if ($identity.Count -ne 1 -or (Get-RuntimeIdentity $target).sha256 -cne $identity[0].sha256) { throw 'installation_changed' }
        $runtimePlans.Add([pscustomobject]@{
            target = $target; source = Join-Path $extract $entry.name
            sibling = Join-Path $install ('.ww-studyapp-new-' + $Plan.id + '-' + $name)
            backup = Join-Path $install ('.ww-studyapp-backup-' + $Plan.id + '-' + $name)
            failed = Join-Path $install ('.ww-studyapp-failed-' + $Plan.id + '-' + $name)
            old_hash = $identity[0].sha256; new_hash = $recorded[$entry.name]
        })
    }
    $installedRuntimes = [Collections.Generic.List[object]]::new()
    $replaced = $false
    try {
        [IO.Directory]::CreateDirectory($distribution) | Out-Null
        foreach ($entry in $entries) {
            if ($entry.name -ceq ($Plan.package_root + '/WW-StudyApp.exe') -or [IO.Path]::GetExtension($entry.name) -ieq '.dll') { continue }
            $relative = $entry.name.Substring($Plan.package_root.Length + 1)
            $destination = Assert-PlainPath (Join-Path $distribution $relative)
            [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destination)) | Out-Null
            $null = Assert-PlainPath $destination
            [IO.File]::Copy((Join-Path $extract $entry.name), $destination, $false)
            if ((Get-Sha256 $destination) -cne $recorded[$entry.name]) { throw 'candidate_changed' }
        }
        $null = Assert-PlainPath $sibling
        [IO.File]::Copy($candidate, $sibling, $false)
        if ((Get-Sha256 $sibling) -cne $prepared.candidate_sha256) { throw 'candidate_changed' }
        foreach ($runtime in $runtimePlans) {
            foreach ($path in @($runtime.sibling, $runtime.backup, $runtime.failed)) {
                $null = Assert-PlainPath $path
                if (Test-Path -LiteralPath $path) { throw 'transaction_path_exists' }
            }
            [IO.File]::Copy($runtime.source, $runtime.sibling, $false)
            if ((Get-Sha256 $runtime.sibling) -cne $runtime.new_hash -or (Get-RuntimeIdentity $runtime.target).sha256 -cne $runtime.old_hash) { throw 'installation_changed' }
            if ($runtime.old_hash) { [IO.File]::Replace($runtime.sibling, $runtime.target, $runtime.backup, $false) }
            else { [IO.File]::Move($runtime.sibling, $runtime.target) }
            $installedRuntimes.Add($runtime)
        }
        Assert-OldExecutable $Plan
        $null = Assert-PlainPath $backup
        # Same-directory Replace is atomic; the original is preserved under one explicit backup path.
        [IO.File]::Replace($sibling, $Plan.target_exe, $backup, $false)
        $replaced = $true
        Write-UpdateStatus $Plan 'succeeded' 'updated' $backup $distribution
        & $Restart $Plan
    } catch {
        $failure = $_
        if ($replaced -or $installedRuntimes.Count -gt 0) {
            try {
                if ($replaced) {
                    $null = Assert-PlainPath $Plan.target_exe
                    $null = Assert-PlainPath $backup
                    if ((Get-Sha256 $Plan.target_exe) -cne $prepared.candidate_sha256 -or (Get-Sha256 $backup) -cne $Plan.old_sha256) { throw 'rollback_identity_changed' }
                    $restore = Join-Path $install ('.ww-studyapp-restore-' + $Plan.id + '.exe')
                    $failedBinary = Join-Path $install ('.ww-studyapp-failed-' + $Plan.id + '.exe')
                    $null = Assert-PlainPath $restore
                    $null = Assert-PlainPath $failedBinary
                    [IO.File]::Copy($backup, $restore, $false)
                    [IO.File]::Replace($restore, $Plan.target_exe, $failedBinary, $false)
                }
                Restore-UpdatedRuntimes $installedRuntimes
                Write-UpdateStatus $Plan 'rolled_back' 'restart_failed' $backup $distribution
                # A best-effort normal restart of the restored executable lets it show the result.
                try { & $Restart $Plan } catch { }
            } catch {
                Write-UpdateStatus $Plan 'failed' 'rollback_needs_help' $backup $distribution
                throw 'rollback_needs_help'
            }
        } else {
            try { Write-UpdateStatus $Plan 'failed' 'replacement_failed' '' $distribution } catch { }
            # Permissions or a lock can change after preflight. Bring the original
            # application back only if its identity is still the one we prepared.
            try { Assert-OldExecutable $Plan; & $Restart $Plan } catch { }
        }
        throw $failure
    }
}

if ($MyInvocation.InvocationName -ne '.') {
    $plan = $null
    try {
        if (-not $Mode -or -not $PlanPath) { throw 'missing_arguments' }
        $fullPlanPath = Assert-PlainPath $PlanPath
        if ((Get-Item -LiteralPath $fullPlanPath).Length -gt 64KB) { throw 'invalid_plan' }
        $plan = [IO.File]::ReadAllText($fullPlanPath) | ConvertFrom-Json
        if ($fullPlanPath -ne (Assert-PlainPath (Join-Path $plan.stage_dir 'plan.json'))) { throw 'invalid_plan' }
        if ($Mode -eq 'Prepare') { Prepare-Update $plan }
        else {
            Assert-Plan $plan
            Wait-ParentExit $plan
            Invoke-UpdateReplacement $plan ${function:Restart-UpdatedApp}
        }
        exit 0
    } catch {
        # Structured known codes only. Do not place raw exceptions or local paths in UI messages.
        $code = [string]$_.Exception.Message
        if ($code -notmatch '^[a-z_]{1,64}$') { $code = 'update_failed' }
        if ($null -ne $plan) {
            try {
                $existing = $null
                if ([IO.File]::Exists($plan.status_path)) { $existing = [IO.File]::ReadAllText($plan.status_path) | ConvertFrom-Json }
                if ($null -eq $existing -or $existing.id -ne $plan.id -or $existing.state -notin @('rolled_back', 'failed')) { Write-UpdateStatus $plan 'failed' $code }
            } catch { }
        }
        [Console]::Error.WriteLine($code)
        exit 1
    }
}
