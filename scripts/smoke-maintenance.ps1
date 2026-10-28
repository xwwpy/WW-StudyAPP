#requires -Version 7.0
[CmdletBinding()]
param(
    [ValidateSet('Installer','Ui','All')][string]$Phase = 'All',
    [Parameter(Mandatory)][string]$OutputDirectory,
    [string]$PackageDirectory,
    [ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version,
    [string]$PreviousPackageDirectory,
    [ValidatePattern('^\d+\.\d+\.\d+$')][string]$PreviousVersion = '0.0.1',
    [string]$UiTestBinaryPath,
    [string]$UiTestRuntimeDirectory,
    [string]$ExecutionEvidenceDirectory
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows' -or -not $IsWindows) { throw 'Native maintenance regression runs only on the isolated Windows CI runner' }
$output = [IO.Path]::GetFullPath($OutputDirectory)
$runnerRoot = [IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\') + '\'
if (-not $output.StartsWith($runnerRoot,[StringComparison]::OrdinalIgnoreCase) -or (Test-Path -LiteralPath $output)) { throw 'Maintenance evidence requires a fresh child of RUNNER_TEMP' }
[IO.Directory]::CreateDirectory($output) | Out-Null
$results = [Collections.Generic.List[object]]::new()
$oldBrowserFolder=[Environment]::GetEnvironmentVariable('WEBVIEW2_BROWSER_EXECUTABLE_FOLDER','Process')
$msiexec = Join-Path ([Environment]::GetFolderPath('System')) 'msiexec.exe'
. (Join-Path $PSScriptRoot 'installer-policy.ps1')

function Get-Owners {
    param([string[]]$ExpectedProducts=@(),[string]$ExpectedVersion='', [int]$ExpectedCount=-1,[string]$Label='lifecycle-owner')
    $root=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'Programs/WW-StudyApp'
    Get-VerifiedInstallerOwners -OutputDirectory $output -Label $Label -ExpectedRoot $root -ExpectedProducts $ExpectedProducts -ExpectedVersion $ExpectedVersion -ExpectedCount $ExpectedCount
}

function Invoke-Msi {
    param([string]$Label,[string[]]$MsiArguments,[int[]]$ExpectedCodes = @(0,3010))
    $log = Join-Path $output "$Label.msi.log"
    $process = Start-Process -FilePath $msiexec -ArgumentList ($MsiArguments + @('/qn','/norestart','/l*v',('"' + $log + '"'))) -WindowStyle Hidden -PassThru
    if (-not $process.WaitForExit(180000)) { $process.Kill(); throw "MSI exceeded three minutes: $Label" }
    $results.Add([ordered]@{scenario=$Label;exit_code=$process.ExitCode;reboot_required=($process.ExitCode -eq 3010);log=$log})
    if ($process.ExitCode -notin $ExpectedCodes) { throw "MSI $Label returned $($process.ExitCode); inspect $log" }
}

function Invoke-Setup {
    param([string]$Label,[string]$Path)
    # Keep the MSI log of every Setup run so a failure explains itself in evidence.
    $setupLog = Join-Path $output "$Label.setup.msi.log"
    $previousLog = $env:WW_STUDYAPP_SETUP_LOG
    $env:WW_STUDYAPP_SETUP_LOG = $setupLog
    try {
        $process = Start-Process -FilePath $Path -ArgumentList '/quiet' -WindowStyle Hidden -PassThru
        if (-not $process.WaitForExit(180000)) { $process.Kill(); throw "Setup exceeded three minutes: $Label" }
    } finally {
        $env:WW_STUDYAPP_SETUP_LOG = $previousLog
    }
    $results.Add([ordered]@{scenario=$Label;exit_code=$process.ExitCode;reboot_required=($process.ExitCode -eq 3010);log=$setupLog})
    if ($process.ExitCode -notin @(0,3010)) { throw "Setup $Label returned $($process.ExitCode); inspect $setupLog" }
}

function Assert-Owner {
    param([string]$ProductCode,[string]$ExpectedVersion)
    $owners = @(Get-Owners -ExpectedProducts @($ProductCode) -ExpectedVersion $ExpectedVersion -ExpectedCount 1 -Label 'assert-owner')
    if ($owners.Count -ne 1 -or $owners[0].ProductCode -ine $ProductCode -or -not $owners[0].Scope -or
        $owners[0].Version -cne $ExpectedVersion -or [IO.Path]::GetFullPath($owners[0].InstallLocation).TrimEnd('\') -ine $installRoot) { throw 'Installer has not registered exactly one expected current-user owner/location' }
    $owners | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $output "owner-$($results.Count).json") -Encoding utf8
}

function Assert-InstalledPayload {
    $manifest = Get-Content -LiteralPath (Join-Path $installRoot 'support/manifest.json') -Raw | ConvertFrom-Json
    if ($manifest.version -cne $Version) { throw 'Installed manifest version differs' }
    foreach ($entry in $manifest.files) {
        if ($entry.relative_path -cnotin @('WW-StudyApp.exe','Uninstall.exe','vcruntime140.dll','vcruntime140_1.dll','support/LICENSE','support/THIRD-PARTY-NOTICES.txt')) { throw 'Unexpected installed resource' }
        $file = Join-Path $installRoot $entry.relative_path
        if ((Get-Item -LiteralPath $file).Length -ne $entry.size -or (Get-FileHash -LiteralPath $file).Hash.ToLowerInvariant() -cne $entry.sha256) { throw 'Installed payload differs from manifest' }
    }
    if ($manifest.files.Count -ne 6 -or (Get-FileHash -LiteralPath (Join-Path $installRoot 'Uninstall.exe')).Hash -cne (Get-FileHash -LiteralPath (Join-Path $installRoot 'WW-StudyApp.exe')).Hash) { throw 'Installed payload executable equality/count differs' }
}

function Assert-Preserved {
    foreach ($path in $sentinels.Keys) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf) -or (Get-FileHash -LiteralPath $path).Hash -cne $sentinels[$path]) { throw 'Raw MSI maintenance changed unknown/user data' }
    }
}

try {
    if ($Phase -in @('Installer','All')) {
        if (-not $PackageDirectory -or -not $Version -or -not $PreviousPackageDirectory) { throw 'Installer matrix requires current and previous/synthetic packages, with the current version' }
        if ([version]$PreviousVersion -ge [version]$Version) { throw 'Previous installer version must be lower' }
        $packages = (Resolve-Path -LiteralPath $PackageDirectory).Path
        $previous = (Resolve-Path -LiteralPath $PreviousPackageDirectory).Path
        $name = "WW-StudyApp-$Version-windows-x86_64"
        $oldName = "WW-StudyApp-$PreviousVersion-windows-x86_64"
        $msi = Join-Path $packages "$name.msi"
        $setup = Join-Path $packages "$name-Setup.exe"
        $oldMsi = Join-Path $previous "$oldName.msi"
        $oldSetup = Join-Path $previous "$oldName-Setup.exe"
        $tables = Get-InstallerTables -MsiPath $msi
        Assert-InstallerTables -Tables $tables -Version $Version
        $tables | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $output 'msi-tables.json') -Encoding utf8
        $productCode = @($tables.Property | Where-Object { $_.Property -ceq 'ProductCode' })[0].Value
        $oldTables = Get-InstallerTables -MsiPath $oldMsi
        $oldProductCode = @($oldTables.Property | Where-Object { $_.Property -ceq 'ProductCode' })[0].Value
        $installRoot = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'Programs/WW-StudyApp'
        $installRoot = [IO.Path]::GetFullPath($installRoot).TrimEnd('\')
        if (@(Get-Owners -ExpectedCount 0).Count -ne 0 -or (Test-Path -LiteralPath $installRoot)) { throw 'Installer smoke requires a disposable runner profile without an existing application/root' }
        $sentinels = @{}
        try {
            Invoke-Msi 'clean-raw-msi' @('/i',('"' + $msi + '"'))
            Assert-Owner $productCode $Version
            Assert-InstalledPayload
            # Deliberately keep user files inside and outside the install directory.
            foreach ($path in @((Join-Path $installRoot 'unknown-neighbour.txt'),(Join-Path $installRoot 'library/original-note.md'),(Join-Path $installRoot 'library/studyapp.sqlite3'),(Join-Path $installRoot 'library/settings/custom.json'),(Join-Path $output 'library/original-note.md'))) {
                [IO.Directory]::CreateDirectory((Split-Path -Parent $path)) | Out-Null
                [IO.File]::WriteAllText($path,'preserve-'+[guid]::NewGuid().ToString('N'))
                $sentinels[$path] = (Get-FileHash -LiteralPath $path).Hash
            }
            $repairPath = Join-Path $installRoot 'Uninstall.exe'
            # One explicitly verified owned file is removed to exercise real MSI repair.
            if ((Get-FileHash -LiteralPath $repairPath).Hash -cne (Get-FileHash -LiteralPath (Join-Path $installRoot 'WW-StudyApp.exe')).Hash) { throw 'Repair fixture is not the verified owned executable' }
            Remove-Item -LiteralPath $repairPath
            Invoke-Msi 'repair' @('/fa',('"' + $msi + '"'))
            Assert-InstalledPayload
            Assert-Preserved
            Invoke-Setup 'same-version-setup-one-owner' $setup
            Assert-Owner $productCode $Version
            Invoke-Msi 'raw-uninstall-retain-data' @('/x',$productCode)
            if (@(Get-Owners -ExpectedCount 0).Count -ne 0) { throw 'Raw uninstall left ARP owner' }
            Assert-Preserved
            Invoke-Setup 'clean-setup' $setup
            Assert-Owner $productCode $Version
            Assert-InstalledPayload
            Invoke-Msi 'uninstall-setup-owner' @('/x',$productCode)
            Assert-Preserved

            foreach ($direction in @('msi-to-setup','setup-to-msi')) {
                if ($direction -eq 'msi-to-setup') { Invoke-Msi 'old-raw-msi' @('/i',('"' + $oldMsi + '"')) }
                else { Invoke-Setup 'old-setup' $oldSetup }
                Assert-Owner $oldProductCode $PreviousVersion
                # Synthetic baseline has a retired owned component. A real historical
                # baseline may omit it; record that coverage explicitly in the report.
                $retired = Join-Path $installRoot 'retired-runtime.txt'
                $hadRetired = Test-Path -LiteralPath $retired -PathType Leaf
                if ($direction -eq 'msi-to-setup') {
                    $failure = Join-Path $previous 'FailedUpgrade.msi'
                    if (-not (Test-Path -LiteralPath $failure -PathType Leaf)) { throw 'Installer regression requires its controlled failed-upgrade fixture' }
                    $oldOwnedPaths=@(Get-InstallerOwnedPaths -Tables $oldTables)
                    $incomingOwnedPaths=@(Get-InstallerOwnedPaths -Tables $tables)
                    $newOwnedPaths=@($incomingOwnedPaths | Where-Object { $_ -cnotin $oldOwnedPaths })
                    $beforeRollback=Get-InstallerRollbackSnapshot -Root $installRoot -OwnedPaths $oldOwnedPaths -AbsentPaths $newOwnedPaths -Registration @(Get-Owners)
                    if(@($beforeRollback.AbsentPaths.Values | Where-Object { $_ -ne $true }).Count) { throw 'Failed-upgrade fixture has an existing incoming-path collision' }
                    $beforeRollback | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $output 'rollback-before.json') -Encoding utf8
                    Invoke-Msi 'failed-upgrade-rollback' @('/i',('"'+$failure+'"')) @(1603)
                    Assert-Owner $oldProductCode $PreviousVersion
                    $afterRollback=Get-InstallerRollbackSnapshot -Root $installRoot -OwnedPaths $oldOwnedPaths -AbsentPaths $newOwnedPaths -Registration @(Get-Owners)
                    $afterRollback | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $output 'rollback-after.json') -Encoding utf8
                    Assert-InstallerRollbackState -Before $beforeRollback -After $afterRollback
                    Assert-Preserved
                }
                if ($direction -eq 'msi-to-setup') { Invoke-Setup $direction $setup }
                else { Invoke-Msi $direction @('/i',('"' + $msi + '"')) }
                Assert-Owner $productCode $Version
                Assert-InstalledPayload
                Assert-Preserved
                if ($hadRetired -and (Test-Path -LiteralPath $retired)) { throw 'Upgrade retained the retired MSI-owned component' }
                $results.Add([ordered]@{scenario="$direction-old-component-removal";exercised=$hadRetired})
                Invoke-Msi "$direction-downgrade-rejected" @('/i',('"' + $oldMsi + '"')) @(1603)
                Assert-Owner $productCode $Version
                Assert-InstalledPayload
                Invoke-Msi "$direction-uninstall" @('/x',$productCode)
                Assert-Preserved
            }
        } finally {
            foreach ($owner in @(Get-Owners -ExpectedProducts @($productCode,$oldProductCode) -Label 'lifecycle-finally-before-uninstall')) {
                if ($owner.ProductCode -in @($productCode,$oldProductCode) -and $owner.Scope -and [IO.Path]::GetFullPath($owner.InstallLocation).TrimEnd('\') -ieq $installRoot) { Invoke-Msi 'final-owned-uninstall' @('/x',$owner.ProductCode) }
            }
        }
    }

    if ($Phase -in @('Ui','All')) {
        if (-not $UiTestBinaryPath) { throw 'UI matrix requires a separate ui-test binary' }
        $binary = (Resolve-Path -LiteralPath $UiTestBinaryPath).Path
        # Copy a complete bounded runtime fixture away from the checkout binary.
        $runtime = Join-Path $output 'ui-runtime'
        [IO.Directory]::CreateDirectory($runtime) | Out-Null
        $sourceRoot = Split-Path -Parent $binary
        if (-not $UiTestRuntimeDirectory) {
            . (Join-Path $PSScriptRoot 'windows-runtime.ps1')
            $uiRuntime = Get-WindowsRuntime
            $UiTestRuntimeDirectory = $uiRuntime.Directory
        }
        foreach ($name in @('WW-StudyApp.exe','vcruntime140.dll','vcruntime140_1.dll')) {
            $source = if ($name -eq 'WW-StudyApp.exe') { $binary } else { Join-Path $UiTestRuntimeDirectory $name }
            Copy-Item -LiteralPath $source -Destination (Join-Path $runtime $name)
        }
        $unavailableWebview=Join-Path $output 'unavailable-webview2'
        [IO.Directory]::CreateDirectory($unavailableWebview) | Out-Null
        if(@(Get-ChildItem -LiteralPath $unavailableWebview).Count -ne 0) {throw 'Browser-unavailable fixture must be empty'}
        $env:WEBVIEW2_BROWSER_EXECUTABLE_FOLDER=$unavailableWebview
        foreach ($scenario in @('missing','corrupt','confirmations')) {
            $data = Join-Path $output "ui-$scenario"
            $process = Start-Process -FilePath (Join-Path $runtime 'WW-StudyApp.exe') -ArgumentList @('--maintenance-smoke',$scenario,'--data-dir',('"'+$data+'"')) -WindowStyle Hidden -PassThru -RedirectStandardOutput "$data.stdout.log" -RedirectStandardError "$data.stderr.log"
            if (-not $process.WaitForExit(120000)) { $process.Kill(); throw "Maintenance UI timed out: $scenario" }
            if ($process.ExitCode -ne 0) { throw "Maintenance UI failed: $scenario" }
            $evidence = Get-Content -LiteralPath (Join-Path $data 'maintenance-smoke.json') -Raw | ConvertFrom-Json
            $calls = if ($scenario -eq 'confirmations') { 1 } else { 0 }
            if ($evidence.scenario -cne $scenario -or $evidence.passed -ne $true -or $evidence.stages.Count -eq 0 -or
                $evidence.executor_calls -ne $calls -or $evidence.database_unchanged -ne $true -or $evidence.settings_unchanged -ne $true -or $evidence.settings_absent -ne ($scenario -ne 'corrupt') -or
                $evidence.selection.configuration -ne $false -or $evidence.selection.database -ne ($scenario -eq 'confirmations')) { throw 'Maintenance UI evidence is incomplete or disagrees with the scenario' }
            foreach ($image in @('maintenance-selection.png') + $(if ($scenario -eq 'confirmations') { @('maintenance-database-review.png','maintenance-database-final.png') } else { @() })) {
                if (-not (Test-Path -LiteralPath (Join-Path $data $image) -PathType Leaf)) { throw 'Maintenance UI screenshot missing' }
            }
            $results.Add([ordered]@{scenario="ui-$scenario";passed=$true;executor='fake-only';evidence=$data})
        }
        $results.Add([ordered]@{scenario='maintenance-without-webview2';passed=$true;executor='real';details=@{runtime_mode='browser-executable-folder-forced-unavailable';browser_folder=$unavailableWebview;native_views=@('missing','corrupt','confirmations');screenshots_verified=$true}})
    }

    if ($Phase -eq 'All') {
        # These are independent, authentic executor/native requirements. Fake UI
        # callback coverage above cannot satisfy them. Fail closed until supplied.
        $required = @('recovery-missing-runtime','recovery-mixed-runtime','installer-cancel','installer-running-app','locked-running-app','actual-storage-ready-health','failed-transaction-rollback','reboot-3010','uninstall-default-retain','uninstall-config-only','uninstall-db-only','uninstall-both','cancel-selection','cancel-database-review','cancel-database-final','uninstall-corrupt-db','uninstall-missing-db','modified-and-unknown-adjacent','tracked-original-inside-and-outside','shared-library-lock','separate-installed-and-portable','maintenance-without-webview2','staged-root-handles-released','legacy-schema1-accepts-legacy-rejects-modern','legacy-skip-release','same-version-repair','unknown-uninstaller-conflict','offline-repair-retry')
        if (-not $ExecutionEvidenceDirectory) { throw 'Full maintenance gate awaits authentic executor/native evidence; Installer/Ui phases alone do not complete Task 3/4' }
        $execution = Get-Content -LiteralPath (Join-Path $ExecutionEvidenceDirectory 'maintenance-execution.json') -Raw | ConvertFrom-Json
        if ($execution.schema -ne 1 -or $execution.native_windows_runner -ne $true -or $execution.version -cne $Version -or
            $execution.source_sha -cne $env:GITHUB_SHA -or $execution.msi_sha256 -cne (Get-FileHash -LiteralPath $msi).Hash.ToLowerInvariant()) { throw 'Invalid authentic executor evidence identity' }
        $actualRecords=@($execution.scenarios)+@($results | Where-Object {$_.scenario -ceq 'maintenance-without-webview2'})
        foreach ($scenario in $required) {
            $records = @($actualRecords | Where-Object { $_.scenario -ceq $scenario -and $_.passed -eq $true -and $_.executor -ceq 'real' })
            if ($records.Count -ne 1) { throw "Missing authentic regression evidence: $scenario" }
        }
        [ordered]@{schema=1;source_sha=$env:GITHUB_SHA;version=$Version;msi_sha256=$execution.msi_sha256;scenarios=$actualRecords} | ConvertTo-Json -Depth 30 | Set-Content -LiteralPath (Join-Path $output 'maintenance-native-acceptance.json') -Encoding utf8
    }
    Write-Output "Maintenance $Phase phase passed; evidence: $output"
} finally {
    [Environment]::SetEnvironmentVariable('WEBVIEW2_BROWSER_EXECUTABLE_FOLDER',$oldBrowserFolder,'Process')
    $results | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $output 'maintenance-results.json') -Encoding utf8
}
