#requires -Version 7.0
param([Alias('Case')][string[]]$SelectedCase = @())
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'installer-policy.ps1')

function New-ValidTables {
    $files = @(Get-InstallerPayload)
    $components = @($files | ForEach-Object { @{Component=$_.Id;ComponentId=([guid]::NewGuid().ToString('B'));Directory_=$_.Directory;Attributes=260;KeyPath=$_.Id} })
    $components += @{Component='Shortcuts';ComponentId=([guid]::NewGuid().ToString('B'));Directory_='APPLICATIONMENU';Attributes=260;KeyPath='ShortcutKey'}
    @{
        Property=@(@{Property='ProductName';Value='WW-StudyApp'},@{Property='ProductVersion';Value='1.2.3'},@{Property='ProductCode';Value='{11111111-1111-4111-8111-111111111111}'},@{Property='UpgradeCode';Value='{4CF5A7C2-094D-4D97-AB25-24C6C2BAA4CD}'},@{Property='ARPNOMODIFY';Value='1'},@{Property='MSIRESTARTMANAGERCONTROL';Value='Disable'})
        Directory=@(@{Directory='TARGETDIR';Directory_Parent='';DefaultDir='SourceDir'},@{Directory='LocalAppDataFolder';Directory_Parent='TARGETDIR';DefaultDir='.'},@{Directory='PROGRAMS';Directory_Parent='LocalAppDataFolder';DefaultDir='Programs'},@{Directory='INSTALLFOLDER';Directory_Parent='PROGRAMS';DefaultDir='WW-StudyApp'},@{Directory='SUPPORT';Directory_Parent='INSTALLFOLDER';DefaultDir='support'},@{Directory='ProgramMenuFolder';Directory_Parent='TARGETDIR';DefaultDir='.'},@{Directory='APPLICATIONMENU';Directory_Parent='ProgramMenuFolder';DefaultDir='WW-StudyApp'})
        File=@($files | ForEach-Object { @{File=$_.Id;Component_=$_.Id;FileName=$_.Name;FileSize=10} })
        Component=$components
        Registry=@($components | ForEach-Object { @{Registry=$_.KeyPath;Root=1;Key='Software\WW-StudyApp\Installer';Name=$_.Component;Value='1';Component_=$_.Component} })
        Upgrade=@(@{UpgradeCode='{4CF5A7C2-094D-4D97-AB25-24C6C2BAA4CD}';VersionMin='';VersionMax='1.2.3';Attributes=513;ActionProperty='WIX_UPGRADE_DETECTED'},@{UpgradeCode='{4CF5A7C2-094D-4D97-AB25-24C6C2BAA4CD}';VersionMin='1.2.3';VersionMax='';Attributes=2;ActionProperty='WIX_DOWNGRADE_DETECTED'})
        InstallExecuteSequence=@(@{Action='InstallInitialize';Sequence=1500;Condition=''},@{Action='RemoveExistingProducts';Sequence=1501;Condition=''},@{Action='InstallFiles';Sequence=4000;Condition=''},@{Action='InstallFinalize';Sequence=6600;Condition=''},@{Action='SetInstallLocation';Sequence=1000;Condition='NOT Installed'})
        CustomAction=@(@{Action='SetInstallLocation';Type=51;Source='ARPINSTALLLOCATION';Target='[INSTALLFOLDER]'})
        LaunchCondition=@(@{Condition='NOT WIX_DOWNGRADE_DETECTED';Description='A newer version is installed.'})
        Shortcut=@(@{Shortcut='ApplicationShortcut';Directory_='APPLICATIONMENU';Component_='Shortcuts';Target='[INSTALLFOLDER]WW-StudyApp.exe'},@{Shortcut='UninstallShortcut';Directory_='APPLICATIONMENU';Component_='Shortcuts';Target='[INSTALLFOLDER]Uninstall.exe'})
        RemoveFile=@(@{FileKey='RemoveSupport';Component_='License';FileName='';DirProperty='SUPPORT';InstallMode=2},@{FileKey='RemoveInstall';Component_='Application';FileName='';DirProperty='INSTALLFOLDER';InstallMode=2},@{FileKey='RemoveMenu';Component_='Shortcuts';FileName='';DirProperty='APPLICATIONMENU';InstallMode=2},@{FileKey='RemovePrograms';Component_='Application';FileName='';DirProperty='PROGRAMS';InstallMode=2})
        Binary=@()
    }
}
$cases = @(
    @{Name='valid';Valid=$true;Mutate={param($t)}},
    @{Name='no-forced-close';Valid=$true;Mutate={param($t)}},
    @{Name='forced-close-missing';Valid=$false;Mutate={param($t) $t.Property=@($t.Property | Where-Object {$_.Property -cne 'MSIRESTARTMANAGERCONTROL'})}},
    @{Name='forced-close-enabled';Valid=$false;Mutate={param($t) $t.Property[-1].Value='DisableShutdown'}},
    @{Name='machine scope';Valid=$false;Mutate={param($t) $t.Property += @{Property='ALLUSERS';Value='1'}}},
    @{Name='wrong identity';Valid=$false;Mutate={param($t) $t.Property[3].Value='{AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA}'}},
    @{Name='data payload';Valid=$false;Mutate={param($t) $t.File[0].FileName='studyapp.sqlite3'}},
    @{Name='misplaced resource';Valid=$false;Mutate={param($t) $t.Component[0].Directory_='SUPPORT'}},
    @{Name='reused component guid';Valid=$false;Mutate={param($t) $t.Component[1].ComponentId=$t.Component[0].ComponentId}},
    @{Name='unbounded removal';Valid=$false;Mutate={param($t) $t.RemoveFile[0].FileName='*'}},
    @{Name='unsafe custom action';Valid=$false;Mutate={param($t) $t.CustomAction += @{Action='RunDll';Type=1;Source='Dll';Target='Entry'}}},
    @{Name='unexpected binary';Valid=$false;Mutate={param($t) $t.Binary += @{Name='Util';Data='dll'}}},
    @{Name='removal before transaction';Valid=$false;Mutate={param($t) $t.InstallExecuteSequence[1].Sequence=1400}},
    @{Name='wrong install root';Valid=$false;Mutate={param($t) $t.Directory[2].Directory_Parent='TARGETDIR'}},
    @{Name='missing downgrade gate';Valid=$false;Mutate={param($t) $t.LaunchCondition=@()}},
    @{Name='two-owner same version';Valid=$false;Mutate={param($t) $t.Upgrade[0].Attributes=1}},
    @{Name='machine keypath';Valid=$false;Mutate={param($t) $t.Registry[0].Root=2}},
    @{Name='missing arp location';Valid=$false;Mutate={param($t) $t.CustomAction[0].Target='C:\Wrong'}},
    @{Name='shortcuts without owning component';Valid=$false;Mutate={param($t) $t.Component[-1].Component='Other';$t.Registry[-1].Component_='Other'}},
    @{Name='incorrect downgrade lower bound';Valid=$false;Mutate={param($t) $t.Upgrade[1].Attributes=258}},
    @{Name='failure ignored during upgrade removal';Valid=$false;Mutate={param($t) $t.Upgrade[0].Attributes=517}},
    @{Name='programs-empty-only';Valid=$true;Mutate={param($t)}},
    @{Name='programs-missing-row';Valid=$false;Mutate={param($t) $t.RemoveFile=@($t.RemoveFile | Where-Object { $_.DirProperty -ne 'PROGRAMS' })}},
    @{Name='programs-file-removal';Valid=$false;Mutate={param($t) $t.RemoveFile[-1].FileName='*'}},
    @{Name='programs-wrong-component';Valid=$false;Mutate={param($t) $t.RemoveFile[-1].Component_='Shortcuts'}},
    @{Name='programs-install-cleanup';Valid=$false;Mutate={param($t) $t.RemoveFile[-1].InstallMode=1}},
    @{Name='programs-duplicate-removal';Valid=$false;Mutate={param($t) $t.RemoveFile += $t.RemoveFile[-1]}}
)
$rollbackCases = @(
    @{Name='rollback-intact';Valid=$true;Mutate={param($s)}},
    @{Name='rollback-app-missing';Valid=$false;Mutate={param($s) [void]$s.Files.Remove('WW-StudyApp.exe')}},
    @{Name='rollback-runtime-corrupt';Valid=$false;Mutate={param($s) $s.Files['vcruntime140.dll'].sha256='b'*64}},
    @{Name='rollback-uninstaller-size';Valid=$false;Mutate={param($s) $s.Files['Uninstall.exe'].size=99}},
    @{Name='rollback-manifest-replaced';Valid=$false;Mutate={param($s) $s.Files['support/manifest.json'].sha256='c'*64}},
    @{Name='rollback-new-owned-file';Valid=$false;Mutate={param($s) $s.AbsentPaths['new-owned.dll']=$false}},
    @{Name='rollback-registration-code';Valid=$false;Mutate={param($s) $s.Registration[0].ProductCode='{22222222-2222-4222-8222-222222222222}'}},
    @{Name='rollback-registration-version';Valid=$false;Mutate={param($s) $s.Registration[0].Version='9.9.9'}},
    @{Name='rollback-registration-root';Valid=$false;Mutate={param($s) $s.Registration[0].InstallLocation='C:\Other'}},
    @{Name='rollback-registration-scope';Valid=$false;Mutate={param($s) $s.Registration[0].Scope=$false}}
)
function New-RollbackState {
    $files=@{}
    foreach($path in @('WW-StudyApp.exe','Uninstall.exe','vcruntime140.dll','vcruntime140_1.dll','support/LICENSE','support/THIRD-PARTY-NOTICES.txt','support/manifest.json','retired-runtime.txt')) {
        $files[$path]=@{size=10;sha256='a'*64}
    }
    @{Files=$files;AbsentPaths=@{'new-owned.dll'=$true};Registration=@(@{ProductCode='{11111111-1111-4111-8111-111111111111}';Version='0.0.1';InstallLocation='C:\Fixture';Scope=$true})}
}
$comCases = @('com-record-table-read', 'com-database-table-read')
if ($SelectedCase.Count) {
    if (@($SelectedCase | Where-Object { $_ -cnotin (@($cases.Name)+@($rollbackCases.Name)+$comCases) }).Count) { throw 'Unknown installer policy case' }
    $cases = @($cases | Where-Object { $_.Name -cin $SelectedCase })
    $rollbackCases = @($rollbackCases | Where-Object { $_.Name -cin $SelectedCase })
    $comCases = @($comCases | Where-Object { $_ -cin $SelectedCase })
}
foreach ($case in $cases) {
    $tables = New-ValidTables
    & $case.Mutate $tables
    $accepted = $true
    try { Assert-InstallerTables -Tables $tables -Version '1.2.3' } catch { $accepted = $false }
    if ($accepted -ne $case.Valid) { throw "Installer policy case failed: $($case.Name)" }
}
foreach($case in $rollbackCases) {
    $before=New-RollbackState
    $after=New-RollbackState
    & $case.Mutate $after
    $accepted=$true
    try { Assert-InstallerRollbackState -Before $before -After $after } catch { $accepted=$false }
    if($accepted -ne $case.Valid) { throw "Installer rollback case failed: $($case.Name)" }
}
foreach ($case in @($comCases | Where-Object { $_ -ceq 'com-database-table-read' })) {
    if (-not $IsWindows) { throw 'The read-only MSI database regression requires Windows COM' }
    $fixtureDirectory = Join-Path $PSScriptRoot ('../.scratch/installer-table-read-fixtures/' + [guid]::NewGuid().ToString('N'))
    [void][IO.Directory]::CreateDirectory($fixtureDirectory)
    $fixturePath = [IO.Path]::GetFullPath((Join-Path $fixtureDirectory 'database-reader.msi'))
    if (Test-Path -LiteralPath $fixturePath) { throw 'MSI database fixture must be new' }
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $database = $null
    try {
        # These three data tables cannot install anything. Close the creator before
        # exercising the entire production table reader through a read-only handle.
        $database = $installer.OpenDatabase($fixturePath, 3)
        foreach ($sql in @(
            'CREATE TABLE `Property` (`Property` CHAR(72) NOT NULL, `Value` CHAR(255) PRIMARY KEY `Property`)',
            "INSERT INTO ``Property`` (``Property``, ``Value``) VALUES ('ProductName', 'Reader fixture')",
            "INSERT INTO ``Property`` (``Property``, ``Value``) VALUES ('ProductVersion', '1.2.3')",
            'CREATE TABLE `Directory` (`Directory` CHAR(72) NOT NULL, `Directory_Parent` CHAR(72), `DefaultDir` CHAR(255) NOT NULL PRIMARY KEY `Directory`)',
            'CREATE TABLE `File` (`File` CHAR(72) NOT NULL, `Component_` CHAR(72) NOT NULL, `FileName` CHAR(255) NOT NULL, `FileSize` LONG NOT NULL, `Version` CHAR(72), `Language` CHAR(20), `Attributes` SHORT, `Sequence` SHORT NOT NULL PRIMARY KEY `File`)',
            "INSERT INTO ``File`` (``File``, ``Component_``, ``FileName``, ``FileSize``, ``Version``, ``Attributes``, ``Sequence``) VALUES ('A', 'Application', 'first.txt', 42, '1.2.3', 512, 1)",
            "INSERT INTO ``File`` (``File``, ``Component_``, ``FileName``, ``FileSize``, ``Attributes``, ``Sequence``) VALUES ('B', 'Support', 'second.txt', 7, 0, 2)"
        )) {
            $view = $database.OpenView($sql)
            try { [void]$view.Execute() } finally {
                [void]$view.Close()
                [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($view)
            }
        }
        [void]$database.Commit()
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($database)
        $database = $null
        $before = (Get-FileHash -LiteralPath $fixturePath -Algorithm SHA256).Hash
        $database = $installer.OpenDatabase($fixturePath, 0)
        $returned = @(Read-InstallerDatabaseTables -Database $database)
        $types = @($returned | ForEach-Object { if ($null -eq $_) { '<null>' } else { $_.GetType().FullName } })
        Write-Output "MSI reader success stream: count=$($returned.Count); types=$($types -join ',')"
        if ($returned.Count -ne 1 -or $returned[0] -isnot [hashtable]) { throw 'MSI table reader must return exactly one Hashtable without COM method results' }
        $tables = $returned[0]
        $expectedNames = @('Property','Directory','File','Component','Registry','Upgrade','InstallExecuteSequence','CustomAction','LaunchCondition','Shortcut','RemoveFile','Binary')
        if ($tables.Count -ne $expectedNames.Count -or @($expectedNames | Where-Object { -not $tables.ContainsKey($_) }).Count) { throw 'MSI reader did not return every expected table' }
        foreach ($name in @($expectedNames | Where-Object { $_ -cnotin @('Property','File') })) {
            if ($tables[$name] -isnot [array] -or $tables[$name].Count -ne 0) { throw "Absent or empty MSI table must be an empty array: $name" }
        }
        $properties = @($tables.Property | Sort-Object Property)
        if ($properties.Count -ne 2 -or $properties[0].Count -ne 2 -or $properties[1].Count -ne 2 -or
            $properties[0].Property -cne 'ProductName' -or $properties[0].Value -cne 'Reader fixture' -or
            $properties[1].Property -cne 'ProductVersion' -or $properties[1].Value -cne '1.2.3') { throw 'MSI Property table values or rows were lost' }
        $files = @($tables.File | Sort-Object File)
        if ($files.Count -ne 2 -or $files[0].Count -ne 8 -or $files[1].Count -ne 8 -or
            $files[0].File -cne 'A' -or $files[0].Component_ -cne 'Application' -or $files[0].FileName -cne 'first.txt' -or $files[0].FileSize -cne '42' -or $files[0].Version -cne '1.2.3' -or $files[0].Language -cne '' -or $files[0].Attributes -cne '512' -or $files[0].Sequence -cne '1' -or
            $files[1].File -cne 'B' -or $files[1].Component_ -cne 'Support' -or $files[1].FileName -cne 'second.txt' -or $files[1].FileSize -cne '7' -or $files[1].Version -cne '' -or $files[1].Language -cne '' -or $files[1].Attributes -cne '0' -or $files[1].Sequence -cne '2') { throw 'MSI File table strings, integers, null fields, columns or rows were lost' }
        if ((Get-FileHash -LiteralPath $fixturePath -Algorithm SHA256).Hash -cne $before) { throw 'Read-only MSI table inspection modified its database' }
    } finally {
        if ($null -ne $database) { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($database) }
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer)
    }
}
foreach ($case in @($comCases | Where-Object { $_ -ceq 'com-record-table-read' })) {
    if (-not $IsWindows) { throw 'The read-only MSI record regression requires Windows COM' }
    # This is a fresh database containing only a Probe table, never an installable
    # package. Do not invoke msiexec, an MSI session, or the CI-only audit entry point.
    $fixtureDirectory = Join-Path $PSScriptRoot ('../.scratch/installer-table-read-fixtures/' + [guid]::NewGuid().ToString('N'))
    [void][IO.Directory]::CreateDirectory($fixtureDirectory)
    $fixturePath = [IO.Path]::GetFullPath((Join-Path $fixtureDirectory 'reader.msi'))
    if (Test-Path -LiteralPath $fixturePath) { throw 'MSI record fixture must be new' }
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $database = $null
    $view = $null
    $columns = $null
    try {
        $database = $installer.OpenDatabase($fixturePath, 3)
        foreach ($sql in @(
            'CREATE TABLE `Probe` (`Key` CHAR(72) NOT NULL, `Text` CHAR(255), `Number` SHORT, `Optional` CHAR(72) PRIMARY KEY `Key`)',
            "INSERT INTO ``Probe`` (``Key``, ``Text``, ``Number``, ``Optional``) VALUES ('A', 'first row', 42, 'value')",
            "INSERT INTO ``Probe`` (``Key``, ``Text``, ``Number``) VALUES ('B', 'second row', -7)"
        )) {
            $view = $database.OpenView($sql)
            try { $view.Execute() } finally {
                $view.Close()
                [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($view)
                $view = $null
            }
        }
        $database.Commit()
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($database)
        $database = $null
        $before = (Get-FileHash -LiteralPath $fixturePath -Algorithm SHA256).Hash
        $database = $installer.OpenDatabase($fixturePath, 0)
        $view = $database.OpenView('SELECT * FROM `Probe` ORDER BY `Key`')
        $view.Execute()
        $columns = $view.ColumnInfo(0)
        $actual = @()
        while ($null -ne ($record = $view.Fetch())) {
            try { $actual += (ConvertFrom-InstallerRecord -Record $record -Columns $columns) }
            finally { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($record) }
        }
        if ($actual.Count -ne 2 -or $actual[0].Count -ne 4 -or $actual[1].Count -ne 4 -or
            $actual[0].Key -cne 'A' -or $actual[0].Text -cne 'first row' -or $actual[0].Number -cne '42' -or $actual[0].Optional -cne 'value' -or
            $actual[1].Key -cne 'B' -or $actual[1].Text -cne 'second row' -or $actual[1].Number -cne '-7' -or $actual[1].Optional -cne '') {
            throw 'MSI COM record conversion lost a column, row, string, integer, or null field'
        }
        if ((Get-FileHash -LiteralPath $fixturePath -Algorithm SHA256).Hash -cne $before) { throw 'Read-only MSI inspection modified its database' }
    } finally {
        if ($null -ne $columns) { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($columns) }
        if ($null -ne $view) { $view.Close(); [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($view) }
        if ($null -ne $database) { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($database) }
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer)
    }
}
Write-Output "Installer policy: $($cases.Count) table and $($rollbackCases.Count) rollback synthetic cases passed."
if ($comCases.Count) { Write-Output "Installer policy: $($comCases.Count) read-only COM record case passed." }
