param(
    [string]$Helper = (Join-Path $PSScriptRoot '../crates/studyapp-desktop/src/update_helper.ps1'),
    [string]$FixtureDirectory = (Join-Path $PSScriptRoot '../.scratch'),
    [switch]$RequireReparseFixture
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
. $Helper
$fixtureRoot = [IO.Path]::GetFullPath((Join-Path $FixtureDirectory ('updater-fixtures-' + [guid]::NewGuid().ToString('N'))))
[IO.Directory]::CreateDirectory($fixtureRoot) | Out-Null
$script:passed = 0
$script:skipped = 0
function Assert-True($Value, [string]$Message) { if (-not $Value) { throw "ASSERTION: $Message" } }
function Test-Case([string]$Name, [scriptblock]$Body) {
    try { & $Body; $script:passed++; Write-Output "PASS $Name" }
    catch [NotSupportedException] {
        if ($RequireReparseFixture) { throw "Required fixture unavailable: $Name" }
        $script:skipped++; Write-Output "SKIP $Name : $($_.Exception.Message)"
    }
}
function New-Fixture([string[]]$Names, [switch]$Link, [switch]$Bomb) {
    $base = Join-Path $fixtureRoot ([guid]::NewGuid().ToString('N'))
    $stage = Join-Path $base 'stage'
    $install = Join-Path $base '中文 安装目录'
    [IO.Directory]::CreateDirectory($stage) | Out-Null
    [IO.Directory]::CreateDirectory($install) | Out-Null
    $target = Join-Path $install '我的 学习.exe'
    [IO.File]::WriteAllText($target, 'old fixture binary')
    [IO.File]::WriteAllText((Join-Path $install 'user-note.md'), 'must survive')
    $archivePath = Join-Path $stage 'package.zip'
    $archive = [IO.Compression.ZipFile]::Open($archivePath, [IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($name in $Names) {
            $entry = $archive.CreateEntry($name)
            if ($Link -and $name.EndsWith('link')) { $entry.ExternalAttributes = [int]0xA1FF0000 }
            $stream = $entry.Open()
            try {
                if ($Bomb) { $data = [byte[]]::new(2MB) } else { $data = [Text.Encoding]::UTF8.GetBytes('new fixture binary and license') }
                $stream.Write($data, 0, $data.Length)
            } finally { $stream.Dispose() }
        }
    } finally { $archive.Dispose() }
    $file = Get-Item -LiteralPath $target
    return [pscustomobject]@{
        schema = 1; id = [IO.Path]::GetFileName($base); version = '0.7.0'; stage_dir = $stage;
        archive_path = $archivePath; archive_sha256 = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant();
        archive_size = (Get-Item -LiteralPath $archivePath).Length; package_root = 'WW-StudyApp-0.7.0-windows-x86_64';
        target_exe = $target; old_sha256 = (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant();
        old_size = $file.Length; old_modified_ticks = $file.LastWriteTimeUtc.Ticks.ToString();
        parent_pid = $PID; restart_args = @('--data-dir', (Join-Path $install '数据 目录'));
        status_path = (Join-Path $base 'status.json');
    }
}
$root = 'WW-StudyApp-0.7.0-windows-x86_64'
$valid = @("$root/WW-StudyApp.exe", "$root/LICENSE", "$root/THIRD-PARTY-NOTICES.txt")
Test-Case 'runtime DLLs install beside the executable and preserve their originals' {
    $plan = New-Fixture ($valid + "$root/vcruntime140.dll" + "$root/vcruntime140_1.dll")
    $runtime = Join-Path ([IO.Path]::GetDirectoryName($plan.target_exe)) 'vcruntime140.dll'
    [IO.File]::WriteAllText($runtime, 'old runtime')
    Prepare-Update $plan
    Invoke-UpdateReplacement $plan { param($p) }
    Assert-True ([IO.File]::ReadAllText($runtime) -eq 'new fixture binary and license') 'runtime not installed beside executable'
    Assert-True (Test-Path -LiteralPath (Join-Path ([IO.Path]::GetDirectoryName($runtime)) 'vcruntime140_1.dll')) 'new runtime missing'
    $backup = Join-Path ([IO.Path]::GetDirectoryName($runtime)) ('.ww-studyapp-backup-' + $plan.id + '-vcruntime140.dll')
    Assert-True ([IO.File]::ReadAllText($backup) -eq 'old runtime') 'old runtime backup missing'
}
Test-Case 'failed restart restores old DLL and removes newly installed DLL from load path without deleting' {
    $plan = New-Fixture ($valid + "$root/vcruntime140.dll" + "$root/vcruntime140_1.dll")
    $install = [IO.Path]::GetDirectoryName($plan.target_exe)
    $runtime = Join-Path $install 'vcruntime140.dll'
    [IO.File]::WriteAllText($runtime, 'old runtime')
    Prepare-Update $plan
    $failed = $false
    try { Invoke-UpdateReplacement $plan { param($p) throw 'simulated restart failure' } } catch { $failed = $true }
    Assert-True $failed 'restart failure not reported'
    Assert-True ([IO.File]::ReadAllText($runtime) -eq 'old runtime') 'old runtime not restored'
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $install 'vcruntime140_1.dll'))) 'new runtime remains on DLL load path'
    Assert-True (Test-Path -LiteralPath (Join-Path $install ('.ww-studyapp-failed-' + $plan.id + '-vcruntime140_1.dll'))) 'failed runtime was not retained'
    Assert-True ((Get-Sha256 $plan.target_exe) -eq $plan.old_sha256) 'old executable not restored'
}
Test-Case 'external runtime change after preparation prevents all replacements' {
    $plan = New-Fixture ($valid + "$root/vcruntime140.dll")
    $runtime = Join-Path ([IO.Path]::GetDirectoryName($plan.target_exe)) 'vcruntime140.dll'
    [IO.File]::WriteAllText($runtime, 'old runtime')
    Prepare-Update $plan
    [IO.File]::WriteAllText($runtime, 'external runtime')
    $failed = $false
    try { Invoke-UpdateReplacement $plan { param($p) } } catch { $failed = $true }
    Assert-True $failed 'external DLL replacement was overwritten'
    Assert-True ([IO.File]::ReadAllText($runtime) -eq 'external runtime') 'external DLL changed'
    Assert-True ((Get-Sha256 $plan.target_exe) -eq $plan.old_sha256) 'exe changed after stale DLL preparation'
}
Test-Case 'locked second DLL rolls back the first DLL before restarting the original' {
    $plan = New-Fixture ($valid + "$root/vcruntime140.dll" + "$root/vcruntime140_1.dll")
    $install = [IO.Path]::GetDirectoryName($plan.target_exe)
    $first = Join-Path $install 'vcruntime140.dll'
    $second = Join-Path $install 'vcruntime140_1.dll'
    [IO.File]::WriteAllText($first, 'old first runtime')
    [IO.File]::WriteAllText($second, 'old second runtime')
    Prepare-Update $plan
    $lock = [IO.File]::Open($second, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    $script:runtimeRecoveryRestarts = 0
    $failed = $false
    try { Invoke-UpdateReplacement $plan { param($p) $script:runtimeRecoveryRestarts++ } } catch { $failed = $true }
    finally { $lock.Dispose() }
    Assert-True $failed 'locked DLL was overwritten'
    Assert-True ([IO.File]::ReadAllText($first) -eq 'old first runtime') 'first DLL not restored'
    Assert-True ([IO.File]::ReadAllText($second) -eq 'old second runtime') 'locked DLL changed'
    Assert-True ($script:runtimeRecoveryRestarts -eq 1) 'original was not restarted after runtime rollback'
    Assert-True ((Get-Sha256 $plan.target_exe) -eq $plan.old_sha256) 'exe changed after partial runtime failure'
}
Test-Case 'valid archive and Chinese-space target preserves original during preparation' {
    $plan = New-Fixture $valid
    Prepare-Update $plan
    Assert-True ([IO.File]::ReadAllText($plan.target_exe) -eq 'old fixture binary') 'prepare changed target'
    Assert-True (Test-Path -LiteralPath (Join-Path $plan.stage_dir 'prepared.json')) 'missing prepared record'
}
foreach ($entry in @('../escape', '/absolute', 'C:/drive', "$root/name:ads", "$root\ambiguous", "$root/../escape", "$root/CON", "$root/COM¹.txt", "$root/CONOUT$", "$root/trailing. ", "$root/aux.txt", "$root/another.exe", "$root/unknown.dll", "$root/lib/vcruntime140.dll", "WRONG/LICENSE")) {
    Test-Case "reject unsafe archive entry $entry" {
        $plan = New-Fixture ($valid + $entry)
        $failed = $false
        try { Prepare-Update $plan } catch { $failed = $true }
        Assert-True $failed 'unsafe archive accepted'
        Assert-True ([IO.File]::ReadAllText($plan.target_exe) -eq 'old fixture binary') 'rejection changed target'
    }
}
Test-Case 'read-only installed exe is rejected during preparation' {
    $plan = New-Fixture $valid
    [IO.File]::SetAttributes($plan.target_exe, [IO.FileAttributes]::ReadOnly)
    $rejected = $false
    try { Prepare-Update $plan } catch { $rejected = $_.Exception.Message -eq 'manual_update_required' }
    finally { [IO.File]::SetAttributes($plan.target_exe, [IO.FileAttributes]::Normal) }
    Assert-True $rejected 'read-only installed executable accepted for restart'
}
Test-Case 'replacement failure restarts the unchanged original' {
    $plan = New-Fixture $valid
    Prepare-Update $plan
    [IO.File]::SetAttributes($plan.target_exe, [IO.FileAttributes]::ReadOnly)
    $script:originalRestarts = 0
    $failed = $false
    try { Invoke-UpdateReplacement $plan { param($p) $script:originalRestarts++ } }
    catch { $failed = $true }
    finally { [IO.File]::SetAttributes($plan.target_exe, [IO.FileAttributes]::Normal) }
    Assert-True $failed 'read-only replace unexpectedly succeeded'
    Assert-True ($script:originalRestarts -eq 1) 'unchanged original was not restarted'
    Assert-True ((Get-Sha256 $plan.target_exe) -eq $plan.old_sha256) 'failed replace changed original'
}
Test-Case 'reject case-insensitive duplicate' {
    $plan = New-Fixture ($valid + "$root/license")
    $failed = $false; try { Prepare-Update $plan } catch { $failed = $true }
    Assert-True $failed 'duplicate accepted'
}
Test-Case 'reject symbolic link metadata' {
    $plan = New-Fixture ($valid + "$root/link") -Link
    $failed = $false; try { Prepare-Update $plan } catch { $failed = $true }
    Assert-True $failed 'link accepted'
}
Test-Case 'reject suspicious expansion' {
    $plan = New-Fixture $valid -Bomb
    $failed = $false; try { Prepare-Update $plan } catch { $failed = $true }
    Assert-True $failed 'expansion bomb accepted'
}
Test-Case 'reject archive hash mismatch' {
    $plan = New-Fixture $valid; $plan.archive_sha256 = '0' * 64
    $failed = $false; try { Prepare-Update $plan } catch { $failed = $true }
    Assert-True $failed 'bad hash accepted'
}
Test-Case 'Windows restart argument quoting preserves spaces, quotes and final backslash' {
    Assert-True ((Quote-WindowsArgument 'C:\中文 目录\') -ceq '"C:\中文 目录\\"') 'trailing slash quoting'
    Assert-True ((Quote-WindowsArgument 'a"b') -ceq '"a\"b"') 'embedded quote quoting'
    Assert-True ((Quote-WindowsArgument '--data-dir') -ceq '"--data-dir"') 'argument boundary quoting'
}
Test-Case 'reject changed old exe before replacement' {
    $plan = New-Fixture $valid; Prepare-Update $plan
    [IO.File]::WriteAllText($plan.target_exe, 'external replacement')
    $failed = $false; try { Invoke-UpdateReplacement $plan { param($p) } } catch { $failed = $true }
    Assert-True $failed 'changed old exe overwritten'
    Assert-True ([IO.File]::ReadAllText($plan.target_exe) -eq 'external replacement') 'external replacement lost'
}
Test-Case 'reject reparse installation directory before any replacement' {
    $plan = New-Fixture $valid
    $originalTarget = $plan.target_exe
    $alias = Join-Path ([IO.Path]::GetDirectoryName($plan.stage_dir)) 'install-junction'
    try { New-Item -ItemType Junction -Path $alias -Target ([IO.Path]::GetDirectoryName($plan.target_exe)) | Out-Null }
    catch { throw [NotSupportedException]::new('Sandbox denies junction creation; real reparse fixture remains unverified') }
    $plan.target_exe = Join-Path $alias ([IO.Path]::GetFileName($plan.target_exe))
    $failed = $false; try { Prepare-Update $plan } catch { $failed = $true }
    Assert-True $failed 'junction installation accepted'
    Assert-True ([IO.File]::ReadAllText($originalTarget) -eq 'old fixture binary') 'junction rejection changed original'
}
Test-Case 'tampered extracted exe plus forged manifest still fails archive comparison' {
    $plan = New-Fixture $valid; Prepare-Update $plan
    $candidate = Join-Path $plan.stage_dir "extracted/$root/WW-StudyApp.exe"
    [IO.File]::WriteAllText($candidate, 'tampered executable')
    $recordPath = Join-Path $plan.stage_dir 'prepared.json'
    $record = [IO.File]::ReadAllText($recordPath) | ConvertFrom-Json
    $record.candidate_sha256 = (Get-FileHash -LiteralPath $candidate -Algorithm SHA256).Hash.ToLowerInvariant()
    foreach ($file in $record.files) { if ($file.relative -eq "$root/WW-StudyApp.exe") { $file.sha256 = $record.candidate_sha256 } }
    [IO.File]::WriteAllText($recordPath, ($record | ConvertTo-Json -Depth 8))
    $failed = $false; try { Invoke-UpdateReplacement $plan { param($p) } } catch { $failed = $true }
    Assert-True $failed 'forged manifest accepted'
    Assert-True ([IO.File]::ReadAllText($plan.target_exe) -eq 'old fixture binary') 'tamper rejection changed original'
}
Test-Case 'replacement preserves backup, docs, renamed exe, data arguments and unrelated file' {
    $plan = New-Fixture $valid; Prepare-Update $plan
    $script:restart = $null
    Invoke-UpdateReplacement $plan { param($p) $script:restart = $p }
    Assert-True ([IO.File]::ReadAllText($plan.target_exe) -eq 'new fixture binary and license') 'new binary absent'
    Assert-True ([IO.File]::ReadAllText((Join-Path ([IO.Path]::GetDirectoryName($plan.target_exe)) 'user-note.md')) -eq 'must survive') 'unrelated file changed'
    Assert-True ($script:restart.restart_args[1] -like '*数据 目录') 'restart args changed'
    $status = [IO.File]::ReadAllText($plan.status_path) | ConvertFrom-Json
    Assert-True ($status.state -eq 'succeeded') 'success status missing'
    Assert-True ([IO.File]::ReadAllText($status.backup_path) -eq 'old fixture binary') 'backup missing'
    Assert-True (Test-Path -LiteralPath (Join-Path $status.distribution_path 'LICENSE')) 'license missing'
}
Test-Case 'restart failure rolls back exactly old executable with files retained' {
    $plan = New-Fixture $valid; Prepare-Update $plan
    $failed = $false
    try { Invoke-UpdateReplacement $plan { param($p) throw 'fixture restart failed' } } catch { $failed = $true }
    Assert-True $failed 'failure not reported'
    Assert-True ([IO.File]::ReadAllText($plan.target_exe) -eq 'old fixture binary') 'rollback failed'
    $status = [IO.File]::ReadAllText($plan.status_path) | ConvertFrom-Json
    Assert-True ($status.state -eq 'rolled_back') 'rollback status missing'
    Assert-True (Test-Path -LiteralPath $status.backup_path) 'backup not retained'
}
Test-Case 'serialized helper handoff waits for normal parent exit and restores nonlaunchable fixture' {
    $plan = New-Fixture $valid
    $powershell = Join-Path $env:SystemRoot 'System32/WindowsPowerShell/v1.0/powershell.exe'
    $parent = Start-Process -FilePath $powershell -ArgumentList @('-NoProfile', '-NonInteractive', '-Command', 'Start-Sleep -Seconds 6') -PassThru -WindowStyle Hidden
    $plan.parent_pid = $parent.Id
    $planPath = Join-Path $plan.stage_dir 'plan.json'
    $helperPath = Join-Path $plan.stage_dir 'update-helper.ps1'
    [IO.File]::WriteAllText($planPath, ($plan | ConvertTo-Json -Depth 8))
    [IO.File]::Copy([IO.Path]::GetFullPath($Helper), $helperPath, $false)
    $prepareArgs = @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', (Quote-WindowsArgument $helperPath), '-Mode', 'Prepare', '-PlanPath', (Quote-WindowsArgument $planPath))
    $prepare = Start-Process -FilePath $powershell -ArgumentList $prepareArgs -PassThru -WindowStyle Hidden
    Assert-True ($prepare.WaitForExit(15000)) 'prepare helper did not finish'
    Assert-True ($prepare.ExitCode -eq 0) 'serialized plan preparation failed'
    $applyArgs = @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', (Quote-WindowsArgument $helperPath), '-Mode', 'Apply', '-PlanPath', (Quote-WindowsArgument $planPath))
    $apply = Start-Process -FilePath $powershell -ArgumentList $applyArgs -PassThru -WindowStyle Hidden
    Assert-True (-not $apply.WaitForExit(500)) 'helper exited while parent remained alive'
    Assert-True ([IO.File]::ReadAllText($plan.target_exe) -eq 'old fixture binary') 'helper replaced before normal parent exit'
    Assert-True ($parent.WaitForExit(15000)) 'fixture parent did not exit normally'
    Assert-True ($apply.WaitForExit(15000)) 'helper did not finish after parent exit'
    Assert-True ($apply.ExitCode -ne 0) 'dummy text executable unexpectedly launched'
    Assert-True ([IO.File]::ReadAllText($plan.target_exe) -eq 'old fixture binary') 'real process launch failure did not restore fixture'
    $status = [IO.File]::ReadAllText($plan.status_path) | ConvertFrom-Json
    Assert-True ($status.state -eq 'rolled_back') 'serialized handoff rollback status missing'
    $prepare.Dispose(); $apply.Dispose(); $parent.Dispose()
}
Write-Output "RESULT $script:passed passed, $script:skipped skipped; fixtures retained: $fixtureRoot"
