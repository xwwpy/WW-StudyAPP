#requires -Version 7.0
# Shared by silent synthetic tests and the runner's actual MSI-table audit.
function Get-InstallerApiInventory {
    if($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows' -or -not $IsWindows) {throw 'MSI inventory is restricted to the isolated Windows CI runner'}
    if(-not ('StudyAppInstallerInventory.Native' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Text;
namespace StudyAppInstallerInventory {
public static class Native {
    [DllImport("msi.dll", CharSet=CharSet.Unicode, ExactSpelling=true)]
    static extern uint MsiEnumRelatedProductsW(string family, uint reserved, uint index, StringBuilder product);
    [DllImport("msi.dll", CharSet=CharSet.Unicode, ExactSpelling=true)]
    static extern uint MsiEnumProductsExW(string product, string sid, uint contexts, uint index, StringBuilder installed, out uint context, StringBuilder user, ref uint length);
    [DllImport("msi.dll", CharSet=CharSet.Unicode, ExactSpelling=true)]
    static extern uint MsiGetProductInfoExW(string product, string sid, uint context, string property, StringBuilder value, ref uint length);
    public static object Collect() {
        const string family="{4CF5A7C2-094D-4D97-AB25-24C6C2BAA4CD}";
        var related=new List<string>(); var instances=new List<object>();
        var calls=new List<object>(); var errors=new List<string>(); string current="";
        try {
            using(var identity=WindowsIdentity.GetCurrent()) {current=identity.User.Value;}
            bool relatedDone=false;
            for(uint i=0;i<128;i++) {
                var product=new StringBuilder(39);
                uint code=MsiEnumRelatedProductsW(family,0,i,product);
                calls.Add(new {Api="MsiEnumRelatedProductsW",Index=i,Code=code,ProductCode=product.ToString()});
                if(code==259) {relatedDone=true;break;}
                if(code!=0) {errors.Add("MsiEnumRelatedProductsW:"+code);break;}
                string productCode=product.ToString(); related.Add(productCode);
                bool instanceDone=false;
                for(uint j=0;j<128;j++) {
                    var installed=new StringBuilder(39); var sid=new StringBuilder(185);
                    uint length=185,context;
                    // Every call for this enumeration stays on this thread. Everyone + all
                    // contexts makes foreign-user/machine instances visible as conflicts.
                    code=MsiEnumProductsExW(productCode,"S-1-1-0",7,j,installed,out context,sid,ref length);
                    calls.Add(new {Api="MsiEnumProductsExW",Index=j,Code=code,ProductCode=productCode,InstalledProduct=installed.ToString(),Context=context,UserSid=sid.ToString(),Length=length});
                    if(code==259) {instanceDone=true;break;}
                    if(code!=0 || length>=185) {errors.Add("MsiEnumProductsExW:"+code+"/length="+length);break;}
                    var row=new Dictionary<string,object> {{"ProductCode",installed.ToString()},{"Context",context},{"UserSid",sid.ToString()}};
                    foreach(string property in new [] {"VersionString","InstallLocation","ProductName","State"}) {
                        var value=new StringBuilder(32768); uint count=32768;
                        code=MsiGetProductInfoExW(installed.ToString(),context==4?null:sid.ToString(),context,property,value,ref count);
                        calls.Add(new {Api="MsiGetProductInfoExW",Code=code,ProductCode=installed.ToString(),Context=context,UserSid=sid.ToString(),Property=property,Length=count});
                        string key=property=="VersionString"?"Version":property=="State"?"ProductState":property;
                        row[key]=value.ToString();
                        if(code!=0 || count>=32768) {errors.Add("MsiGetProductInfoExW:"+property+":"+code+"/length="+count);}
                    }
                    instances.Add(row);
                }
                if(!instanceDone) {errors.Add("MSI instance enumeration incomplete");}
            }
            if(!relatedDone) {errors.Add("MSI family enumeration incomplete");}
        } catch(Exception error) {errors.Add(error.GetType().FullName+":"+error.Message);}
        return new Dictionary<string,object> {{"Family",family},{"CurrentSid",current},{"Related",related.ToArray()},{"Instances",instances.ToArray()},{"Calls",calls.ToArray()},{"Errors",errors.ToArray()}};
    }
}
}
'@
    }
    [StudyAppInstallerInventory.Native]::Collect()
}

function Get-InstallerArpOwners {
    param([string]$CurrentSid='', [string[]]$RelatedProducts=@(), [hashtable]$Diagnostics)
    foreach($registryRoot in @('HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall','HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall','HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall')) {
        if(-not (Test-Path -LiteralPath $registryRoot)) {continue}
        foreach($key in Get-ChildItem -LiteralPath $registryRoot) {
            $properties=Get-ItemProperty -LiteralPath $key.PSPath
            if($null -eq $properties) {continue}
            # A multi-string DisplayName may still name this product; a skipped
            # row would hide a conflicting registration from the diagnostics.
            $displayName=$properties.PSObject.Properties['DisplayName']
            $named=$false
            if($null -ne $displayName) {
                $displayValue=$displayName.Value
                if($displayValue -is [string]) {$named=($displayValue -ceq 'WW-StudyApp')}
                elseif($null -ne $displayValue -and $displayValue -isnot [ValueType] -and $displayValue -is [Collections.IEnumerable]) {
                    foreach($item in $displayValue) {if($item -is [string] -and $item -ceq 'WW-StudyApp') {$named=$true;break}}
                }
            }
            if(-not $named -and $key.PSChildName -notin $RelatedProducts) {continue}
            $perUser=$registryRoot.StartsWith('HKCU:')
            $raw=[ordered]@{ProductCode=$key.PSChildName;RegistryPath=$key.PSPath;CurrentUser=$perUser;Scope=$perUser;Context=$(if($perUser){2}else{4});UserSid=$(if($perUser){$CurrentSid}else{''});ProductName=$null;Version=$null;Root=$null;InstallLocation=$null;WindowsInstaller=$null}
            foreach($entry in @(@('DisplayName','ProductName'),@('DisplayVersion','Version'),@('InstallLocation','InstallLocation'),@('WindowsInstaller','WindowsInstaller'))) {
                $property=$properties.PSObject.Properties[$entry[0]]
                if($null -ne $property) {$raw[$entry[1]]=$property.Value}
            }
            $raw.Root=$raw.InstallLocation
            if($null -ne $Diagnostics) {$Diagnostics.Arp+= [pscustomobject]$raw}
            # Preserve strict malformed-target failures, after persisting its raw fields.
            $null=$properties.DisplayVersion;$null=$properties.InstallLocation
            [pscustomobject]$raw
        }
    }
}

function ConvertTo-InstallerRoot {
    param([AllowEmptyString()][string]$Path)
    if($Path -notmatch '^[A-Za-z]:[\\/]' -or $Path.Substring(2) -match '[:*?"<>|]' -or @($Path.Substring(3) -split '[\\/]' | Where-Object {$_ -in @('.','..') -or ($_ -ne '' -and ($_ -match '[. ]$'))}).Count) {throw 'Invalid MSI installation root'}
    [IO.Path]::GetFullPath($Path).TrimEnd('\','/')
}

function Test-InstallerProductCode {
    param($Value)
    if($Value -isnot [string]) {return $false}
    $guid=[guid]::Empty
    return [guid]::TryParseExact($Value,'B',[ref]$guid)
}

function Test-InstallerScalarString {
    param($Value,[string]$Expected)
    if($Value -isnot [string]) {return $false}
    return $Value -ceq $Expected
}

function Test-InstallerScalarInteger {
    param($Value,[long]$Expected)
    foreach($type in @([byte],[sbyte],[int16],[uint16],[int32],[uint32],[int64],[uint64])) {
        if($Value -is $type) {
            try {return ([int64]$Value -eq $Expected)} catch {return $false}
        }
    }
    return $false
}

function ConvertTo-VerifiedInstallerOwners {
    param([Parameter(Mandatory)]$Api,[object[]]$Arp=@(),[Parameter(Mandatory)][string]$ExpectedRoot,[string[]]$ExpectedProducts=@(),[string]$ExpectedVersion='', [int]$ExpectedCount=-1,[Parameter(Mandatory)][hashtable]$Diagnostics)
    $family='{4CF5A7C2-094D-4D97-AB25-24C6C2BAA4CD}'
    $root=ConvertTo-InstallerRoot $ExpectedRoot
    $owners=[Collections.Generic.List[object]]::new()
    $conflicts=[Collections.Generic.List[object]]::new()
    $checks=[Collections.Generic.List[object]]::new()
    if($Api.Family -ine $family -or $Api.CurrentSid -notmatch '^S-1-\d+(?:-\d+)+$' -or @($Api.Errors).Count) {$conflicts.Add(@{Kind='api-authority';Errors=@($Api.Errors)})}
    $related=[Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach($code in @($Api.Related)) {
        if(-not (Test-InstallerProductCode $code) -or -not $related.Add($code)) {$conflicts.Add(@{Kind='related-product';ProductCode=$code})}
    }
    $seen=[Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach($row in @($Api.Instances)) {
        $location='';try {$location=ConvertTo-InstallerRoot $row.InstallLocation} catch { }
        $check=[ordered]@{Source='API';ProductCode=$row.ProductCode;Guid=(Test-InstallerProductCode $row.ProductCode);Family=($row.ProductCode -is [string] -and $related.Contains($row.ProductCode));Context=(Test-InstallerScalarInteger $row.Context 2);Sid=(Test-InstallerScalarString $row.UserSid $Api.CurrentSid);Name=(Test-InstallerScalarString $row.ProductName 'WW-StudyApp');State=(Test-InstallerScalarString $row.ProductState '5');Version=($row.Version -is [string] -and $row.Version -match '^\d+\.\d+\.\d+$');Root=($row.InstallLocation -is [string] -and $location -ne '' -and $location -ieq $root);ExpectedProduct=($ExpectedProducts.Count -eq 0 -or ($row.ProductCode -is [string] -and $row.ProductCode -in $ExpectedProducts));ExpectedVersion=($ExpectedVersion -eq '' -or (Test-InstallerScalarString $row.Version $ExpectedVersion));Unique=$seen.Add("$($row.ProductCode)|$($row.Context)|$($row.UserSid)")}
        $checks.Add($check)
        # Every named comparison must be a Boolean; a collection or other malformed
        # value is a conflict, never silently skipped. Source/ProductCode are labels.
        $comparisons=@($check.GetEnumerator() | Where-Object {$_.Key -notin @('Source','ProductCode')})
        if(@($comparisons | Where-Object {$_.Value -isnot [bool] -or -not $_.Value}).Count) {$conflicts.Add(@{Kind='api-instance';Row=$row;Checks=$check});continue}
        $owners.Add([pscustomobject]@{ProductCode=$row.ProductCode;CurrentUser=$true;Scope=$true;Context=2;UserSid=$row.UserSid;Version=$row.Version;Root=$location;InstallLocation=$location})
    }
    foreach($code in $related) {if(@($Api.Instances | Where-Object ProductCode -IEQ $code).Count -eq 0) {$conflicts.Add(@{Kind='related-without-instance';ProductCode=$code})}}
    $arpSeen=[Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach($row in $Arp) {
        $location='';try {$location=ConvertTo-InstallerRoot $row.InstallLocation} catch { }
        $matches=@($owners | Where-Object {$_.ProductCode -ieq $row.ProductCode -and $_.Context -eq $row.Context -and $_.UserSid -ceq $row.UserSid})
        # A per-user MSI install can mirror its ARP registration into the machine
        # hive without a matching MSI instance; the API enumeration stays
        # authoritative. Only the machine hive (context 4, no user SID) can be a
        # mirror, and only when every identity field equals the single owner.
        $same=@($owners | Where-Object {$_.ProductCode -ieq $row.ProductCode})
        $mirror=($matches.Count -eq 0 -and $same.Count -eq 1 -and
            (Test-InstallerScalarInteger $row.Context 4) -and (Test-InstallerScalarString $row.UserSid '') -and
            (Test-InstallerScalarString $row.ProductName 'WW-StudyApp') -and
            (Test-InstallerScalarString $row.Version $same[0].Version) -and
            $row.InstallLocation -is [string] -and $location -ne '' -and $location -ieq $same[0].Root)
        $owner=$null
        if($matches.Count -eq 1) {$owner=$matches[0]} elseif($mirror) {$owner=$same[0]}
        $check=[ordered]@{Source='ARP';ProductCode=$row.ProductCode;Guid=(Test-InstallerProductCode $row.ProductCode);Family=($row.ProductCode -is [string] -and $related.Contains($row.ProductCode));Context=($mirror -or (Test-InstallerScalarInteger $row.Context 2));Sid=($mirror -or (Test-InstallerScalarString $row.UserSid $Api.CurrentSid));Name=(Test-InstallerScalarString $row.ProductName 'WW-StudyApp');Msi=(Test-InstallerScalarInteger $row.WindowsInstaller 1);ApiInstance=($matches.Count -eq 1 -or $mirror);Mirror=$mirror;Root=($null -ne $owner -and $row.InstallLocation -is [string] -and $location -ne '' -and $location -ieq $owner.Root);Version=($null -ne $owner -and (Test-InstallerScalarString $row.Version $owner.Version));Unique=$arpSeen.Add("$($row.ProductCode)|$($row.Context)|$($row.UserSid)")}
        $checks.Add($check)
        $comparisons=@($check.GetEnumerator() | Where-Object {$_.Key -notin @('Source','ProductCode','Mirror')})
        if(@($comparisons | Where-Object {$_.Value -isnot [bool] -or -not $_.Value}).Count) {$conflicts.Add(@{Kind='arp-conflict';Row=$row;Checks=$check})}
    }
    $countCheck=[ordered]@{Source='Inventory';SingleOwner=($owners.Count -le 1);ExpectedCount=($ExpectedCount -lt 0 -or $owners.Count -eq $ExpectedCount)}
    $checks.Add($countCheck)
    if(-not $countCheck.SingleOwner -or -not $countCheck.ExpectedCount) {$conflicts.Add(@{Kind='owner-count';Actual=$owners.Count;Expected=$ExpectedCount})}
    $Diagnostics.Checks=$checks.ToArray();$Diagnostics.Conflicts=$conflicts.ToArray()
    if($conflicts.Count) {throw 'MSI inventory conflicts with the verified current-user installation; inspect inventory diagnostics'}
    $owners.ToArray()
}

function Get-VerifiedInstallerOwners {
    param([Parameter(Mandatory)][string]$OutputDirectory,[Parameter(Mandatory)][string]$Label,[Parameter(Mandatory)][string]$ExpectedRoot,[string[]]$ExpectedProducts=@(),[string]$ExpectedVersion='', [int]$ExpectedCount=-1)
    $record=@{Label=$Label;Expected=@{Family='{4CF5A7C2-094D-4D97-AB25-24C6C2BAA4CD}';Root=$ExpectedRoot;Products=$ExpectedProducts;Version=$ExpectedVersion;Count=$ExpectedCount;Context=2};Api=$null;Arp=@();Checks=@();Conflicts=@();Errors=@();Passed=$false}
    $failure=$null;$owners=@()
    try {
        $api=Get-InstallerApiInventory;$record.Api=$api
        $arp=@(Get-InstallerArpOwners -CurrentSid $api.CurrentSid -RelatedProducts @($api.Related) -Diagnostics $record)
        $owners=@(ConvertTo-VerifiedInstallerOwners -Api $api -Arp $arp -ExpectedRoot $ExpectedRoot -ExpectedProducts $ExpectedProducts -ExpectedVersion $ExpectedVersion -ExpectedCount $ExpectedCount -Diagnostics $record)
        $record.Passed=$true
    } catch {$failure=$_;$record.Errors+= $_.Exception.ToString()}
    $path=Join-Path $OutputDirectory ('msi-inventory-'+[guid]::NewGuid().ToString('N')+'.json')
    [IO.File]::WriteAllText($path,($record | ConvertTo-Json -Depth 18))
    if($null -ne $failure) {throw $failure}
    $owners
}

function Get-InstallerPayload {
    @(
        @{Id='Application';Name='WW-StudyApp.exe';Directory='INSTALLFOLDER'},
        @{Id='Uninstaller';Name='Uninstall.exe';Directory='INSTALLFOLDER'},
        @{Id='Runtime';Name='vcruntime140.dll';Directory='INSTALLFOLDER'},
        @{Id='Runtime1';Name='vcruntime140_1.dll';Directory='INSTALLFOLDER'},
        @{Id='License';Name='LICENSE';Directory='SUPPORT'},
        @{Id='Notices';Name='THIRD-PARTY-NOTICES.txt';Directory='SUPPORT'},
        @{Id='Manifest';Name='manifest.json';Directory='SUPPORT'}
    )
}

function Get-InstallerOwnedPaths {
    param([Parameter(Mandatory)][hashtable]$Tables)
    $components=@{}
    foreach($component in $Tables.Component) { $components[$component.Component]=$component.Directory_ }
    $paths=[Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach($file in $Tables.File) {
        $name=($file.FileName -split '\|')[-1]
        if($name -notmatch '^[A-Za-z0-9][A-Za-z0-9_.-]*$' -or $name.EndsWith('.') -or -not $components.ContainsKey($file.Component_)) { throw 'Unsafe MSI-owned snapshot name' }
        $directory=$components[$file.Component_]
        $relative=if($directory -ceq 'INSTALLFOLDER') { $name } elseif($directory -ceq 'SUPPORT') { "support/$name" } else { throw 'Unexpected MSI-owned snapshot directory' }
        if(-not $paths.Add($relative)) { throw 'Duplicate MSI-owned snapshot path' }
        $relative
    }
}

function Get-InstallerRollbackSnapshot {
    param([Parameter(Mandatory)][string]$Root,[Parameter(Mandatory)][string[]]$OwnedPaths,[string[]]$AbsentPaths=@(),[Parameter(Mandatory)][object[]]$Registration)
    $files=@{}
    foreach($relative in $OwnedPaths) {
        if($relative -notmatch '^(?:support/)?[A-Za-z0-9][A-Za-z0-9_.-]*$' -or $relative.EndsWith('.')) { throw 'Unsafe rollback snapshot path' }
        $file=Get-Item -LiteralPath (Join-Path $Root $relative)
        if($file.PSIsContainer -or ($file.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Rollback snapshot requires regular owned files' }
        $files[$relative]=@{size=$file.Length;sha256=(Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()}
    }
    $absent=@{}
    foreach($relative in $AbsentPaths) {
        if($relative -notmatch '^(?:support/)?[A-Za-z0-9][A-Za-z0-9_.-]*$' -or $relative.EndsWith('.')) { throw 'Unsafe rollback absent path' }
        $absent[$relative]=-not (Test-Path -LiteralPath (Join-Path $Root $relative))
    }
    @{Files=$files;AbsentPaths=$absent;Registration=$Registration}
}

function Assert-InstallerRollbackState {
    param([Parameter(Mandatory)][hashtable]$Before,[Parameter(Mandatory)][hashtable]$After)
    if($Before.Files.Count -eq 0 -or $Before.Files.Count -ne $After.Files.Count) { throw 'Rollback lost owned payload paths' }
    foreach($path in $Before.Files.Keys) {
        if(-not $After.Files.ContainsKey($path) -or $Before.Files[$path].size -ne $After.Files[$path].size -or $Before.Files[$path].sha256 -cne $After.Files[$path].sha256) { throw "Rollback did not restore owned payload: $path" }
    }
    if($Before.AbsentPaths.Count -ne $After.AbsentPaths.Count) { throw 'Rollback absent-path snapshot differs' }
    foreach($path in $Before.AbsentPaths.Keys) {
        if($Before.AbsentPaths[$path] -ne $true -or -not $After.AbsentPaths.ContainsKey($path) -or $After.AbsentPaths[$path] -ne $true) { throw "Rollback introduced incoming owned payload: $path" }
    }
    $registrations=@()
    foreach($state in @($Before,$After)) {
        $rows=@($state.Registration | Sort-Object ProductCode | ForEach-Object { [ordered]@{ProductCode=$_.ProductCode;Scope=$_.Scope;Version=$_.Version;InstallLocation=$_.InstallLocation} })
        $registrations+=,($rows | ConvertTo-Json -Compress)
    }
    if($Before.Registration.Count -ne 1 -or $registrations[0] -cne $registrations[1]) { throw 'Rollback did not restore the exact previous registration' }
}

function ConvertFrom-InstallerRecord {
    param([Parameter(Mandatory)][object]$Record, [Parameter(Mandatory)][object]$Columns)
    $row = @{}
    # PowerShell 7 may not expose this non-indexed IDispatch property on __ComObject.
    $fieldCount = [int]$Record.GetType().InvokeMember('FieldCount', [Reflection.BindingFlags]::GetProperty, $null, $Record, $null)
    for ($i=1; $i -le $fieldCount; $i++) { $row[$Columns.StringData($i)] = $Record.StringData($i) }
    return $row
}

function Read-InstallerDatabaseTables {
    param([Parameter(Mandatory)][object]$Database)
    $tables = @{}
    $available = $database.OpenView('SELECT `Name` FROM `_Tables`')
    # COM void methods can emit a null success-stream item in PowerShell.
    # Keep every Execute/Close result out of the returned table collection.
    [void]$available.Execute()
    $names = @()
    while ($null -ne ($record = $available.Fetch())) { $names += $record.StringData(1) }
    [void]$available.Close()
    foreach ($name in @('Property','Directory','File','Component','Registry','Upgrade','InstallExecuteSequence','CustomAction','LaunchCondition','Shortcut','RemoveFile','Binary')) {
        $rows = @()
        if ($name -in $names) {
            $view = $database.OpenView("SELECT * FROM ``$name``")
            [void]$view.Execute()
            $columns = $view.ColumnInfo(0)
            while ($null -ne ($record = $view.Fetch())) {
                $rows += (ConvertFrom-InstallerRecord -Record $record -Columns $columns)
            }
            [void]$view.Close()
            [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($view)
        }
        $tables[$name] = $rows
    }
    return $tables
}

function Get-InstallerTables {
    param([Parameter(Mandatory)][string]$MsiPath)
    if ($env:GITHUB_ACTIONS -ne 'true' -or -not $IsWindows) { throw 'Actual MSI inspection is restricted to the Windows CI runner' }
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $database = $installer.OpenDatabase([IO.Path]::GetFullPath($MsiPath),0)
    try {
        Read-InstallerDatabaseTables -Database $database
    } finally {
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($database)
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer)
    }
}

function Assert-InstallerTables {
    param([Parameter(Mandatory)][hashtable]$Tables,[Parameter(Mandatory)][string]$Version)
    $properties = @{}
    foreach ($row in $Tables.Property) { $properties[$row.Property] = $row.Value }
    if (-not $properties.ContainsKey('MSIRESTARTMANAGERCONTROL') -or $properties.MSIRESTARTMANAGERCONTROL -cne 'Disable') { throw 'MSI must preserve running applications without forced Restart Manager shutdown' }
    if ($properties.ProductName -cne 'WW-StudyApp' -or $properties.ProductVersion -cne $Version -or
        $properties.UpgradeCode -ine '{4CF5A7C2-094D-4D97-AB25-24C6C2BAA4CD}' -or
        $properties.ProductCode -notmatch '^\{[0-9A-Fa-f-]{36}\}$' -or $properties.ARPNOMODIFY -ne '1' -or
        ($properties.ContainsKey('ALLUSERS') -and $properties.ALLUSERS -ne '') -or $properties.ContainsKey('MSIINSTALLPERUSER')) { throw 'Unexpected MSI identity or installation scope' }
    $directories = @{}
    foreach ($row in $Tables.Directory) { $directories[$row.Directory] = $row }
    $expectedDirectories = @{
        PROGRAMS=@('LocalAppDataFolder','Programs');INSTALLFOLDER=@('PROGRAMS','WW-StudyApp');SUPPORT=@('INSTALLFOLDER','support');APPLICATIONMENU=@('ProgramMenuFolder','WW-StudyApp')
    }
    foreach ($key in $expectedDirectories.Keys) {
        if (-not $directories.ContainsKey($key) -or $directories[$key].Directory_Parent -cne $expectedDirectories[$key][0] -or
            ($directories[$key].DefaultDir -split '\|')[-1] -cne $expectedDirectories[$key][1]) { throw 'Unexpected MSI destination directory' }
    }
    if ($Tables.Directory.Count -ne 7) { throw 'Unexpected extra MSI directories' }
    $components = @{}
    $guids = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($row in $Tables.Component) {
        if (-not $guids.Add($row.ComponentId) -or $row.ComponentId -notmatch '^\{[0-9A-Fa-f-]{36}\}$' -or
            ([int]$row.Attributes -band 260) -ne 260) { throw 'MSI component lacks distinct per-user registry key path or x64 identity' }
        $components[$row.Component] = $row
        $registry = @($Tables.Registry | Where-Object { $_.Registry -ceq $row.KeyPath -and $_.Component_ -ceq $row.Component })
        if ($registry.Count -ne 1 -or $registry[0].Root -ne 1 -or $registry[0].Key -cne 'Software\WW-StudyApp\Installer') { throw 'Unexpected component registry ownership' }
    }
    $payload = @(Get-InstallerPayload)
    if ($Tables.File.Count -ne $payload.Count -or $components.Count -ne 8 -or $Tables.Registry.Count -ne 8 -or -not $components.ContainsKey('Shortcuts') -or $components.Shortcuts.Directory_ -cne 'APPLICATIONMENU') { throw 'Unexpected MSI resource count' }
    foreach ($file in $payload) {
        $rows = @($Tables.File | Where-Object { $_.File -ceq $file.Id -and $_.Component_ -ceq $file.Id -and ($_.FileName -split '\|')[-1] -ceq $file.Name -and [long]$_.FileSize -gt 0 })
        if ($rows.Count -ne 1 -or -not $components.ContainsKey($file.Id) -or $components[$file.Id].Directory_ -cne $file.Directory) { throw 'Unexpected MSI file ownership' }
    }
    if ($Tables.Binary.Count -ne 0 -or $Tables.CustomAction.Count -ne 1) { throw 'MSI must not contain executable custom actions' }
    $action = $Tables.CustomAction[0]
    if ($action.Action -cne 'SetInstallLocation' -or [int]$action.Type -ne 51 -or $action.Source -cne 'ARPINSTALLLOCATION' -or $action.Target -cne '[INSTALLFOLDER]') { throw 'MSI install-location property action differs' }
    $sequence = @{}
    foreach ($row in $Tables.InstallExecuteSequence) { $sequence[$row.Action] = [int]$row.Sequence }
    if ($sequence.RemoveExistingProducts -le $sequence.InstallInitialize -or $sequence.RemoveExistingProducts -ge $sequence.InstallFiles -or
        $sequence.InstallFiles -ge $sequence.InstallFinalize -or -not $sequence.ContainsKey('SetInstallLocation')) { throw 'MSI major upgrade is outside the rollback transaction' }
    $up = @($Tables.Upgrade | Where-Object { $_.ActionProperty -ceq 'WIX_UPGRADE_DETECTED' })
    $down = @($Tables.Upgrade | Where-Object { $_.ActionProperty -ceq 'WIX_DOWNGRADE_DETECTED' })
    if ($Tables.Upgrade.Count -ne 2 -or $up.Count -ne 1 -or $down.Count -ne 1 -or $up[0].VersionMax -cne $Version -or
        ([int]$up[0].Attributes -band 512) -eq 0 -or ([int]$up[0].Attributes -band 6) -ne 0 -or
        $down[0].VersionMin -cne $Version -or ([int]$down[0].Attributes -band 2) -eq 0 -or ([int]$down[0].Attributes -band 256) -ne 0 -or
        @($Tables.Upgrade | Where-Object { $_.UpgradeCode -ine $properties.UpgradeCode }).Count -gt 0 -or
        @($Tables.LaunchCondition | Where-Object { $_.Condition -ceq 'NOT WIX_DOWNGRADE_DETECTED' }).Count -ne 1) { throw 'MSI version range fails downgrade/single-owner policy' }
    if ($Tables.Shortcut.Count -ne 2 -or @($Tables.Shortcut | Where-Object { $_.Component_ -cne 'Shortcuts' -or $_.Directory_ -cne 'APPLICATIONMENU' -or $_.Target -cnotin @('[INSTALLFOLDER]WW-StudyApp.exe','[INSTALLFOLDER]Uninstall.exe') }).Count) { throw 'Unexpected MSI shortcuts' }
    $emptyDirectories = @{
        RemoveSupport=@('License','SUPPORT');RemoveInstall=@('Application','INSTALLFOLDER');
        RemoveMenu=@('Shortcuts','APPLICATIONMENU');RemovePrograms=@('Application','PROGRAMS')
    }
    $removals = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($row in $Tables.RemoveFile) {
        if (-not $emptyDirectories.ContainsKey($row.FileKey) -or -not $removals.Add($row.FileKey) -or
            $row.FileName -ne '' -or $row.Component_ -cne $emptyDirectories[$row.FileKey][0] -or
            $row.DirProperty -cne $emptyDirectories[$row.FileKey][1] -or [int]$row.InstallMode -ne 2) { throw 'Only explicitly owned empty-directory uninstall rows are permitted' }
    }
    if ($removals.Count -ne $emptyDirectories.Count) { throw 'Missing user-profile directory cleanup row' }
}
