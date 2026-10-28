#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][Alias('BinaryPath')][string]$UiTestBinaryPath,
    [Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [string]$PackageDirectory,
    [string]$PreviousPackageDirectory,
    [string]$PortablePackage,
    [string]$MsiPath,
    [string]$RuntimeDirectory
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
if($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows' -or -not $IsWindows) { throw 'Authentic native maintenance acceptance runs only on the isolated Windows CI runner' }
$output=[IO.Path]::GetFullPath($OutputDirectory)
$runner=[IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\')+'\'
if(-not $output.StartsWith($runner,[StringComparison]::OrdinalIgnoreCase) -or (Test-Path -LiteralPath $output)) { throw 'Native maintenance acceptance requires a fresh child of RUNNER_TEMP' }
$binary=(Resolve-Path -LiteralPath $UiTestBinaryPath).Path
if(-not $PortablePackage) { $PortablePackage=Join-Path $PackageDirectory "WW-StudyApp-$Version-windows-x86_64-portable.zip" }
if(-not $MsiPath) { $MsiPath=Join-Path $PackageDirectory "WW-StudyApp-$Version-windows-x86_64.msi" }
$portable=(Resolve-Path -LiteralPath $PortablePackage).Path
$msi=(Resolve-Path -LiteralPath $MsiPath).Path
[IO.Directory]::CreateDirectory($output) | Out-Null
if(-not $RuntimeDirectory) { . (Join-Path $PSScriptRoot 'windows-runtime.ps1');$runtime=Get-WindowsRuntime;$RuntimeDirectory=$runtime.Directory }
. (Join-Path $PSScriptRoot 'installer-policy.ps1')

function Get-FamilyOwners {
    param([string[]]$ExpectedProducts=@(),[string]$ExpectedVersion='',[string]$ExpectedRoot='', [int]$ExpectedCount=-1,[string]$Label='maintenance-owner')
    if(-not $ExpectedRoot) {$ExpectedRoot=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'Programs/WW-StudyApp'}
    Get-VerifiedInstallerOwners -OutputDirectory $output -Label $Label -ExpectedRoot $ExpectedRoot -ExpectedProducts $ExpectedProducts -ExpectedVersion $ExpectedVersion -ExpectedCount $ExpectedCount
}

function Invoke-NativeProcess {
    param([string]$Executable,[string[]]$Arguments,[string]$Label,[int[]]$Expected=@(0,3010))
    $process=Start-Process -FilePath $Executable -ArgumentList $Arguments -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $output "$Label.stdout.log") -RedirectStandardError (Join-Path $output "$Label.stderr.log")
    if(-not $process.WaitForExit(180000)) {$process.Kill();throw "Native installer timed out: $Label"}
    if($process.ExitCode -notin $Expected) {throw "Native installer $Label returned $($process.ExitCode)"}
    return $process.ExitCode
}

function Add-ActualMsiCases {
    param($Report)
    if(-not $PreviousPackageDirectory) {throw 'Authentic native acceptance requires private lifecycle fixtures'}
    $fixtures=(Resolve-Path -LiteralPath $PreviousPackageDirectory).Path
    $install=[IO.Path]::GetFullPath((Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'Programs/WW-StudyApp')).TrimEnd('\')
    if(@(Get-FamilyOwners -ExpectedCount 0).Count -ne 0 -or (Test-Path -LiteralPath $install)) {throw 'Native MSI acceptance requires a fresh disposable runner install root'}
    $cancelLog=Join-Path $output 'actual-installer-cancel.msi.log'
    $cancelCode=Invoke-NativeProcess (Join-Path $fixtures 'MsiControlNativeTests.exe') @(('"'+$msi+'"'),('"'+$cancelLog+'"')) 'actual-msi-cancel' @(1602)
    $cancel=Get-Content -LiteralPath "$cancelLog.json" -Raw | ConvertFrom-Json
    if($cancelCode -ne 1602 -or $cancel.msi_exit_code -ne 1602 -or $cancel.callback_requested -ne $true -or @(Get-FamilyOwners -ExpectedCount 0).Count -ne 0 -or (Test-Path -LiteralPath (Join-Path $install 'WW-StudyApp.exe'))) {throw 'Actual Windows Installer cancellation did not roll back registration/resources'}
    $Report.scenarios+= [pscustomobject]@{scenario='installer-cancel';passed=$true;executor='real';details=$cancel}
    $rebootMsi=Join-Path $fixtures 'RebootFixture.msi'
    $tables=Get-InstallerTables -MsiPath $rebootMsi
    $product=@($tables.Property|Where-Object {$_.Property -ceq 'ProductCode'})[0].Value
    if($product -notmatch '^\{[0-9A-Fa-f-]{36}\}$') {throw 'Reboot fixture product identity is invalid'}
    $msiexec=Join-Path ([Environment]::GetFolderPath('System')) 'msiexec.exe'
    try {
        $rebootCode=Invoke-NativeProcess (Join-Path $fixtures 'RebootSetup.exe') @('/quiet') 'actual-reboot-setup' @(3010)
        $owners=@(Get-FamilyOwners -ExpectedProducts @($product) -ExpectedVersion $Version -ExpectedRoot $install -ExpectedCount 1 -Label 'reboot-3010-postcondition')
        if($owners.Count -ne 1 -or $owners[0].ProductCode -ine $product -or -not $owners[0].CurrentUser -or $owners[0].Version -cne $Version -or [IO.Path]::GetFullPath($owners[0].Root).TrimEnd('\') -ine $install) {throw 'ScheduleReboot wrapper registered an unexpected owner/root'}
        $Report.scenarios+= [pscustomobject]@{scenario='reboot-3010';passed=$true;executor='real';details=@{actual_msiexec_and_wrapper_code=$rebootCode;standard_action='ScheduleReboot';automatic_restart=$false;product_code=$product}}
    } finally {
        foreach($owner in @(Get-FamilyOwners -ExpectedProducts @($product) -ExpectedVersion $Version -ExpectedRoot $install -Label 'reboot-finally-before-uninstall')) {
            if($owner.ProductCode -ieq $product -and $owner.CurrentUser -and [IO.Path]::GetFullPath($owner.Root).TrimEnd('\') -ieq $install) {$null=Invoke-NativeProcess $msiexec @('/x',$product,'/qn','/norestart','/l*v',('"'+(Join-Path $output 'reboot-uninstall.msi.log')+'"')) 'reboot-fixture-uninstall'}
        }
    }
    if(@(Get-FamilyOwners -ExpectedCount 0).Count -ne 0) {throw 'Native MSI fixture owner remains after cleanup'}
}

function Get-DataSnapshot {
    param([string]$Root,[switch]$SkipRuntimeLocks)
    $files=@{}
    foreach($file in Get-ChildItem -LiteralPath $Root -File -Recurse) {
        if($file.Attributes -band [IO.FileAttributes]::ReparsePoint) {throw 'Unexpected link in disposable application data'}
        # A launched application holds studyapp.lock and its SQLite database for
        # its whole lifetime. Only the live-process comparison may treat them as
        # runtime state: the lock is skipped and the database files are checked
        # for existence, because SQLite legitimately writes WAL frames while the
        # application is running. Fixture inventories still hash every file.
        if($SkipRuntimeLocks -and $file.Name -ieq 'studyapp.lock') {continue}
        if($SkipRuntimeLocks -and $file.Name -like 'studyapp.sqlite3*') {
            $files[[IO.Path]::GetRelativePath($Root,$file.FullName)]=@{runtime=$true}
            continue
        }
        $sha=$null
        try {$sha=(Get-FileHash -LiteralPath $file.FullName).Hash.ToLowerInvariant()}
        catch {
            if(-not $SkipRuntimeLocks) {throw}
        }
        $files[[IO.Path]::GetRelativePath($Root,$file.FullName)]=@{size=$file.Length;mtime=$file.LastWriteTimeUtc.Ticks;sha256=$sha}
    }
    return $files
}

function Assert-DataSnapshot {
    param([hashtable]$Before,[hashtable]$After)
    if($Before.Count -ne $After.Count -or $Before.Count -lt 2) {throw 'Running installer changed fixture data inventory'}
    foreach($name in $Before.Keys) {
        if(-not $After.ContainsKey($name)) {throw "Running installer changed fixture data: $name"}
        if($After[$name].ContainsKey('runtime')) {continue}
        if($After[$name].size -ne $Before[$name].size) {throw "Running installer changed fixture data: $name"}
        if($null -ne $Before[$name].sha256 -and $null -ne $After[$name].sha256) {
            if($After[$name].sha256 -cne $Before[$name].sha256) {throw "Running installer changed fixture data: $name"}
        } elseif($After[$name].mtime -ne $Before[$name].mtime) {
            throw "Running installer changed fixture data: $name"
        }
    }
}

function Get-OwnerSnapshot {
    @(Get-FamilyOwners | ForEach-Object {@{ProductCode=$_.ProductCode;Scope=$_.CurrentUser;Version=$_.Version;InstallLocation=$_.Root}})
}

function Assert-CurrentInstallerPayload {
    param([string]$Root,[hashtable]$Tables,[string]$ExpectedProduct)
    $owners=@(Get-FamilyOwners -ExpectedProducts @($ExpectedProduct) -ExpectedVersion $Version -ExpectedRoot $Root -ExpectedCount 1 -Label 'current-installer-payload')
    if($owners.Count -ne 1 -or $owners[0].ProductCode -ine $ExpectedProduct -or -not $owners[0].CurrentUser -or $owners[0].Version -cne $Version -or [IO.Path]::GetFullPath($owners[0].Root).TrimEnd('\') -ine $Root) {throw 'Running installer registered an incomplete or unexpected owner'}
    $zip=[IO.Compression.ZipFile]::OpenRead($portable)
    try {
        $entry=$zip.GetEntry("WW-StudyApp-$Version-windows-x86_64-portable/support/manifest.json")
        if($null -eq $entry) {throw 'Verified current package manifest missing'}
        $stream=$entry.Open();$memory=[IO.MemoryStream]::new()
        try {$stream.CopyTo($memory);$expectedManifest=$memory.ToArray()} finally {$stream.Dispose();$memory.Dispose()}
    } finally {$zip.Dispose()}
    $installedManifest=[IO.File]::ReadAllBytes((Join-Path $Root 'support/manifest.json'))
    if([Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($installedManifest)) -cne [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($expectedManifest))) {throw 'Installed current manifest differs from verified release archive'}
    $manifest=[Text.Encoding]::UTF8.GetString($expectedManifest) | ConvertFrom-Json
    if($manifest.version -cne $Version -or $manifest.files.Count -ne 6) {throw 'Running installer manifest differs'}
    foreach($resource in $manifest.files) {
        if($resource.relative_path -notin @(Get-InstallerOwnedPaths -Tables $Tables)) {throw 'Unexpected current owned path'}
        $file=Get-Item -LiteralPath (Join-Path $Root $resource.relative_path)
        if($file.Length -ne $resource.size -or (Get-FileHash -LiteralPath $file.FullName).Hash.ToLowerInvariant() -cne $resource.sha256) {throw 'Running installer did not complete exact current payload'}
    }
    if(Test-Path -LiteralPath (Join-Path $Root 'retired-runtime.txt')) {throw 'Obsolete MSI-owned payload remains'}
    return (Get-InstallerRollbackSnapshot -Root $Root -OwnedPaths @(Get-InstallerOwnedPaths -Tables $Tables) -Registration @(Get-OwnerSnapshot))
}

function Add-RunningInstallerCases {
    param($Report)
    $fixtures=(Resolve-Path -LiteralPath $PreviousPackageDirectory).Path
    $oldMsi=Join-Path $fixtures 'WW-StudyApp-0.0.1-windows-x86_64.msi'
    $setup=Join-Path (Split-Path -Parent $msi) "WW-StudyApp-$Version-windows-x86_64-Setup.exe"
    $oldTables=Get-InstallerTables -MsiPath $oldMsi
    $newTables=Get-InstallerTables -MsiPath $msi
    $oldProduct=@($oldTables.Property|Where-Object {$_.Property -ceq 'ProductCode'})[0].Value
    $newProduct=@($newTables.Property|Where-Object {$_.Property -ceq 'ProductCode'})[0].Value
    $install=[IO.Path]::GetFullPath((Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'Programs/WW-StudyApp')).TrimEnd('\')
    $msiexec=Join-Path ([Environment]::GetFolderPath('System')) 'msiexec.exe'
    $observed=@()
    foreach($kind in @('msi','setup')) {
        if(@(Get-FamilyOwners -ExpectedCount 0).Count -ne 0 -or (Test-Path -LiteralPath $install)) {throw 'Running-installer fixture requires fresh MSI ownership/root'}
        $data=Join-Path $output "running-$kind-data"
        [IO.Directory]::CreateDirectory($data) | Out-Null
        [IO.File]::WriteAllText((Join-Path $data 'retained-original.md'),'CI user original must survive a live installer')
        $application=$null;$lease=$null
        try {
            $null=Invoke-NativeProcess $msiexec @('/i',('"'+$oldMsi+'"'),'/qn','/norestart','/l*v',('"'+(Join-Path $output "running-$kind-old.msi.log")+'"')) "running-$kind-old"
            $image=Join-Path $install 'WW-StudyApp.exe'
            # Retain our launched child's OS handle; never find/kill by name or PID.
            $application=Start-Process -FilePath $image -ArgumentList @('--data-dir',('"'+$data+'"')) -WindowStyle Hidden -PassThru
            $null=$application.SafeHandle
            $created=$application.StartTime.ToUniversalTime().ToFileTimeUtc()
            # The image path can transiently fail to read right after launch.
            $imageDeadline=[DateTime]::UtcNow.AddSeconds(10)
            $actualImage=$null
            while($null -eq $actualImage) {
                try {$actualImage=[IO.Path]::GetFullPath($application.MainModule.FileName)} catch {$actualImage=$null}
                if($null -ne $actualImage) {break}
                if([DateTime]::UtcNow -ge $imageDeadline -or $application.HasExited) {throw 'Running fixture image is unreadable'}
                Start-Sleep -Milliseconds 100
                try {$application.Refresh()} catch { }
            }
            if($actualImage -ine $image) {throw 'Running fixture image differs'}
            $deadline=[DateTime]::UtcNow.AddSeconds(90)
            while(-not (Test-Path -LiteralPath (Join-Path $data 'studyapp.sqlite3'))) {
                if($application.HasExited -or [DateTime]::UtcNow -gt $deadline) {throw 'Installed application did not open its fresh data fixture'}
                Start-Sleep -Milliseconds 200
            }
            Start-Sleep -Seconds 2
            if($application.HasExited) {throw 'Installed application exited before the in-use transaction'}
            $beforeData=Get-DataSnapshot $data -SkipRuntimeLocks
            $before=Get-InstallerRollbackSnapshot -Root $install -OwnedPaths @(Get-InstallerOwnedPaths -Tables $oldTables) -Registration @(Get-OwnerSnapshot)
            $lease=[IO.File]::Open((Join-Path $install 'vcruntime140.dll'),[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)
            if($kind -ceq 'msi') {
                $code=Invoke-NativeProcess $msiexec @('/i',('"'+$msi+'"'),'/qn','/norestart','REINSTALLMODE=amus','MSIRESTARTMANAGERCONTROL=Disable','/l*v',('"'+(Join-Path $output 'running-msi-upgrade.msi.log')+'"')) 'running-msi-upgrade' @(0,3010,1603)
            } else {
                $code=Invoke-NativeProcess $setup @('/quiet') 'running-setup-upgrade' @(0,3010,1603)
            }
            if($application.HasExited -or $application.StartTime.ToUniversalTime().ToFileTimeUtc() -ne $created) {throw 'Installer closed/replaced the exact running application'}
            Assert-DataSnapshot $beforeData (Get-DataSnapshot $data -SkipRuntimeLocks)
            if($code -eq 1603) {
                $after=Get-InstallerRollbackSnapshot -Root $install -OwnedPaths @(Get-InstallerOwnedPaths -Tables $oldTables) -Registration @(Get-OwnerSnapshot)
                Assert-InstallerRollbackState -Before $before -After $after
                $outcome='refused-complete-old-state-retained'
            } else {
                $after=Assert-CurrentInstallerPayload $install $newTables $newProduct
                $outcome='complete-current-payload-and-owner'
            }
            [ordered]@{before=$before;after=$after;data_before=$beforeData;data_after=(Get-DataSnapshot $data -SkipRuntimeLocks)} | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath (Join-Path $output "running-$kind-snapshots.json") -Encoding utf8
            $observed+=@{installer=$kind;actual_exit_code=$code;outcome=$outcome;reboot_required=($code -eq 3010);live_process_id=$application.Id;live_process_created=$created;same_process_still_running=$true;data_bytes_unchanged=$true;synthetic_old_uses_current_desktop_bytes=$true}
        } finally {
            if($null -ne $lease) {$lease.Dispose()}
            if($null -ne $application) {
                try {if(-not $application.HasExited) {$application.Kill();if(-not $application.WaitForExit(30000)) {throw 'Exact disposable child did not exit'}}} finally {$application.Dispose()}
            }
            foreach($owner in @(Get-FamilyOwners -ExpectedProducts @($oldProduct,$newProduct) -ExpectedRoot $install -Label 'running-installer-finally-before-uninstall')) {
                if($owner.ProductCode -in @($oldProduct,$newProduct) -and $owner.CurrentUser -and [IO.Path]::GetFullPath($owner.Root).TrimEnd('\') -ieq $install) {$null=Invoke-NativeProcess $msiexec @('/x',$owner.ProductCode,'/qn','/norestart','/l*v',('"'+(Join-Path $output "running-$kind-uninstall.msi.log")+'"')) "running-$kind-uninstall"}
            }
        }
        if(@(Get-FamilyOwners -ExpectedCount 0).Count -ne 0) {throw 'Running-installer fixture left ownership after exact cleanup'}
        if(Test-Path -LiteralPath $install) {
            # Windows Installer defers removal of a file that was in use during
            # the deliberate in-use upgrade by scheduling a temporary rename; the
            # scheduler deletes that *.tmp at the next boot. Everything else must
            # be gone, so only deferred-removal temp files are tolerated.
            $remaining=@(Get-ChildItem -LiteralPath $install -Force -Recurse -File | ForEach-Object {$_.FullName.Substring($install.Length)})
            $unexpected=@($remaining | Where-Object {$_ -notmatch '(?i)\.tmp$'})
            if($unexpected.Count) {throw ('Running-installer fixture left resources after exact cleanup: '+($unexpected -join ','))}
            foreach($relative in $remaining) {Remove-Item -LiteralPath ($install + $relative) -Force}
            if(@(Get-ChildItem -LiteralPath $install -Force).Count -eq 0) {Remove-Item -LiteralPath $install -Force}
            $observed[-1]['deferred_removal_temp']=$remaining
        }
    }
    $Report.scenarios+= [pscustomobject]@{scenario='installer-running-app';passed=$true;executor='real';details=$observed}
}

function Get-AuthenticatedHelper {
    param($Handoff)
    $stage=[IO.Path]::GetFullPath($Handoff.stage)
    if(-not $stage.StartsWith($runner,[StringComparison]::OrdinalIgnoreCase)) {throw 'Authenticated handoff returned an unrelated fixture path'}
    if($Handoff.helper_pid -le 0 -or $Handoff.helper_created -le 0) {throw 'Authenticated helper process identity is missing'}
    $helper=Get-Process -Id $Handoff.helper_pid -ErrorAction SilentlyContinue
    if($null -eq $helper) {throw 'Authenticated helper is not observable before completion'}
    try {
        # The helper process may still be initializing when the parent returns, so
        # binding a live identity retries briefly instead of failing on a transient
        # Identity fields are checked once; only the MainModule read retries,
        # because it can transiently fail while the helper initializes.
        $null=$helper.SafeHandle
        if($helper.Id -ne $Handoff.helper_pid) {throw 'Authenticated helper process ID differs'}
        $created=$helper.StartTime.ToUniversalTime().ToFileTimeUtc()
        if($created -ne $Handoff.helper_created) {throw 'Authenticated helper creation time differs'}
        $deadline=[DateTime]::UtcNow.AddSeconds(5)
        $image=$null
        while($null -eq $image) {
            $module=$null
            try { $module=$helper.MainModule } catch { $module=$null }
            $name=$null
            if($null -ne $module) { try { $name=$module.FileName } catch { $name=$null } }
            if($null -ne $name) { $image=[IO.Path]::GetFullPath($name); break }
            if([DateTime]::UtcNow -ge $deadline) {
                $detail=''
                foreach($file in @(Get-ChildItem -LiteralPath $stage -File -ErrorAction SilentlyContinue | Where-Object {$_.Name -match '\.json$|\.log$'} | Select-Object -First 3)) {
                    try {
                        $text=(Get-Content -LiteralPath $file.FullName -Raw -ErrorAction Stop)
                        if($null -ne $text -and $text.Length -gt 400) {$text=$text.Substring(0,400)}
                        $detail+=(' '+$file.Name+': '+$text)
                    } catch { }
                }
                $helper.Dispose()
                throw ('Authenticated helper identity unavailable: image unreadable'+$detail)
            }
            Start-Sleep -Milliseconds 100
            try { $helper.Refresh() } catch { }
        }
        if($image -ine (Join-Path $stage 'WW-StudyApp.exe')) {throw 'Authenticated helper image differs from its unique stage'}
        return [pscustomobject]@{Process=$helper;Id=$helper.Id;Created=$created;Image=$image;Transaction=$Handoff.transaction_id}
    } catch {$helper.Dispose();throw}
}

function Wait-AuthenticatedResult {
    param($Handoff,[string]$Label,[ValidateSet('succeeded','rolled_back')][string]$ExpectedState='succeeded',[ValidateSet('window','silent-update')][string]$ResultMode='window',$AuthenticatedHelper)
    $stage=[IO.Path]::GetFullPath($handoff.stage)
    if(-not $stage.StartsWith($runner,[StringComparison]::OrdinalIgnoreCase)) { throw 'Authenticated handoff returned an unrelated fixture path' }
    $helper=$null
    if($ResultMode -ceq 'silent-update') {
        if($ExpectedState -cne 'succeeded') {throw 'Silent update requires succeeded state'}
        if($null -eq $AuthenticatedHelper) {throw 'Silent update requires a previously bound authenticated helper'}
        $helper=$AuthenticatedHelper.Process
        if($null -eq $helper -or $helper.Id -ne $handoff.helper_pid -or $AuthenticatedHelper.Id -ne $handoff.helper_pid -or $AuthenticatedHelper.Created -ne $handoff.helper_created -or $AuthenticatedHelper.Image -ine (Join-Path $stage 'WW-StudyApp.exe') -or $AuthenticatedHelper.Transaction -cne $handoff.transaction_id) {throw 'Bound authenticated helper identity differs'}
    } else {
        if($handoff.helper_pid -le 0 -or $handoff.helper_created -le 0) {throw 'Authenticated helper process identity is missing'}
        $helper=Get-Process -Id $handoff.helper_pid -ErrorAction SilentlyContinue
        if($null -ne $helper) {
            try {
                # Bind the live process handle before reading identity or waiting. A
                # reused PID is never waited on or terminated by this acceptance test.
                $null=$helper.SafeHandle
                if($helper.StartTime.ToUniversalTime().ToFileTimeUtc() -ne $handoff.helper_created) {
                    $helper.Dispose();$helper=$null
                } else {
                    $imageDeadline=[DateTime]::UtcNow.AddSeconds(5)
                    $actualImage=$null
                    while($null -eq $actualImage) {
                        try {$actualImage=[IO.Path]::GetFullPath($helper.MainModule.FileName)} catch {$actualImage=$null}
                        if($null -ne $actualImage) {break}
                        if($helper.HasExited -or [DateTime]::UtcNow -ge $imageDeadline) {throw 'Authenticated helper image is unreadable'}
                        Start-Sleep -Milliseconds 100
                        try {$helper.Refresh()} catch { }
                    }
                    if($actualImage -ine (Join-Path $stage 'WW-StudyApp.exe')) {throw 'Authenticated helper image differs from its unique stage'}
                }
            } catch {
                if($null -ne $helper -and $helper.HasExited) {$helper.Dispose();$helper=$null} else {throw}
            }
        }
    }
    try {
        $resultPath=Join-Path $stage 'result.json'
        $screenshot=Join-Path $stage 'maintenance-result.png'
        $deadline=[DateTime]::UtcNow.AddSeconds(150)
        $result=$null
        while([DateTime]::UtcNow -lt $deadline) {
            if((Test-Path -LiteralPath $resultPath -PathType Leaf) -and ($ResultMode -ceq 'silent-update' -or (Test-Path -LiteralPath $screenshot -PathType Leaf))) {
                # The helper creates the tracked file before writing its JSON.
                # Empty content (and JSON null) parses without throwing.
                try {
                    $result=Get-Content -LiteralPath $resultPath -Raw | ConvertFrom-Json
                    if($null -ne $result) {break}
                } catch { }
            }
            Start-Sleep -Milliseconds 200
        }
        if($null -eq $result -or $result.transaction_id -cne $handoff.transaction_id -or $result.state -cne $ExpectedState) { throw 'Authenticated child did not finish the exact transaction' }
        if($null -ne $helper) {
            if(-not $helper.WaitForExit(30000)) {throw 'Authenticated helper did not exit after its result'}
            if($helper.ExitCode -ne 0) {throw 'Authenticated helper failed after its result'}
        }
        if($ResultMode -ceq 'window') {
            $screenshotBytes=[IO.File]::ReadAllBytes($screenshot)
            if($screenshotBytes.Length -lt 100 -or [Convert]::ToHexString([byte[]]$screenshotBytes[0..7]) -cne '89504E470D0A1A0A') {throw 'Authenticated result window screenshot is missing or invalid'}
            Copy-Item -LiteralPath $screenshot -Destination (Join-Path $execution "$Label-result.png")
        }
        Copy-Item -LiteralPath $resultPath -Destination (Join-Path $execution "$Label-result.json")
        return $result
    } finally {if($null -ne $helper) {$helper.Dispose()}}
}

function Test-RecoveryRuntimePairDistinct {
    param([hashtable]$Candidate,[hashtable]$Current)
    return @('vcruntime140.dll','vcruntime140_1.dll').Where({$Candidate.ContainsKey($_) -and $Current.ContainsKey($_) -and $Candidate[$_] -cne $Current[$_]}).Count -eq 2
}

function Get-RecoveryRuntimePair {
    $zip=[IO.Compression.ZipFile]::OpenRead($portable)
    $current=@{}
    try {
        foreach($name in @('vcruntime140.dll','vcruntime140_1.dll')) {
            $entry=$zip.GetEntry("WW-StudyApp-$Version-windows-x86_64-portable/$name")
            if($null -eq $entry) {throw 'Current package runtime absent'}
            $stream=$entry.Open();$sha=[Security.Cryptography.SHA256]::Create()
            try {$current[$name]=[Convert]::ToHexString($sha.ComputeHash($stream)).ToLowerInvariant()} finally {$stream.Dispose();$sha.Dispose()}
        }
    } finally {$zip.Dispose()}
    $directories=[Collections.Generic.List[string]]::new()
    $directories.Add((Join-Path $env:WINDIR 'System32'))
    $locator=Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $installations=@(& $locator -all -products '*' -property installationPath)
    if($LASTEXITCODE -ne 0) {throw 'Cannot inspect runner Visual Studio runtime inventory'}
    foreach($installation in $installations) {
        $redist=Join-Path $installation 'VC/Redist/MSVC'
        if(-not (Test-Path -LiteralPath $redist -PathType Container)) {continue}
        foreach($versionDirectory in Get-ChildItem -LiteralPath $redist -Directory | Sort-Object Name -Descending) {
            $x64=Join-Path $versionDirectory.FullName 'x64'
            if(Test-Path -LiteralPath $x64 -PathType Container) {
                foreach($crt in Get-ChildItem -LiteralPath $x64 -Directory | Where-Object Name -Match '^Microsoft\.VC\d+\.CRT$') {$directories.Add($crt.FullName)}
            }
        }
    }
    $inventory=@();$selected=$null
    foreach($directory in $directories | Select-Object -Unique) {
        $files=@();$valid=$true;$candidateHashes=@{};$reason=$null
        foreach($name in @('vcruntime140.dll','vcruntime140_1.dll')) {
            try {
                $path=Join-Path $directory $name
                $info=Get-Item -LiteralPath $path
                if($info.PSIsContainer -or ($info.Attributes -band [IO.FileAttributes]::ReparsePoint)) {throw 'Runtime must be a regular file'}
                $stream=[IO.File]::OpenRead($path);$reader=[IO.BinaryReader]::new($stream)
                try {
                    $stream.Position=0x3c;$offset=$reader.ReadInt32()
                    if($offset -lt 64 -or $offset -gt $stream.Length-6) {throw 'Invalid PE offset'}
                    $stream.Position=$offset
                    if($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne 0x8664) {throw 'Runtime is not an x64 PE image'}
                } finally {$reader.Dispose();$stream.Dispose()}
                $signature=Get-AuthenticodeSignature -LiteralPath $path
                if($signature.Status -ne 'Valid' -or $null -eq $signature.SignerCertificate -or $signature.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation(?:,|$)') {throw 'Runtime lacks a valid Microsoft Corporation signature'}
                $hash=(Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant()
                $candidateHashes[$name]=$hash
                $files+=@{name=$name;source=$path;sha256=$hash;version=$info.VersionInfo.FileVersion;signature_status=[string]$signature.Status;signer=$signature.SignerCertificate.Subject;certificate_thumbprint=$signature.SignerCertificate.Thumbprint;machine='x64'}
            } catch {$valid=$false;$reason=$_.Exception.Message;break}
        }
        $different=$valid -and (Test-RecoveryRuntimePairDistinct $candidateHashes $current)
        $inventory+=@{directory=$directory;valid_complete_pair=$valid;both_differ_from_current=$different;files=$files;reason=$reason}
        if($valid -and $different -and $null -eq $selected) {$selected=$directory}
    }
    [ordered]@{current_package_hashes=$current;selected=$selected;candidates=$inventory;acquisition='existing-runner-files-only-no-download-or-byte-modification'} | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $output 'recovery-runtime-inventory.json') -Encoding utf8
    if($null -eq $selected) {throw 'No complete signed Microsoft x64 runtime pair with both DLL hashes different found; inspect recovery-runtime-inventory.json. Missing-DLL case evidence is retained separately.'}
    return $selected
}

function Assert-RecoveryHashes {
    param([string]$Root,$Expected,[switch]$CompleteTree)
    $rootPath=[IO.Path]::GetFullPath($Root).TrimEnd('\')+'\'
    $properties=@($Expected.PSObject.Properties)
    foreach($entry in $properties) {
        $path=[IO.Path]::GetFullPath((Join-Path $Root $entry.Name))
        if(-not $path.StartsWith($rootPath,[StringComparison]::OrdinalIgnoreCase) -or (Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant() -cne $entry.Value) {throw 'Recovery changed captured old program/receipt/user bytes'}
    }
    if($CompleteTree -and (Get-DataSnapshot $Root).Count -ne $properties.Count) {throw 'Recovery changed complete user fixture inventory'}
}

function Add-ActualRecoveryCases {
    param($Report,[string]$Driver)
    foreach($case in @('recovery-missing-runtime','recovery-mixed-runtime')) {
        $sourceRuntime=if($case -ceq 'recovery-missing-runtime') {$RuntimeDirectory} else {Get-RecoveryRuntimePair}
        $prepareRoot=Join-Path $output "$case-interrupted"
        $prepareSuite=if($case -ceq 'recovery-missing-runtime') {'recovery-prepare-missing'} else {'recovery-prepare-mixed'}
        $null=Invoke-Acceptance $Driver $prepareRoot '--maintenance-execution-smoke' $prepareSuite -ExpectedExit 86 -RecoveryRuntime $sourceRuntime
        $boundary=Get-Content -LiteralPath (Join-Path $prepareRoot 'recovery-boundary.json') -Raw | ConvertFrom-Json
        $snapshot=[IO.Path]::GetFullPath($boundary.snapshot)
        $expectedSnapshot=Join-Path $boundary.install_root ("WW-StudyApp-Recovery-"+$boundary.transaction_id)
        if($boundary.scenario -cne $case -or $snapshot -ine $expectedSnapshot -or -not $snapshot.StartsWith($runner,[StringComparison]::OrdinalIgnoreCase)) {throw 'Recovery snapshot path/scenario is unrelated to its actual interrupted transaction'}
        if($case -ceq 'recovery-missing-runtime') {
            if($boundary.boundary -cne 'backed_up' -or (Test-Path -LiteralPath (Join-Path $boundary.install_root $boundary.runtime_name))) {throw 'Required root runtime was not actually missing before snapshot launch'}
        } else {
            if($boundary.boundary -cne 'installed' -or $boundary.actual_mixed_generation -ne $true -or @($boundary.runtime_operations).Count -ne 2) {throw 'Actual mixed-runtime boundary evidence missing'}
            $oldPair=@{};$incomingPair=@{};$actualPair=@{}
            foreach($operation in $boundary.runtime_operations) {
                if($operation.relative_path -notin @('vcruntime140.dll','vcruntime140_1.dll')) {throw 'Unexpected runtime operation'}
                $oldPair[$operation.relative_path]=$operation.old.sha256
                $incomingPair[$operation.relative_path]=$operation.incoming.sha256
                $actualPair[$operation.relative_path]=(Get-FileHash -LiteralPath (Join-Path $boundary.install_root $operation.relative_path)).Hash.ToLowerInvariant()
                if($actualPair[$operation.relative_path] -cne $boundary.observed_runtime_hashes.($operation.relative_path)) {throw 'Actual runtime differs from checkpoint observation'}
            }
            if(-not (Test-RecoveryRuntimePairDistinct $oldPair $incomingPair)) {throw 'Both actual old runtime hashes must differ from incoming'}
            $completeOld=$true;$completeNew=$true
            foreach($name in @('vcruntime140.dll','vcruntime140_1.dll')) {
                if($actualPair[$name] -cne $oldPair[$name]) {$completeOld=$false}
                if($actualPair[$name] -cne $incomingPair[$name]) {$completeNew=$false}
                if($actualPair[$name] -cne $oldPair[$name] -and $actualPair[$name] -cne $incomingPair[$name]) {throw 'Actual runtime is neither captured old nor incoming'}
            }
            if($completeOld -or $completeNew) {throw 'Runtime pair is complete old/new rather than genuinely mixed'}
        }
        Assert-RecoveryHashes $boundary.data_root $boundary.data_hashes -CompleteTree
        $viewRoot=Join-Path $output "$case-view"
        # Start the complete old snapshot itself while the root remains broken;
        # no DLL/app/receipt is copied back before this genuine UI process runs.
        $null=Invoke-Acceptance (Join-Path $boundary.snapshot 'Uninstall.exe') $viewRoot '--maintenance-execution-smoke' 'recovery-ui'
        $recoveryHandoff=Get-Content -LiteralPath (Join-Path $viewRoot 'recovery-handoff.json') -Raw | ConvertFrom-Json
        if($recoveryHandoff.snapshot -ine $snapshot -or $recoveryHandoff.install_root -ine $boundary.install_root -or $recoveryHandoff.transaction_id -cne $boundary.transaction_id -or $recoveryHandoff.actual_recovery_button -ne $true -or $recoveryHandoff.production_ready_observed -ne $true) {throw 'Actual recovery UI did not authenticate the exact snapshot transaction'}
        $result=Wait-AuthenticatedResult $recoveryHandoff $case 'rolled_back'
        if($result.code -cne 'interrupted_update_recovered' -or $result.version -cne '0.0.1') {throw 'Real recovery helper did not restore the exact old version'}
        Assert-RecoveryHashes $boundary.install_root $boundary.old_hashes
        Assert-RecoveryHashes $boundary.data_root $boundary.data_hashes -CompleteTree
        $image=[IO.File]::ReadAllBytes((Join-Path $viewRoot 'recovery-ui.png'))
        if($image.Length -lt 100 -or [Convert]::ToHexString([byte[]]$image[0..7]) -cne '89504E470D0A1A0A') {throw 'Actual snapshot recovery UI screenshot missing'}
        Copy-Item -LiteralPath (Join-Path $viewRoot 'recovery-ui.png') -Destination (Join-Path $execution "$case-ui.png")
        $Report.scenarios+= [pscustomobject]@{scenario=$case;passed=$true;executor='real';details=@{actual_snapshot_executable=(Join-Path $snapshot 'Uninstall.exe');actual_crash_exit=86;transaction_id=$boundary.transaction_id;actual_recovery_ui_button=$true;authenticated_helper_exited=$true;result=$result;old_owned_receipt_and_unknown_unchanged=$true;user_data_tree_unchanged=$true;runtime_source=$sourceRuntime;boundary_evidence=(Join-Path $prepareRoot 'recovery-boundary.json');ui_screenshot="$case-ui.png";result_screenshot="$case-result.png"}}
        # Preserve the independent missing-DLL result even if inventory or the
        # later mixed-runtime case fails. The full gate still fails closed.
        $Report | ConvertTo-Json -Depth 30 | Set-Content -LiteralPath (Join-Path $execution 'maintenance-execution.json') -Encoding utf8
    }
}

function New-Driver {
    param([string]$Name,[switch]$Uninstaller)
    $path=Join-Path $output $Name
    [IO.Directory]::CreateDirectory($path) | Out-Null
    Copy-Item -LiteralPath $binary -Destination (Join-Path $path 'WW-StudyApp.exe')
    if($Uninstaller) { Copy-Item -LiteralPath $binary -Destination (Join-Path $path 'Uninstall.exe') }
    foreach($dll in @('vcruntime140.dll','vcruntime140_1.dll')) { Copy-Item -LiteralPath (Join-Path $RuntimeDirectory $dll) -Destination (Join-Path $path $dll) }
    return (Join-Path $path 'WW-StudyApp.exe')
}

function Invoke-Acceptance {
    param([string]$Driver,[string]$Root,[string]$Mode,[ValidateSet('journal','msi-owner','msi-cleanup','health','repair','cancel-selection','cancel-database-review','cancel-database-final','recovery-prepare-missing','recovery-prepare-mixed','recovery-ui')][string]$Suite='journal',[int]$ExpectedExit=0,[string]$RecoveryRuntime,[ref]$AuthenticatedHelper)
    if($PSBoundParameters.ContainsKey('AuthenticatedHelper') -and $Suite -cne 'msi-owner') {throw 'Early authenticated helper observation is reserved for the MSI update fixture'}
    $evidence=Join-Path $Root 'maintenance-execution.json'
    $arguments=@($Mode,'--fixture-root',('"'+$Root+'"'),'--package',('"'+$portable+'"'),'--msi',('"'+$msi+'"'),'--evidence',('"'+$evidence+'"'),'--data-dir',('"'+(Join-Path $Root 'fallback-data')+'"'))
    if($Suite -ne 'journal') {$arguments+=@('--suite',$Suite)}
    if($RecoveryRuntime) {$arguments+=@('--recovery-runtime',('"'+$RecoveryRuntime+'"'))}
    # Windows' default TEMP can differ from RUNNER_TEMP. Keep every helper
    # stage inside this run's already validated, fresh fixture directory.
    $fixtureTemp=Join-Path $output 'process-temp'
    [IO.Directory]::CreateDirectory($fixtureTemp) | Out-Null
    $oldTemp=[Environment]::GetEnvironmentVariable('TEMP','Process')
    $oldTmp=[Environment]::GetEnvironmentVariable('TMP','Process')
    try {
        $env:TEMP=$fixtureTemp;$env:TMP=$fixtureTemp
        $process=Start-Process -FilePath $Driver -ArgumentList $arguments -WindowStyle Hidden -PassThru -RedirectStandardOutput "$Root.stdout.log" -RedirectStandardError "$Root.stderr.log"
    } finally {
        [Environment]::SetEnvironmentVariable('TEMP',$oldTemp,'Process')
        [Environment]::SetEnvironmentVariable('TMP',$oldTmp,'Process')
    }
    $observingHelper=$PSBoundParameters.ContainsKey('AuthenticatedHelper')
    $driverBound=$false
    try {
        if($observingHelper) {
            $driverHandle=$process.SafeHandle
            if($null -eq $driverHandle) {throw 'MSI driver OS handle could not be bound'}
            $driverBound=$true
            # Bind before waiting for the parent, while the real MSI transaction
            # is in progress. An absent PID is never inferred to mean exit code 0.
            $handoffPath=Join-Path $Root 'msi-handoff.json'
            $deadline=[DateTime]::UtcNow.AddSeconds(120)
            $handoff=$null
            while([DateTime]::UtcNow -lt $deadline) {
                if(Test-Path -LiteralPath $handoffPath -PathType Leaf) {
                    try {$handoff=Get-Content -LiteralPath $handoffPath -Raw | ConvertFrom-Json;break} catch { }
                }
                if($process.HasExited) {throw 'MSI driver exited before its authenticated handoff was observed'}
                Start-Sleep -Milliseconds 50
            }
            if($null -eq $handoff) {throw 'MSI driver did not issue its authenticated handoff before timeout'}
            $AuthenticatedHelper.Value=Get-AuthenticatedHelper $handoff
        }
        if(-not $process.WaitForExit(600000)) { $process.Kill();throw "Authentic maintenance driver timed out: $Mode" }
        if($process.ExitCode -ne $ExpectedExit) { throw "Authentic maintenance driver failed ($Mode): expected $ExpectedExit, inspect $Root.stderr.log and its partial evidence" }
        $report=Get-Content -LiteralPath $evidence -Raw | ConvertFrom-Json
        if($report.schema -ne 1 -or $report.native_windows_runner -ne $true -or $report.version -cne $Version -or $report.source_sha -cne $env:GITHUB_SHA -or
            $report.msi_sha256 -cne (Get-FileHash -LiteralPath $msi).Hash.ToLowerInvariant() -or $report.complete -ne ($ExpectedExit -eq 0)) { throw 'Authentic maintenance report identity/completion differs' }
        if($ExpectedExit -eq 86 -and $report.intentional_crash_exit -ne 86) {throw 'Actual journal interruption evidence missing'}
        return $report
    } catch {
        $primaryError=$_
        if($driverBound) {
            try {
                # Only operate on our own retained OS handle. The helper may
                # remain active, so this never authorizes fixture uninstall.
                if(-not $process.HasExited -and -not $process.WaitForExit(30000)) {
                    $process.Kill()
                    if(-not $process.WaitForExit(30000)) {throw 'Bound MSI driver did not stop after termination'}
                }
            } catch {Write-Warning "MSI driver teardown also failed; fixture retained: $($_.Exception.Message)"}
        }
        throw $primaryError
    } finally {
        if($observingHelper) {
            try {$process.Dispose()} catch {Write-Warning "MSI driver handle disposal failed; fixture retained: $($_.Exception.Message)"}
        }
    }
}

$execution=Join-Path $output 'execution'
$driver=New-Driver 'driver'
$report=Invoke-Acceptance $driver $execution '--maintenance-execution-smoke'
$health=Invoke-Acceptance $driver (Join-Path $output 'health') '--maintenance-execution-smoke' 'health'
$report.scenarios+=@($health.scenarios)
foreach($cancelSuite in @('cancel-selection','cancel-database-review','cancel-database-final')) {
    $cancelReport=Invoke-Acceptance $driver (Join-Path $output $cancelSuite) '--maintenance-execution-smoke' $cancelSuite
    $report.scenarios+=@($cancelReport.scenarios)
}
$repair=Invoke-Acceptance $driver (Join-Path $output 'repair') '--maintenance-execution-smoke' 'repair'
$report.scenarios+=@($repair.scenarios)
Add-ActualRecoveryCases $report $driver
Add-ActualMsiCases $report
Add-RunningInstallerCases $report
$fixtures=(Resolve-Path -LiteralPath $PreviousPackageDirectory).Path
$oldMsi=Join-Path $fixtures 'NativeValidation-0.0.1.msi'
$oldTables=Get-InstallerTables -MsiPath $oldMsi
$newTables=Get-InstallerTables -MsiPath $msi
$oldProduct=@($oldTables.Property|Where-Object {$_.Property -ceq 'ProductCode'})[0].Value
$newProduct=@($newTables.Property|Where-Object {$_.Property -ceq 'ProductCode'})[0].Value
$install=[IO.Path]::GetFullPath((Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'Programs/WW-StudyApp')).TrimEnd('\')
$msiexec=Join-Path ([Environment]::GetFolderPath('System')) 'msiexec.exe'
if(@(Get-FamilyOwners -ExpectedCount 0).Count -ne 0 -or (Test-Path -LiteralPath $install)) {throw 'Real MSI adapter requires a fresh disposable runner installation'}
$msiHelper=$null
$msiCleanupSafe=$false
try {
    $null=Invoke-NativeProcess $msiexec @('/i',('"'+$oldMsi+'"'),'/qn','/norestart','/l*v',('"'+(Join-Path $output 'msi-adapter-old-install.log')+'"')) 'msi-adapter-old-install'
    $adapterRoot=Join-Path $output 'msi-owner'
    $null=Invoke-Acceptance (Join-Path $install 'WW-StudyApp.exe') $adapterRoot '--maintenance-execution-smoke' 'msi-owner' -AuthenticatedHelper ([ref]$msiHelper)
    $handoff=Get-Content -LiteralPath (Join-Path $adapterRoot 'msi-handoff.json') -Raw | ConvertFrom-Json
    if($handoff.install_root -ine $install -or $handoff.expected_version -cne $Version) {throw 'Authenticated MSI handoff root/version differs'}
    $result=Wait-AuthenticatedResult $handoff 'msi' -ResultMode 'silent-update' -AuthenticatedHelper $msiHelper
    $msiCleanupSafe=$true
    if($result.version -cne $Version -or $result.code -cne 'complete') {throw 'MSI helper did not complete real fresh-data storage-ready restart'}
    $null=Assert-CurrentInstallerPayload $install $newTables $newProduct
    if(Test-Path -LiteralPath $handoff.package_path) {throw 'Authenticated MSI retained its external tracked package'}
    foreach($entry in $handoff.portable_hashes.PSObject.Properties) {
        if($entry.Name -notmatch '^(?:support/)?[A-Za-z0-9][A-Za-z0-9_.-]*$' -or (Get-FileHash -LiteralPath (Join-Path $handoff.portable_root $entry.Name)).Hash.ToLowerInvariant() -cne $entry.Value) {throw 'Authenticated MSI update changed separate portable resources'}
    }
    if(-not [IO.Path]::GetFullPath($handoff.data_root).StartsWith($adapterRoot.TrimEnd('\')+'\',[StringComparison]::OrdinalIgnoreCase) -or -not (Test-Path -LiteralPath (Join-Path $handoff.data_root 'studyapp.sqlite3'))) {throw 'MSI actual restart did not use its fresh explicit data directory'}
    $applications=@()
    foreach($candidate in Get-Process) {
        try {
            $null=$candidate.SafeHandle
            if([IO.Path]::GetFullPath($candidate.MainModule.FileName) -ieq (Join-Path $install 'WW-StudyApp.exe') -and -not $candidate.HasExited) {$applications+=,$candidate;continue}
        } catch { }
        $candidate.Dispose()
    }
    if($applications.Count -ne 1) {foreach($candidate in $applications){$candidate.Dispose()};throw 'Expected one exact installed process after production storage-ready restart'}
    $application=$applications[0]
    try {
        $restartedPid=$application.Id;$restartedCreated=$application.StartTime.ToUniversalTime().ToFileTimeUtc()
        $application.Kill();if(-not $application.WaitForExit(30000)) {throw 'Exact restarted fixture process did not stop'}
    } finally {$application.Dispose()}
    $dataBeforeCleanup=Get-DataSnapshot $handoff.data_root
    $null=Invoke-Acceptance $driver (Join-Path $output 'msi-cleanup') '--maintenance-execution-smoke' 'msi-cleanup'
    Assert-DataSnapshot $dataBeforeCleanup (Get-DataSnapshot $handoff.data_root)
    $report.scenarios+= [pscustomobject]@{scenario='separate-installed-and-portable';passed=$true;executor='real';details=@{authenticated_handoff=$true;authenticated_helper_exited=$true;result=$result;separate_portable_unchanged=$true;actual_production_restart_pid=$restartedPid;actual_production_restart_created=$restartedCreated;fresh_data_root=$handoff.data_root;production_default_uninstall_cleanup=$true;result_window_expected=$false;result_json='msi-result.json'}}
} finally {
    if($null -ne $msiHelper) {$msiHelper.Process.Dispose()}
    if($msiCleanupSafe) {
        foreach($owner in @(Get-FamilyOwners -ExpectedProducts @($oldProduct,$newProduct) -ExpectedRoot $install -Label 'msi-adapter-finally-before-uninstall')) {
            if($owner.ProductCode -in @($oldProduct,$newProduct) -and $owner.CurrentUser -and [IO.Path]::GetFullPath($owner.Root).TrimEnd('\') -ieq $install) {$null=Invoke-NativeProcess $msiexec @('/x',$owner.ProductCode,'/qn','/norestart','/l*v',('"'+(Join-Path $output 'msi-adapter-uninstall.log')+'"')) 'msi-adapter-uninstall'}
        }
    } else {
        Write-Warning 'MSI fixture retained: authenticated helper completion was not safely observed; automatic uninstall is disabled'
    }
}
if(@(Get-FamilyOwners -ExpectedCount 0).Count -ne 0 -or (Test-Path -LiteralPath $install)) {throw 'Real MSI adapter left product/control files before the main installer lifecycle gate'}
$stagedDriver=New-Driver 'staged-runtime' -Uninstaller
$stagedRoot=Join-Path $output 'staged'
$null=Invoke-Acceptance $stagedDriver $stagedRoot '--maintenance-staged-uninstall-smoke'
$handoff=Get-Content -LiteralPath (Join-Path $stagedRoot 'staged-handoff.json') -Raw | ConvertFrom-Json
if($handoff.install_root -ine (Split-Path -Parent $stagedDriver)) {throw 'Authenticated uninstall root differs'}
$result=Wait-AuthenticatedResult $handoff 'maintenance'
foreach($relative in $handoff.owned_paths) {
    if($relative -notmatch '^(?:support/)?[A-Za-z0-9][A-Za-z0-9_.-]*$' -or (Test-Path -LiteralPath (Join-Path $handoff.install_root $relative))) { throw 'Root application/DLL resources remain loaded or were not removed' }
}
foreach($entry in $handoff.data) {
    $dataPath=[IO.Path]::GetFullPath($entry.path)
    if(-not $dataPath.StartsWith($stagedRoot.TrimEnd('\')+'\',[StringComparison]::OrdinalIgnoreCase) -or (Get-FileHash -LiteralPath $dataPath).Hash.ToLowerInvariant() -cne $entry.sha256) { throw 'Staged default uninstall changed fixture data' }
}
$report.scenarios+= [pscustomobject]@{scenario='staged-root-handles-released';passed=$true;executor='real';details=@{transaction_id=$handoff.transaction_id;result=$result;root_exe_and_dll_removed=$true;default_data_preserved=$true;result_screenshot='maintenance-result.png';helper_exited=$true}}
$report | ConvertTo-Json -Depth 30 | Set-Content -LiteralPath (Join-Path $execution 'maintenance-execution.json') -Encoding utf8
if($env:GITHUB_OUTPUT) { "maintenance-execution=$execution" | Add-Content -LiteralPath $env:GITHUB_OUTPUT -Encoding utf8 }
Write-Output "Authentic maintenance scenarios executed: $($report.scenarios.Count). Evidence: $execution"
